use std::{
    ffi::CString,
    io::{Read, Write},
    os::{
        fd::{FromRawFd, IntoRawFd},
        unix::{fs::PermissionsExt, net::UnixStream as SyncUnixStream},
    },
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::Context;
use async_pidfd::AsyncPidFd;
use bon::{Builder, builder};
use derive_more::Debug;
#[cfg(target_os = "linux")]
use nix::libc::c_int;
use nix::{
    mount::{MntFlags, MsFlags, mount, umount2},
    sched::{CloneFlags, unshare},
    sys::{
        resource::{UsageWho, getrusage},
        signal::{Signal, kill},
        wait::{WaitStatus, waitpid},
    },
    unistd::{
        ForkResult, Pid, chdir, dup2_stderr, dup2_stdin, dup2_stdout, execve, fork, getgid, getuid,
        pipe, pivot_root,
    },
};
use serde::{Deserialize, Serialize};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream as AsyncUnixStream,
    process::Command,
    time::Instant,
};
use tokio_stream::StreamExt;
use uuid::Uuid;
use zbus::{
    proxy,
    zvariant::{OwnedObjectPath, Value},
};

use crate::core::{
    domain::{
        Artifact, ArtifactKind, CompilationLimitType, CompilationLimits, ExecutionLimits, Language,
        TestLimitType,
    },
    traits::executor::{CompileError, Executor, RunError, RunResult},
};

const CONTAINER_ARTIFACT_PATH: &str = "/tmp/artifact";

#[derive(Clone, Debug, Builder)]
#[builder(on(PathBuf, into))]
pub struct NativeExecutor {
    dir: PathBuf,
    gnucpp_path: PathBuf,
    static_linking: bool,
    systemd_run_path: PathBuf,
    journalctl_path: PathBuf,
    rootfs: PathBuf,
    systemd_manager: SystemdManagerProxy<'static>,

    #[builder(default = bincode::config::standard())]
    #[debug(skip)]
    bincode_config: bincode::config::Configuration,
}

#[async_trait::async_trait]
impl Executor for NativeExecutor {
    async fn compile(
        &self,
        source: &str,
        language: &Language,
        limits: &CompilationLimits,
    ) -> Result<Artifact, CompileError> {
        let artifact_id = Uuid::new_v4();
        let artifact_path = self.artifact_path(&artifact_id);
        let source_path = self.dir.join(format!("{}.cpp", artifact_id));
        let scope_name = format!("coderunner-compile-{}", artifact_id);

        fs::create_dir_all(&self.dir)
            .await
            .map_err(|e| format!("Failed to create dirs when compiling: {e}"))?;
        fs::write(&source_path, source)
            .await
            .map_err(|e| format!("Failed to write source file when compiling: {e}"))?;

        let gnucpp_path = self.gnucpp_path.to_string_lossy().to_string();
        let artifact_path = artifact_path.to_string_lossy().to_string();
        let source_path = source_path.to_string_lossy().to_string();

        let mut cmd = vec![gnucpp_path, "-o".to_string(), artifact_path, source_path];
        if self.static_linking {
            cmd.push("-static".to_string());
        }

        let out = Command::new(&self.systemd_run_path)
            .arg("--user")
            .arg("--scope")
            .arg("-u")
            .arg(&scope_name)
            .args({
                let mut args = Vec::new();
                if let Some(memory_bytes) = limits.memory_bytes {
                    args.push("-p".to_string());
                    args.push("MemorySwapMax=0".to_string());
                    args.push("-p".to_string());
                    args.push(format!("MemoryMax={}", memory_bytes));
                }
                if let Some(time_ms) = limits.time_ms {
                    args.push("-p".to_string());
                    args.push(format!("RuntimeMaxSec={}ms", time_ms));
                }
                args
            })
            .arg("--")
            .args(cmd)
            .output()
            .await
            .map_err(|e| format!("Failed to execute command: {e}"))?;

        if !out.status.success() {
            let (journal_stdout, crash_reason) = self
                .journal_logs()
                .init_delay_ms(10)
                .max_retries(8)
                .scope_name(&scope_name)
                .stdout()
                .await?;

            match crash_reason {
                CrashReason::OomKiller => Err(CompileError::CompilationLimitsExceeded(
                    CompilationLimitType::Ram,
                )),
                CrashReason::Timeout => Err(CompileError::CompilationLimitsExceeded(
                    CompilationLimitType::Time,
                )),
                CrashReason::Other => {
                    let msg = format!(
                        "Journal: {}, stdout: {}, stderr: {}",
                        journal_stdout,
                        String::from_utf8_lossy(&out.stdout),
                        String::from_utf8_lossy(&out.stderr)
                    );

                    if journal_stdout.contains("Started") {
                        Err(CompileError::CompilationFailed { msg })
                    } else {
                        Err(CompileError::Internal { msg })
                    }
                }
            }
        } else {
            Ok(Artifact {
                id: artifact_id,
                kind: ArtifactKind::Executable,
            })
        }
    }

