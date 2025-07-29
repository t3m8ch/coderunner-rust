use crate::compiler::basic::BasicCompiler;
use crate::compiler::traits::Compiler;
use crate::domain::{CompilationLimits, ExecutionLimits, Language, TestLimitType};
use crate::runner::basic::BasicRunner;
use crate::runner::traits::{Runner, RunnerError};

#[tokio::test]
async fn test_hello_world_compilation_and_execution() {
    let compiler = BasicCompiler::new().expect("Failed to create compiler");
    let runner = BasicRunner::new().expect("Failed to create runner");

    let source_code = r#"
#include <iostream>
using namespace std;

int main() {
    cout << "Hello, Integration Test!" << endl;
    return 0;
}
"#;

    let compilation_limits = CompilationLimits {
        time_ms: Some(10000),
        memory_bytes: Some(512 * 1024 * 1024),
    };

    let execution_limits = ExecutionLimits {
        time_ms: Some(5000),
        memory_bytes: Some(256 * 1024 * 1024),
        pids_count: Some(1),
        stdout_size_bytes: Some(1024 * 1024),
        stderr_size_bytes: Some(1024 * 1024),
    };

    // Test compilation
    let artifact = compiler
        .compile(source_code, &Language::GnuCpp, &compilation_limits)
        .await
        .expect("Compilation should succeed");

    // Test execution
    let result = runner
        .run(&artifact, "", &execution_limits)
        .await
        .expect("Execution should succeed");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout.trim(), "Hello, Integration Test!");
    assert!(result.stderr.is_empty());
    assert!(result.execution_time_ms > 0);
    assert!(result.peak_memory_usage_bytes > 0);
}

#[tokio::test]
async fn test_input_output_handling() {
    let compiler = BasicCompiler::new().expect("Failed to create compiler");
    let runner = BasicRunner::new().expect("Failed to create runner");

    let source_code = r#"
#include <iostream>
#include <string>
using namespace std;

int main() {
    string name;
    getline(cin, name);
    cout << "Hello, " << name << "!" << endl;
    return 0;
}
"#;

    let compilation_limits = CompilationLimits {
        time_ms: Some(10000),
        memory_bytes: Some(512 * 1024 * 1024),
    };

    let execution_limits = ExecutionLimits {
        time_ms: Some(5000),
        memory_bytes: Some(256 * 1024 * 1024),
        pids_count: Some(1),
        stdout_size_bytes: Some(1024 * 1024),
        stderr_size_bytes: Some(1024 * 1024),
    };

    let artifact = compiler
        .compile(source_code, &Language::GnuCpp, &compilation_limits)
        .await
        .expect("Compilation should succeed");

    let result = runner
        .run(&artifact, "Rust", &execution_limits)
        .await
        .expect("Execution should succeed");

    assert_eq!(result.status, 0);
    assert_eq!(result.stdout.trim(), "Hello, Rust!");
}

#[tokio::test]
async fn test_compilation_error() {
    let compiler = BasicCompiler::new().expect("Failed to create compiler");

    let invalid_source = r#"
#include <iostream>
using namespace std;

int main() {
    cout << "Missing semicolon here"
    return 0; // This should cause a compilation error
}
"#;

    let compilation_limits = CompilationLimits {
        time_ms: Some(10000),
        memory_bytes: Some(512 * 1024 * 1024),
    };

    let result = compiler
        .compile(invalid_source, &Language::GnuCpp, &compilation_limits)
        .await;

    assert!(result.is_err());
    match result.unwrap_err() {
        crate::compiler::errors::CompilationError::CompilationFailed { msg } => {
            assert!(msg.contains("error") || msg.contains("ошибка"));
        }
        _ => panic!("Expected CompilationFailed error"),
    }
}

#[tokio::test]
async fn test_execution_timeout() {
    let compiler = BasicCompiler::new().expect("Failed to create compiler");
    let runner = BasicRunner::new().expect("Failed to create runner");

    let source_code = r#"
#include <iostream>
#include <thread>
#include <chrono>
using namespace std;

int main() {
    cout << "Starting sleep..." << endl;
    this_thread::sleep_for(chrono::seconds(5));
    cout << "Sleep finished!" << endl;
    return 0;
}
"#;

    let compilation_limits = CompilationLimits {
        time_ms: Some(10000),
        memory_bytes: Some(512 * 1024 * 1024),
    };

    let execution_limits = ExecutionLimits {
        time_ms: Some(1000), // 1 second timeout
        memory_bytes: Some(256 * 1024 * 1024),
        pids_count: Some(1),
        stdout_size_bytes: Some(1024 * 1024),
        stderr_size_bytes: Some(1024 * 1024),
    };

    let artifact = compiler
        .compile(source_code, &Language::GnuCpp, &compilation_limits)
        .await
        .expect("Compilation should succeed");

    let result = runner.run(&artifact, "", &execution_limits).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        RunnerError::LimitsExceeded { limit_type, .. } => {
            assert_eq!(limit_type, TestLimitType::Time);
        }
        _ => panic!("Expected LimitsExceeded error with Time limit"),
    }
}

