use crate::domain;
use crate::grpc::models::{self, Empty, compilation_limits_exceeded, task, test_limits_exceeded};
use uuid::Uuid;

const MAX_CODE_SIZE: usize = 200 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    #[error("Missing required field: {field}")]
    MissingField { field: String },
    #[error("Code is too large, max size is {MAX_CODE_SIZE} bytes")]
    CodeIsTooLarge,
}

impl TryFrom<models::SubmitCodeRequest> for domain::Task {
    type Error = ConversionError;

    fn try_from(req: models::SubmitCodeRequest) -> Result<Self, ConversionError> {
        if req.code.len() > MAX_CODE_SIZE {
            return Err(ConversionError::CodeIsTooLarge);
        }

        let language = req.language().into();
        let compilation_limits = req
            .compilation_limits
            .ok_or_else(|| ConversionError::MissingField {
                field: "compilation_limits".to_string(),
            })?
            .into();
        let execution_limits = req
            .execution_limits
            .ok_or_else(|| ConversionError::MissingField {
                field: "execution_limits".to_string(),
            })?
            .into();

        let now = chrono::Utc::now();

        Ok(Self {
            id: Uuid::new_v4(),
            created_at: now,
            updated_at: now,
            code: req.code,
            language,
            compilation_limits,
            execution_limits,
            test_data: req.test_data.into_iter().map(Into::into).collect(),
            state: domain::TaskState::default(),
        })
    }
}

impl From<models::Language> for domain::Language {
    fn from(lang: models::Language) -> Self {
        match lang {
            models::Language::GnuCpp => domain::Language::GnuCpp,
        }
    }
}

impl From<models::CompilationLimits> for domain::CompilationLimits {
    fn from(limits: models::CompilationLimits) -> Self {
        Self {
            time_ms: limits.time_ms,
            memory_bytes: limits.memory_bytes,
            executable_size_bytes: limits.executable_size_bytes,
        }
    }
}

impl From<models::ExecutionLimits> for domain::ExecutionLimits {
    fn from(limits: models::ExecutionLimits) -> Self {
        Self {
            time_ms: limits.time_ms,
            memory_bytes: limits.memory_bytes,
            pids_count: limits.pids_count,
            stdout_size_bytes: limits.stdout_size_bytes,
            stderr_size_bytes: limits.stderr_size_bytes,
        }
    }
}

impl From<models::TestData> for domain::TestData {
    fn from(test_data: models::TestData) -> Self {
        Self {
            stdin: test_data.stdin,
            stdout: test_data.stdout,
            stderr: test_data.stderr,
        }
    }
}

impl From<models::TestResources> for domain::TestResources {
    fn from(resources: models::TestResources) -> Self {
        Self {
            execution_time_ms: resources.execution_time_ms as u64,
            peak_memory_usage_bytes: resources.peak_memory_usage_bytes as u64,
        }
    }
}

impl From<compilation_limits_exceeded::LimitType> for domain::CompilationLimitType {
    fn from(limit_type: compilation_limits_exceeded::LimitType) -> Self {
        match limit_type {
            compilation_limits_exceeded::LimitType::Ram => domain::CompilationLimitType::Ram,
            compilation_limits_exceeded::LimitType::Time => domain::CompilationLimitType::Time,
            compilation_limits_exceeded::LimitType::ExecutableSize => {
                domain::CompilationLimitType::ExecutableSize
            }
        }
    }
}

impl From<test_limits_exceeded::LimitType> for domain::TestLimitType {
    fn from(limit_type: test_limits_exceeded::LimitType) -> Self {
        match limit_type {
            test_limits_exceeded::LimitType::Ram => domain::TestLimitType::Ram,
            test_limits_exceeded::LimitType::Time => domain::TestLimitType::Time,
            test_limits_exceeded::LimitType::Pids => domain::TestLimitType::PidsCount,
            test_limits_exceeded::LimitType::StdoutSize => domain::TestLimitType::StdoutSize,
            test_limits_exceeded::LimitType::StderrSize => domain::TestLimitType::StderrSize,
        }
    }
}

impl From<domain::Task> for models::Task {
    fn from(task: domain::Task) -> Self {
        Self {
            id: task.id.to_string(),
            created_at: Some(models::chrono_to_prost(task.created_at)),
            updated_at: Some(models::chrono_to_prost(task.updated_at)),
            language: Into::<models::Language>::into(task.language) as i32,
            state: Some(task.state.into()),
        }
    }
}

impl From<domain::Language> for models::Language {
    fn from(lang: domain::Language) -> Self {
        match lang {
            domain::Language::GnuCpp => models::Language::GnuCpp,
        }
    }
}

impl From<domain::CompilationLimits> for models::CompilationLimits {
    fn from(limits: domain::CompilationLimits) -> Self {
        Self {
            time_ms: limits.time_ms,
            memory_bytes: limits.memory_bytes,
            executable_size_bytes: limits.executable_size_bytes,
        }
    }
}

