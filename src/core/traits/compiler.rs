use crate::core::domain::{Artifact, CompilationLimitType, CompilationLimits, Language};

#[derive(Debug, Clone)]
pub enum CompileError {
    CompilationFailed { msg: String },
    Internal { msg: String },
    CompilationLimitsExceeded(CompilationLimitType),
}

#[async_trait::async_trait]
pub trait Compiler: std::fmt::Debug + Send + Sync {
    async fn compile(
        &self,
        source: &str,
        language: &Language,
        limits: &CompilationLimits,
    ) -> Result<Artifact, CompileError>;
}
