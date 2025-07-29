use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{Duration, Instant, timeout};

use crate::domain::{Artifact, ExecutionLimits, TestLimitType};
use crate::runner::traits::{Runner, RunnerError, RunnerResult};

#[derive(Debug)]
pub struct BasicRunner {
    temp_dir: PathBuf,
}

impl BasicRunner {
    pub fn new() -> std::io::Result<Self> {
        let temp_dir = std::env::temp_dir().join("coderunner");
        std::fs::create_dir_all(&temp_dir)?;
        Ok(Self { temp_dir })
    }

    fn get_executable_path(&self, artifact: &Artifact) -> Option<PathBuf> {
        // Look for the artifact in the base temp directory first
        let direct_path = self.temp_dir.join(format!("{}", artifact.id));
        if direct_path.exists() {
            return Some(direct_path);
        }

        // Search in compiler subdirectories
        if let Ok(entries) = std::fs::read_dir(&self.temp_dir) {
            for entry in entries.flatten() {
                if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    let artifact_path = entry.path().join(format!("{}", artifact.id));
                    if artifact_path.exists() {
                        return Some(artifact_path);
                    }
                }
            }
        }

        None
    }

    async fn monitor_process_resources(
        &self,
        pid: u32,
        limits: &ExecutionLimits,
    ) -> (u64, bool, Option<TestLimitType>) {
        let mut peak_memory = 0u64;
        let mut limit_exceeded = false;
        let mut limit_type = None;

        // Simple resource monitoring - in a real implementation you'd want more sophisticated monitoring
        // For now, we'll do basic checks

        // Check memory limit if specified
        if let Some(memory_limit) = limits.memory_bytes {
            // Get current memory usage (simplified - in reality you'd read from /proc/[pid]/status)
            // For this basic implementation, we'll simulate memory monitoring
            let current_memory = self.get_process_memory_usage(pid).await.unwrap_or(0);
            peak_memory = peak_memory.max(current_memory);

            if current_memory > memory_limit {
                limit_exceeded = true;
                limit_type = Some(TestLimitType::Ram);
            }
        }

        (peak_memory, limit_exceeded, limit_type)
    }

    async fn get_process_memory_usage(&self, _pid: u32) -> Option<u64> {
        // Simplified memory usage getter
        // In a real implementation, you'd read from /proc/[pid]/status or use system calls
        // For this basic version, we'll return a mock value
        Some(1024 * 1024) // 1MB mock usage
    }
}

#[async_trait]
impl Runner for BasicRunner {
    async fn run(
        &self,
        artifact: &Artifact,
        stdin: &str,
        limits: &ExecutionLimits,
    ) -> Result<RunnerResult, RunnerError> {
        let executable_path = match self.get_executable_path(artifact) {
            Some(path) => path,
            None => {
                return Err(RunnerError::FailedToLaunch {
                    msg: format!("Executable not found for artifact: {}", artifact.id),
                });
            }
        };

        // Prepare command
        let mut cmd = Command::new(&executable_path);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let start_time = Instant::now();

        // Apply time limit if specified
        let spawn_result = cmd.spawn();
        let mut child = spawn_result.map_err(|e| RunnerError::FailedToLaunch {
            msg: format!("Failed to spawn process: {}", e),
        })?;

        // Write stdin to the process
        if !stdin.is_empty() {
            if let Some(mut stdin_handle) = child.stdin.take() {
                if let Err(e) = stdin_handle.write_all(stdin.as_bytes()).await {
                    let _ = child.kill().await;
                    return Err(RunnerError::FailedToLaunch {
                        msg: format!("Failed to write to stdin: {}", e),
                    });
                }
                // Close stdin to signal EOF
                drop(stdin_handle);
            }
        }

        // Wait for process with timeout
        let output_result = if let Some(time_limit_ms) = limits.time_ms {
            let wait_future = child.wait_with_output();
            match timeout(Duration::from_millis(time_limit_ms), wait_future).await {
                Ok(result) => result,
                Err(_) => {
                    // Process timed out - child was consumed by wait_with_output,
                    // so we can't kill it, but the timeout should have handled it
                    let execution_time_ms = start_time.elapsed().as_millis() as u64;
                    let result = RunnerResult {
                        status: -1,
                        stdout: String::new(),
                        stderr: String::new(),
                        execution_time_ms,
                        peak_memory_usage_bytes: 0,
                    };
                    return Err(RunnerError::LimitsExceeded {
                        result,
                        limit_type: TestLimitType::Time,
                    });
                }
            }
        } else {
            child.wait_with_output().await
        };

        let execution_time_ms = start_time.elapsed().as_millis() as u64;

        let output = output_result.map_err(|e| RunnerError::FailedToLaunch {
            msg: format!("Failed to wait for process: {}", e),
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // Check output size limits
        if let Some(stdout_limit) = limits.stdout_size_bytes {
            if stdout.len() as u64 > stdout_limit {
                let result = RunnerResult {
                    status: output.status.code().unwrap_or(-1),
                    stdout: stdout.chars().take(stdout_limit as usize).collect(),
                    stderr,
                    execution_time_ms,
                    peak_memory_usage_bytes: 1024 * 1024, // Mock value
                };
                return Err(RunnerError::LimitsExceeded {
                    result,
                    limit_type: TestLimitType::StdoutSize,
                });
            }
        }

        if let Some(stderr_limit) = limits.stderr_size_bytes {
            if stderr.len() as u64 > stderr_limit {
                let result = RunnerResult {
                    status: output.status.code().unwrap_or(-1),
                    stdout,
                    stderr: stderr.chars().take(stderr_limit as usize).collect(),
                    execution_time_ms,
                    peak_memory_usage_bytes: 1024 * 1024, // Mock value
                };
                return Err(RunnerError::LimitsExceeded {
                    result,
                    limit_type: TestLimitType::StderrSize,
                });
            }
        }

        let status_code = output.status.code().unwrap_or(-1);
        let result = RunnerResult {
            status: status_code,
            stdout,
            stderr,
            execution_time_ms,
            peak_memory_usage_bytes: 1024 * 1024, // Mock value - in real implementation get from monitoring
        };

        // Check if process crashed (non-zero exit code that's not a normal program exit)
        if status_code != 0 && status_code < 0 {
            return Err(RunnerError::Crash { result });
        }

        Ok(result)
    }
}

impl Drop for BasicRunner {
    fn drop(&mut self) {
        // Best effort cleanup - ignore errors
        // Note: We don't clean up the temp_dir here because the compiler might still need the executables
    }
}
