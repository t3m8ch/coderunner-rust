use std::{
    ffi::CString,
    io::{Read, Write},
    os::{
        fd::{FromRawFd, IntoRawFd},
        unix::{fs::PermissionsExt, net::UnixStream as SyncUnixStream},
    },
    path::PathBuf,
    time::Duration,
};

use anyhow::Context;
use async_pidfd::AsyncPidFd;
use bon::{Builder, builder};
use nix::{
    mount::{MntFlags, MsFlags, mount, umount2},
    sched::{CloneFlags, unshare},
    sys::wait::{WaitStatus, waitpid},
    unistd::{
        ForkResult, chdir, dup2_stderr, dup2_stdin, dup2_stdout, execve, fork, getgid, getuid,
        pipe, pivot_root,
    },
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream as AsyncUnixStream,
    process::Command,
    time::Instant,
};
use uuid::Uuid;

use crate::core::{
    domain::{
        Artifact, ArtifactKind, CompilationLimitType, CompilationLimits, ExecutionLimits, Language,
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

        let cmd = if let Some(executable_size_bytes) = limits.executable_size_bytes {
            vec![
                "sh".to_string(),
                "-c".to_string(),
                if self.static_linking {
                    format!(
                        "ulimit -Sf {}; {} -static -o {} {}",
                        executable_size_bytes / 512,
                        &gnucpp_path,
                        &artifact_path,
                        &source_path
                    )
                } else {
                    format!(
                        "ulimit -Sf {}; {} -o {} {}",
                        executable_size_bytes / 512,
                        &gnucpp_path,
                        &artifact_path,
                        &source_path
                    )
                },
            ]
        } else {
            let mut args = vec![gnucpp_path, "-o".to_string(), artifact_path, source_path];
            if self.static_linking {
                args.push("-static".to_string());
            }
            args
        };

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
            let journal_stdout = self
                .journal_logs()
                .init_delay_ms(10)
                .max_retries(5)
                .scope_name(&scope_name)
                .stdout()
                .await?;

            if journal_stdout.contains("Failed with result 'timeout'") {
                return Err(CompileError::CompilationLimitsExceeded(
                    CompilationLimitType::Time,
                ));
            }

            if journal_stdout.contains("Failed with result 'oom-kill'") {
                return Err(CompileError::CompilationLimitsExceeded(
                    CompilationLimitType::Ram,
                ));
            }

            if journal_stdout.contains("dumped core") {
                return Err(CompileError::CompilationLimitsExceeded(
                    CompilationLimitType::ExecutableSize,
                ));
            }

            let msg = format!(
                "Journal: {}, stdout: {}, stderr: {}",
                journal_stdout,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );

            if journal_stdout.contains("Started") {
                return Err(CompileError::CompilationFailed { msg });
            }

            return Err(CompileError::Internal { msg });
        }

        Ok(Artifact {
            id: artifact_id,
            kind: ArtifactKind::Executable,
        })
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
                // TODO: Think about little-endian encoding
                let grandchild_pid = grandchild_pid_psock.read_i32_le().await.map_err(|e| {
                    format!("Failed to read grandchild_pid from grandchild_pid_psock: {e}")
                })?;

                // TODO: Setup cgroups here!

                let start = Instant::now();
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

                // TODO: Handle errors from child
                let child_status = pidfd
                    .wait()
                    .await
                    .map_err(|e| format!("Failed to wait for child process: {e}"))?;
                let duration = start.elapsed();

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
                let grandchild_status = grandchild_status_psock
                    .read_i32_le()
                    .await
                    .map_err(|e| format!("Failed to read grandchild status: {e}"))?;

                let stdout = read_from_pipe(&mut stdout_read).await;
                let stderr = read_from_pipe(&mut stderr_read).await;
                let execution_time_ms = duration.as_millis() as u64;

                Ok(RunResult {
                    status: grandchild_status,
                    stdout,
                    stderr,
                    execution_time_ms,
                    peak_memory_usage_bytes: 0,
                })
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
                        // TODO: Think about little-endian encoding
                        grandchild_pid_csock
                            .write_all(&grandchild.as_raw().to_le_bytes())
                            .expect("Failed to send grandchild pid");

                        // TODO: Change to wait4 for memory peak usage
                        let status = waitpid(grandchild, None).expect("Failed to waitpid in child");
                        grandchild_status_csock
                            .write_all(&match status {
                                WaitStatus::Exited(_, status) => status.to_le_bytes(),
                                _ => todo!("Serialize WaitStatus and send it"),
                            })
                            .expect("Failed to send grandchild status");

                        std::process::exit(0);
                    }
                    ForkResult::Child => {
                        std::panic::set_hook(Box::new(|panic_info| {
                            eprintln!("Panic occurred in grandchild: {}", panic_info);
                            std::process::exit(101);
                        }));

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

                        execve(&program, &args, &env).expect("Failed to execve");
                        unreachable!()
                    }
                }
            }
        }
    }
}

