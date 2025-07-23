use crate::grpc::models::{
    CompilationFailed, CompilationLimits, CompilationLimitsExceeded, Language,
};

#[async_trait::async_trait]
pub trait Compiler: std::fmt::Debug + Send + Sync {
    async fn compile(
        &self,
        source: &str,
        language: &Language,
        limits: &CompilationLimits,
    ) -> Result<(), CompilationError>;
}

#[derive(Debug, Clone)]
pub enum CompilationError {
    CompilationFailed(CompilationFailed),
    CompilationLimitsExceeded(CompilationLimitsExceeded),
}
