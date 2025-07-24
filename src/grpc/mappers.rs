use crate::domain;
use crate::grpc::models::{self, Empty, compilation_limits_exceeded, task, test_limits_exceeded};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    #[error("Missing required field: {field}")]
    MissingField { field: String },
}

impl TryFrom<models::SubmitCodeRequest> for domain::Task {
    type Error = ConversionError;

    fn try_from(req: models::SubmitCodeRequest) -> Result<Self, ConversionError> {
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

impl From<compilation_limits_exceeded::LimitType> for domain::CompilationLimitType {
    fn from(limit_type: compilation_limits_exceeded::LimitType) -> Self {
        match limit_type {
            compilation_limits_exceeded::LimitType::Ram => domain::CompilationLimitType::Ram,
            compilation_limits_exceeded::LimitType::Time => domain::CompilationLimitType::Time,
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

impl TryFrom<models::Test> for domain::Test {
    type Error = ConversionError;

    fn try_from(test: models::Test) -> Result<Self, ConversionError> {
        let state = match test.state {
            Some(task_state) => match task_state {
                models::test::State::Pending(_) => domain::TestState::Pending,
                models::test::State::Executing(_) => domain::TestState::Executing,
                models::test::State::Checking(_) => domain::TestState::Checking,
                models::test::State::Correct(_) => domain::TestState::Correct,
                models::test::State::Wrong(wrong) => domain::TestState::Wrong {
                    expected_stdout: wrong.expected_stdout,
                    expected_stderr: wrong.expected_stderr,
                },
                models::test::State::LimitsExceeded(exceeded) => {
                    domain::TestState::LimitsExceeded(exceeded.r#type().into())
                }
                models::test::State::Crash(_) => domain::TestState::Crash,
            },
            None => {
                return Err(ConversionError::MissingField {
                    field: "state".to_string(),
                });
            }
        };

        Ok(Self {
            current_stdout: test.stdout,
            current_stderr: test.stderr,
            state,
        })
    }
}

impl TryFrom<models::TestResult> for domain::TestResult {
    type Error = ConversionError;

    fn try_from(result: models::TestResult) -> Result<Self, ConversionError> {
        let test = result
            .state
            .ok_or_else(|| ConversionError::MissingField {
                field: "state".to_string(),
            })?
            .try_into()?;

        Ok(Self {
            test,
            execution_time_ms: result.execution_time_ms as u64,
            peak_memory_usage_bytes: result.peak_memory_usage_bytes as u64,
        })
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

impl From<domain::CompilationLimitType> for compilation_limits_exceeded::LimitType {
    fn from(limit_type: domain::CompilationLimitType) -> Self {
        match limit_type {
            domain::CompilationLimitType::Ram => compilation_limits_exceeded::LimitType::Ram,
            domain::CompilationLimitType::Time => compilation_limits_exceeded::LimitType::Time,
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
            domain::TestState::Checking => models::test::State::Checking(Empty {}),
            domain::TestState::Correct => models::test::State::Correct(Empty {}),
            domain::TestState::Wrong {
                expected_stdout,
                expected_stderr,
            } => models::test::State::Wrong(models::TestWrong {
                expected_stdout,
                expected_stderr,
            }),
            domain::TestState::LimitsExceeded(limit_type) => {
                models::test::State::LimitsExceeded(models::TestLimitsExceeded {
                    r#type: Into::<test_limits_exceeded::LimitType>::into(limit_type) as i32,
                })
            }
            domain::TestState::Crash => models::test::State::Crash(Empty {}),
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

impl From<domain::TestResult> for models::TestResult {
    fn from(result: domain::TestResult) -> Self {
        Self {
            state: Some(result.test.into()),
            execution_time_ms: result.execution_time_ms as i64,
            peak_memory_usage_bytes: result.peak_memory_usage_bytes as i64,
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
            domain::TaskState::Compiled => task::State::Compiled(Empty {}),
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
