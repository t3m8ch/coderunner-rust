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