    async fn run(
        &self,
        artifact: &Artifact,
        stdin: &str,
        limits: &ExecutionLimits,
    ) -> Result<RunResult, RunError> {
        if !matches!(
            tokio::fs::try_exists(self.artifact_path(&artifact.id)).await,
            Ok(true)
        ) {
            return Err(RunError::Internal {
                msg: "Artifact not found".to_string(),
            });
        }

        // TODO: Check this when refactoring:
        // https://github.com/youki-dev/youki/blob/main/crates/libcontainer/src/process/channel.rs

        let (stdout_read, stdout_write) =
            pipe().map_err(|e| format!("Failed to create stdout pipe: {e}"))?;
        let (stderr_read, stderr_write) =
            pipe().map_err(|e| format!("Failed to create stderr pipe: {e}"))?;
        let (stdin_read, stdin_write) =
            pipe().map_err(|e| format!("Failed to create stdin pipe: {e}"))?;

        let (wait_user_ns_psock, wait_user_ns_csock) = SyncUnixStream::pair()
            .map_err(|e| format!("Failed to create wait user namespace sockets: {e}"))?;
        let (grandchild_pid_psock, mut grandchild_pid_csock) = SyncUnixStream::pair()
            .map_err(|e| format!("Failed to create grandchild pid sockets: {e}"))?;
        let (grandchild_status_psock, mut grandchild_status_csock) = SyncUnixStream::pair()
            .map_err(|e| format!("Failed to create grandchild status sockets: {e}"))?;
        let (start_time_psock, start_time_csock) = SyncUnixStream::pair()
            .map_err(|e| format!("Failed to create start time sockets: {e}"))?;
        let (end_time_psock, end_time_csock) = SyncUnixStream::pair()
            .map_err(|e| format!("Failed to create end time sockets: {e}"))?;
        let (peak_memory_usage_psock, mut peak_memory_usage_csock) = SyncUnixStream::pair()
            .map_err(|e| format!("Failed to create peak memory usage sockets: {e}"))?;
        let (wait_cgroups_psock, wait_cgroups_csock) = SyncUnixStream::pair()
            .map_err(|e| format!("Failed to create wait cgroups sockets: {e}"))?;

        match unsafe { fork().map_err(|e| format!("Failed to fork: {e}"))? } {
            ForkResult::Parent { child } => {
                drop(stdout_write);
                drop(stderr_write);
                drop(stdin_read);

                let mut stdout_read = tokio::fs::File::from_std(unsafe {
                    std::fs::File::from_raw_fd(stdout_read.into_raw_fd())
                });

                let mut stderr_read = tokio::fs::File::from_std(unsafe {
                    std::fs::File::from_raw_fd(stderr_read.into_raw_fd())
                });

                let mut stdin_write = tokio::fs::File::from_std(unsafe {
                    std::fs::File::from_raw_fd(stdin_write.into_raw_fd())
                });

                wait_user_ns_psock.set_nonblocking(true).map_err(|e| {
                    format!("Failed to set non-blocking mode for wait_user_ns_psock: {e}")
                })?;
                let wait_user_ns_psock =
                    AsyncUnixStream::from_std(wait_user_ns_psock).map_err(|e| {
                        format!("Failed to convert wait_user_ns_psock to AsyncUnixStream: {e}")
                    })?;

                wait_for_signal_async(wait_user_ns_psock)
                    .await
                    .map_err(|e| format!("Failed to wait for user namespace signal: {e}"))?;
                setup_id_mapping(child.as_raw())
                    .await
                    .map_err(|e| format!("Failed to setup id mapping: {e}"))?;

                grandchild_pid_psock.set_nonblocking(true).map_err(|e| {
                    format!("Failed to set non-blocking mode for grandchild_pid_psock: {e}")
                })?;
                let mut grandchild_pid_psock = AsyncUnixStream::from_std(grandchild_pid_psock)
                    .map_err(|e| {
                        format!("Failed to convert grandchild_pid_psock to AsyncUnixStream: {e}")
                    })?;

                let grandchild_pid = grandchild_pid_psock.read_i32().await.map_err(|e| {
                    format!("Failed to read grandchild_pid from grandchild_pid_psock: {e}")
                })?;

                self.cgroups_scope()
                    .artifact_id(&artifact.id)
                    .pid(grandchild_pid)
                    .limits(limits)
                    .create()
                    .await?;

                wait_cgroups_psock.set_nonblocking(true).map_err(|e| {
                    format!("Failed to set non-blocking mode on wait_cgroups_psock: {e}")
                })?;
                let mut wait_cgroups_psock = AsyncUnixStream::from_std(wait_cgroups_psock)
                    .map_err(|e| {
                        format!("Failed to convert wait_cgroups_psock to AsyncUnixStream: {e}")
                    })?;
                send_signal_async(&mut wait_cgroups_psock)
                    .await
                    .map_err(|e| format!("Failed to send signal to wait_cgroups_psock: {e}"))?;

                let pidfd = AsyncPidFd::from_pid(child.as_raw())
                    .map_err(|e| format!("Failed to create AsyncPidFd from child pid: {e}"))?;

                let stdin = stdin.to_string();
                let stdin_task = tokio::spawn(async move {
                    if !stdin.is_empty() {
                        stdin_write
                            .write_all(stdin.as_bytes())
                            .await
                            .expect("Failed to write to stdin")
                    }
                    drop(stdin_write);
                });

                start_time_psock.set_nonblocking(true).map_err(|e| {
                    format!("Failed to set nonblocking mode for start_time_psock: {e}")
                })?;
                let start_time_psock =
                    AsyncUnixStream::from_std(start_time_psock).map_err(|e| {
                        format!("Failed to create AsyncUnixStream from start_time_psock: {e}")
                    })?;

                end_time_psock.set_nonblocking(true).map_err(|e| {
                    format!("Failed to set nonblocking mode for end_time_psock: {e}")
                })?;
                let end_time_psock = AsyncUnixStream::from_std(end_time_psock).map_err(|e| {
                    format!("Failed to create AsyncUnixStream from end_time_psock: {e}")
                })?;

                let execution_time_ms = Arc::new(AtomicU64::new(0));
                let time_limit_exceeded = Arc::new(AtomicBool::new(false));

                {
                    let execution_time_ms = execution_time_ms.clone();
                    let limits = limits.clone();
                    let time_limit_exceeded = time_limit_exceeded.clone();
                    // TODO: Add timeout
                    tokio::spawn(async move {
                        wait_for_signal_async(start_time_psock)
                            .await
                            .expect("Failed to wait for start_time_psock");
                        let instant = Instant::now();

                        if let Some(time_ms) = limits.time_ms {
                            let deadline = instant + Duration::from_millis(time_ms);
                            tokio::select! {
                                result = wait_for_signal_async(end_time_psock) => {
                                    result.expect("Failed to wait for end_time_psock");
                                    execution_time_ms
                                        .store(instant.elapsed().as_millis() as u64, Ordering::Relaxed);
                                },
                                _ = tokio::time::sleep_until(deadline) => {
                                    time_limit_exceeded.store(true, Ordering::Relaxed);
                                    kill(Pid::from_raw(grandchild_pid), Signal::SIGKILL)
                                        .expect("Failed to kill grandchild after timeout");
                                }
                            }
                        } else {
                            wait_for_signal_async(end_time_psock)
                                .await
                                .expect("Failed to wait for end_time_psock");
                            execution_time_ms
                                .store(instant.elapsed().as_millis() as u64, Ordering::Relaxed);
                        }
                    });
                }

                // TODO: Handle errors from child
                let child_status = pidfd
                    .wait()
                    .await
                    .map_err(|e| format!("Failed to wait for child process: {e}"))?;

                stdin_task
                    .await
                    .map_err(|e| format!("Failed to wait for stdin: {e}"))?;

                grandchild_status_psock.set_nonblocking(true).map_err(|e| {
                    format!("Failed to set non-blocking mode for grandchild status pipe: {e}")
                })?;
                let mut grandchild_status_psock =
                    AsyncUnixStream::from_std(grandchild_status_psock).map_err(|e| {
                        format!("Failed to convert grandchild status pipe to async stream: {e}")
                    })?;
                let grandchild_status = self
                    .receive_wait_status(&mut grandchild_status_psock)
                    .await
                    .map_err(|e| {
                        format!("Failed to receive wait status from grandchild status pipe: {e}")
                    })?;

                peak_memory_usage_psock.set_nonblocking(true).map_err(|e| {
                    format!("Failed to set non-blocking mode for peak memory usage pipe: {e}")
                })?;
                let mut peak_memory_usage_psock =
                    AsyncUnixStream::from_std(peak_memory_usage_psock).map_err(|e| {
                        format!("Failed to convert peak memory usage pipe to async stream: {e}")
                    })?;
                let peak_memory_usage_bytes = peak_memory_usage_psock
                    .read_u64()
                    .await
                    .map_err(|e| format!("Failed to read peak memory usage: {e}"))?;

                let stdout = read_from_pipe(&mut stdout_read).await;
                let stderr = read_from_pipe(&mut stderr_read).await;
                let execution_time_ms = execution_time_ms.load(Ordering::Relaxed);

                if time_limit_exceeded.load(Ordering::Relaxed) {
                    return Err(RunError::LimitsExceeded {
                        result: RunResult {
                            status: -1,
                            stdout,
                            stderr,
                            execution_time_ms,
                            peak_memory_usage_bytes,
                        },
                        limit_type: TestLimitType::Time,
                    });
                }

                if let WaitStatus::Exited(_, status) = grandchild_status {
                    Ok(RunResult {
                        status,
                        stdout,
                        stderr,
                        execution_time_ms,
                        peak_memory_usage_bytes,
                    })
                } else {
                    Err(RunError::LimitsExceeded {
                        result: RunResult {
                            status: -1,
                            stdout,
                            stderr,
                            execution_time_ms,
                            peak_memory_usage_bytes,
                        },
                        limit_type: TestLimitType::Ram,
                    })
                }
            }
            ForkResult::Child => {
                std::panic::set_hook(Box::new(|panic_info| {
                    eprintln!("Panic occurred in child: {}", panic_info);
                    std::process::exit(100);
                }));

                unshare(CloneFlags::CLONE_NEWUSER).expect("Failed to unshare user namespace");
                send_signal_sync(&wait_user_ns_csock)
                    .expect("Failed to send user namespace signal");
                unshare(CloneFlags::CLONE_NEWPID).expect("Failed to unshare pid namespace");

                drop(stdin_write);
                drop(stdout_read);
                drop(stderr_read);

                match unsafe { fork().expect("Failed to second fork") } {
                    ForkResult::Parent { child: grandchild } => {
                        grandchild_pid_csock
                            .write_all(&grandchild.as_raw().to_be_bytes())
                            .expect("Failed to send grandchild pid");

                        let status = waitpid(grandchild, None).expect("Failed to waitpid in child");
                        send_signal_sync(&end_time_csock).expect("Failed to send end time signal");

                        let rusage =
                            getrusage(UsageWho::RUSAGE_CHILDREN).expect("Failed to getrusage");
                        let peak_memory_usage_bytes = (rusage.max_rss() * 1024) as u64;
                        peak_memory_usage_csock
                            .write_all(&peak_memory_usage_bytes.to_be_bytes())
                            .expect("Failed to send peak memory usage");

                        self.send_wait_status(&mut grandchild_status_csock, status)
                            .expect("Failed to send wait status");

                        std::process::exit(0);
                    }
                    ForkResult::Child => {
                        std::panic::set_hook(Box::new(|panic_info| {
                            eprintln!("Panic occurred in grandchild: {}", panic_info);
                            std::process::exit(101);
                        }));

                        wait_for_signal_sync(wait_cgroups_csock)
                            .expect("Failed to wait for cgroup signal");

                        unshare(
                            CloneFlags::CLONE_NEWNS
                                | CloneFlags::CLONE_NEWUTS
                                | CloneFlags::CLONE_NEWIPC
                                | CloneFlags::CLONE_NEWNET,
                        )
                        .expect("Failed to unshare namespaces");
                        self.setup_rootfs(artifact);

                        dup2_stdin(&stdin_read).expect("Failed to duplicate stdin");
                        drop(stdin_read);

                        dup2_stdout(&stdout_write).expect("Failed to duplicate stdout");
                        drop(stdout_write);

                        dup2_stderr(&stderr_write).expect("Failed to duplicate stderr");
                        drop(stderr_write);

                        // TODO: Move to const
                        let program = CString::new(CONTAINER_ARTIFACT_PATH)
                            .expect("Failed to create CString for program");

                        let args = vec![program.clone()];

                        let env = vec![
                            CString::new("PATH=/usr/local/bin:/usr/bin:/bin")
                                .expect("Failed to create CString for env"),
                        ];

                        send_signal_sync(&start_time_csock)
                            .expect("Failed to send start time signal");

                        execve(&program, &args, &env).expect("Failed to execve");
                        unreachable!()
                    }
                }
            }
        }
    }
}

