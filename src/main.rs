use std::sync::Arc;

use dashmap::DashMap;
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::Mutex;
use tokio::sync::mpsc::channel;
use tonic::transport::Server;

use crate::grpc::models::testing_service_client::TestingServiceClient;
use crate::grpc::models::testing_service_server::TestingServiceServer;
use crate::grpc::services::TestingServiceImpl;
use crate::models::{FileToCompile, FileToRun, MsgToHandle, MsgToRes};
use crate::pipeline::accepting::accept_connections;
use crate::pipeline::compiling::compile_files;
use crate::pipeline::msg_handling::handle_messages;
use crate::pipeline::reading::read_sockets;
use crate::pipeline::respondning::response;
use crate::pipeline::running::run_files;

mod grpc;
mod models;
mod pipeline;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50051".parse()?;
    let testing_service = TestingServiceImpl;
    let service = TestingServiceServer::new(testing_service);
    Server::builder().add_service(service).serve(addr).await?;

    Ok(())
}
