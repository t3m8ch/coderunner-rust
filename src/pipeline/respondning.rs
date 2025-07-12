use std::sync::Arc;

use dashmap::DashMap;
use tokio::{
    io::AsyncWriteExt,
    net::tcp::OwnedWriteHalf,
    sync::{Mutex, mpsc::Receiver},
};

use crate::models::MsgToRes;

pub fn response(
    sockets: Arc<DashMap<uuid::Uuid, Arc<Mutex<OwnedWriteHalf>>>>,
    mut res_rx: Receiver<MsgToRes>,
) {
    tokio::spawn(async move {
        while let Some(msg) = res_rx.recv().await {
            println!("Received message: {:?}", msg);

            let Some(socket) = sockets.get(&msg.id) else {
                eprintln!("Socket not found for ID {}", msg.id);
                continue;
            };

            let socket_clone = socket.clone();
            let message = format!("{}\n", msg.text);

            let result = {
                let mut stream = socket_clone.lock().await;
                let write_result = stream.write_all(message.as_bytes()).await;
                if let Err(e) = write_result {
                    eprintln!("Failed to write to socket: {}", e);
                    return;
                }
                stream.flush().await
            };

            if let Err(e) = result {
                eprintln!("Failed to flush socket: {}", e);
            }
        }
    });
}
