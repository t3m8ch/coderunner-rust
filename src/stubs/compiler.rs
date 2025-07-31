use std::time::Duration;

use uuid::Uuid;

use crate::{
    core::domain::{Artifact, ArtifactKind, CompilationLimits, Language},
    core::traits::compiler::{CompileError, Compiler},
};

#[derive(Debug, Clone)]
pub struct CompilerStub {
    result: Result<(), CompileError>,
    delay: Duration,
}

impl CompilerStub {
    pub fn new(result: Result<(), CompileError>, delay: Duration) -> Self {
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
    ) -> Result<Artifact, CompileError> {
        tracing::debug!(
            "Start compilation: source={:?}, language={:?}, limits={:?}",
            source,
            language,
            limits
        );
        tokio::time::sleep(self.delay).await;
        tracing::debug!("Compilation result: {:?}", self.result);

        self.result.clone().map(|_| Artifact {
            id: Uuid::new_v4(),
            kind: ArtifactKind::Executable,
        })
    }
}
