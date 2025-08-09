use std::sync::Arc;

use tokio::sync::mpsc::{Receiver, Sender};

use crate::{
    constants::{RUN_TX_ERR, TASK_TX_ERR},
    core::{
        domain::{Task, TaskState},
        traits::executor::{CompileError, Executor},
    },
};

#[tracing::instrument]
pub fn handle_compiling(
    res_tx: Sender<Task>,
    run_tx: Sender<Task>,
    mut compile_rx: Receiver<Task>,
    executor: Arc<dyn Executor>,
) {
    tokio::spawn(async move {
        while let Some(task) = compile_rx.recv().await {
            let compiler = executor.clone();
            let res_tx = res_tx.clone();
            let run_tx = run_tx.clone();

            tokio::spawn(async move {
                handle_task(task, res_tx, run_tx, compiler).await;
            });
        }
    });
}

async fn handle_task(
    task: Task,
    res_tx: Sender<Task>,
    run_tx: Sender<Task>,
    executor: Arc<dyn Executor>,
) {
    let task = task.change_state(TaskState::Compiling);
    res_tx.send(task.clone()).await.expect(TASK_TX_ERR);

    tracing::debug!("Start compiling");
    let compilation_result = executor
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
            CompileError::CompilationFailed { msg } => {
                let task = task.change_state(TaskState::CompilationFailed { msg });
                res_tx.send(task).await.expect(TASK_TX_ERR);
            }
            CompileError::CompilationLimitsExceeded(limit_type) => {
                let task = task.change_state(TaskState::CompilationLimitsExceeded(limit_type));
                res_tx.send(task).await.expect(TASK_TX_ERR);
            }
            CompileError::Internal { msg } => {
                tracing::error!("Internal error after compilation: {}", msg);
                let task = task.change_state(TaskState::InternalError);
                res_tx.send(task).await.expect(TASK_TX_ERR);
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        domain::{
            Artifact, ArtifactKind, CompilationLimitType, CompilationLimits, ExecutionLimits,
            Language, TestData,
        },
        traits::executor::MockExecutor,
    };
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use uuid::Uuid;

    fn create_test_task() -> Task {
        Task {
            id: Uuid::new_v4(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            code: "int main() { return 0; }".to_string(),
            language: Language::GnuCpp,
            compilation_limits: CompilationLimits {
                time_ms: Some(5000),
                memory_bytes: Some(128 * 1024 * 1024),
            },
            execution_limits: ExecutionLimits {
                time_ms: Some(1000),
                memory_bytes: Some(64 * 1024 * 1024),
                pids_count: Some(1),
                stdout_size_bytes: Some(1024),
                stderr_size_bytes: Some(1024),
            },
            test_data: vec![TestData {
                stdin: "".to_string(),
                stdout: "".to_string(),
                stderr: "".to_string(),
                status: 0,
            }],
            state: TaskState::Accepted,
        }
    }

    #[tokio::test]
    async fn test_successful_compilation() {
        let artifact = Artifact {
            id: Uuid::new_v4(),
            kind: ArtifactKind::Executable,
        };

        let mut executor = MockExecutor::new();
        executor.expect_compile().return_const(Ok(artifact.clone()));
        let executor = Arc::new(executor);

        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, mut run_rx) = mpsc::channel(10);
        let (compile_tx, compile_rx) = mpsc::channel(10);

        handle_compiling(res_tx, run_tx, compile_rx, executor);

        let task = create_test_task();
        compile_tx.send(task.clone()).await.unwrap();

        // Should receive task with Compiling state
        let compiling_task = res_rx.recv().await.unwrap();
        assert!(matches!(compiling_task.state, TaskState::Compiling));
        assert_eq!(compiling_task.id, task.id);

        // Should receive task with Compiled state
        let compiled_task = res_rx.recv().await.unwrap();
        assert!(matches!(compiled_task.state, TaskState::Compiled(_)));
        assert_eq!(compiled_task.id, task.id);

        // Should also receive task in run channel
        let run_task = run_rx.recv().await.unwrap();
        assert!(matches!(run_task.state, TaskState::Compiled(_)));
        assert_eq!(run_task.id, task.id);

        if let TaskState::Compiled(received_artifact) = run_task.state {
            assert_eq!(received_artifact, artifact);
        }
    }

    #[tokio::test]
    async fn test_compilation_failed() {
        let mut executor = MockExecutor::new();
        executor
            .expect_compile()
            .return_const(Err(CompileError::CompilationFailed {
                msg: "syntax error".to_string(),
            }));
        let executor = Arc::new(executor);

        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, mut run_rx) = mpsc::channel(10);
        let (compile_tx, compile_rx) = mpsc::channel(10);

        handle_compiling(res_tx, run_tx, compile_rx, executor);

        let task = create_test_task();
        compile_tx.send(task.clone()).await.unwrap();

        // Should receive task with Compiling state
        let compiling_task = res_rx.recv().await.unwrap();
        assert!(matches!(compiling_task.state, TaskState::Compiling));
        assert_eq!(compiling_task.id, task.id);

        // Should receive task with CompilationFailed state
        let failed_task = res_rx.recv().await.unwrap();
        assert!(matches!(
            failed_task.state,
            TaskState::CompilationFailed { .. }
        ));
        assert_eq!(failed_task.id, task.id);

        if let TaskState::CompilationFailed { msg } = failed_task.state {
            assert_eq!(msg, "syntax error");
        }

        // Should not receive anything in run channel
        tokio::time::timeout(std::time::Duration::from_millis(100), run_rx.recv())
            .await
            .expect_err("Should not receive task in run channel on compilation failure");
    }

    #[tokio::test]
    async fn test_compilation_limits_exceeded() {
        let limit_types = vec![CompilationLimitType::Time, CompilationLimitType::Ram];

        for expected_limit_type in limit_types {
            let mut executor = MockExecutor::new();
            executor
                .expect_compile()
                .return_const(Err(CompileError::CompilationLimitsExceeded(
                    expected_limit_type.clone(),
                )));
            let executor = Arc::new(executor);

            let (res_tx, mut res_rx) = mpsc::channel(10);
            let (run_tx, mut run_rx) = mpsc::channel(10);
            let (compile_tx, compile_rx) = mpsc::channel(10);

            handle_compiling(res_tx, run_tx, compile_rx, executor);

            let task = create_test_task();
            compile_tx.send(task.clone()).await.unwrap();

            // Should receive task with Compiling state
            let compiling_task = res_rx.recv().await.unwrap();
            assert!(matches!(compiling_task.state, TaskState::Compiling));
            assert_eq!(compiling_task.id, task.id);

            // Should receive task with CompilationLimitsExceeded state
            let limits_exceeded_task = res_rx.recv().await.unwrap();
            assert!(matches!(
                limits_exceeded_task.state,
                TaskState::CompilationLimitsExceeded(_)
            ));
            assert_eq!(limits_exceeded_task.id, task.id);

            if let TaskState::CompilationLimitsExceeded(actual_limit_type) =
                limits_exceeded_task.state
            {
                assert_eq!(actual_limit_type, expected_limit_type);
            }

            // Should not receive anything in run channel
            tokio::time::timeout(std::time::Duration::from_millis(100), run_rx.recv())
                .await
                .expect_err("Should not receive task in run channel on limits exceeded");
        }
    }

    #[tokio::test]
    async fn test_compilation_internal_error() {
        let mut executor = MockExecutor::new();
        executor
            .expect_compile()
            .return_const(Err(CompileError::Internal {
                msg: "Tux is sad and won't work :(".to_string(),
            }));
        let executor = Arc::new(executor);

        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, mut run_rx) = mpsc::channel(10);
        let (compile_tx, compile_rx) = mpsc::channel(10);

        handle_compiling(res_tx, run_tx, compile_rx, executor);

        let task = create_test_task();
        compile_tx.send(task.clone()).await.unwrap();

        // Should receive task with Compiling state
        let compiling_task = res_rx.recv().await.unwrap();
        assert!(matches!(compiling_task.state, TaskState::Compiling));
        assert_eq!(compiling_task.id, task.id);

        // Should receive task with InternalError state
        let internal_error_task = res_rx.recv().await.unwrap();
        assert!(matches!(
            internal_error_task.state,
            TaskState::InternalError
        ));
        assert_eq!(internal_error_task.id, task.id);

        // Should not receive anything in run channel
        tokio::time::timeout(std::time::Duration::from_millis(100), run_rx.recv())
            .await
            .expect_err("Should not receive task in run channel on limits exceeded");
    }

    #[tokio::test]
    async fn test_multiple_tasks() {
        let artifact = Artifact {
            id: Uuid::new_v4(),
            kind: ArtifactKind::Executable,
        };

        let mut executor = MockExecutor::new();
        executor.expect_compile().return_const(Ok(artifact.clone()));
        let executor = Arc::new(executor);

        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, mut run_rx) = mpsc::channel(10);
        let (compile_tx, compile_rx) = mpsc::channel(10);

        handle_compiling(res_tx, run_tx, compile_rx, executor);

        let task1 = create_test_task();
        let task2 = create_test_task();

        compile_tx.send(task1.clone()).await.unwrap();
        compile_tx.send(task2.clone()).await.unwrap();

        // Should receive messages for both tasks
        let mut received_tasks = Vec::new();
        for _ in 0..4 {
            // 2 tasks * 2 messages each (Compiling + Compiled)
            received_tasks.push(res_rx.recv().await.unwrap());
        }

        // Should receive 2 tasks in run channel
        let run_task1 = run_rx.recv().await.unwrap();
        let run_task2 = run_rx.recv().await.unwrap();

        // Verify we received tasks for both IDs
        let task_ids: std::collections::HashSet<_> = received_tasks.iter().map(|t| t.id).collect();
        assert!(task_ids.contains(&task1.id));
        assert!(task_ids.contains(&task2.id));

        let run_ids: std::collections::HashSet<_> =
            [run_task1.id, run_task2.id].into_iter().collect();
        assert!(run_ids.contains(&task1.id));
        assert!(run_ids.contains(&task2.id));
    }
}
