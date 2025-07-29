//! # Domain Layer
//!
//! Contains models that describe the domain and are not tied to specific
//! implementations, data transmission and storage methods.

use uuid::Uuid;

/// Task entity in the system. A task represents code that the user sends for
/// verification, as well as a set of test data that will be used to check the
/// code. The code receives input data through stdin and outputs data to stdout
/// and stderr.
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
            updated_at: chrono::Utc::now(),
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum TaskState {
    /// Task accepted for verification.
    #[default]
    Accepted,
    /// Server cannot execute the task due to internal reasons.
    Unavailable,
    /// Task cancelled by user.
    Cancelled,

    /// Task is in the compilation process.
    Compiling,
    /// Task failed to compile.
    CompilationFailed { msg: String },
    /// Limits were exceeded during compilation.
    CompilationLimitsExceeded(CompilationLimitType),
    /// Task compiled successfully.
    Compiled(Artifact),

    /// Task is in the execution process. At this moment, the compiled code is
    /// run multiple times with different test data, after which the results
    /// of each run are compared with the expected result.
    Executing { tests: Vec<Test> },
    /// Task executed successfully.
    Done { results: Vec<Test> },
}

/// Artifact is an executable file obtained during code compilation.
/// When the `Compiler` completes compilation, it saves the executable file
/// somewhere on disk, after which it assigns an identifier to it, by which
/// the `Runner` can access this executable file.
#[derive(Clone, Debug)]
pub struct Artifact {
    pub id: Uuid,
    pub kind: ArtifactKind,
}

#[derive(Clone, Debug)]
pub enum ArtifactKind {
    Executable,
}

/// Programming language in which the code sent by the user is written, as well
/// as the compiler/interpreter/environment that will be used.
#[derive(Clone, Debug)]
pub enum Language {
    /// C++ with G++ compiler
    GnuCpp,
}

/// Constraints imposed on the compilation process. If a field has a value of
/// `None`, then the corresponding constraint is not applied.
#[derive(Clone, Debug)]
pub struct CompilationLimits {
    /// Maximum amount of time in milliseconds that can be spent on compilation.
    pub time_ms: Option<u64>,
    /// Maximum amount of RAM in bytes that can be used for compilation.
    pub memory_bytes: Option<u64>,
}

/// Constraints imposed on the execution process. If a field has a value of
/// `None`, then the corresponding constraint is not applied.
#[derive(Clone, Debug)]
pub struct ExecutionLimits {
    /// Maximum amount of time in milliseconds that can be spent on execution.
    pub time_ms: Option<u64>,
    /// Maximum amount of RAM in bytes that can be used for execution.
    pub memory_bytes: Option<u64>,
    /// Maximum number of PIDs that can be created by the tested code during
    /// execution. Roughly speaking, this is the maximum number of processes,
    /// although from the Linux scheduler's perspective, threads and processes
    /// are the same thing, so this is also the maximum number of threads.
    /// For 64-bit systems, the value of /proc/sys/kernel/pid_max is usually
    /// 4194304 = 2^22, which is less than 2^32, so the u32 type is sufficient
    /// for storing PIDs.
    pub pids_count: Option<u32>,
    /// Maximum size of data in stdout in bytes.
    pub stdout_size_bytes: Option<u64>,
    /// Maximum size of data in stderr in bytes.
    pub stderr_size_bytes: Option<u64>,
}

/// Test data.
#[derive(Clone, Debug)]
pub struct TestData {
    /// Input data for the test.
    pub stdin: String,
    /// Expected stdout for the test.
    pub stdout: String,
    /// Expected stderr for the test.
    pub stderr: String,
}

#[derive(Clone, Debug)]
pub enum CompilationLimitType {
    Ram,
    Time,
}

/// Value representing the current execution state.
#[derive(Clone, Debug)]
pub struct Test {
    /// Current stdout
    pub current_stdout: String,
    /// Current stderr
    pub current_stderr: String,
    /// Current state
    pub state: TestState,
}

/// Execution state
#[derive(Clone, Debug)]
pub enum TestState {
    /// Test not yet started
    Pending,
    /// Test is executing
    Executing,
    /// Expected output data is being compared with actual data
    Checking { resources: TestResources },
    /// Output data matched expected data, test passed successfully
    Correct { resources: TestResources },
    /// Output data did not match expected data, test failed
    Wrong {
        expected_stdout: String,
        expected_stderr: String,
        resources: TestResources,
    },
    /// Limits exceeded during execution
    LimitsExceeded {
        limit_type: TestLimitType,
        resources: TestResources,
    },
    /// Program crashed during execution
    Crash { resources: TestResources },
    /// Internal error occurred during test execution
    InternalError { message: String },
}

/// Resources used during test execution
#[derive(Clone, Debug)]
pub struct TestResources {
    /// How much time was required for test execution in milliseconds
    pub execution_time_ms: u64,
    /// How much memory was used during execution at peak in bytes
    pub peak_memory_usage_bytes: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TestLimitType {
    Ram,
    Time,
    PidsCount,
    StdoutSize,
    StderrSize,
}
