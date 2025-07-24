use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct Task {
    pub id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub code: String,
    pub language: Language,
    pub compilation_limits: CompilationLimits,
    pub execution_limits: ExecutionLimits,
    pub test_data: Vec<TestData>,
    pub state: TaskState,
}

impl Task {
    pub fn change_state(&self, new_state: TaskState) -> Self {
        Self {
            state: new_state,
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug)]
pub enum TaskState {
    Pending,
    Accepted,
    Unavailable,
    Cancelled,
    InvalidRequest { msg: String },

    Compiling,
    CompilationFailed { msg: String },
    CompilationLimitsExceeded(CompilationLimitType),
    Compiled,

    Executing { tests: Vec<Test> },
    Done { results: Vec<TestResult> },
}

impl Default for TaskState {
    fn default() -> Self {
        TaskState::Pending
    }
}

#[derive(Clone, Debug)]
pub enum Language {
    GnuCpp,
}

#[derive(Clone, Debug)]
pub struct CompilationLimits {
    pub time_ms: Option<u64>,
    pub memory_bytes: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct ExecutionLimits {
    pub time_ms: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub pids_count: Option<u32>,
    pub stdout_size_bytes: Option<u64>,
    pub stderr_size_bytes: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct TestData {
    pub stdin: String,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug)]
pub enum CompilationLimitType {
    Ram,
    Time,
}

#[derive(Clone, Debug)]
pub struct Test {
    pub current_stdout: String,
    pub current_stderr: String,
    pub state: TestState,
}

#[derive(Clone, Debug)]
pub struct TestResult {
    pub test: Test,
    pub execution_time_ms: u64,
    pub peak_memory_usage_bytes: u64,
}

#[derive(Clone, Debug)]
pub enum TestState {
    Pending,
    Executing,
    Checking,
    Correct,
    Wrong {
        expected_stdout: String,
        expected_stderr: String,
    },
    LimitsExceeded(TestLimitType),
    Crash,
}

#[derive(Clone, Debug)]
pub enum TestLimitType {
    Ram,
    Time,
    PidsCount,
    StdoutSize,
    StderrSize,
}
