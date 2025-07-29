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
        // TODO: Think about 'pending' state
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
