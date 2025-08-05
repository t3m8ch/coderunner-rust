use crate::core::domain::{
    Artifact, CompilationLimitType, CompilationLimits, ExecutionLimits, Language, TestLimitType,
};

#[mockall::automock]
#[async_trait::async_trait]
pub trait Executor: std::fmt::Debug + Send + Sync {
    async fn compile(
        &self,
        source: &str,
        language: &Language,
        limits: &CompilationLimits,
    ) -> Result<Artifact, CompileError>;

    async fn run(
        &self,
        artifact: &Artifact,
        stdin: &str,
        limits: &ExecutionLimits,
    ) -> Result<RunResult, RunError>;
}

#[derive(Debug, Clone)]
pub enum CompileError {
    // TODO: Change msg to stdout and stderr
    CompilationFailed { msg: String },
    Internal { msg: String },
    CompilationLimitsExceeded(CompilationLimitType),
}

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
    LimitsExceeded {
        result: RunResult,
        limit_type: TestLimitType,
    },
    Internal {
        msg: String,
    },
}
