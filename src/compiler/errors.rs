use crate::domain::CompilationLimitType;
use thiserror::Error;

// TODO: Separate student error and server error when compiling
#[derive(Debug, Clone, Error)]
pub enum CompilationError {
    #[error("Compilation failed: {msg}")]
    CompilationFailed { msg: String },
    #[error("Compilation limits exceeded: {0:?}")]
    CompilationLimitsExceeded(CompilationLimitType),
}
