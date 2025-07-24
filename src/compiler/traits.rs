use crate::{
    compiler::errors::CompilationError,
    domain::{CompilationLimits, Language},
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
