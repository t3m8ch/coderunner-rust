use std::sync::Arc;

use tokio::sync::mpsc::{Receiver, Sender};

use crate::{
    compiler::{errors::CompilationError, traits::Compiler},
    domain::{Task, TaskState},
};

const TASK_TX_ERR: &'static str = "Failed to send task to task_tx";
const RUN_TX_ERR: &'static str = "Failed to send task to run_tx";

#[tracing::instrument]
pub fn handle_compiling(
    res_tx: Sender<Task>,
    run_tx: Sender<Task>,
    mut compile_rx: Receiver<Task>,
    compiler: Arc<dyn Compiler>,
) {
    tokio::spawn(async move {
        while let Some(task) = compile_rx.recv().await {
            let compiler = compiler.clone();
            let res_tx = res_tx.clone();
            let run_tx = run_tx.clone();

            tokio::spawn(async move {
                let task = task.change_state(TaskState::Compiling);
                res_tx.send(task.clone()).await.expect(TASK_TX_ERR);

                tracing::debug!("Start compiling");
                let compilation_result = compiler
                    .compile(&task.code, &task.language, &task.compilation_limits)
                    .await;
                tracing::debug!("Compilation result: {:?}", compilation_result);

                match compilation_result {
                    Ok(artifact_id) => {
                        let task = task.change_state(TaskState::Compiled(artifact_id));
                        run_tx.send(task.clone()).await.expect(RUN_TX_ERR);
                        res_tx.send(task).await.expect(TASK_TX_ERR);
                    }
                    Err(e) => match e {
                        CompilationError::CompilationFailed { msg } => {
                            let task = task.change_state(TaskState::CompilationFailed { msg });
                            res_tx.send(task).await.expect(TASK_TX_ERR);
                        }
                        CompilationError::CompilationLimitsExceeded(limit_type) => {
                            let task =
                                task.change_state(TaskState::CompilationLimitsExceeded(limit_type));
                            res_tx.send(task).await.expect(TASK_TX_ERR);
                        }
                    },
                }
            });
        }
    });
}
