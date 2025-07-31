use crate::domain::{Artifact, ExecutionLimits, TestLimitType};

#[derive(Clone, Debug)]
pub struct RunnerResult {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    pub execution_time_ms: u64,
    pub peak_memory_usage_bytes: u64,
}

// TODO: Separate crashes based on whether they are caused by
// the submitted code or by server problems
#[derive(Debug, Clone)]
pub enum RunnerError {
    Crash {
        result: RunnerResult,
    },
    LimitsExceeded {
        result: RunnerResult,
        limit_type: TestLimitType,
    },
    FailedToLaunch {
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
    ) -> Result<RunnerResult, RunnerError>;
}
