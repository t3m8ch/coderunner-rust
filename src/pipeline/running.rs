use futures::stream::FuturesUnordered;
use std::sync::Arc;
use tokio_stream::StreamExt;

use tokio::sync::mpsc::{Receiver, Sender};

use crate::{
    domain::{Task, TaskState, Test, TestResources, TestState},
    runner::traits::{Runner, RunnerError},
};

const TASK_TX_ERR: &'static str = "Failed to send task to task_tx";
const RUN_TX_ERR: &'static str = "Failed to send task to run_tx";

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