impl NativeExecutor {
    fn setup_rootfs(&self, artifact: &Artifact) {
        // TODO: Create /proc, /sys, /dev and /tmp if not exists

        mount(
            None::<&str>,
            "/",
            None::<&str>,
            MsFlags::MS_REC | MsFlags::MS_PRIVATE,
            None::<&str>,
        )
        .expect("Failed root make private and recursive");

        mount(
            Some(&self.rootfs),
            &self.rootfs,
            None::<&str>,
            MsFlags::MS_BIND,
            None::<&str>,
        )
        .expect("Failed to bind mount rootfs");

        chdir(&self.rootfs).expect("Failed to change directory to rootfs");
        std::fs::create_dir_all(&self.rootfs.join("old_root"))
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
    ) -> Result<String, CompileError> {
        let mut delay = Duration::from_millis(init_delay_ms);

        for attempt in 1..=(max_retries - 1) {
            let journal_out = Command::new(&self.journalctl_path)
                .arg("--user")
                .arg("--unit")
                .arg(&format!("{}.scope", scope_name))
                .arg("--no-pager")
                .output()
                .await
                .map_err(|e| CompileError::Internal { msg: e.to_string() })?;

            let logs = String::from_utf8_lossy(&journal_out.stdout).to_string();

            if logs.contains("Failed with result 'timeout'")
                || logs.contains("Failed with result 'oom-kill'")
                || logs.contains("dumped core")
            {
                return Ok(logs);
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

        return Ok(String::from_utf8_lossy(&journal_out.stdout).to_string());
    }
}

// TODO: Write tests with compiling limits
// TODO: Create Dockerfile for executor testing with fixed g++ and filesystem

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use bon::builder;
    use futures::future::join_all;
    use tokio::process::Command;
    use uuid::Uuid;

    use crate::{
        core::{
            domain::{
                Artifact, ArtifactKind, CompilationLimitType, CompilationLimits, ExecutionLimits,
                Language,
            },
            traits::executor::{CompileError, Executor, RunError, RunResult},
        },
        native::executor::NativeExecutor,
    };

    #[tokio::test]
    async fn test_compile_success() {
        let (executor, executor_dir) = executor().use_glibc_compiler(true).create().await;

        let result = executor
            .compile(
                CORRECT_CODE,
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: None,
                    executable_size_bytes: None,
                },
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
        let (executor, _) = executor().create().await;

        let result = executor
            .compile(
                INCORRECT_CODE,
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: None,
                    executable_size_bytes: None,
                },
            )
            .await;

        println!("result: {:#?}", result);
        assert!(matches!(
            result,
            Err(CompileError::CompilationFailed { .. })
        ));
    }

    #[tokio::test]
    async fn test_compile_compiler_not_found() {
        let (executor, _) = executor().with_wrong_gnucpp(true).create().await;

        let result = executor
            .compile(
                CORRECT_CODE,
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: None,
                    executable_size_bytes: None,
                },
            )
            .await;

        println!("{:#?}", result);

