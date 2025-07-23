use std::panic;
use std::sync::Arc;
use std::time::Duration;

use tonic::transport::Server;
use tracing_subscriber::EnvFilter;

use crate::compiler::stubs::CompilerStub;
use crate::grpc::models::testing_service_server::TestingServiceServer;
use crate::grpc::services::TestingServiceImpl;

mod compiler;
mod grpc;
mod pipeline;

#[tokio::main]
#[tracing::instrument]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    set_panic_hook();

    let addr = "[::1]:50051".parse()?;
    let testing_service =
        TestingServiceImpl::new(Arc::new(CompilerStub::new(Ok(()), Duration::from_secs(1))));

    let service = TestingServiceServer::new(testing_service);

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
