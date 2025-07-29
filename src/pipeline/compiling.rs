use std::sync::Arc;

use tokio::sync::mpsc::{Receiver, Sender};

use crate::{
    compiler::{errors::CompilationError, traits::Compiler},
    constants::{RUN_TX_ERR, TASK_TX_ERR},
    domain::{Task, TaskState},
};

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
                handle_task(task, res_tx, run_tx, compiler).await;
            });
        }
    });
}

async fn handle_task(
    task: Task,
    res_tx: Sender<Task>,
    run_tx: Sender<Task>,
    compiler: Arc<dyn Compiler>,
) {
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
                let task = task.change_state(TaskState::CompilationLimitsExceeded(limit_type));
                res_tx.send(task).await.expect(TASK_TX_ERR);
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        compiler::errors::CompilationError,
        domain::{
            Artifact, ArtifactKind, CompilationLimitType, ExecutionLimits, Language, TestData,
        },
    };
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use uuid::Uuid;

    #[derive(Debug)]
    struct MockCompiler {
        result: Result<Artifact, CompilationError>,
    }

    #[async_trait::async_trait]
    impl Compiler for MockCompiler {
        async fn compile(
            &self,
            _source: &str,
            _language: &Language,
            _limits: &crate::domain::CompilationLimits,
        ) -> Result<Artifact, CompilationError> {
            self.result.clone()
        }
    }

    fn create_test_task() -> Task {
        Task {
            id: Uuid::new_v4(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            code: "int main() { return 0; }".to_string(),
            language: Language::GnuCpp,
            compilation_limits: crate::domain::CompilationLimits {
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
            }],
            state: crate::domain::TaskState::Accepted,
        }
    }

    #[tokio::test]
    async fn test_successful_compilation() {
        let artifact = Artifact {
            id: Uuid::new_v4(),
            kind: ArtifactKind::Executable,
        };

        let compiler = Arc::new(MockCompiler {
            result: Ok(artifact.clone()),
        });

        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, mut run_rx) = mpsc::channel(10);
        let (compile_tx, compile_rx) = mpsc::channel(10);

        handle_compiling(res_tx, run_tx, compile_rx, compiler);

        let task = create_test_task();
        compile_tx.send(task.clone()).await.unwrap();

        // Should receive task with Compiling state
        let compiling_task = res_rx.recv().await.unwrap();
        assert!(matches!(
            compiling_task.state,
            crate::domain::TaskState::Compiling
        ));
        assert_eq!(compiling_task.id, task.id);

        // Should receive task with Compiled state
        let compiled_task = res_rx.recv().await.unwrap();
        assert!(matches!(
            compiled_task.state,
            crate::domain::TaskState::Compiled(_)
        ));
        assert_eq!(compiled_task.id, task.id);

        // Should also receive task in run channel
        let run_task = run_rx.recv().await.unwrap();
        assert!(matches!(
            run_task.state,
            crate::domain::TaskState::Compiled(_)
        ));
        assert_eq!(run_task.id, task.id);

        if let crate::domain::TaskState::Compiled(received_artifact) = run_task.state {
            assert_eq!(received_artifact.id, artifact.id);
        }
    }

    #[tokio::test]
    async fn test_compilation_failed() {
        let compiler = Arc::new(MockCompiler {
            result: Err(CompilationError::CompilationFailed {
                msg: "syntax error".to_string(),
            }),
        });

        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, mut run_rx) = mpsc::channel(10);
        let (compile_tx, compile_rx) = mpsc::channel(10);

        handle_compiling(res_tx, run_tx, compile_rx, compiler);

        let task = create_test_task();
        compile_tx.send(task.clone()).await.unwrap();

        // Should receive task with Compiling state
        let compiling_task = res_rx.recv().await.unwrap();
        assert!(matches!(
            compiling_task.state,
            crate::domain::TaskState::Compiling
        ));

        // Should receive task with CompilationFailed state
        let failed_task = res_rx.recv().await.unwrap();
        assert!(matches!(
            failed_task.state,
            crate::domain::TaskState::CompilationFailed { .. }
        ));
        assert_eq!(failed_task.id, task.id);

        if let crate::domain::TaskState::CompilationFailed { msg } = failed_task.state {
            assert_eq!(msg, "syntax error");
        }

        // Should not receive anything in run channel
        tokio::time::timeout(std::time::Duration::from_millis(100), run_rx.recv())
            .await
            .expect_err("Should not receive task in run channel on compilation failure");
    }

    #[tokio::test]
    async fn test_compilation_limits_exceeded() {
        let compiler = Arc::new(MockCompiler {
            result: Err(CompilationError::CompilationLimitsExceeded(
                CompilationLimitType::Time,
            )),
        });

        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, mut run_rx) = mpsc::channel(10);
        let (compile_tx, compile_rx) = mpsc::channel(10);

        handle_compiling(res_tx, run_tx, compile_rx, compiler);

        let task = create_test_task();
        compile_tx.send(task.clone()).await.unwrap();

        // Should receive task with Compiling state
        let compiling_task = res_rx.recv().await.unwrap();
        assert!(matches!(
            compiling_task.state,
            crate::domain::TaskState::Compiling
        ));

        // Should receive task with CompilationLimitsExceeded state
        let limits_exceeded_task = res_rx.recv().await.unwrap();
        assert!(matches!(
            limits_exceeded_task.state,
            crate::domain::TaskState::CompilationLimitsExceeded(_)
        ));
        assert_eq!(limits_exceeded_task.id, task.id);

        if let crate::domain::TaskState::CompilationLimitsExceeded(limit_type) =
            limits_exceeded_task.state
        {
            assert!(matches!(limit_type, CompilationLimitType::Time));
        }

        // Should not receive anything in run channel
        tokio::time::timeout(std::time::Duration::from_millis(100), run_rx.recv())
            .await
            .expect_err("Should not receive task in run channel on limits exceeded");
    }

    #[tokio::test]
    async fn test_compilation_limits_exceeded_ram() {
        let compiler = Arc::new(MockCompiler {
            result: Err(CompilationError::CompilationLimitsExceeded(
                CompilationLimitType::Ram,
            )),
        });

        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, _) = mpsc::channel(10);
        let (compile_tx, compile_rx) = mpsc::channel(10);

        handle_compiling(res_tx, run_tx, compile_rx, compiler);

        let task = create_test_task();
        compile_tx.send(task.clone()).await.unwrap();

        // Skip compiling state message
        let _compiling_task = res_rx.recv().await.unwrap();

        // Should receive task with CompilationLimitsExceeded state
        let limits_exceeded_task = res_rx.recv().await.unwrap();
        if let crate::domain::TaskState::CompilationLimitsExceeded(limit_type) =
            limits_exceeded_task.state
        {
            assert!(matches!(limit_type, CompilationLimitType::Ram));
        } else {
            panic!("Expected CompilationLimitsExceeded state");
        }
    }

    #[tokio::test]
    async fn test_multiple_tasks() {
        let artifact = Artifact {
            id: Uuid::new_v4(),
            kind: ArtifactKind::Executable,
        };

        let compiler = Arc::new(MockCompiler {
            result: Ok(artifact.clone()),
        });

        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, mut run_rx) = mpsc::channel(10);
        let (compile_tx, compile_rx) = mpsc::channel(10);

        handle_compiling(res_tx, run_tx, compile_rx, compiler);

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
