use std::time::SystemTime;

use chrono::{DateTime, Utc};

use crate::grpc::models::task::State;

tonic::include_proto!("coderunnerapi");

#[tracing::instrument]
pub fn create_init_grpc_task() -> Task {
    let created_at = chrono::Utc::now();

    let task = Task {
        id: "UNDEFINED".to_string(),
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
