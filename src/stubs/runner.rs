use std::time::Duration;

use crate::{
    core::domain::{Artifact, ExecutionLimits},
    core::traits::runner::{Runner, RunnerError, RunnerResult},
};

#[derive(Debug, Clone)]
pub struct RunnerStub {
    result: Result<RunnerResult, RunnerError>,
    delay: Duration,
}

impl RunnerStub {
    pub fn new(result: Result<RunnerResult, RunnerError>, delay: Duration) -> Self {
        Self { result, delay }
    }
}

#[async_trait::async_trait]
impl Runner for RunnerStub {
    #[tracing::instrument]
    async fn run(
        &self,
        artifact: &Artifact,
        stdin: &str,
        limits: &ExecutionLimits,
    ) -> Result<RunnerResult, RunnerError> {
        tracing::debug!(
            "Start execution: artifact={:?}, stdin={:?}, limits={:?}",
            artifact,
            stdin,
            limits
        );
        tokio::time::sleep(self.delay).await;
        tracing::debug!("Execution result: {:?}", self.result);

        self.result.clone()
    }
}
