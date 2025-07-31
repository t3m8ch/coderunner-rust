use futures::stream::FuturesUnordered;
use std::sync::Arc;
use tokio_stream::StreamExt;

use tokio::sync::mpsc::{Receiver, Sender};

use crate::{
    constants::TASK_TX_ERR,
    domain::{Artifact, Task, TaskState, Test, TestData, TestResources, TestState},
    runner::traits::{Runner, RunnerError, RunnerResult},
};

#[tracing::instrument]
pub fn handle_running(res_tx: Sender<Task>, mut run_rx: Receiver<Task>, runner: Arc<dyn Runner>) {
    tokio::spawn(async move {
        while let Some(task) = run_rx.recv().await {
            process_task(task, &res_tx, &runner).await;
        }
    });
}

/// Processes a single task by running all its tests and updating the task state.
async fn process_task(task: Task, res_tx: &Sender<Task>, runner: &Arc<dyn Runner>) {
    tracing::debug!("Running task: {:?}", task);

    let TaskState::Compiled(artifact) = task.state.clone() else {
        tracing::error!("Task is not compiled");
        return;
    };

    let mut tests: Vec<Test> = (&task).into();
    let task = task.change_state(TaskState::Executing {
        tests: tests.clone(),
    });

    run_tests_concurrently(&task, &artifact, &mut tests, res_tx, runner).await;

    let task = task.change_state(TaskState::Done { results: tests });
    tracing::info!("Task completed: {:?}", task);
    res_tx.send(task).await.expect(TASK_TX_ERR);
}

/// Runs all tests for a task concurrently and handles state updates.
async fn run_tests_concurrently(
    task: &Task,
    artifact: &Artifact,
    tests: &mut Vec<Test>,
    res_tx: &Sender<Task>,
    runner: &Arc<dyn Runner>,
) {
    let mut futures = create_test_futures(task, artifact, runner);

    // Start all tests and send initial executing states
    for test_idx in 0..task.test_data.len() {
        tests[test_idx].state = TestState::Executing;
        let task = task.change_state(TaskState::Executing {
            tests: tests.clone(),
        });
        res_tx.send(task).await.expect(TASK_TX_ERR);
    }

    // Process results as they complete
    while let Some((test_idx, result)) = futures.next().await {
        tests[test_idx].state = (&task.test_data[test_idx], result).into();
        let task = task.change_state(TaskState::Executing {
            tests: tests.clone(),
        });
        res_tx.send(task).await.expect(TASK_TX_ERR);
    }
}

/// Creates concurrent futures for executing all tests in a task.
///
/// Each future represents the execution of a single test and returns
/// the test index along with its execution result.
fn create_test_futures(
    task: &Task,
    artifact: &Artifact,
    runner: &Arc<dyn Runner>,
) -> FuturesUnordered<impl std::future::Future<Output = (usize, Result<RunnerResult, RunnerError>)>>
{
    let futures = FuturesUnordered::new();

    for (test_idx, test_data) in task.test_data.iter().enumerate() {
        let runner = runner.clone();
        let artifact = artifact.clone();
        let execution_limits = task.execution_limits.clone();
        let stdin = test_data.stdin.clone();

        tracing::debug!(
            "Running test {} with data {:?} for task {:?}",
            test_idx,
            test_data,
            task
        );

        futures.push(async move {
            (
                test_idx,
                runner.run(&artifact, &stdin, &execution_limits).await,
            )
        });
    }

    futures
}

impl Into<Vec<Test>> for &Task {
    fn into(self) -> Vec<Test> {
        self.test_data
            .iter()
            .map(|_| Test {
                current_stdout: String::new(),
                current_stderr: String::new(),
                state: TestState::Pending,
            })
            .collect()
    }
}

impl Into<TestState> for (&TestData, Result<RunnerResult, RunnerError>) {
    fn into(self) -> TestState {
        let (test_data, result) = self;
        match result {
            Ok(result) => {
                if (&result.stdout, &result.stderr, result.status)
                    == (&test_data.stdout, &test_data.stderr, test_data.status)
                {
                    TestState::Correct {
                        resources: result.into(),
                    }
                } else {
                    TestState::Wrong {
                        expected_stdout: test_data.stdout.clone(),
                        expected_stderr: test_data.stderr.clone(),
                        expected_status: test_data.status,
                        resources: result.into(),
                    }
                }
            }
            Err(err) => match err {
                RunnerError::Crash { result } => TestState::Crash {
                    resources: result.into(),
                },
                RunnerError::LimitsExceeded { result, limit_type } => TestState::LimitsExceeded {
                    resources: result.into(),
                    limit_type,
                },
                RunnerError::Internal { msg } => {
                    tracing::error!("Internal error after running: {}", msg);
                    TestState::InternalError
                }
            },
        }
    }
}

impl Into<TestResources> for RunnerResult {
    fn into(self) -> TestResources {
        TestResources {
            execution_time_ms: self.execution_time_ms,
            peak_memory_usage_bytes: self.peak_memory_usage_bytes,
        }
    }
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
                executable_size_bytes: Some(16 * 1024 * 1024),
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
                status: 0,
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
            status: -1,
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
                expected_status,
                resources,
            } = &results[0].state
            {
                assert_eq!(expected_stdout, "expected_output");
                assert_eq!(expected_stderr, "");
                assert_eq!(*expected_status, 0);
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
    async fn test_execution_internal_error() {
        let runner = Arc::new(MockRunner::new(vec![Err(RunnerError::Internal {
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
            assert_eq!(results[0].state, TestState::InternalError);
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
            Ok(RunnerResult {
                status: -1, // Wrong status code
                stdout: "output3".to_string(),
                stderr: "".to_string(),
                execution_time_ms: 120,
                peak_memory_usage_bytes: 1024,
            }),
            Ok(RunnerResult {
                status: 0,
                stdout: "output4".to_string(),
                stderr: "ERROR".to_string(),
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
                status: 0,
            },
            TestData {
                stdin: "input2".to_string(),
                stdout: "output2".to_string(),
                stderr: "".to_string(),
                status: 0,
            },
            TestData {
                stdin: "input3".to_string(),
                stdout: "output3".to_string(),
                stderr: "".to_string(),
                status: 0,
            },
            TestData {
                stdin: "input4".to_string(),
                stdout: "output4".to_string(),
                stderr: "".to_string(),
                status: 0,
            },
        ];

        run_tx.send(task.clone()).await.unwrap();

        // Should receive multiple executing states and finally done state
        let mut received_messages = Vec::new();
        for _ in 0..9 {
            // 4 executing start + 4 executing results + 1 done
            received_messages.push(res_rx.recv().await.unwrap());
        }

        let done_task = received_messages.last().unwrap();
        if let TaskState::Done { results } = &done_task.state {
            assert_eq!(results.len(), 4);
            assert!(matches!(results[0].state, TestState::Correct { .. }));
            assert!(matches!(results[1].state, TestState::Wrong { .. }));
            assert!(matches!(results[2].state, TestState::Wrong { .. }));
            assert!(matches!(results[3].state, TestState::Wrong { .. }));
        } else {
            panic!("Expected Done state, actual state: {:?}", done_task.state);
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
