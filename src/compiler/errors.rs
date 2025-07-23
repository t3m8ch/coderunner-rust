use crate::grpc::models::{CompilationFailed, CompilationLimitsExceeded};

#[derive(Debug, Clone)]
pub enum CompilationError {
    CompilationFailed(CompilationFailed),
    CompilationLimitsExceeded(CompilationLimitsExceeded),
}
