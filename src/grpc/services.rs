use std::sync::Arc;

use tokio::sync::mpsc::{Receiver, Sender, channel};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::{
    constants::{COMPILE_TX_ERR, STREAM_TX_ERR},
    core::{
        domain,
        pipeline::{compiling::handle_compiling, running::handle_running},
        traits::executor::Executor,
    },
    grpc::{
        mappers::ConversionError,
        models::{SubmitCodeRequest, Task as GrpcTask, testing_service_server::TestingService},
    },
};

#[derive(Clone, Debug)]
pub struct TestingServiceImpl {
    executor: Arc<dyn Executor>,
}

#[tonic::async_trait]
impl TestingService for TestingServiceImpl {
    type SubmitCodeStream = ReceiverStream<Result<GrpcTask, Status>>;

    #[tracing::instrument]
    async fn submit_code(
        &self,
        request: Request<SubmitCodeRequest>,
    ) -> Result<Response<Self::SubmitCodeStream>, Status> {
        tracing::info!("Received request: {:?}", request);

        // TODO: Take a cached artifact if the code hasn't changed
        // TODO: Separate 'musl' and 'glibc' executable artifacts and languages
        // TODO: Add cancelling task by user
        // TODO: Implement Compiler and Runner
        // TODO: Remove magic numbers in channels

        let (stream_tx, stream_rx) = channel::<Result<GrpcTask, Status>>(128);
        let (res_tx, res_rx) = channel::<domain::Task>(128);
        let (run_tx, run_rx) = channel::<domain::Task>(128);
        let (compile_tx, compile_rx) = channel::<domain::Task>(128);

        handle_compiling(res_tx.clone(), run_tx, compile_rx, self.executor.clone());
        handle_running(res_tx, run_rx, self.executor.clone());

        let domain_task: Result<domain::Task, ConversionError> = request.into_inner().try_into();
        match domain_task {
            Ok(domain_task) => {
                self.process_valid_request(domain_task, stream_tx, stream_rx, compile_tx, res_rx)
                    .await
            }
            Err(error) => Err(Status::invalid_argument(error.to_string())),
        }
    }
}

impl TestingServiceImpl {
    pub fn new(executor: Arc<dyn Executor>) -> Self {
        Self { executor }
    }