        assert!(matches!(result, Err(CompileError::Internal { .. })));
    }

    #[tokio::test]
    async fn test_compile_filesystem_error() {
        let (executor, _) = executor().with_readonly_dir(true).create().await;

        let result = executor
            .compile(
                CORRECT_CODE,
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: None,
                    executable_size_bytes: None,
                },
            )
            .await;

        assert!(matches!(result, Err(CompileError::Internal { .. })));
    }

    #[tokio::test]
    async fn test_compile_ram_limit_exceeded() {
        let (executor, _) = executor().create().await;

        let result = executor
            .compile(
                &massive_cpp_code().generate(),
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: Some(1024 * 1024 * 5), // 5 MB
                    executable_size_bytes: None,
                },
            )
            .await;

        println!("result: {:#?}", result);

        assert!(matches!(
            result,
            Err(CompileError::CompilationLimitsExceeded(
                CompilationLimitType::Ram
            ))
        ));
    }

    #[tokio::test]
    async fn test_compile_time_limit_exceeded() {
        let (executor, _) = executor().create().await;

        let result = executor
            .compile(
                &massive_cpp_code().generate(),
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: Some(100),
                    memory_bytes: None,
                    executable_size_bytes: None,
                },
            )
            .await;

        println!("result: {:#?}", result);

        assert!(matches!(
            result,
            Err(CompileError::CompilationLimitsExceeded(
                CompilationLimitType::Time
            ))
        ));
    }

    #[tokio::test]
    async fn test_compile_executable_size_limit_exceeded() {
        let (executor, _) = executor().create().await;

        let result = executor
            .compile(
                &massive_cpp_code().generate(),
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: None,
                    executable_size_bytes: Some(1024 * 512), // 512 KB,
                },
            )
            .await;

        println!("result: {:#?}", result);

        assert!(matches!(
            result,
            Err(CompileError::CompilationLimitsExceeded(
                CompilationLimitType::ExecutableSize
            ))
        ));
    }

    #[tokio::test]
    #[ignore = "race condition stress test - run manually"]
    async fn test_compile_journalctl_race_condition() {
        let (executor, _) = executor().create().await;

        let total_runs = 1000;
        let mut missing_logs = 0;
        for _ in 0..total_runs {
            let result = executor
                .compile(
                    &massive_cpp_code().generate(),
                    &Language::GnuCpp,
                    &CompilationLimits {
                        time_ms: None,
                        memory_bytes: Some(1024),
                        executable_size_bytes: None,
                    },
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

    #[tokio::test]
    async fn test_run_success() {
        let (executor, _) = executor().create().await;
        let artifact = vec![
            cpp_code()
                .status(0)
                .stdout("Hello, world!")
                .stderr("")
                .compile(&executor)
                .await,
            cpp_code()
                .status(1)
                .stdout("Aboba")
                .stderr("Aboba")
                .compile(&executor)
                .await,
        ];

        let result = join_all(artifact.iter().map(|a| {
            tokio::time::timeout(
                Duration::from_secs(20),
                executor.run(
                    a,
                    "",
                    &ExecutionLimits {
                        time_ms: None,
                        memory_bytes: None,
                        pids_count: None,
                        stdout_size_bytes: None,
                        stderr_size_bytes: None,
                    },
                ),
            )
        }))
        .await;

        println!("result: {:#?}", result);

        assert!(matches!(
            result[0],
            Ok(Ok(RunResult { status: 0, ref stdout, ref stderr, .. }))
            if stdout == "Hello, world!" && stderr == ""
        ));

        assert!(matches!(
            result[1],
            Ok(Ok(RunResult { status: 1, ref stdout, ref stderr, .. }))
            if stdout == "Aboba" && stderr == "Aboba"
        ));
    }

    #[tokio::test]
    async fn test_run_stdin() {
        let (executor, _) = executor().create().await;
        let artifact = cpp_stdin_repeater().count(3).compile(&executor).await;

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            executor.run(
                &artifact,
                "Aboba",
                &ExecutionLimits {
                    time_ms: None,
                    memory_bytes: None,
                    pids_count: None,
                    stdout_size_bytes: None,
                    stderr_size_bytes: None,
                },
            ),
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
        let (executor, _) = executor().create().await;
        let artifact = cpp_sleep().time_ms(300).compile(&executor).await;

        let result = tokio::time::timeout(
            Duration::from_secs(20),
            executor.run(
                &artifact,
                "",
                &ExecutionLimits {
                    time_ms: None,
                    memory_bytes: None,
                    pids_count: None,
                    stdout_size_bytes: None,
                    stderr_size_bytes: None,
                },
            ),
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
    async fn test_run_artifact_not_found() {
        let (executor, _) = executor().create().await;
        let artifact = Artifact {
            id: Uuid::new_v4(),
            kind: ArtifactKind::Executable,
        };

        let result = executor
            .run(
                &artifact,
                "",
                &ExecutionLimits {
                    time_ms: None,
                    memory_bytes: None,
                    pids_count: None,
                    stdout_size_bytes: None,
                    stderr_size_bytes: None,
                },
            )
            .await;

        println!("result: {:#?}", result);

        // TODO: Add ArtifactNotFound error variant
        assert!(matches!(result, Err(RunError::Internal { .. })));
    }

    #[tokio::test]
    async fn test_run_process_isolation() {
        let (executor, artifact) = executor_with_testbin("process_isolation").await;
        let result = executor
            .run(
                &artifact,
                "",
                &ExecutionLimits {
                    time_ms: None,
                    memory_bytes: None,
                    pids_count: None,
                    stdout_size_bytes: None,
                    stderr_size_bytes: None,
                },
            )
            .await;
        println!("result: {:#?}", result);

        assert!(matches!(result, Ok(RunResult { status: 0, .. })));
    }

    #[tokio::test]
    async fn test_run_file_isolation() {
        let secret_file = PathBuf::from("/tmp").join("secret.txt");
        tokio::fs::write(&secret_file, "secret data").await.unwrap();

        let (executor, artifact) = executor_with_testbin("file_isolation").await;
        let result = executor
            .run(
                &artifact,
                "",
                &ExecutionLimits {
                    time_ms: None,
                    memory_bytes: None,
                    pids_count: None,
                    stdout_size_bytes: None,
                    stderr_size_bytes: None,
                },
            )
            .await;
        println!("result: {:#?}", result);

        assert!(matches!(result, Ok(RunResult { status: 0, .. })));
    }

    #[tokio::test]
    async fn test_run_net_isolation() {
        let (executor, artifact) = executor_with_testbin("net_isolation").await;
        let result = executor
            .run(
                &artifact,
                "",
                &ExecutionLimits {
                    time_ms: None,
                    memory_bytes: None,
                    pids_count: None,
                    stdout_size_bytes: None,
                    stderr_size_bytes: None,
                },
            )
            .await;
        println!("result: {:#?}", result);

        assert!(matches!(result, Ok(RunResult { status: 0, .. })));
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
            .compile(
                &code,
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: None,
                    executable_size_bytes: None,
                },
            )
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
            .compile(
                &code,
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: None,
                    executable_size_bytes: None,
                },
            )
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
            .compile(
                &code,
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: None,
                    executable_size_bytes: None,
                },
            )
            .await
            .unwrap()
    }

    #[builder(finish_fn = create)]
    async fn executor(
        #[builder(default = false)] use_glibc_compiler: bool,
        #[builder(default = false)] with_readonly_dir: bool,
        #[builder(default = false)] with_wrong_gnucpp: bool,
    ) -> (NativeExecutor, PathBuf) {
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

        let executor = NativeExecutor::builder()
            .dir(executor_dir.clone())
            .gnucpp_path(if with_wrong_gnucpp {
                "/aboba".to_string()
            } else if use_glibc_compiler {
                std::env::var("GNUCPP_GLIBC_PATH").unwrap_or("/usr/bin/g++".to_string())
            } else {
                std::env::var("GNUCPP_MUSL_PATH").unwrap_or(cross_compmiler_path)
            })
            .rootfs(std::env::var("ROOTFS").unwrap_or(rootfs_path))
            .systemd_run_path(
                std::env::var("SYSTEMD_RUN_PATH").unwrap_or("/usr/bin/systemd-run".to_string()),
            )
            .journalctl_path(
                std::env::var("JOURNALCTL_PATH").unwrap_or("/usr/bin/journalctl".to_string()),
            )
            .static_linking(!use_glibc_compiler)
            .build();

        (executor, executor_dir.into())
    }

    async fn executor_with_testbin(testbin: &str) -> (NativeExecutor, Artifact) {
        let (executor, executor_dir) = executor().create().await;
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

        (executor, artifact)
    }
}
