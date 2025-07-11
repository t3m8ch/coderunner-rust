use std::path::Path;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::mpsc::channel;

use crate::models::{FileToCompile, FileToRun};

mod models;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("0.0.0.0:8000").await?;
    println!("Server listening on port 8000");

    let (compile_tx, mut compile_rx) = channel::<FileToCompile>(10);
    let (run_tx, mut run_rx) = channel::<FileToRun>(10);

    let req_handler = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let compile_tx = compile_tx.clone();

            tokio::spawn(async move {
                let mut read_buf = [0u8; 1024];

                loop {
                    let n = socket.read(&mut read_buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    let msg = String::from_utf8_lossy(&read_buf[0..n]);

                    println!("Received message: {}", msg.trim());

                    let mut msg = msg.trim().split(' ');
                    match msg.next() {
                        Some(cmd) => match cmd {
                            "SUBMIT" => {
                                let file_name = msg.next();
                                match file_name {
                                    Some(file_name) => {
                                        let file_name = file_name.trim();

                                        compile_tx
                                            .send(FileToCompile::new(file_name))
                                            .await
                                            .unwrap();

                                        let res_msg = format!("File '{}' submitted\n", file_name);
                                        socket.write_all(res_msg.as_bytes()).await.unwrap();
                                    }
                                    None => {
                                        socket.write_all(b"File name required\n").await.unwrap();
                                    }
                                }
                            }
                            _ => {
                                socket.write_all(b"Invalid command\n").await.unwrap();
                            }
                        },
                        None => {
                            socket.write_all(b"Invalid request\n").await.unwrap();
                        }
                    };
                }
            });
        }
    });

    tokio::spawn(async move {
        while let Some(task) = compile_rx.recv().await {
            let run_tx = run_tx.clone();

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

                println!("Compilation status: {:?}", output.status);
                println!(
                    "Compilation stdout: {:?}",
                    String::from_utf8_lossy(&output.stdout)
                );
                println!(
                    "Compilation stderr: {:?}",
                    String::from_utf8_lossy(&output.stderr)
                );

                run_tx.send(FileToRun::new(&exec_file)).await.unwrap();
            });
        }
    });

    tokio::spawn(async move {
        while let Some(task) = run_rx.recv().await {
            println!("Running file: {:?}", task);

            let output = Command::new(task.file_name).output().await.unwrap();

            println!("Execution status: {:?}", output.status);
            println!(
                "Execution stdout: {:?}",
                String::from_utf8_lossy(&output.stdout)
            );
            println!(
                "Execution stderr: {:?}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    });

    req_handler.await.unwrap();

    Ok(())
}
