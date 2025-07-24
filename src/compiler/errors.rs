use crate::domain::CompilationLimitType;

#[derive(Debug, Clone)]
pub enum CompilationError {
    CompilationFailed { msg: String },
    CompilationLimitsExceeded(CompilationLimitType),
}
