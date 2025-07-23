use std::time::SystemTime;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::grpc::models::task::State;

tonic::include_proto!("coderunnerapi");

#[derive(Debug, Clone)]
pub struct TaskInCh {
    pub task: Task,
    pub req: SubmitCodeRequest,
}

impl TaskInCh {
    #[tracing::instrument]
    pub fn change_state(&self, new_state: State) -> Self {
        tracing::debug!("New state: {:?}", new_state);

        Self {
            task: Task {
                state: Some(new_state),
                ..self.task.clone()
            },
            req: self.req.clone(),
        }
    }
}

#[tracing::instrument]
pub fn create_init_task() -> Task {
    let created_at = chrono::Utc::now();

    let task = Task {
        id: Uuid::new_v4().to_string(),
        created_at: Some(chrono_to_prost(created_at)),
        updated_at: Some(chrono_to_prost(created_at)),
        language: Language::GnuCpp.into(),
        state: Some(State::Pending(Empty {})),
    };

    tracing::debug!("Init task: {:?}", task);
    task
}

pub fn chrono_to_prost(dt: DateTime<Utc>) -> prost_types::Timestamp {
    let dt: SystemTime = dt.into();
    let dt: prost_types::Timestamp = dt.into();
    dt
}
