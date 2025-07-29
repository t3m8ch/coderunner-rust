use futures::stream::FuturesUnordered;
use std::sync::Arc;
use tokio_stream::StreamExt;

use tokio::sync::mpsc::{Receiver, Sender};

use crate::{
    constants::TASK_TX_ERR,
    domain::{Task, TaskState, Test, TestResources, TestState},
    runner::traits::{Runner, RunnerError},
};

#[tracing::instrument]
pub fn handle_running(res_tx: Sender<Task>, mut run_rx: Receiver<Task>, runner: Arc<dyn Runner>) {
    tokio::spawn(async move {
        while let Some(task) = run_rx.recv().await {
            tracing::debug!("Running task: {:?}", task);

            let TaskState::Compiled(artifact) = task.state.clone() else {
                tracing::error!("Task is not compiled");
                continue;
            };

            let mut tests: Vec<_> = task
                .test_data
                .iter()
                .map(|_| Test {
                    current_stdout: String::new(),
                    current_stderr: String::new(),
                    state: TestState::Pending,
                })
                .collect();
            let task = task.change_state(TaskState::Executing {
                tests: tests.clone(),
            });

            let mut futures = FuturesUnordered::new();
            for (test_idx, test_data) in task.test_data.iter().enumerate() {
                let runner = runner.clone();
                let artifact = artifact.clone();
                let execution_limits = task.execution_limits.clone();

                tracing::debug!(
                    "Running test {} with data {:?} for task {:?}",
                    test_idx,
                    test_data,
                    task
                );
                futures.push(async move {
                    (
                        test_idx,
                        runner
                            .run(&artifact, &test_data.stdin, &execution_limits)
                            .await,
                    )
                });

                tests[test_idx].state = TestState::Executing;
                let task = task.change_state(TaskState::Executing {
                    tests: tests.clone(),
                });
                res_tx.send(task).await.expect(TASK_TX_ERR);
            }

            while let Some((test_idx, result)) = futures.next().await {
                let test_data = &task.test_data[test_idx];
                tests[test_idx].state = match result {
                    Ok(result) => {
                        let resources = TestResources {
                            execution_time_ms: result.execution_time_ms,
                            peak_memory_usage_bytes: result.peak_memory_usage_bytes,
                        };

                        if (&result.stdout, &result.stderr)
                            == (&test_data.stdout, &test_data.stderr)
                        {
                            TestState::Correct { resources }
                        } else {
                            TestState::Wrong {
                                expected_stdout: test_data.stdout.clone(),
                                expected_stderr: test_data.stderr.clone(),
                                resources,
                            }
                        }
                    }
                    Err(err) => match err {
                        RunnerError::Crash { result } => {
                            let resources = TestResources {
                                execution_time_ms: result.execution_time_ms,
                                peak_memory_usage_bytes: result.peak_memory_usage_bytes,
                            };
                            TestState::Crash { resources }
                        }
                        RunnerError::LimitsExceeded { result, limit_type } => {
                            let resources = TestResources {
                                execution_time_ms: result.execution_time_ms,
                                peak_memory_usage_bytes: result.peak_memory_usage_bytes,
                            };
                            TestState::LimitsExceeded {
                                resources,
                                limit_type,
                            }
                        }
                        RunnerError::FailedToLaunch { msg } => {
                            TestState::InternalError { message: msg }
                        }
                    },
                };

                let task = task.change_state(TaskState::Executing {
                    tests: tests.clone(),
                });
                res_tx.send(task).await.expect(TASK_TX_ERR);
            }

            let task = task.change_state(TaskState::Done { results: tests });
            tracing::info!("Task completed: {:?}", task);
            res_tx.send(task).await.expect(TASK_TX_ERR);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{
            Artifact, ArtifactKind, CompilationLimits, ExecutionLimits, Language, TestData,
            TestLimitType,
        },
        runner::traits::{RunnerError, RunnerResult},
    };
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use uuid::Uuid;

    #[derive(Debug)]
    struct MockRunner {
        results: Vec<Result<RunnerResult, RunnerError>>,
        call_count: std::sync::atomic::AtomicUsize,
    }

    impl MockRunner {
        fn new(results: Vec<Result<RunnerResult, RunnerError>>) -> Self {
            Self {
                results,
                call_count: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl Runner for MockRunner {
        async fn run(
            &self,
            _artifact: &Artifact,
            _stdin: &str,
            _limits: &ExecutionLimits,
        ) -> Result<RunnerResult, RunnerError> {
            let idx = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.results[idx % self.results.len()].clone()
        }
    }

    fn create_compiled_task() -> Task {
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
                stdin: "input".to_string(),
                stdout: "expected_output".to_string(),
                stderr: "".to_string(),
            }],
            state: TaskState::Compiled(Artifact {
                id: Uuid::new_v4(),
                kind: ArtifactKind::Executable,
            }),
        }
    }

    #[tokio::test]
    async fn test_successful_execution_correct_output() {
        let runner_result = RunnerResult {
            status: 0,
            stdout: "expected_output".to_string(),
            stderr: "".to_string(),
            execution_time_ms: 100,
            peak_memory_usage_bytes: 1024,
        };

        let runner = Arc::new(MockRunner::new(vec![Ok(runner_result)]));
        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, run_rx) = mpsc::channel(10);

        handle_running(res_tx, run_rx, runner);

        let task = create_compiled_task();
        run_tx.send(task.clone()).await.unwrap();

        // Should receive task in Executing state with Executing test
        let executing_task = res_rx.recv().await.unwrap();
        assert!(matches!(executing_task.state, TaskState::Executing { .. }));

        // Should receive task in Executing state with Correct test result
        let updated_task = res_rx.recv().await.unwrap();
        if let TaskState::Executing { tests } = &updated_task.state {
            assert_eq!(tests.len(), 1);
            assert!(matches!(tests[0].state, TestState::Correct { .. }));
        } else {
            panic!("Expected Executing state");
        }

        // Should receive final Done state
        let done_task = res_rx.recv().await.unwrap();
        if let TaskState::Done { results } = &done_task.state {
            assert_eq!(results.len(), 1);
            assert!(matches!(results[0].state, TestState::Correct { .. }));
        } else {
            panic!("Expected Done state");
        }
    }

    #[tokio::test]
    async fn test_successful_execution_wrong_output() {
        let runner_result = RunnerResult {
            status: 0,
            stdout: "actual_output".to_string(),
            stderr: "actual_error".to_string(),
            execution_time_ms: 150,
            peak_memory_usage_bytes: 2048,
        };

        let runner = Arc::new(MockRunner::new(vec![Ok(runner_result)]));
        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, run_rx) = mpsc::channel(10);

        handle_running(res_tx, run_rx, runner);

        let task = create_compiled_task();
        run_tx.send(task.clone()).await.unwrap();

        // Skip to final result
        let _executing1 = res_rx.recv().await.unwrap();
        let _executing2 = res_rx.recv().await.unwrap();

        let done_task = res_rx.recv().await.unwrap();
        if let TaskState::Done { results } = &done_task.state {
            assert_eq!(results.len(), 1);
            if let TestState::Wrong {
                expected_stdout,
                expected_stderr,
                resources,
            } = &results[0].state
            {
                assert_eq!(expected_stdout, "expected_output");
                assert_eq!(expected_stderr, "");
                assert_eq!(resources.execution_time_ms, 150);
                assert_eq!(resources.peak_memory_usage_bytes, 2048);
            } else {
                panic!("Expected Wrong test state");
            }
        } else {
            panic!("Expected Done state");
        }
    }

    #[tokio::test]
    async fn test_execution_crash() {
        let runner_result = RunnerResult {
            status: -1,
            stdout: "".to_string(),
            stderr: "segmentation fault".to_string(),
            execution_time_ms: 50,
            peak_memory_usage_bytes: 512,
        };

        let runner = Arc::new(MockRunner::new(vec![Err(RunnerError::Crash {
            result: runner_result,
        })]));
        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, run_rx) = mpsc::channel(10);

        handle_running(res_tx, run_rx, runner);

        let task = create_compiled_task();
        run_tx.send(task.clone()).await.unwrap();

        // Skip to final result
        let _executing1 = res_rx.recv().await.unwrap();
        let _executing2 = res_rx.recv().await.unwrap();

        let done_task = res_rx.recv().await.unwrap();
        if let TaskState::Done { results } = &done_task.state {
            assert_eq!(results.len(), 1);
            if let TestState::Crash { resources } = &results[0].state {
                assert_eq!(resources.execution_time_ms, 50);
                assert_eq!(resources.peak_memory_usage_bytes, 512);
            } else {
                panic!("Expected Crash test state, got: {:?}", results[0].state);
            }
        } else {
            panic!("Expected Done state");
        }
    }

    #[tokio::test]
    async fn test_execution_limits_exceeded() {
        let runner_result = RunnerResult {
            status: 0,
            stdout: "partial_output".to_string(),
            stderr: "".to_string(),
            execution_time_ms: 2000,
            peak_memory_usage_bytes: 128 * 1024 * 1024,
        };

        let runner = Arc::new(MockRunner::new(vec![Err(RunnerError::LimitsExceeded {
            result: runner_result,
            limit_type: TestLimitType::Time,
        })]));
        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, run_rx) = mpsc::channel(10);

        handle_running(res_tx, run_rx, runner);

        let task = create_compiled_task();
        run_tx.send(task.clone()).await.unwrap();

        // Skip to final result
        let _executing1 = res_rx.recv().await.unwrap();
        let _executing2 = res_rx.recv().await.unwrap();

        let done_task = res_rx.recv().await.unwrap();
        if let TaskState::Done { results } = &done_task.state {
            assert_eq!(results.len(), 1);
            if let TestState::LimitsExceeded {
                resources,
                limit_type,
            } = &results[0].state
            {
                assert_eq!(resources.execution_time_ms, 2000);
                assert!(matches!(limit_type, TestLimitType::Time));
            } else {
                panic!("Expected LimitsExceeded test state");
            }
        } else {
            panic!("Expected Done state");
        }
    }

    #[tokio::test]
    async fn test_execution_failed_to_launch() {
        let runner = Arc::new(MockRunner::new(vec![Err(RunnerError::FailedToLaunch {
            msg: "binary not found".to_string(),
        })]));
        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, run_rx) = mpsc::channel(10);

        handle_running(res_tx, run_rx, runner);

        let task = create_compiled_task();
        run_tx.send(task.clone()).await.unwrap();

        // Skip to final result
        let _executing1 = res_rx.recv().await.unwrap();
        let _executing2 = res_rx.recv().await.unwrap();

        let done_task = res_rx.recv().await.unwrap();
        if let TaskState::Done { results } = &done_task.state {
            assert_eq!(results.len(), 1);
            if let TestState::InternalError { message } = &results[0].state {
                assert_eq!(message, "binary not found");
            } else {
                panic!("Expected InternalError test state");
            }
        } else {
            panic!("Expected Done state");
        }
    }

    #[tokio::test]
    async fn test_multiple_tests() {
        let runner_results = vec![
            Ok(RunnerResult {
                status: 0,
                stdout: "output1".to_string(),
                stderr: "".to_string(),
                execution_time_ms: 100,
                peak_memory_usage_bytes: 1024,
            }),
            Ok(RunnerResult {
                status: 0,
                stdout: "wrong_output".to_string(),
                stderr: "".to_string(),
                execution_time_ms: 120,
                peak_memory_usage_bytes: 1024,
            }),
        ];

        let runner = Arc::new(MockRunner::new(runner_results));
        let (res_tx, mut res_rx) = mpsc::channel(20);
        let (run_tx, run_rx) = mpsc::channel(10);

        handle_running(res_tx, run_rx, runner);

        let mut task = create_compiled_task();
        task.test_data = vec![
            TestData {
                stdin: "input1".to_string(),
                stdout: "output1".to_string(),
                stderr: "".to_string(),
            },
            TestData {
                stdin: "input2".to_string(),
                stdout: "output2".to_string(),
                stderr: "".to_string(),
            },
        ];

        run_tx.send(task.clone()).await.unwrap();

        // Should receive multiple executing states and finally done state
        let mut received_messages = Vec::new();
        for _ in 0..5 {
            // 2 executing start + 2 executing results + 1 done
            received_messages.push(res_rx.recv().await.unwrap());
        }

        let done_task = received_messages.last().unwrap();
        if let TaskState::Done { results } = &done_task.state {
            assert_eq!(results.len(), 2);
            assert!(matches!(results[0].state, TestState::Correct { .. }));
            assert!(matches!(results[1].state, TestState::Wrong { .. }));
        } else {
            panic!("Expected Done state");
        }
    }

    #[tokio::test]
    async fn test_non_compiled_task_skipped() {
        let runner = Arc::new(MockRunner::new(vec![]));
        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, run_rx) = mpsc::channel(10);

        handle_running(res_tx, run_rx, runner);

        let mut task = create_compiled_task();
        task.state = TaskState::Compiling; // Not compiled

        run_tx.send(task.clone()).await.unwrap();

        // Should not receive any messages since task is not compiled
        tokio::time::timeout(std::time::Duration::from_millis(100), res_rx.recv())
            .await
            .expect_err("Should not receive any messages for non-compiled task");
    }
}
