use crate::domain::CompilationLimitType;

// TODO: Separate student error and server error when compiling
#[derive(Debug, Clone)]
pub enum CompilationError {
    CompilationFailed { msg: String },
    CompilationLimitsExceeded(CompilationLimitType),
}
