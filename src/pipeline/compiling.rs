use std::path::Path;

use tokio::{
    process::Command,
    sync::mpsc::{Receiver, Sender},
    task::JoinHandle,
};

use crate::models::{FileToCompile, FileToRun, MsgToRes};

pub fn compile_files(
    res_tx: Sender<MsgToRes>,
    run_tx: Sender<FileToRun>,
    mut compile_rx: Receiver<FileToCompile>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(task) = compile_rx.recv().await {
            let run_tx = run_tx.clone();
            let res_tx = res_tx.clone();

            tokio::spawn(async move {
                println!("Compiling file: {:?}", task);

                let file_name = Path::new(&task.file_name);
                let file_stem = file_name.file_stem().unwrap().to_str().unwrap();

                let exec_file = format!("executables/{}.out", file_stem);
                let output = Command::new("g++")
                    .arg("-o")
                    .arg(exec_file.clone())
                    .arg(format!("files/{}.cpp", file_stem))
                    .output()
                    .await
                    .unwrap();

                let res_text = format!(
                    "Compilation status: {:?}\nCompilation stdout: {:?}\nCompilation stderr: {:?}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                println!("{}", res_text);
                res_tx
                    .send(MsgToRes::new(task.id, &res_text))
                    .await
                    .unwrap();

                run_tx
                    .send(FileToRun::new(task.id, &exec_file))
                    .await
                    .unwrap();
            });
        }
    })
}
