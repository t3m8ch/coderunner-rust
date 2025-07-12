use std::sync::Arc;

use tokio::{
    io,
    net::{TcpListener, TcpStream},
    sync::{Mutex, mpsc::Sender},
    task::JoinHandle,
};

pub fn accept_connections(
    read_tx: Sender<Arc<Mutex<TcpStream>>>,
    addr: &str,
) -> JoinHandle<io::Result<()>> {
    let addr = addr.to_string();
    tokio::spawn(async move {
        let listener = TcpListener::bind(addr).await?;
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            read_tx.send(Arc::new(Mutex::new(socket))).await.unwrap();
        }
    })
}
