use crate::core::domain::{Artifact, ExecutionLimits, TestLimitType};

#[derive(Clone, Debug)]
pub struct RunResult {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    pub execution_time_ms: u64,
    pub peak_memory_usage_bytes: u64,
}

#[derive(Debug, Clone)]
pub enum RunError {
    Crash {
        result: RunResult,
    },
    LimitsExceeded {
        result: RunResult,
        limit_type: TestLimitType,
    },
    Internal {
        msg: String,
    },
}

#[async_trait::async_trait]
pub trait Runner: std::fmt::Debug + Send + Sync {
    async fn run(
        &self,
        artifact: &Artifact,
        stdin: &str,
        limits: &ExecutionLimits,
    ) -> Result<RunResult, RunError>;
}
