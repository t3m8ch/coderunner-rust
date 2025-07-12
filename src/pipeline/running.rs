use tokio::{
    process::Command,
    sync::mpsc::{Receiver, Sender},
};

use crate::models::{FileToRun, MsgToRes};

pub fn run_files(res_tx: Sender<MsgToRes>, mut run_rx: Receiver<FileToRun>) {
    tokio::spawn(async move {
        while let Some(task) = run_rx.recv().await {
            let res_tx = res_tx.clone();
            tokio::spawn(async move {
                println!("Running file: {:?}", task);

                let output = Command::new(task.file_name).output().await.unwrap();

                let res_text = format!(
                    "Execution status: {:?}\nExecution stdout: {:?}\nExecution stderr: {:?}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                println!("{}", res_text);
                res_tx
                    .send(MsgToRes::new(task.id, &res_text))
                    .await
                    .unwrap();
            });
        }
    });
}
