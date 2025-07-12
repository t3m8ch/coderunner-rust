use std::sync::Arc;

use dashmap::DashMap;
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::Mutex;
use tokio::sync::mpsc::channel;

use crate::models::{FileToCompile, FileToRun, MsgToHandle, MsgToRes};
use crate::pipeline::accepting::accept_connections;
use crate::pipeline::compiling::compile_files;
use crate::pipeline::msg_handling::handle_messages;
use crate::pipeline::reading::read_sockets;
use crate::pipeline::respondning::response;
use crate::pipeline::running::run_files;

mod models;
mod pipeline;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (read_tx, read_rx) = channel::<Arc<Mutex<TcpStream>>>(10);
    let (msg_handle_tx, msg_handle_rx) = channel::<MsgToHandle>(10);
    let (res_tx, res_rx) = channel::<MsgToRes>(10);
    let (compile_tx, compile_rx) = channel::<FileToCompile>(10);
    let (run_tx, run_rx) = channel::<FileToRun>(10);
    let sockets: Arc<DashMap<uuid::Uuid, Arc<Mutex<OwnedWriteHalf>>>> = Arc::new(DashMap::new());

    read_sockets(sockets.clone(), read_rx, msg_handle_tx);
    handle_messages(res_tx.clone(), compile_tx, msg_handle_rx);
    compile_files(res_tx.clone(), run_tx, compile_rx);
    run_files(res_tx.clone(), run_rx);
    response(sockets.clone(), res_rx);

    println!("Server listening on port 8000");
    accept_connections(read_tx, "0.0.0.0:8000")
        .await
        .unwrap()
        .unwrap();

    Ok(())
}
