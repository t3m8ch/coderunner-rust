use std::sync::Arc;

use tokio::sync::mpsc::channel;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::{
    compiler::traits::Compiler,
    domain,
    grpc::{
        mappers::ConversionError,
        models::{
            InvalidRequest, SubmitCodeRequest, Task as GrpcTask, create_init_grpc_task,
            task::State, testing_service_server::TestingService,
        },
    },
    pipeline::compiling::handle_compiling,
};

#[derive(Clone, Debug)]
pub struct TestingServiceImpl {
    compiler: Arc<dyn Compiler>,
}

impl TestingServiceImpl {
    pub fn new(compiler: Arc<dyn Compiler>) -> Self {
        Self { compiler }
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
        // TODO: Think about 'pending' state

        // TODO: Remove magic numbers
        let (stream_tx, stream_rx) = channel::<Result<GrpcTask, Status>>(128);
        let (res_tx, mut res_rx) = channel::<domain::Task>(128);
        let (run_tx, mut run_rx) = channel::<domain::Task>(128);
        let (compile_tx, compile_rx) = channel::<domain::Task>(128);

        let task = create_init_grpc_task();
        stream_tx
            .send(Ok(task.clone()))
            .await
            .expect("Failed to send task to stream_tx");

        let domain_task: Result<domain::Task, ConversionError> = request.into_inner().try_into();
        match domain_task {
            Ok(domain_task) => {
                stream_tx
                    .send(Ok(domain_task.clone().into()))
                    .await
                    .expect("Failed to send task to stream_tx");

                compile_tx
                    .send(domain_task)
                    .await
                    .expect("Failed to send task to compile_tx");

                handle_compiling(res_tx, run_tx, compile_rx, self.compiler.clone());

                tokio::spawn(async move {
                    while let Some(task) = run_rx.recv().await {
                        tracing::debug!("Running: {:?}", task);
                    }
                });

                tokio::spawn(async move {
                    while let Some(task) = res_rx.recv().await {
                        tracing::debug!("Send new state of task: {:?}", task);
                        stream_tx
                            .send(Ok(task.into()))
                            .await
                            .expect("Failed to send task to stream_tx");
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
                stream_tx
                    .send(Ok(task))
                    .await
                    .expect("Failed to send task to stream_tx");
            }
        }

        Ok(Response::new(ReceiverStream::new(stream_rx)))
    }
}
