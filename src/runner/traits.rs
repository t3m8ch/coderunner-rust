use crate::domain::{Artifact, ExecutionLimits, TestLimitType};
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct RunnerResult {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    pub execution_time_ms: u64,
    pub peak_memory_usage_bytes: u64,
}

#[derive(Debug, Clone, Error)]
pub enum RunnerError {
    #[error("Program crashed")]
    Crash { result: RunnerResult },
    #[error("Execution limits exceeded: {limit_type:?}")]
    LimitsExceeded {
        result: RunnerResult,
        limit_type: TestLimitType,
    },
    #[error("Failed to launch: {msg}")]
    FailedToLaunch { msg: String },
}

#[async_trait::async_trait]
pub trait Runner: std::fmt::Debug + Send + Sync {
    async fn run(
        &self,
        artifact: &Artifact,
        stdin: &str,
        limits: &ExecutionLimits,
    ) -> Result<RunnerResult, RunnerError>;
}