async fn setup_id_mapping(child_pid: i32) -> anyhow::Result<()> {
    let current_uid = getuid();
    let current_gid = getgid();

    tokio::fs::write(format!("/proc/{child_pid}/setgroups"), "deny")
        .await
        .with_context(|| format!("Failed to write 'deny' to /proc/{child_pid}/set_groups"))?;

    tokio::fs::write(
        format!("/proc/{child_pid}/uid_map"),
        format!("0 {current_uid} 1"),
    )
    .await
    .with_context(|| format!("Failed to write uid map for child process {child_pid}"))?;

    tokio::fs::write(
        format!("/proc/{child_pid}/gid_map"),
        format!("0 {current_gid} 1"),
    )
    .await
    .with_context(|| format!("Failed to write gid map for child process {child_pid}"))?;

    Ok(())
}

async fn read_from_pipe(pipe: &mut tokio::fs::File) -> String {
    let mut buffer = Vec::new();
    match pipe.read_to_end(&mut buffer).await {
        Ok(bytes_read) if bytes_read > 0 => {
            String::from_utf8_lossy(&buffer[..bytes_read]).to_string()
        }
        _ => String::new(),
    }
}

fn send_signal_sync(mut sock: &SyncUnixStream) -> std::io::Result<()> {
    sock.write_all(&[1])?;
    Ok(())
}