impl From<domain::ExecutionLimits> for models::ExecutionLimits {
    fn from(limits: domain::ExecutionLimits) -> Self {
        Self {
            time_ms: limits.time_ms,
            memory_bytes: limits.memory_bytes,
            pids_count: limits.pids_count,
            stdout_size_bytes: limits.stdout_size_bytes,
            stderr_size_bytes: limits.stderr_size_bytes,
        }
    }
}

impl From<domain::TestData> for models::TestData {
    fn from(test_data: domain::TestData) -> Self {
        Self {
            stdin: test_data.stdin,
            stdout: test_data.stdout,
            stderr: test_data.stderr,
        }
    }
}

impl From<domain::TestResources> for models::TestResources {
    fn from(resources: domain::TestResources) -> Self {
        Self {
            execution_time_ms: resources.execution_time_ms as i64,
            peak_memory_usage_bytes: resources.peak_memory_usage_bytes as i64,
        }
    }
}

impl From<domain::CompilationLimitType> for compilation_limits_exceeded::LimitType {
    fn from(limit_type: domain::CompilationLimitType) -> Self {
        match limit_type {
            domain::CompilationLimitType::Ram => compilation_limits_exceeded::LimitType::Ram,
            domain::CompilationLimitType::Time => compilation_limits_exceeded::LimitType::Time,
            domain::CompilationLimitType::ExecutableSize => {
                compilation_limits_exceeded::LimitType::ExecutableSize
            }
        }
    }
}

impl From<domain::TestLimitType> for test_limits_exceeded::LimitType {
    fn from(limit_type: domain::TestLimitType) -> Self {
        match limit_type {
            domain::TestLimitType::Ram => test_limits_exceeded::LimitType::Ram,
            domain::TestLimitType::Time => test_limits_exceeded::LimitType::Time,
            domain::TestLimitType::PidsCount => test_limits_exceeded::LimitType::Pids,
            domain::TestLimitType::StdoutSize => test_limits_exceeded::LimitType::StdoutSize,
            domain::TestLimitType::StderrSize => test_limits_exceeded::LimitType::StderrSize,
        }
    }
}

impl From<domain::TestState> for models::test::State {
    fn from(state: domain::TestState) -> Self {
        match state {
            domain::TestState::Pending => models::test::State::Pending(Empty {}),
            domain::TestState::Executing => models::test::State::Executing(Empty {}),
            domain::TestState::Checking { resources } => {
                models::test::State::Checking(models::TestChecking {
                    resources: Some(resources.into()),
                })
            }
            domain::TestState::Correct { resources } => {
                models::test::State::Correct(models::TestCorrect {
                    resources: Some(resources.into()),
                })
            }
            domain::TestState::Wrong {
                expected_stdout,
                expected_stderr,
                resources,
            } => models::test::State::Wrong(models::TestWrong {
                expected_stdout,
                expected_stderr,
                resources: Some(resources.into()),
            }),
            domain::TestState::LimitsExceeded {
                limit_type,
                resources,
            } => models::test::State::LimitsExceeded(models::TestLimitsExceeded {
                r#type: Into::<test_limits_exceeded::LimitType>::into(limit_type) as i32,
                resources: Some(resources.into()),
            }),
            domain::TestState::Crash { resources } => {
                models::test::State::Crash(models::TestCrash {
                    resources: Some(resources.into()),
                })
            }
            domain::TestState::InternalError { message: _ } => {
                models::test::State::InternalError(Empty {})
            }
        }
    }
}

impl From<domain::Test> for models::Test {
    fn from(test: domain::Test) -> Self {
        Self {
            state: Some(test.state.into()),
            stdout: test.current_stdout,
            stderr: test.current_stderr,
        }
    }
}

impl From<domain::Test> for models::TestResult {
    fn from(test: domain::Test) -> Self {
        Self {
            state: Some(test.into()),
        }
    }
}

impl From<domain::TaskState> for task::State {
    fn from(state: domain::TaskState) -> Self {
        match state {
            domain::TaskState::Accepted => task::State::Accepted(Empty {}),
            domain::TaskState::Unavailable => task::State::Unavailable(Empty {}),
            domain::TaskState::Cancelled => task::State::Cancelled(Empty {}),
            domain::TaskState::Compiling => task::State::Compiling(Empty {}),
            domain::TaskState::CompilationFailed { msg } => {
                task::State::CompilationFailed(models::CompilationFailed { message: msg })
            }
            domain::TaskState::CompilationLimitsExceeded(limit_type) => {
                task::State::LimitsExceeded(models::CompilationLimitsExceeded {
                    r#type: Into::<compilation_limits_exceeded::LimitType>::into(limit_type) as i32,
                })
            }
            domain::TaskState::Compiled(_) => task::State::Compiled(Empty {}),
            domain::TaskState::Executing { tests } => {
                task::State::Executing(models::TestsExecuting {
                    tests: tests.into_iter().map(Into::into).collect(),
                })
            }
            domain::TaskState::Done { results } => task::State::Done(models::TestsDone {
                tests: results.into_iter().map(Into::into).collect(),
            }),
        }
    }
}