    async fn process_valid_request(
        &self,
        domain_task: domain::Task,
        stream_tx: Sender<Result<GrpcTask, Status>>,
        stream_rx: Receiver<Result<GrpcTask, Status>>,
        compile_tx: Sender<domain::Task>,
        mut res_rx: Receiver<domain::Task>,
    ) -> Result<Response<ReceiverStream<Result<GrpcTask, Status>>>, Status> {
        stream_tx
            .send(domain_task.clone().try_into())
            .await
            .expect(STREAM_TX_ERR);

        compile_tx.send(domain_task).await.expect(COMPILE_TX_ERR);

        tokio::spawn(async move {
            while let Some(task) = res_rx.recv().await {
                tracing::debug!("Send new state of task: {:?}", task);
                stream_tx.send(task.try_into()).await.expect(STREAM_TX_ERR);
            }
        });

        Ok(Response::new(ReceiverStream::new(stream_rx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        core::{
            domain::{
                Artifact, ArtifactKind, CompilationLimitType, CompilationLimits, ExecutionLimits,
                Language, TestLimitType,
            },
            traits::{
                compiler::CompileError,
                runner::{RunError, RunResult},
            },
        },
        grpc::models::{
            CompilationLimits as GrpcCompilationLimits, ExecutionLimits as GrpcExecutionLimits,
            Language as GrpcLanguage, SubmitCodeRequest, TestData as GrpcTestData,
        },
    };
    use std::sync::Arc;
    use tokio_stream::StreamExt;
    use tonic::Request;
    use uuid::Uuid;

    #[derive(Debug)]
    struct MockCompiler {
        result: Result<Artifact, CompileError>,
    }

    #[async_trait::async_trait]
    impl Compiler for MockCompiler {
        async fn compile(
            &self,
            _source: &str,
            _language: &Language,
            _limits: &CompilationLimits,
        ) -> Result<Artifact, CompileError> {
            self.result.clone()
        }
    }

    #[derive(Debug)]
    struct MockRunner {
        result: Result<RunResult, RunError>,
    }

    #[async_trait::async_trait]
    impl Runner for MockRunner {
        async fn run(
            &self,
            _artifact: &Artifact,
            _stdin: &str,
            _limits: &ExecutionLimits,
        ) -> Result<RunResult, RunError> {
            self.result.clone()
        }
    }

    fn create_valid_code() -> String {
        "int main() { return 0; }".to_string()
    }

    fn create_valid_language() -> i32 {
        GrpcLanguage::GnuCpp as i32
    }

    fn create_valid_compilation_limits() -> GrpcCompilationLimits {
        GrpcCompilationLimits {
            time_ms: Some(5000),
            memory_bytes: Some(128 * 1024 * 1024),
            executable_size_bytes: Some(16 * 1024 * 1024),
        }
    }

    fn create_valid_execution_limits() -> GrpcExecutionLimits {
        GrpcExecutionLimits {
            time_ms: Some(1000),
            memory_bytes: Some(64 * 1024 * 1024),
            pids_count: Some(1),
            stdout_size_bytes: Some(1024),
            stderr_size_bytes: Some(1024),
        }
    }

    fn create_valid_test_data() -> Vec<GrpcTestData> {
        vec![GrpcTestData {
            stdin: "input".to_string(),
            stdout: "expected_output".to_string(),
            stderr: "".to_string(),
            status: 0,
        }]
    }

    fn create_valid_request() -> SubmitCodeRequest {
        SubmitCodeRequest {
            code: create_valid_code(),
            language: create_valid_language(),
            compilation_limits: Some(create_valid_compilation_limits()),
            execution_limits: Some(create_valid_execution_limits()),
            test_data: create_valid_test_data(),
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

        let runner_result = RunResult {
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

        // Should receive accepted task
        let accepted_task = stream.next().await.unwrap().unwrap();
        if let Some(crate::grpc::models::task::State::Accepted(_)) = accepted_task.state {
            // Test passed
        } else {
            panic!("Expected Accepted state, got: {:?}", accepted_task.state);
        }

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
            result: Err(CompileError::CompilationFailed {
                msg: "syntax error".to_string(),
            }),
        });

        let runner = Arc::new(MockRunner {
            result: Ok(RunResult {
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

        // Skip accepted and compiling messages
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
            result: Err(CompileError::CompilationLimitsExceeded(
                CompilationLimitType::Time,
            )),
        });

        let runner = Arc::new(MockRunner {
            result: Ok(RunResult {
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

        // Skip accepted and compiling messages
        let _accepted = stream.next().await.unwrap().unwrap();
        let _compiling = stream.next().await.unwrap().unwrap();

        // Should receive compilation limits exceeded task
        let limits_exceeded_task = stream.next().await.unwrap().unwrap();
        if let Some(crate::grpc::models::task::State::LimitsExceeded(_)) =
            limits_exceeded_task.state
        {
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
            result: Err(RunError::Crash {
                result: RunResult {
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
            result: Err(RunError::LimitsExceeded {
                result: RunResult {
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
    async fn test_submit_code_invalid_request_missing_compilation_limits() {
        let compiler = Arc::new(MockCompiler {
            result: Ok(Artifact {
                id: Uuid::new_v4(),
                kind: ArtifactKind::Executable,
            }),
        });

        let runner = Arc::new(MockRunner {
            result: Ok(RunResult {
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
            code: create_valid_code(),
            language: create_valid_language(),
            compilation_limits: None, // Missing required field
            execution_limits: Some(create_valid_execution_limits()),
            test_data: create_valid_test_data(),
        };

        let request = Request::new(invalid_request);
        let response = service.submit_code(request).await;

        // Should return an error with InvalidArgument status
        assert!(response.is_err());
        let error = response.unwrap_err();
        assert_eq!(error.code(), tonic::Code::InvalidArgument);
        assert!(error.message().contains("compilation_limits"));
    }

    #[tokio::test]
    async fn test_submit_code_invalid_request_missing_execution_limits() {
        let compiler = Arc::new(MockCompiler {
            result: Ok(Artifact {
                id: Uuid::new_v4(),
                kind: ArtifactKind::Executable,
            }),
        });

        let runner = Arc::new(MockRunner {
            result: Ok(RunResult {
                status: 0,
                stdout: "".to_string(),
                stderr: "".to_string(),
                execution_time_ms: 0,
                peak_memory_usage_bytes: 0,
            }),
        });

        let service = TestingServiceImpl::new(compiler, runner);

        // Create invalid request (missing execution_limits)
        let invalid_request = SubmitCodeRequest {
            code: create_valid_code(),
            language: create_valid_language(),
            compilation_limits: Some(create_valid_compilation_limits()),
            execution_limits: None, // Missing required field
            test_data: create_valid_test_data(),
        };

        let request = Request::new(invalid_request);
        let response = service.submit_code(request).await;

        // Should return an error with InvalidArgument status
        assert!(response.is_err());
        let error = response.unwrap_err();
        assert_eq!(error.code(), tonic::Code::InvalidArgument);
        assert!(error.message().contains("execution_limits"));
    }

    #[tokio::test]
    async fn test_submit_code_is_too_large() {
        let compiler = Arc::new(MockCompiler {
            result: Ok(Artifact {
                id: Uuid::new_v4(),
                kind: ArtifactKind::Executable,
            }),
        });

        let runner = Arc::new(MockRunner {
            result: Ok(RunResult {
                status: 0,
                stdout: "".to_string(),
                stderr: "".to_string(),
                execution_time_ms: 0,
                peak_memory_usage_bytes: 0,
            }),
        });

        let service = TestingServiceImpl::new(compiler, runner);

        // Create invalid request (code is too large)
        let invalid_request = SubmitCodeRequest {
            code: "a".repeat(205_000), // Is too large, >200 KB
            language: create_valid_language(),
            compilation_limits: Some(create_valid_compilation_limits()),
            execution_limits: Some(create_valid_execution_limits()),
            test_data: create_valid_test_data(),
        };

        let request = Request::new(invalid_request);
        let response = service.submit_code(request).await;

        // Should return an error with InvalidArgument status
        assert!(response.is_err());
        let error = response.unwrap_err();
        assert_eq!(error.code(), tonic::Code::InvalidArgument);
        assert!(error.message().to_lowercase().contains("code is too large"));
    }

    #[tokio::test]
    async fn test_submit_code_no_test_data() {
        let compiler = Arc::new(MockCompiler {
            result: Ok(Artifact {
                id: Uuid::new_v4(),
                kind: ArtifactKind::Executable,
            }),
        });

        let runner = Arc::new(MockRunner {
            result: Ok(RunResult {
                status: 0,
                stdout: "".to_string(),
                stderr: "".to_string(),
                execution_time_ms: 0,
                peak_memory_usage_bytes: 0,
            }),
        });

        let service = TestingServiceImpl::new(compiler, runner);

        // Create invalid request (no test data)
        let invalid_request = SubmitCodeRequest {
            code: create_valid_code(),
            language: create_valid_language(),
            compilation_limits: Some(create_valid_compilation_limits()),
            execution_limits: Some(create_valid_execution_limits()),
            test_data: vec![], // No test data
        };

        let request = Request::new(invalid_request);
        let response = service.submit_code(request).await;

        // Should return an error with InvalidArgument status
        assert!(response.is_err());
        let error = response.unwrap_err();
        assert_eq!(error.code(), tonic::Code::InvalidArgument);
        assert!(error.message().to_lowercase().contains("no test data"));
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
            result: Ok(RunResult {
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
                status: 0,
            },
            GrpcTestData {
                stdin: "input2".to_string(),
                stdout: "expected_output".to_string(),
                stderr: "".to_string(),
                status: 0,
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

    #[tokio::test]
    async fn test_submit_code_compilation_internal_error() {
        let compiler = Arc::new(MockCompiler {
            result: Err(CompileError::Internal {
                msg: "Tux is sad and won't work :(".to_string(),
            }),
        });

        let runner = Arc::new(MockRunner {
            result: Ok(RunResult {
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

        // Skip accepted and compiling messages
        let _accepted = stream.next().await.unwrap().unwrap();
        let _compiling = stream.next().await.unwrap().unwrap();

        // Next message should be an internal error
        let error_result = stream.next().await.unwrap();
        assert!(error_result.is_err());

        let error = error_result.unwrap_err();
        assert_eq!(error.code(), tonic::Code::Internal);
        assert_eq!(error.message(), "Internal error");
    }

    #[tokio::test]
    async fn test_submit_code_execution_internal_error() {
        let artifact = Artifact {
            id: Uuid::new_v4(),
            kind: ArtifactKind::Executable,
        };

        let compiler = Arc::new(MockCompiler {
            result: Ok(artifact),
        });

        let runner = Arc::new(MockRunner {
            result: Err(RunError::Internal {
                msg: "Internal runner error".to_string(),
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

        // Last message should be Done with InternalError test state
        let done_task = messages.last().unwrap();
        if let Some(crate::grpc::models::task::State::Done(done)) = &done_task.state {
            assert_eq!(done.tests.len(), 1);
            if let Some(test) = &done.tests[0].state {
                if let Some(crate::grpc::models::test::State::InternalError(_)) = &test.state {
                    // Test passed - internal error detected
                } else {
                    panic!("Expected InternalError test state, got: {:?}", test.state);
                }
            } else {
                panic!("Expected test state");
            }
        } else {
            panic!("Expected Done state");
        }
    }
}
