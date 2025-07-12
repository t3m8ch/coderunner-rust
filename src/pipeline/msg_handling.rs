use tokio::{
    sync::mpsc::{Receiver, Sender},
    task::JoinHandle,
};

use crate::models::{FileToCompile, MsgToHandle, MsgToRes};

pub fn handle_messages(
    res_tx: Sender<MsgToRes>,
    compile_tx: Sender<FileToCompile>,
    mut msg_handle_rx: Receiver<MsgToHandle>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(msg) = msg_handle_rx.recv().await {
            let mut msg_text = msg.text.split(' ');

            let Some(cmd) = msg_text.next() else {
                res_tx
                    .send(MsgToRes::new(msg.id, "Invalid request"))
                    .await
                    .unwrap();
                continue;
            };

            match cmd {
                "SUBMIT" => {
                    let Some(file_name) = msg_text.next() else {
                        res_tx
                            .send(MsgToRes::new(msg.id, "File name required"))
                            .await
                            .unwrap();
                        continue;
                    };
                    let file_name = file_name.trim();

                    compile_tx
                        .send(FileToCompile::new(msg.id, file_name))
                        .await
                        .unwrap();

                    res_tx
                        .send(MsgToRes::new(
                            msg.id,
                            &format!("File {} submitted", file_name),
                        ))
                        .await
                        .unwrap();
                }
                _ => {
                    res_tx
                        .send(MsgToRes::new(msg.id, "Invalid command"))
                        .await
                        .unwrap();
                }
            }
        }
    })
}
