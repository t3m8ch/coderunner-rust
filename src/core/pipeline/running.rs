use futures::stream::FuturesUnordered;
use std::sync::Arc;
use tokio_stream::StreamExt;

use tokio::sync::mpsc::{Receiver, Sender};

use crate::{
    constants::TASK_TX_ERR,
    core::{
        domain::{Artifact, Task, TaskState, Test, TestData, TestResources, TestState},
        traits::executor::{Executor, RunError, RunResult},
    },
};

#[tracing::instrument]
pub fn handle_running(
    res_tx: Sender<Task>,
    mut run_rx: Receiver<Task>,
    executor: Arc<dyn Executor>,
) {
    tokio::spawn(async move {
        while let Some(task) = run_rx.recv().await {
            process_task(task, &res_tx, &executor).await;
        }
    });
}

/// Processes a single task by running all its tests and updating the task state.
async fn process_task(task: Task, res_tx: &Sender<Task>, executor: &Arc<dyn Executor>) {
    tracing::debug!("Running task: {:?}", task);

    let TaskState::Compiled(artifact) = task.state.clone() else {
        tracing::error!("Task is not compiled");
        return;
    };

    let mut tests: Vec<Test> = (&task).into();
    let task = task.change_state(TaskState::Executing {
        tests: tests.clone(),
    });
    res_tx.send(task.clone()).await.expect(TASK_TX_ERR);

    run_tests_concurrently(&task, &artifact, &mut tests, res_tx, executor).await;

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
    executor: &Arc<dyn Executor>,
) {
    let mut futures = create_test_futures(task, artifact, executor);

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
        tests[test_idx] = (&task.test_data[test_idx], result).into();
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
    executor: &Arc<dyn Executor>,
) -> FuturesUnordered<impl std::future::Future<Output = (usize, Result<RunResult, RunError>)>> {
    let futures = FuturesUnordered::new();

    for (test_idx, test_data) in task.test_data.iter().enumerate() {
        let runner = executor.clone();
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

impl Into<Test> for (&TestData, Result<RunResult, RunError>) {
    fn into(self) -> Test {
        let (test_data, result) = self;
        match result {
            Ok(result) => {
                if (&result.stdout, &result.stderr, result.status)
                    == (&test_data.stdout, &test_data.stderr, test_data.status)
                {
                    Test {
                        current_stdout: result.stdout.clone(),
                        current_stderr: result.stderr.clone(),
                        state: TestState::Correct {
                            resources: result.clone().into(),
                            status: result.status,
                        },
                    }
                } else {
                    Test {
                        current_stdout: result.stdout.clone(),
                        current_stderr: result.stderr.clone(),
                        state: TestState::Wrong {
                            expected_stdout: test_data.stdout.clone(),
                            expected_stderr: test_data.stderr.clone(),
                            expected_status: test_data.status,
                            actual_status: result.status,
                            resources: result.into(),
                        },
                    }
                }
            }
            Err(err) => match err {
                RunError::LimitsExceeded { result, limit_type } => Test {
                    current_stdout: result.stdout.clone(),
                    current_stderr: result.stderr.clone(),
                    state: TestState::LimitsExceeded {
                        resources: result.into(),
                        limit_type,
                    },
                },
                RunError::Internal { msg } => {
                    tracing::error!("Internal error after running: {}", msg);
                    Test {
                        current_stdout: String::new(),
                        current_stderr: String::new(),
                        state: TestState::InternalError,
                    }
                }
            },
        }
    }
}

impl Into<TestResources> for RunResult {
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
    use crate::core::{
        domain::{
            Artifact, ArtifactKind, CompilationLimits, ExecutionLimits, Language, TestData,
            TestLimitType,
        },
        traits::executor::MockExecutor,
    };
    use itertools::Itertools;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use uuid::Uuid;

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
        let run_result = RunResult {
            status: 0,
            stdout: "expected_output".to_string(),
            stderr: "".to_string(),
            execution_time_ms: 100,
            peak_memory_usage_bytes: 1024,
        };

        let mut executor = MockExecutor::new();
        executor.expect_run().return_const(Ok(run_result));
        let executor = Arc::new(executor);

        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, run_rx) = mpsc::channel(10);
        handle_running(res_tx, run_rx, executor);

        let task = create_compiled_task();
        run_tx.send(task.clone()).await.unwrap();

        // Should receive task in Executing state with Pending test
        let pending_task = res_rx.recv().await.unwrap();
        if let TaskState::Executing { tests } = &pending_task.state {
            assert_eq!(tests.len(), 1);
            assert!(matches!(tests[0].state, TestState::Pending { .. }));
        } else {
            panic!("Expected Executing task state");
        }

        // Should receive task in Executing state with Executing test
        let executing_task = res_rx.recv().await.unwrap();
        if let TaskState::Executing { tests } = &executing_task.state {
            assert_eq!(tests.len(), 1);
            assert!(matches!(tests[0].state, TestState::Executing { .. }));
        } else {
            panic!("Expected Executing task state");
        }

        // Should receive task in Executing state with Correct test result
        let updated_task = res_rx.recv().await.unwrap();
        if let TaskState::Executing { tests } = &updated_task.state {
            assert_eq!(tests.len(), 1);
            assert!(matches!(tests[0].state, TestState::Correct { .. }));
        } else {
            panic!("Expected Executing task state");
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
        let run_result = RunResult {
            status: -1,
            stdout: "actual_output".to_string(),
            stderr: "actual_error".to_string(),
            execution_time_ms: 150,
            peak_memory_usage_bytes: 2048,
        };

        let mut executor = MockExecutor::new();
        executor.expect_run().return_const(Ok(run_result));
        let executor = Arc::new(executor);

        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, run_rx) = mpsc::channel(10);
        handle_running(res_tx, run_rx, executor);

        let task = create_compiled_task();
        run_tx.send(task.clone()).await.unwrap();

        // Skip to final result
        res_rx.recv().await.unwrap();
        res_rx.recv().await.unwrap();
        res_rx.recv().await.unwrap();

        let done_task = res_rx.recv().await.unwrap();
        if let TaskState::Done { results } = &done_task.state {
            assert_eq!(results.len(), 1);
            if let TestState::Wrong {
                expected_stdout,
                expected_stderr,
                expected_status,
                actual_status,
                resources,
            } = &results[0].state
            {
                assert_eq!(expected_stdout, "expected_output");
                assert_eq!(expected_stderr, "");
                assert_eq!(*expected_status, 0);
                assert_eq!(*actual_status, -1);
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
    async fn test_execution_error() {
        let run_responses = vec![
            Err(RunError::LimitsExceeded {
                result: RunResult {
                    status: 0,
                    stdout: "partial_output".to_string(),
                    stderr: "".to_string(),
                    execution_time_ms: 2000,
                    peak_memory_usage_bytes: 128 * 1024 * 1024,
                },
                limit_type: TestLimitType::Time,
            }),
            Err(RunError::Internal {
                msg: "binary not found".to_string(),
            }),
        ];

        for run_res in run_responses {
            let mut executor = MockExecutor::new();
            executor.expect_run().return_const(run_res.clone());
            let executor = Arc::new(executor);

            let (res_tx, mut res_rx) = mpsc::channel(10);
            let (run_tx, run_rx) = mpsc::channel(10);
            handle_running(res_tx, run_rx, executor);

            let task = create_compiled_task();
            run_tx.send(task.clone()).await.unwrap();

            // Skip to final result
            res_rx.recv().await.unwrap();
            res_rx.recv().await.unwrap();
            res_rx.recv().await.unwrap();

            let done_task = res_rx.recv().await.unwrap();
            if let TaskState::Done { results } = &done_task.state {
                assert_eq!(results.len(), 1);
                assert!(
                    matches!(
                        (results[0].state.clone(), run_res.clone().unwrap_err()),
                        (
                            TestState::LimitsExceeded { resources, limit_type: actual_limit_type },
                            RunError::LimitsExceeded { result, limit_type: expected_limit_type }
                        )
                        if result.execution_time_ms == resources.execution_time_ms &&
                           result.peak_memory_usage_bytes == resources.peak_memory_usage_bytes &&
                           actual_limit_type == expected_limit_type
                    ) || matches!(
                        (results[0].state.clone(), run_res.clone().unwrap_err()),
                        (TestState::InternalError { .. }, RunError::Internal { .. })
                    )
                );
            } else {
                panic!("Unexpected task state");
            }
        }
    }

    #[tokio::test]
    async fn test_multiple_tests() {
        let mut executor = MockExecutor::new();
        executor.expect_run().times(1).return_const(Ok(RunResult {
            status: 0,
            stdout: "output1".to_string(),
            stderr: "".to_string(),
            execution_time_ms: 100,
            peak_memory_usage_bytes: 1024,
        }));
        executor.expect_run().times(1).return_const(Ok(RunResult {
            status: 0,
            stdout: "wrong_output".to_string(),
            stderr: "".to_string(),
            execution_time_ms: 120,
            peak_memory_usage_bytes: 1024,
        }));
        executor.expect_run().times(1).return_const(Ok(RunResult {
            status: -1, // Wrong status code
            stdout: "output3".to_string(),
            stderr: "".to_string(),
            execution_time_ms: 120,
            peak_memory_usage_bytes: 1024,
        }));
        executor.expect_run().times(1).return_const(Ok(RunResult {
            status: 0,
            stdout: "output4".to_string(),
            stderr: "ERROR".to_string(),
            execution_time_ms: 120,
            peak_memory_usage_bytes: 1024,
        }));

        let executor = Arc::new(executor);
        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, run_rx) = mpsc::channel(10);
        handle_running(res_tx, run_rx, executor);

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

        let mut received_messages = Vec::new();
        for _ in 0..9 {
            // 1 init state + (2 changes for each test) * 4 tests = 9
            received_messages.push(res_rx.recv().await.unwrap());
        }

        // Each test runs in parallel, which makes the order of intermediate
        // states non-deterministic, so comparisons need to be done without
        // considering the order. Therefore, we will convert the task state
        // vector to a vector of vectors, where the index of each inner vector
        // corresponds to the test index. Each inner vector will store the
        // history of how the test state changed without duplicates. It is
        // precisely this vector of vectors that we will assert.
        let tests_states_changes: Vec<Vec<_>> = (0..task.test_data.len())
            .map(|i| {
                received_messages
                    .iter()
                    .filter_map(|msg| match msg.state {
                        TaskState::Executing { ref tests } => tests.get(i),
                        _ => None,
                    })
                    .unique()
                    .collect()
            })
            .collect();

        println!("{:#?}", tests_states_changes);

        // Assert 1st test
        assert!(matches!(
            tests_states_changes[0][0].state,
            TestState::Pending { .. }
        ));
        assert!(matches!(
            tests_states_changes[0][1].state,
            TestState::Executing { .. }
        ));
        assert!(matches!(
            tests_states_changes[0][2].state,
            TestState::Correct { .. }
        ));
        assert_eq!(tests_states_changes[0][2].current_stdout, "output1");
        assert_eq!(tests_states_changes[0][2].current_stderr, "");
        // TODO: assert current status

        // Assert 2nd test
        assert!(matches!(
            tests_states_changes[1][0].state,
            TestState::Pending { .. }
        ));
        assert!(matches!(
            tests_states_changes[1][1].state,
            TestState::Executing { .. }
        ));
        assert!(matches!(
            &tests_states_changes[1][2].state,
            TestState::Wrong { expected_status, expected_stdout, expected_stderr, .. }
            if *expected_status == 0 && expected_stdout == "output2" && expected_stderr == ""
        ));
        assert_eq!(tests_states_changes[1][2].current_stdout, "wrong_output");
        assert_eq!(tests_states_changes[1][2].current_stderr, "");
        // TODO: assert current status

        // Assert 3rd test
        assert!(matches!(
            tests_states_changes[2][0].state,
            TestState::Pending { .. }
        ));
        assert!(matches!(
            tests_states_changes[2][1].state,
            TestState::Executing { .. }
        ));
        assert!(matches!(
            &tests_states_changes[2][2].state,
            TestState::Wrong { expected_status, expected_stdout, expected_stderr, .. }
            if *expected_status == 0 && expected_stdout == "output3" && expected_stderr == ""
        ));
        assert_eq!(tests_states_changes[2][2].current_stdout, "output3");
        assert_eq!(tests_states_changes[2][2].current_stderr, "");
        // TODO: assert current status

        // Assert 4th test
        assert!(matches!(
            tests_states_changes[3][0].state,
            TestState::Pending { .. }
        ));
        assert!(matches!(
            tests_states_changes[3][1].state,
            TestState::Executing { .. }
        ));
        assert!(matches!(
            &tests_states_changes[3][2].state,
            TestState::Wrong { expected_status, expected_stdout, expected_stderr, .. }
            if *expected_status == 0 && expected_stdout == "output4" && expected_stderr == ""
        ));
        assert_eq!(tests_states_changes[3][2].current_stdout, "output4");
        assert_eq!(tests_states_changes[3][2].current_stderr, "ERROR");
        // TODO: assert current status

        let done_task = res_rx.recv().await.unwrap();
        if let TaskState::Done { results } = &done_task.state {
            assert_eq!(results.len(), 4);
            assert!(matches!(results[0].state, TestState::Correct { .. }));
            assert!(matches!(results[1].state, TestState::Wrong { .. }));
            assert!(matches!(results[2].state, TestState::Wrong { .. }));
            assert!(matches!(results[3].state, TestState::Wrong { .. }));
        } else {
            panic!("Expected Done state, actual state: {:#?}", done_task.state);
        }
    }

    #[tokio::test]
    async fn test_non_compiled_task_skipped() {
        let executor = Arc::new(MockExecutor::new());
        let (res_tx, mut res_rx) = mpsc::channel(10);
        let (run_tx, run_rx) = mpsc::channel(10);

        handle_running(res_tx, run_rx, executor);

        let mut task = create_compiled_task();
        task.state = TaskState::Compiling; // Not compiled

        run_tx.send(task.clone()).await.unwrap();

        // Should not receive any messages since task is not compiled
        tokio::time::timeout(std::time::Duration::from_millis(100), res_rx.recv())
            .await
            .expect_err("Should not receive any messages for non-compiled task");
    }
}
