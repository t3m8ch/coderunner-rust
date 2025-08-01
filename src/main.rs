use std::panic;
use std::sync::Arc;

use tonic::transport::Server;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use crate::core::domain::{Artifact, ArtifactKind};
use crate::core::traits::executor::{MockExecutor, RunResult};
use crate::grpc::models::testing_service_server::TestingServiceServer;
use crate::grpc::services::TestingServiceImpl;

mod constants;
mod core;
mod grpc;

#[tokio::main]
#[tracing::instrument]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    set_panic_hook();

    let addr = "[::1]:50051".parse()?;

    let mut mock_executor = MockExecutor::new();
    mock_executor.expect_compile().return_const(Ok(Artifact {
        id: Uuid::new_v4(),
        kind: ArtifactKind::Executable,
    }));
    mock_executor.expect_run().return_const(Ok(RunResult {
        status: 0,
        stdout: "Hello world".to_string(),
        stderr: "".to_string(),
        execution_time_ms: 100,
        peak_memory_usage_bytes: 1024,
    }));

    let service = TestingServiceServer::new(TestingServiceImpl::new(Arc::new(mock_executor)));

    tracing::info!("gRPC server listening on port 50051");
    Server::builder().add_service(service).serve(addr).await?;

    Ok(())
}

fn set_panic_hook() {
    panic::set_hook(Box::new(|panic_info| {
        tracing::error!(
            message = "panic occurred",
            panic = %panic_info
        );
    }));
}