#[tokio::test]
async fn test_stdout_size_limit() {
    let compiler = BasicCompiler::new().expect("Failed to create compiler");
    let runner = BasicRunner::new().expect("Failed to create runner");

    let source_code = r#"
#include <iostream>
using namespace std;

int main() {
    for (int i = 0; i < 1000; i++) {
        cout << "This is a long line of output that will exceed the limit" << endl;
    }
    return 0;
}
"#;

    let compilation_limits = CompilationLimits {
        time_ms: Some(10000),
        memory_bytes: Some(512 * 1024 * 1024),
    };

    let execution_limits = ExecutionLimits {
        time_ms: Some(5000),
        memory_bytes: Some(256 * 1024 * 1024),
        pids_count: Some(1),
        stdout_size_bytes: Some(1000), // Small limit to trigger the error
        stderr_size_bytes: Some(1024 * 1024),
    };

    let artifact = compiler
        .compile(source_code, &Language::GnuCpp, &compilation_limits)
        .await
        .expect("Compilation should succeed");

    let result = runner.run(&artifact, "", &execution_limits).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        RunnerError::LimitsExceeded { limit_type, .. } => {
            assert_eq!(limit_type, TestLimitType::StdoutSize);
        }
        _ => panic!("Expected LimitsExceeded error with StdoutSize limit"),
    }
}

#[tokio::test]
async fn test_mathematical_program() {
    let compiler = BasicCompiler::new().expect("Failed to create compiler");
    let runner = BasicRunner::new().expect("Failed to create runner");

    let source_code = r#"
#include <iostream>
using namespace std;

int main() {
    int a, b;
    cin >> a >> b;
    cout << "Sum: " << (a + b) << endl;
    cout << "Product: " << (a * b) << endl;
    return 0;
}
"#;

    let compilation_limits = CompilationLimits {
        time_ms: Some(10000),
        memory_bytes: Some(512 * 1024 * 1024),
    };

    let execution_limits = ExecutionLimits {
        time_ms: Some(5000),
        memory_bytes: Some(256 * 1024 * 1024),
        pids_count: Some(1),
        stdout_size_bytes: Some(1024 * 1024),
        stderr_size_bytes: Some(1024 * 1024),
    };

    let artifact = compiler
        .compile(source_code, &Language::GnuCpp, &compilation_limits)
        .await
        .expect("Compilation should succeed");

    let result = runner
        .run(&artifact, "15 25", &execution_limits)
        .await
        .expect("Execution should succeed");

    assert_eq!(result.status, 0);
    let output_lines: Vec<&str> = result.stdout.trim().split('\n').collect();
    assert_eq!(output_lines.len(), 2);
    assert_eq!(output_lines[0], "Sum: 40");
    assert_eq!(output_lines[1], "Product: 375");
}

#[tokio::test]
async fn test_multiple_compilations() {
    let compiler = BasicCompiler::new().expect("Failed to create compiler");
    let runner = BasicRunner::new().expect("Failed to create runner");

    let source_codes = vec![
        r#"
#include <iostream>
using namespace std;
int main() { cout << "Program 1" << endl; return 0; }
"#,
        r#"
#include <iostream>
using namespace std;
int main() { cout << "Program 2" << endl; return 0; }
"#,
        r#"
#include <iostream>
using namespace std;
int main() { cout << "Program 3" << endl; return 0; }
"#,
    ];

    let compilation_limits = CompilationLimits {
        time_ms: Some(10000),
        memory_bytes: Some(512 * 1024 * 1024),
    };

    let execution_limits = ExecutionLimits {
        time_ms: Some(5000),
        memory_bytes: Some(256 * 1024 * 1024),
        pids_count: Some(1),
        stdout_size_bytes: Some(1024 * 1024),
        stderr_size_bytes: Some(1024 * 1024),
    };

    for (i, source_code) in source_codes.iter().enumerate() {
        let artifact = compiler
            .compile(source_code, &Language::GnuCpp, &compilation_limits)
            .await
            .expect("Compilation should succeed");

        let result = runner
            .run(&artifact, "", &execution_limits)
            .await
            .expect("Execution should succeed");

        assert_eq!(result.status, 0);
        assert_eq!(result.stdout.trim(), format!("Program {}", i + 1));
    }
}

#[cfg(test)]
mod test_helpers {
    use super::*;

    pub async fn compile_and_run_simple(source: &str, input: &str) -> String {
        let compiler = BasicCompiler::new().expect("Failed to create compiler");
        let runner = BasicRunner::new().expect("Failed to create runner");

        let compilation_limits = CompilationLimits {
            time_ms: Some(10000),
            memory_bytes: Some(512 * 1024 * 1024),
        };

        let execution_limits = ExecutionLimits {
            time_ms: Some(5000),
            memory_bytes: Some(256 * 1024 * 1024),
            pids_count: Some(1),
            stdout_size_bytes: Some(1024 * 1024),
            stderr_size_bytes: Some(1024 * 1024),
        };

        let artifact = compiler
            .compile(source, &Language::GnuCpp, &compilation_limits)
            .await
            .expect("Compilation should succeed");

        let result = runner
            .run(&artifact, input, &execution_limits)
            .await
            .expect("Execution should succeed");

        result.stdout
    }
}

#[tokio::test]
async fn test_helper_function() {
    let source = r#"
#include <iostream>
using namespace std;
int main() {
    cout << "Helper test works!" << endl;
    return 0;
}
"#;

    let output = test_helpers::compile_and_run_simple(source, "").await;
    assert_eq!(output.trim(), "Helper test works!");
}
