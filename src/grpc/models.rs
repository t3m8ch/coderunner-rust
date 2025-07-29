use std::time::SystemTime;

use chrono::{DateTime, Utc};

tonic::include_proto!("coderunnerapi");

pub fn chrono_to_prost(dt: DateTime<Utc>) -> prost_types::Timestamp {
    let dt: SystemTime = dt.into();
    let dt: prost_types::Timestamp = dt.into();
    dt
}
