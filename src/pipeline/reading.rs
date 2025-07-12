use std::sync::Arc;

use dashmap::DashMap;
use tokio::{
    io::AsyncReadExt,
    net::{TcpStream, tcp::OwnedWriteHalf},
    sync::{
        Mutex,
        mpsc::{Receiver, Sender},
    },
    task::JoinHandle,
};

use crate::models::MsgToHandle;

pub fn read_sockets(
    sockets: Arc<DashMap<uuid::Uuid, Arc<Mutex<OwnedWriteHalf>>>>,
    mut read_rx: Receiver<Arc<Mutex<TcpStream>>>,
    msg_handle_tx: Sender<MsgToHandle>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(socket) = read_rx.recv().await {
            let task_id = uuid::Uuid::new_v4();

            let socket_inner = Arc::try_unwrap(socket).unwrap().into_inner();
            let (mut read_half, write_half) = socket_inner.into_split();
            sockets.insert(task_id, Arc::new(Mutex::new(write_half)));

            let msg_handle_tx = msg_handle_tx.clone();
            tokio::spawn(async move {
                loop {
                    let mut buf = [0u8; 1024];

                    let n = read_half.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }

                    msg_handle_tx
                        .send(MsgToHandle::new(
                            task_id,
                            &String::from_utf8_lossy(&buf[0..n]).to_string(),
                        ))
                        .await
                        .unwrap();
                }
            });
        }
    })
}
