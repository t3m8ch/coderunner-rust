use std::sync::Arc;

use tokio::sync::mpsc::channel;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::{
    compiler::traits::Compiler,
    constants::{COMPILE_TX_ERR, STREAM_TX_ERR},
    domain,
    grpc::{
        mappers::ConversionError,
        models::{
            InvalidRequest, SubmitCodeRequest, Task as GrpcTask, create_init_grpc_task,
            task::State, testing_service_server::TestingService,
        },
    },
    pipeline::{compiling::handle_compiling, running::handle_running},
    runner::traits::Runner,
};

#[derive(Clone, Debug)]
pub struct TestingServiceImpl {
    compiler: Arc<dyn Compiler>,
    runner: Arc<dyn Runner>,
}

impl TestingServiceImpl {
    pub fn new(compiler: Arc<dyn Compiler>, runner: Arc<dyn Runner>) -> Self {
        Self { compiler, runner }
    }
}

#[tonic::async_trait]
impl TestingService for TestingServiceImpl {
    type SubmitCodeStream = ReceiverStream<Result<GrpcTask, Status>>;

    // TODO: Implement
    #[tracing::instrument]
    async fn submit_code(
        &self,
        request: Request<SubmitCodeRequest>,
    ) -> Result<Response<Self::SubmitCodeStream>, Status> {
        tracing::info!("Received request: {:?}", request);

        // TODO: Add limits to code size
        // TODO: Add limits to executable size
        // TODO: Add limits to stdin size
        // TODO: Think about 'pending' state in task
        // TODO: Think about 'checking' state in test
        // TODO: Take a cached artifact if the code hasn't changed
        // TODO: Separate 'musl' and 'glibc' executable artifacts and languages
        // TODO: Rename 'Unavailable' to 'InternalError'
        // TODO: Add expected status to TestData

        // TODO: Remove magic numbers
        let (stream_tx, stream_rx) = channel::<Result<GrpcTask, Status>>(128);
        let (res_tx, mut res_rx) = channel::<domain::Task>(128);
        let (run_tx, run_rx) = channel::<domain::Task>(128);
        let (compile_tx, compile_rx) = channel::<domain::Task>(128);

        let task = create_init_grpc_task();
        stream_tx.send(Ok(task.clone())).await.expect(STREAM_TX_ERR);

        let domain_task: Result<domain::Task, ConversionError> = request.into_inner().try_into();
        match domain_task {
            Ok(domain_task) => {
                stream_tx
                    .send(Ok(domain_task.clone().into()))
                    .await
                    .expect(STREAM_TX_ERR);

                compile_tx.send(domain_task).await.expect(COMPILE_TX_ERR);

                handle_compiling(res_tx.clone(), run_tx, compile_rx, self.compiler.clone());
                handle_running(res_tx, run_rx, self.runner.clone());

                tokio::spawn(async move {
                    while let Some(task) = res_rx.recv().await {
                        tracing::debug!("Send new state of task: {:?}", task);
                        stream_tx.send(Ok(task.into())).await.expect(STREAM_TX_ERR);
                    }
                });
            }
            Err(e) => {
                let task = GrpcTask {
                    state: Some(State::InvalidRequest(InvalidRequest {
                        message: e.to_string(),
                    })),
                    ..task
                };
                stream_tx.send(Ok(task)).await.expect(STREAM_TX_ERR);
            }
        }

        Ok(Response::new(ReceiverStream::new(stream_rx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        compiler::errors::CompilationError,
        domain::{
            Artifact, ArtifactKind, CompilationLimitType, ExecutionLimits, Language, TestLimitType,
        },
        grpc::models::{
            CompilationLimits as GrpcCompilationLimits, ExecutionLimits as GrpcExecutionLimits,
            Language as GrpcLanguage, SubmitCodeRequest, TestData as GrpcTestData,
        },
        runner::traits::{RunnerError, RunnerResult},
    };
    use std::sync::Arc;
    use tokio_stream::StreamExt;
    use tonic::Request;
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

    #[derive(Debug)]
    struct MockRunner {
        result: Result<RunnerResult, RunnerError>,
    }

    #[async_trait::async_trait]
    impl Runner for MockRunner {
        async fn run(
            &self,
            _artifact: &Artifact,
            _stdin: &str,
            _limits: &ExecutionLimits,
        ) -> Result<RunnerResult, RunnerError> {
            self.result.clone()
        }
    }

    fn create_valid_request() -> SubmitCodeRequest {
        SubmitCodeRequest {
            code: "int main() { return 0; }".to_string(),
            language: GrpcLanguage::GnuCpp as i32,
            compilation_limits: Some(GrpcCompilationLimits {
                time_ms: Some(5000),
                memory_bytes: Some(128 * 1024 * 1024),
            }),
            execution_limits: Some(GrpcExecutionLimits {
                time_ms: Some(1000),
                memory_bytes: Some(64 * 1024 * 1024),
                pids_count: Some(1),
                stdout_size_bytes: Some(1024),
                stderr_size_bytes: Some(1024),
            }),
            test_data: vec![GrpcTestData {
                stdin: "input".to_string(),
                stdout: "expected_output".to_string(),
                stderr: "".to_string(),
            }],
        }
    }

    #[tokio::test]
    async fn test_submit_code_successful_flow() {
        let artifact = Artifact {
            id: Uuid::new_v4(),
            kind: ArtifactKind::Executable,
        };

        let compiler = Arc::new(MockCompiler {
            result: Ok(artifact.clone()),
        });

        let runner_result = RunnerResult {
            status: 0,
            stdout: "expected_output".to_string(),
            stderr: "".to_string(),
            execution_time_ms: 100,
            peak_memory_usage_bytes: 1024,
        };

        let runner = Arc::new(MockRunner {
            result: Ok(runner_result),
        });

        let service = TestingServiceImpl::new(compiler, runner);
        let request = Request::new(create_valid_request());

        let response = service.submit_code(request).await.unwrap();
        let mut stream = response.into_inner();

        // Should receive initial pending task
        let initial_task = stream.next().await.unwrap().unwrap();
        assert_eq!(initial_task.id, "UNDEFINED");

        // Should receive accepted task
        let accepted_task = stream.next().await.unwrap().unwrap();
        assert_ne!(accepted_task.id, "UNDEFINED");

        // Should receive compiling task
        let _compiling_task = stream.next().await.unwrap().unwrap();
        // Should receive compiled task
        let _compiled_task = stream.next().await.unwrap().unwrap();
        // Should receive executing task
        let _executing_task = stream.next().await.unwrap().unwrap();
        // Should receive updated executing task
        let _updated_executing = stream.next().await.unwrap().unwrap();
        // Should receive done task
        let done_task = stream.next().await.unwrap().unwrap();

        // Verify final state is Done
        if let Some(crate::grpc::models::task::State::Done(_)) = done_task.state {
            // Test passed
        } else {
            panic!("Expected Done state, got: {:?}", done_task.state);
        }
    }

    #[tokio::test]
    async fn test_submit_code_compilation_failed() {
        let compiler = Arc::new(MockCompiler {
            result: Err(CompilationError::CompilationFailed {
                msg: "syntax error".to_string(),
            }),
        });

        let runner = Arc::new(MockRunner {
            result: Ok(RunnerResult {
                status: 0,
                stdout: "".to_string(),
                stderr: "".to_string(),
                execution_time_ms: 0,
                peak_memory_usage_bytes: 0,
            }),
        });

        let service = TestingServiceImpl::new(compiler, runner);
        let request = Request::new(create_valid_request());

        let response = service.submit_code(request).await.unwrap();
        let mut stream = response.into_inner();

        // Skip initial messages
        let _initial = stream.next().await.unwrap().unwrap();
        let _accepted = stream.next().await.unwrap().unwrap();
        let _compiling = stream.next().await.unwrap().unwrap();

        // Should receive compilation failed task
        let failed_task = stream.next().await.unwrap().unwrap();
        if let Some(crate::grpc::models::task::State::CompilationFailed(failed)) = failed_task.state
        {
            assert_eq!(failed.message, "syntax error");
        } else {
            panic!("Expected CompilationFailed state");
        }
    }

    #[tokio::test]
    async fn test_submit_code_compilation_limits_exceeded() {
        let compiler = Arc::new(MockCompiler {
            result: Err(CompilationError::CompilationLimitsExceeded(
                CompilationLimitType::Time,
            )),
        });

        let runner = Arc::new(MockRunner {
            result: Ok(RunnerResult {
                status: 0,
                stdout: "".to_string(),
                stderr: "".to_string(),
                execution_time_ms: 0,
                peak_memory_usage_bytes: 0,
            }),
        });

        let service = TestingServiceImpl::new(compiler, runner);
        let request = Request::new(create_valid_request());

        let response = service.submit_code(request).await.unwrap();
        let mut stream = response.into_inner();

        // Skip initial messages
        let _initial = stream.next().await.unwrap().unwrap();
        let _accepted = stream.next().await.unwrap().unwrap();
        let _compiling = stream.next().await.unwrap().unwrap();

        // Should receive limits exceeded task
        let limits_task = stream.next().await.unwrap().unwrap();
        if let Some(crate::grpc::models::task::State::LimitsExceeded(_)) = limits_task.state {
            // Test passed
        } else {
            panic!("Expected LimitsExceeded state");
        }
    }

    #[tokio::test]
    async fn test_submit_code_execution_crash() {
        let artifact = Artifact {
            id: Uuid::new_v4(),
            kind: ArtifactKind::Executable,
        };

        let compiler = Arc::new(MockCompiler {
            result: Ok(artifact),
        });

        let runner = Arc::new(MockRunner {
            result: Err(RunnerError::Crash {
                result: RunnerResult {
                    status: -1,
                    stdout: "".to_string(),
                    stderr: "segmentation fault".to_string(),
                    execution_time_ms: 50,
                    peak_memory_usage_bytes: 512,
                },
            }),
        });

        let service = TestingServiceImpl::new(compiler, runner);
        let request = Request::new(create_valid_request());

        let response = service.submit_code(request).await.unwrap();
        let mut stream = response.into_inner();

        // Collect all messages
        let mut messages = Vec::new();
        while let Some(msg) = stream.next().await {
            messages.push(msg.unwrap());
        }

        // Last message should be Done with Crash test state
        let done_task = messages.last().unwrap();
        if let Some(crate::grpc::models::task::State::Done(done)) = &done_task.state {
            assert_eq!(done.tests.len(), 1);
            if let Some(test) = &done.tests[0].state {
                if let Some(crate::grpc::models::test::State::Crash(_)) = &test.state {
                    // Test passed
                } else {
                    panic!("Expected Crash test state, got: {:?}", test.state);
                }
            } else {
                panic!("Expected test state");
            }
        } else {
            panic!("Expected Done state");
        }
    }

    #[tokio::test]
    async fn test_submit_code_execution_limits_exceeded() {
        let artifact = Artifact {
            id: Uuid::new_v4(),
            kind: ArtifactKind::Executable,
        };

        let compiler = Arc::new(MockCompiler {
            result: Ok(artifact),
        });

        let runner = Arc::new(MockRunner {
            result: Err(RunnerError::LimitsExceeded {
                result: RunnerResult {
                    status: 0,
                    stdout: "partial".to_string(),
                    stderr: "".to_string(),
                    execution_time_ms: 2000,
                    peak_memory_usage_bytes: 128 * 1024 * 1024,
                },
                limit_type: TestLimitType::Time,
            }),
        });

        let service = TestingServiceImpl::new(compiler, runner);
        let request = Request::new(create_valid_request());

        let response = service.submit_code(request).await.unwrap();
        let mut stream = response.into_inner();

        // Collect all messages
        let mut messages = Vec::new();
        while let Some(msg) = stream.next().await {
            messages.push(msg.unwrap());
        }

        // Last message should be Done with LimitsExceeded test state
        let done_task = messages.last().unwrap();
        if let Some(crate::grpc::models::task::State::Done(done)) = &done_task.state {
            assert_eq!(done.tests.len(), 1);
            if let Some(test) = &done.tests[0].state {
                if let Some(crate::grpc::models::test::State::LimitsExceeded(_)) = &test.state {
                    // Test passed
                } else {
                    panic!("Expected LimitsExceeded test state, got: {:?}", test.state);
                }
            } else {
                panic!("Expected test state");
            }
        } else {
            panic!("Expected Done state");
        }
    }

    #[tokio::test]
    async fn test_submit_code_invalid_request() {
        let compiler = Arc::new(MockCompiler {
            result: Ok(Artifact {
                id: Uuid::new_v4(),
                kind: ArtifactKind::Executable,
            }),
        });

        let runner = Arc::new(MockRunner {
            result: Ok(RunnerResult {
                status: 0,
                stdout: "".to_string(),
                stderr: "".to_string(),
                execution_time_ms: 0,
                peak_memory_usage_bytes: 0,
            }),
        });

        let service = TestingServiceImpl::new(compiler, runner);

        // Create invalid request (missing compilation_limits)
        let invalid_request = SubmitCodeRequest {
            code: "int main() { return 0; }".to_string(),
            language: GrpcLanguage::GnuCpp as i32,
            compilation_limits: None, // Missing required field
            execution_limits: Some(GrpcExecutionLimits {
                time_ms: Some(1000),
                memory_bytes: Some(64 * 1024 * 1024),
                pids_count: Some(1),
                stdout_size_bytes: Some(1024),
                stderr_size_bytes: Some(1024),
            }),
            test_data: vec![],
        };

        let request = Request::new(invalid_request);
        let response = service.submit_code(request).await.unwrap();
        let mut stream = response.into_inner();

        // Should receive initial pending task
        let _initial = stream.next().await.unwrap().unwrap();

        // Should receive invalid request task
        let invalid_task = stream.next().await.unwrap().unwrap();
        if let Some(crate::grpc::models::task::State::InvalidRequest(invalid)) = invalid_task.state
        {
            assert!(invalid.message.contains("compilation_limits"));
        } else {
            panic!("Expected InvalidRequest state");
        }
    }

    #[tokio::test]
    async fn test_submit_code_multiple_test_data() {
        let artifact = Artifact {
            id: Uuid::new_v4(),
            kind: ArtifactKind::Executable,
        };

        let compiler = Arc::new(MockCompiler {
            result: Ok(artifact),
        });

        let runner = Arc::new(MockRunner {
            result: Ok(RunnerResult {
                status: 0,
                stdout: "expected_output".to_string(),
                stderr: "".to_string(),
                execution_time_ms: 100,
                peak_memory_usage_bytes: 1024,
            }),
        });

        let service = TestingServiceImpl::new(compiler, runner);

        let mut request = create_valid_request();
        request.test_data = vec![
            GrpcTestData {
                stdin: "input1".to_string(),
                stdout: "expected_output".to_string(),
                stderr: "".to_string(),
            },
            GrpcTestData {
                stdin: "input2".to_string(),
                stdout: "expected_output".to_string(),
                stderr: "".to_string(),
            },
        ];

        let request = Request::new(request);
        let response = service.submit_code(request).await.unwrap();
        let mut stream = response.into_inner();

        // Collect all messages
        let mut messages = Vec::new();
        while let Some(msg) = stream.next().await {
            messages.push(msg.unwrap());
        }

        // Last message should be Done with 2 test results
        let done_task = messages.last().unwrap();
        if let Some(crate::grpc::models::task::State::Done(done)) = &done_task.state {
            assert_eq!(done.tests.len(), 2);
        } else {
            panic!("Expected Done state");
        }
    }
}
