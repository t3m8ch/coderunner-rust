use std::path::Path;
use std::sync::Arc;

use dashmap::DashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::mpsc::channel;

use crate::models::{FileToCompile, FileToRun, MsgToHandle, MsgToRes};

mod models;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("0.0.0.0:8000").await?;
    println!("Server listening on port 8000");

    let (read_tx, mut read_rx) = channel::<Arc<Mutex<TcpStream>>>(10);
    let (msg_handle_tx, mut msg_handle_rx) = channel::<MsgToHandle>(10);
    let (res_tx, mut res_rx) = channel::<MsgToRes>(10);
    let (compile_tx, mut compile_rx) = channel::<FileToCompile>(10);
    let (run_tx, mut run_rx) = channel::<FileToRun>(10);

    let sockets: Arc<DashMap<uuid::Uuid, Arc<Mutex<OwnedWriteHalf>>>> = Arc::new(DashMap::new());

    let accept_task = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            read_tx.send(Arc::new(Mutex::new(socket))).await.unwrap();
        }
    });

    let read_task_sockets = Arc::clone(&sockets);
    tokio::spawn(async move {
        while let Some(socket) = read_rx.recv().await {
            let task_id = uuid::Uuid::new_v4();

            let socket_inner = Arc::try_unwrap(socket).unwrap().into_inner();
            let (mut read_half, write_half) = socket_inner.into_split();
            read_task_sockets.insert(task_id, Arc::new(Mutex::new(write_half)));

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
    });

    let msg_handle_res_tx = res_tx.clone();
    tokio::spawn(async move {
        let res_tx = msg_handle_res_tx;
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
    });

    let compile_res_tx = res_tx.clone();
    tokio::spawn(async move {
        while let Some(task) = compile_rx.recv().await {
            let task_res_tx = compile_res_tx.clone();
            let run_tx = run_tx.clone();

            tokio::spawn(async move {
                let res_tx = task_res_tx;

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
    });

    let run_res_tx = res_tx.clone();
    tokio::spawn(async move {
        while let Some(task) = run_rx.recv().await {
            let task_res_tx = run_res_tx.clone();
            tokio::spawn(async move {
                let res_tx = task_res_tx;
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

    let res_task_sockets = Arc::clone(&sockets);
    tokio::spawn(async move {
        while let Some(msg) = res_rx.recv().await {
            println!("Received message: {:?}", msg);

            let Some(socket) = res_task_sockets.get(&msg.id) else {
                eprintln!("Socket not found for ID {}", msg.id);
                continue;
            };

            let socket_clone = socket.clone();
            let message = format!("{}\n", msg.text);

            tokio::spawn(async move {
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
            });
        }
    });

    accept_task.await.unwrap();

    Ok(())
}