async fn send_signal_async(sock: &mut AsyncUnixStream) -> std::io::Result<()> {
    sock.write_all(&[1]).await?;
    Ok(())
}

fn wait_for_signal_sync(mut sock: SyncUnixStream) -> std::io::Result<()> {
    let mut buf = [0u8; 1];
    sock.set_read_timeout(None)?;
    sock.read_exact(&mut buf)?;
    Ok(())
}

async fn wait_for_signal_async(mut sock: AsyncUnixStream) -> std::io::Result<()> {
    let mut buf = [0u8; 1];
    sock.read_exact(&mut buf).await?;
    Ok(())
}

impl From<String> for CompileError {
    fn from(err: String) -> Self {
        Self::Internal { msg: err }
    }
}

impl From<String> for RunError {
    fn from(err: String) -> Self {
        Self::Internal { msg: err }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum CrashReason {
    OomKiller,
    Timeout,
    Other,
}

#[bon::bon]
impl NativeExecutor {
    fn artifact_path(&self, artifact_id: &Uuid) -> PathBuf {
        self.dir.join(format!("{}.out", artifact_id))
    }

    #[builder(finish_fn = stdout)]
    async fn journal_logs(
        &self,
        scope_name: &str,
        max_retries: u32,
        init_delay_ms: u64,
    ) -> Result<(String, CrashReason), CompileError> {
        let mut delay = Duration::from_millis(init_delay_ms);

        for _ in 1..=(max_retries - 1) {
            let journal_out = Command::new(&self.journalctl_path)
                .arg("--user")
                .arg("--unit")
                .arg(&format!("{}.scope", scope_name))
                .arg("--no-pager")
                .output()
                .await
                .map_err(|e| CompileError::Internal { msg: e.to_string() })?;

            let logs = String::from_utf8_lossy(&journal_out.stdout).to_string();

            if logs.contains("Failed with result 'timeout'") {
                return Ok((logs, CrashReason::Timeout));
            }

            if logs.contains("Failed with result 'oom-kill'") {
                return Ok((logs, CrashReason::OomKiller));
            }

            tokio::time::sleep(delay).await;
            delay *= 2;
        }

        let journal_out = Command::new(&self.journalctl_path)
            .arg("--user")
            .arg("--unit")
            .arg(&format!("{}.scope", scope_name))
            .arg("--no-pager")
            .output()
            .await
            .map_err(|e| CompileError::Internal { msg: e.to_string() })?;

        return Ok((
            String::from_utf8_lossy(&journal_out.stdout).to_string(),
            CrashReason::Other,
        ));
    }

    fn setup_rootfs(&self, artifact: &Artifact) {
        // TODO: Create /proc, /sys, /dev and /tmp if not exists

        let overlay_upper_dir = self.dir.join(format!("overlayfs_{}/upper", artifact.id));
        let overlay_work_dir = self.dir.join(format!("overlayfs_{}/work", artifact.id));
        let overlay_merged_dir = self.dir.join(format!("overlayfs_{}/merged", artifact.id));

        std::fs::create_dir_all(&overlay_upper_dir)
            .expect("Failed to create overlayfs upper directory");
        std::fs::create_dir_all(&overlay_work_dir)
            .expect("Failed to create overlayfs work directory");
        std::fs::create_dir_all(&overlay_merged_dir)
            .expect("Failed to create overlayfs merged directory");

        let overlay_options = format!(
            "lowerdir={},upperdir={},workdir={},userxattr",
            self.rootfs.display(),
            overlay_upper_dir.display(),
            overlay_work_dir.display()
        );

        let mut delay = std::time::Duration::from_millis(100);
        for i in 1..=5 {
            let result = mount(
                Some("overlay"),
                &overlay_merged_dir,
                Some("overlay"),
                MsFlags::empty(),
                Some(overlay_options.as_str()),
            );

            if let Err(err) = result {
                if i == 5 {
                    panic!("Failed to mount overlayfs after 5 attempts: {err}");
                } else {
                    std::thread::sleep(delay);
                    delay *= 2;
                }
            }
        }

        mount(
            None::<&str>,
            "/",
            None::<&str>,
            MsFlags::MS_REC | MsFlags::MS_PRIVATE,
            None::<&str>,
        )
        .expect("Failed root make private and recursive");

        mount(
            Some(&overlay_merged_dir),
            &overlay_merged_dir,
            None::<&str>,
            MsFlags::MS_BIND,
            None::<&str>,
        )
        .expect("Failed to bind mount rootfs");

        chdir(&overlay_merged_dir).expect("Failed to change directory to rootfs");
        std::fs::create_dir_all(&overlay_merged_dir.join("old_root"))
            .expect("Failed to create old_root directory");
        pivot_root(".", "old_root").expect("Failed to pivot root");
        chdir("/").expect("Failed to change directory to root");

        mount(
            Some("proc"),
            "/proc",
            Some("proc"),
            MsFlags::empty(),
            None::<&str>,
        )
        .expect("Failed to mount /proc");

        mount(
            Some("sysfs"),
            "/sys",
            Some("sysfs"),
            MsFlags::empty(),
            None::<&str>,
        )
        .expect("Failed to mount /sys");

        mount(
            Some("tmpfs"),
            "/tmp",
            Some("tmpfs"),
            MsFlags::empty(),
            None::<&str>,
        )
        .expect("Failed to mount /tmp");

        mount(
            Some("tmpfs"),
            "/dev",
            Some("tmpfs"),
            MsFlags::empty(),
            Some("mode=755"),
        )
        .expect("Failed to mount /dev");

        let artifact_path = PathBuf::from("/old_root").join(
            self.artifact_path(&artifact.id)
                .to_str()
                .unwrap()
                .trim_start_matches('/'),
        );
        std::fs::copy(artifact_path, CONTAINER_ARTIFACT_PATH).expect("Failed to copy artifact");

        std::fs::set_permissions(
            CONTAINER_ARTIFACT_PATH,
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("Failed to set permissions");

        umount2("/old_root", MntFlags::MNT_DETACH).expect("Failed to umount /old_root");
    }

    #[builder(finish_fn = create)]
    async fn cgroups_scope(
        &self,
        artifact_id: &Uuid,
        pid: i32,
        limits: &ExecutionLimits,
    ) -> Result<String, RunError> {
        let scope_name = format!("container-{artifact_id}.scope");

        let props = [
            Some(("Delegate", true.into())),
            Some(("PIDs", vec![pid as u32].into())),
            limits
                .memory_bytes
                .map(|memory_max| ("MemoryMax", memory_max.into())),
            limits
                .pids_count
                .map(|pids_max| ("TasksMax", (pids_max as u64).into())),
            Some(("MemorySwapMax", 0u64.into())),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

        let mut job_removed_stream = self
            .systemd_manager
            .receive_job_removed()
            .await
            .map_err(|e| format!("cgroups: failed to receive job removed: {}", e))?;

        self.systemd_manager
            .start_transient_unit(&scope_name, "fail", &props, &[])
            .await
            .map_err(|e| format!("cgroups: failed to start transient unit: {}", e))?;

        while let Some(msg) = job_removed_stream.next().await {
            let args = msg
                .args()
                .map_err(|e| format!("cgroups: failed to parse job removed message: {}", e))?;

            if args.unit != scope_name {
                continue;
            }

            if args.result == "done" {
                return Ok(scope_name);
            }

            return Err(format!(
                "cgroups: failed to start transient unit with message: {}",
                msg.message()
            )
            .into());
        }

        Err(format!("cgroups: failed to start transient unit with no message").into())
    }

    // TODO: Use fixed buffer
    fn send_wait_status(
        &self,
        stream: &mut SyncUnixStream,
        status: WaitStatus,
    ) -> anyhow::Result<()> {
        let status = SerializableWaitStatus::from(status);
        let bytes = bincode::serde::encode_to_vec(status, self.bincode_config)?;
        stream.write_all(&bytes.len().to_le_bytes())?;
        stream.write_all(&bytes)?;
        stream.flush()?;

        Ok(())
    }

    // TODO: Use fixed buffer
    async fn receive_wait_status(
        &self,
        stream: &mut AsyncUnixStream,
    ) -> anyhow::Result<WaitStatus> {
        let len = stream.read_u64_le().await? as usize;
        let mut buffer = vec![0u8; len];
        stream.read_exact(&mut buffer).await?;

        let (status, _): (SerializableWaitStatus, _) =
            bincode::serde::decode_from_slice(&buffer, self.bincode_config)?;
        let status = WaitStatus::try_from(status)?;

        Ok(status)
    }
}

#[proxy(
    interface = "org.freedesktop.systemd1.Manager",
    gen_blocking = false,
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1"
)]
trait SystemdManager {
    #[zbus(name = "StartTransientUnit")]
    async fn start_transient_unit(
        &self,
        name: &str,
        mode: &str,
        properties: &[(&str, Value<'_>)],
        aux: &[(&str, &[(&str, Value<'_>)])],
    ) -> zbus::Result<OwnedObjectPath>;

    #[zbus(signal)]
    async fn job_removed(
        &self,
        id: u32,
        job: OwnedObjectPath,
        unit: String,
        result: String,
    ) -> zbus::Result<()>;
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SerializableWaitStatus {
    Exited(i32, i32),         // Pid и exit code
    Signaled(i32, i32, bool), // Pid, Signal, core_dump
    Stopped(i32, i32),        // Pid, Signal
    #[cfg(target_os = "linux")]
    PtraceEvent(i32, i32, c_int),
    #[cfg(target_os = "linux")]
    PtraceSyscall(i32),
    Continued(i32),
    StillAlive,
}

impl From<WaitStatus> for SerializableWaitStatus {
    fn from(status: WaitStatus) -> Self {
        match status {
            WaitStatus::Exited(pid, code) => SerializableWaitStatus::Exited(pid.as_raw(), code),
            WaitStatus::Signaled(pid, sig, core_dump) => {
                SerializableWaitStatus::Signaled(pid.as_raw(), sig as i32, core_dump)
            }
            WaitStatus::Stopped(pid, sig) => {
                SerializableWaitStatus::Stopped(pid.as_raw(), sig as i32)
            }
            #[cfg(target_os = "linux")]
            WaitStatus::PtraceEvent(pid, sig, event) => {
                SerializableWaitStatus::PtraceEvent(pid.as_raw(), sig as i32, event)
            }
            #[cfg(target_os = "linux")]
            WaitStatus::PtraceSyscall(pid) => SerializableWaitStatus::PtraceSyscall(pid.as_raw()),
            WaitStatus::Continued(pid) => SerializableWaitStatus::Continued(pid.as_raw()),
            WaitStatus::StillAlive => SerializableWaitStatus::StillAlive,
        }
    }
}

impl TryFrom<SerializableWaitStatus> for WaitStatus {
    type Error = anyhow::Error;

    fn try_from(status: SerializableWaitStatus) -> Result<Self, Self::Error> {
        match status {
            SerializableWaitStatus::Exited(pid, code) => {
                Ok(WaitStatus::Exited(Pid::from_raw(pid), code))
            }
            SerializableWaitStatus::Signaled(pid, sig, core_dump) => Ok(WaitStatus::Signaled(
                Pid::from_raw(pid),
                Signal::try_from(sig)?,
                core_dump,
            )),
            SerializableWaitStatus::Stopped(pid, sig) => Ok(WaitStatus::Stopped(
                Pid::from_raw(pid),
                Signal::try_from(sig)?,
            )),
            #[cfg(target_os = "linux")]
            SerializableWaitStatus::PtraceEvent(pid, sig, event) => Ok(WaitStatus::PtraceEvent(
                Pid::from_raw(pid),
                Signal::try_from(sig)?,
                event,
            )),
            #[cfg(target_os = "linux")]
            SerializableWaitStatus::PtraceSyscall(pid) => {
                Ok(WaitStatus::PtraceSyscall(Pid::from_raw(pid)))
            }
            SerializableWaitStatus::Continued(pid) => Ok(WaitStatus::Continued(Pid::from_raw(pid))),
            SerializableWaitStatus::StillAlive => Ok(WaitStatus::StillAlive),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use bon::builder;
    use tokio::process::Command;
    use uuid::Uuid;
    use yare::parameterized;

    use crate::{
        core::{
            domain::{
                Artifact, ArtifactKind, CompilationLimitType, CompilationLimits, ExecutionLimits,
                Language, TestLimitType,
            },
            traits::executor::{CompileError, Executor, RunError, RunResult},
        },
        native::executor::{NativeExecutor, SystemdManagerProxy},
    };

    #[tokio::test]
    async fn test_compile_success() {
        let (executor, executor_dir, _) = executor().use_glibc_compiler(true).create().await;

        let result = executor
            .compile(
                CORRECT_CODE,
                &Language::GnuCpp,
                &CompilationLimits::no_limits(),
            )
            .await;

        println!("result: {:?}", result);

        assert!(matches!(
            result,
            Ok(Artifact {
                kind: ArtifactKind::Executable,
                ..
            })
        ));

        let executable_path = executor_dir.join(format!("{}.out", result.unwrap().id));
        println!("Executable path: {}", executable_path.display());
        let out = Command::new(executable_path)
            .output()
            .await
            .expect("Failed to run command");

        println!("{:#?}", out);

        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        assert_eq!(stdout, "Hello, World!\n");
    }

    #[tokio::test]
    async fn test_compile_code_error() {
        let (executor, _, _) = executor().create().await;

        let result = executor
            .compile(
                INCORRECT_CODE,
                &Language::GnuCpp,
                &CompilationLimits::no_limits(),
            )
            .await;

        println!("result: {:#?}", result);
        assert!(matches!(
            result,
            Err(CompileError::CompilationFailed { .. })
        ));
    }

    #[parameterized(
        compiler_not_found = { true, false },
        filesystem_error = { false, true }
    )]
    #[test_macro(tokio::test)]
    async fn test_compile_internal_error(with_wrong_gnucpp: bool, with_readonly_dir: bool) {
        let (executor, _, _) = executor()
            .with_wrong_gnucpp(with_wrong_gnucpp)
            .with_readonly_dir(with_readonly_dir)
            .create()
            .await;

        let result = executor
            .compile(
                CORRECT_CODE,
                &Language::GnuCpp,
                &CompilationLimits::no_limits(),
            )
            .await;

        println!("{:#?}", result);
        assert!(matches!(result, Err(CompileError::Internal { .. })));
    }

    #[parameterized(
        time = {
            CompilationLimits::builder().time_ms(100).build(),
            CompilationLimitType::Time
        },
        ram = {
            CompilationLimits::builder().memory_bytes(1024 * 1024 * 5).build(),
            CompilationLimitType::Ram
        },
    )]
    #[test_macro(tokio::test)]
    async fn test_compile_limit_exceeded(
        limits: CompilationLimits,
        limit_type: CompilationLimitType,
    ) {
        let (executor, _, _) = executor().create().await;

        let actual = executor
            .compile(&massive_cpp_code().generate(), &Language::GnuCpp, &limits)
            .await;

        let expected = Err(CompileError::CompilationLimitsExceeded(limit_type));
        println!("result: {:#?}", actual);
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    #[ignore = "race condition stress test - run manually"]
    async fn test_compile_journalctl_race_condition() {
        let (executor, _, _) = executor().create().await;

        let total_runs = 1000;
        let mut missing_logs = 0;
        for _ in 0..total_runs {
            let result = executor
                .compile(
                    &massive_cpp_code().generate(),
                    &Language::GnuCpp,
                    &CompilationLimits::builder().memory_bytes(1024).build(),
                )
                .await;

            let correct = matches!(
                result,
                Err(CompileError::CompilationLimitsExceeded(
                    CompilationLimitType::Ram
                ))
            );

            if !correct {
                missing_logs += 1;
            }
        }

        println!("Total runs: {}, missing logs: {}", total_runs, missing_logs);
        assert_eq!(missing_logs, 0);
    }

    #[parameterized(
        success = { 0, "Hello, world!", "" },
        error = { 1, "Aboba", "Aboba" }
    )]
    #[test_macro(tokio::test)]
    async fn test_run_success(expected_status: i32, expected_stdout: &str, expected_stderr: &str) {
        let (executor, _, _) = executor().create().await;
        let artifact = cpp_code()
            .status(expected_status)
            .stdout(expected_stdout)
            .stderr(expected_stderr)
            .compile(&executor)
            .await;

        let result = tokio::time::timeout(
            Duration::from_secs(20),
            executor.run(&artifact, "", &ExecutionLimits::no_limits()),
        )
        .await;
        println!("result: {:#?}", result);

        assert!(matches!(
            result,
            Ok(Ok(RunResult { status, ref stdout, ref stderr, .. }))
            if status == expected_status && stdout == expected_stdout && stderr == expected_stderr
        ));
    }

    #[tokio::test]
    async fn test_run_stdin() {
        let (executor, _, _) = executor().create().await;
        let artifact = cpp_stdin_repeater().count(3).compile(&executor).await;

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            executor.run(&artifact, "Aboba", &ExecutionLimits::no_limits()),
        )
        .await;

        println!("result: {:#?}", result);

        assert!(matches!(
            result,
            Ok(Ok(RunResult { ref stdout, .. }))
            if stdout == "Aboba\nAboba\nAboba\n"
        ))
    }

    #[tokio::test]
    async fn test_run_execution_time() {
        let (executor, _, _) = executor().create().await;
        let artifact = cpp_sleep().time_ms(300).compile(&executor).await;

        let result = tokio::time::timeout(
            Duration::from_secs(20),
            executor.run(&artifact, "", &ExecutionLimits::no_limits()),
        )
        .await;

        println!("result: {:#?}", result);

        const ACCURACY_MS: u64 = 30;
        assert!(matches!(
            result,
            Ok(Ok(RunResult { execution_time_ms, .. }))
            if 300 - ACCURACY_MS <= execution_time_ms &&
               execution_time_ms <= 300 + ACCURACY_MS
        ));
    }

    #[tokio::test]
    async fn test_run_peak_memory_usage() {
        let (executor, artifact, _) = executor_with_testbin("peak_memory_usage").await;
        let result = executor
            .run(&artifact, "", &ExecutionLimits::no_limits())
            .await;
        println!("result: {:#?}", result);

        const ACCURACY_MB: u64 = 8;
        assert!(matches!(
            result,
            Ok(RunResult { peak_memory_usage_bytes, .. })
            if (64 - ACCURACY_MB) * 1024 * 1024 <= peak_memory_usage_bytes &&
               peak_memory_usage_bytes <= (64 + ACCURACY_MB) * 1024 * 1024
        ));
    }

    #[tokio::test]
    async fn test_run_artifact_not_found() {
        let (executor, _, _) = executor().create().await;
        let artifact = Artifact {
            id: Uuid::new_v4(),
            kind: ArtifactKind::Executable,
        };

        let result = executor
            .run(&artifact, "", &ExecutionLimits::no_limits())
            .await;

        println!("result: {:#?}", result);

        // TODO: Add ArtifactNotFound error variant
        assert!(matches!(result, Err(RunError::Internal { .. })));
    }

    #[parameterized(
        pid = { "process_isolation" },
        net = { "net_isolation" },
        localhost = { "localhost_isolation" },
    )]
    #[test_macro(tokio::test)]
    async fn test_run_isolation(testbin: &str) {
        let (executor, artifact, _) = executor_with_testbin(testbin).await;
        let result = executor
            .run(&artifact, "", &ExecutionLimits::no_limits())
            .await;
        println!("result: {:#?}", result);

        assert!(matches!(result, Ok(RunResult { status: 0, .. })));
    }

    #[tokio::test]
    async fn test_run_reading_file_isolation() {
        let secret_file = PathBuf::from("/tmp").join("top-top-top-secret.txt");
        tokio::fs::write(&secret_file, "secret data").await.unwrap();

        let (executor, artifact, _) = executor_with_testbin("file_isolation").await;
        let result = executor
            .run(&artifact, "", &ExecutionLimits::no_limits())
            .await;
        println!("result: {:#?}", result);

        assert!(matches!(result, Ok(RunResult { status: 0, .. })));

        tokio::fs::remove_file(&secret_file).await.unwrap();
    }

    #[tokio::test]
    async fn test_run_writing_file_isolation() {
        let (executor, artifact, rootfs_path) = executor_with_testbin("file_writer").await;
        let result = executor
            .run(&artifact, "", &ExecutionLimits::no_limits())
            .await;
        println!("result: {:#?}", result);

        let file_on_host_exists = tokio::fs::try_exists(rootfs_path.join("hello"))
            .await
            .unwrap();

        if file_on_host_exists {
            tokio::fs::remove_file(rootfs_path.join("hello"))
                .await
                .unwrap();
        }

        assert!(!file_on_host_exists);
    }

    #[parameterized(
        time = {
            "time_limit",
            &ExecutionLimits::builder()
                .time_ms(300)
                .build(),
            TestLimitType::Time,
        },
        mem = {
            "memory_limit",
            &ExecutionLimits::builder()
                .memory_bytes(5 * 1024 * 1024) // 5 MB
                .build(),
            TestLimitType::Ram,
        }
    )]
    #[test_macro(tokio::test)]
    async fn test_run_limit_exceeded(
        testbin: &str,
        limits: &ExecutionLimits,
        expected_limit_type: TestLimitType,
    ) {
        let (executor, artifact, _) = executor_with_testbin(testbin).await;
        let result = executor.run(&artifact, "", limits).await;
        println!("result: {:#?}", result);

        assert!(matches!(
            result,
            Err(RunError::LimitsExceeded { limit_type, .. })
            if limit_type == expected_limit_type
        ));
    }

    #[tokio::test]
    async fn test_run_pids_limit_exceeded() {
        let (executor, artifact, _) = executor_with_testbin("pids_limit").await;
        let result = executor
            .run(
                &artifact,
                "",
                &ExecutionLimits::builder().pids_count(10).build(),
            )
            .await;
        println!("result: {:#?}", result);

        assert!(matches!(
            result,
            Ok(RunResult { ref stderr, .. })
            if stderr.contains("Failed to spawn child")
        ));
    }

    const CORRECT_CODE: &str = "
            #include <iostream>
            int main() {
                std::cout << \"Hello, World!\" << std::endl;
                return 0;
            }";

    const INCORRECT_CODE: &str = "
            #include <iostream>
            int main() {
                std::cout << \"Hello, World!\" << std::endl
                return 0;
            }";

    #[builder(finish_fn = generate)]
    fn massive_cpp_code(
        #[builder(default = 15000)] num_functions: usize,
        #[builder(default = 12000)] num_variables: usize,
        #[builder(default = 8000)] num_structs: usize,
        #[builder(default = 200)] usage_density: usize,
    ) -> String {
        let mut code = String::new();

        code.push_str("#include <iostream>\n");
        code.push_str("#include <vector>\n\n");

        code.push_str("// Макрос для создания простых функций\n");
        code.push_str("#define GENERATE_FUNCTION(n) \\\n");
        code.push_str("    int function_##n() { \\\n");
        code.push_str("        return n * 2 + 1; \\\n");
        code.push_str("    }\n\n");

        code.push_str("#define GENERATE_VARIABLE(n) \\\n");
        code.push_str("    const int var_##n = n * 3;\n\n");

        code.push_str("#define GENERATE_STRUCT(n) \\\n");
        code.push_str("    struct Struct_##n { \\\n");
        code.push_str("        int value = n; \\\n");
        code.push_str("        int getValue() const { return value; } \\\n");
        code.push_str("    };\n\n");

        if num_functions > 0 {
            code.push_str(&format!("// Генерация {} функций\n", num_functions));
            for i in 0..num_functions {
                code.push_str(&format!("GENERATE_FUNCTION({})\n", i));
            }
        }

        if num_variables > 0 {
            code.push_str(&format!("\n// Генерация {} переменных\n", num_variables));
            for i in 0..num_variables {
                code.push_str(&format!("GENERATE_VARIABLE({})\n", i));
            }
        }

        if num_structs > 0 {
            code.push_str(&format!("\n// Генерация {} структур\n", num_structs));
            for i in 0..num_structs {
                code.push_str(&format!("GENERATE_STRUCT({})\n", i));
            }
        }

        code.push_str("\nint main() {\n");
        code.push_str("    std::cout << \"Starting massive code execution...\\n\";\n");

        if num_functions > 0 && usage_density > 0 {
            code.push_str("    \n    // Массовые вызовы функций\n");
            for i in (0..num_functions).step_by(usage_density) {
                code.push_str(&format!("    volatile int result_{} = ", i));
                let calls_per_line = std::cmp::min(10, usage_density);
                for j in 0..calls_per_line {
                    if i + j < num_functions {
                        code.push_str(&format!("function_{}() + ", i + j));
                    }
                }
                code.push_str("0;\n");
            }
        }

        if num_variables > 0 && usage_density > 0 {
            code.push_str("    \n    // Использование переменных\n");
            code.push_str("    volatile int sum = ");
            for i in (0..num_variables).step_by(usage_density) {
                code.push_str(&format!("var_{} + ", i));
                if i > 0 && i % 20 == 0 {
                    code.push_str("\n        ");
                }
            }
            code.push_str("0;\n");
        }

        if num_structs > 0 && usage_density > 0 {
            code.push_str("    \n    // Создание экземпляров структур\n");
            for i in (0..num_structs).step_by(usage_density) {
                code.push_str(&format!("    Struct_{} obj_{};\n", i, i));
            }
        }

        code.push_str("    \n    std::cout << \"Code execution completed!\\n\";\n");
        code.push_str("    return 0;\n");
        code.push_str("}\n");

        code
    }

    #[builder(finish_fn = compile)]
    async fn cpp_code(
        #[builder(finish_fn)] executor: &NativeExecutor,
        status: i32,
        stdout: &str,
        stderr: &str,
    ) -> Artifact {
        let mut code = String::new();
        code.push_str("#include <iostream>\n");
        code.push_str("int main() {\n");
        code.push_str(&format!("  std::cout << \"{}\";\n", stdout));
        code.push_str(&format!("  std::cerr << \"{}\";\n", stderr));
        code.push_str(&format!("  return {};\n", status));
        code.push_str("}\n");

        println!("code:\n{}", code);

        executor
            .compile(&code, &Language::GnuCpp, &CompilationLimits::no_limits())
            .await
            .unwrap()
    }

    #[builder(finish_fn = compile)]
    async fn cpp_stdin_repeater(
        #[builder(finish_fn)] executor: &NativeExecutor,
        count: u32,
    ) -> Artifact {
        let mut code = String::new();
        code.push_str("#include <iostream>\n");
        code.push_str("#include <string>\n");
        code.push_str("int main() {\n");
        code.push_str("  std::string input;\n");
        code.push_str("  std::getline(std::cin, input);\n");
        code.push_str(&format!(
            "  for (int i = 0; i < {}; i++) {{ std::cout << input << std::endl; }}",
            count
        ));
        code.push_str("}\n");

        println!("code:\n{}", code);

        executor
            .compile(&code, &Language::GnuCpp, &CompilationLimits::no_limits())
            .await
            .unwrap()
    }

    #[builder(finish_fn = compile)]
    async fn cpp_sleep(#[builder(finish_fn)] executor: &NativeExecutor, time_ms: u32) -> Artifact {
        let mut code = String::new();
        code.push_str("#include <iostream>\n");
        code.push_str("#include <chrono>\n");
        code.push_str("#include <thread>\n");
        code.push_str("int main() {\n");
        code.push_str(&format!(
            "  std::this_thread::sleep_for(std::chrono::milliseconds({}));\n",
            time_ms
        ));
        code.push_str("}\n");

        println!("code:\n{}", code);

        executor
            .compile(&code, &Language::GnuCpp, &CompilationLimits::no_limits())
            .await
            .unwrap()
    }

    #[builder(finish_fn = create)]
    async fn executor(
        #[builder(default = false)] use_glibc_compiler: bool,
        #[builder(default = false)] with_readonly_dir: bool,
        #[builder(default = false)] with_wrong_gnucpp: bool,
    ) -> (NativeExecutor, PathBuf, PathBuf) {
        let executor_dir = if with_readonly_dir {
            format!("/proc/coderunner_{}", Uuid::new_v4())
        } else {
            std::env::var("EXECUTOR_DIR")
                .unwrap_or_else(|_| format!("/tmp/coderunner_{}", Uuid::new_v4()))
        };

        let cross_compmiler_path =
            tokio::fs::canonicalize("./musl-cross-compiler/bin/x86_64-linux-musl-g++")
                .await
                .unwrap()
                .to_string_lossy()
                .to_string();

        println!("Cross compiler path: {}", cross_compmiler_path);

        let rootfs_path = tokio::fs::canonicalize("./alpine-rootfs")
            .await
            .unwrap()
            .to_string_lossy()
            .to_string();
        let rootfs_path = std::env::var("ROOTFS").unwrap_or(rootfs_path);

        let zbus_conn = zbus::Connection::session().await.unwrap();
        let executor = NativeExecutor::builder()
            .dir(executor_dir.clone())
            .gnucpp_path(if with_wrong_gnucpp {
                "/aboba".to_string()
            } else if use_glibc_compiler {
                std::env::var("GNUCPP_GLIBC_PATH").unwrap_or("/usr/bin/g++".to_string())
            } else {
                std::env::var("GNUCPP_MUSL_PATH").unwrap_or(cross_compmiler_path)
            })
            .rootfs(&rootfs_path)
            .systemd_run_path(
                std::env::var("SYSTEMD_RUN_PATH").unwrap_or("/usr/bin/systemd-run".to_string()),
            )
            .journalctl_path(
                std::env::var("JOURNALCTL_PATH").unwrap_or("/usr/bin/journalctl".to_string()),
            )
            .static_linking(!use_glibc_compiler)
            .systemd_manager(SystemdManagerProxy::new(&zbus_conn).await.unwrap())
            .build();

        (executor, executor_dir.into(), rootfs_path.into())
    }

    async fn executor_with_testbin(testbin: &str) -> (NativeExecutor, Artifact, PathBuf) {
        let (executor, executor_dir, rootfs_path) = executor().create().await;
        let artifact = Artifact {
            id: Uuid::new_v4(),
            kind: ArtifactKind::Executable,
        };

        let testbin_dir = tokio::fs::canonicalize("./testbins")
            .await
            .unwrap()
            .join(testbin);

        let compilation_output = Command::new("cargo")
            .arg("build")
            .arg("--manifest-path")
            .arg(testbin_dir.join("Cargo.toml"))
            .arg("--release")
            .arg("--target")
            .arg("x86_64-unknown-linux-musl")
            .output()
            .await
            .unwrap();

        if !compilation_output.status.success() {
            panic!(
                "Failed to compile test binary\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&compilation_output.stdout),
                String::from_utf8_lossy(&compilation_output.stderr)
            );
        }

        let testbin_path = testbin_dir
            .join("target/x86_64-unknown-linux-musl/release")
            .join(testbin);

        tokio::fs::create_dir_all(&executor_dir).await.unwrap();
        tokio::fs::copy(
            testbin_path,
            executor_dir.join(&format!("{}.out", artifact.id)),
        )
        .await
        .unwrap();

        (executor, artifact, rootfs_path)
    }
}
