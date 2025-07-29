use std::panic;
use std::sync::Arc;
use std::time::Duration;

use tonic::transport::Server;
use tracing_subscriber::EnvFilter;

use crate::compiler::stubs::CompilerStub;
use crate::grpc::models::testing_service_server::TestingServiceServer;
use crate::grpc::services::TestingServiceImpl;
use crate::runner::stubs::RunnerStub;
use crate::runner::traits::RunnerResult;

mod compiler;
mod constants;
mod domain;
mod examples;
mod grpc;
#[cfg(test)]
mod integration_test;
mod pipeline;
mod runner;
mod service_example;

#[tokio::main]
#[tracing::instrument]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    set_panic_hook();

    // Check if we should run examples instead of the server
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "examples" {
        return examples::run_all_examples().await;
    }
    if args.len() > 1 && args[1] == "service" {
        return service_example::run_all_service_examples().await;
    }

    let addr = "[::1]:50051".parse()?;
    let testing_service = TestingServiceImpl::new(
        Arc::new(CompilerStub::new(Ok(()), Duration::from_secs(1))),
        Arc::new(RunnerStub::new(
            Ok(RunnerResult {
                status: 0,
                stdout: "Hello World\n".to_string(),
                stderr: "".to_string(),
                execution_time_ms: 100,
                peak_memory_usage_bytes: 1024,
            }),
            Duration::from_secs(1),
        )),
    );

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
