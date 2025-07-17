use tokio::sync::mpsc::channel;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::grpc::models::{SubmitCodeRequest, Task, testing_service_server::TestingService};

pub struct TestingServiceImpl;

#[tonic::async_trait]
impl TestingService for TestingServiceImpl {
    type SubmitCodeStream = ReceiverStream<Result<Task, Status>>;

    // TODO: Implement
    async fn submit_code(
        &self,
        request: Request<SubmitCodeRequest>,
    ) -> Result<Response<Self::SubmitCodeStream>, Status> {
        println!("Received request: {:?}", request);

        let (tx, rx) = channel(3);

        tx.send(Ok(Task::default())).await.unwrap();
        tx.send(Ok(Task::default())).await.unwrap();
        tx.send(Ok(Task::default())).await.unwrap();

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}
