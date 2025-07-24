use std::time::Duration;

use crate::{
    compiler::{errors::CompilationError, traits::Compiler},
    domain::{CompilationLimits, Language},
};

#[derive(Debug, Clone)]
pub struct CompilerStub {
    result: Result<(), CompilationError>,
    delay: Duration,
}

impl CompilerStub {
    pub fn new(result: Result<(), CompilationError>, delay: Duration) -> Self {
        Self { result, delay }
    }
}

#[async_trait::async_trait]
impl Compiler for CompilerStub {
    #[tracing::instrument]
    async fn compile(
        &self,
        source: &str,
        language: &Language,
        limits: &CompilationLimits,
    ) -> Result<(), CompilationError> {
        tracing::debug!(
            "Start compilation: source={:?}, language={:?}, limits={:?}",
            source,
            language,
            limits
        );
        tokio::time::sleep(self.delay).await;
        tracing::debug!("Compilation result: {:?}", self.result);

        self.result.clone()
    }
}
