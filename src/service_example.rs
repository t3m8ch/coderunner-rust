use std::sync::Arc;
use std::time::Duration;

use crate::compiler::basic::BasicCompiler;
use crate::compiler::traits::Compiler;
use crate::domain::{
    CompilationLimits, ExecutionLimits, Language, Task, TaskState, TestData, TestLimitType,
};
use crate::grpc::models::testing_service_server::TestingServiceServer;
use crate::grpc::services::TestingServiceImpl;
use crate::runner::basic::BasicRunner;
use crate::runner::traits::{Runner, RunnerError};

/// Example showing how to create a testing service with real implementations
pub async fn create_real_testing_service()
-> Result<TestingServiceServer<TestingServiceImpl>, Box<dyn std::error::Error>> {
    let compiler = Arc::new(BasicCompiler::new()?);
    let runner = Arc::new(BasicRunner::new()?);

    let testing_service = TestingServiceImpl::new(compiler, runner);
    let service = TestingServiceServer::new(testing_service);

    Ok(service)
}

/// Example of a complete code testing workflow
pub async fn complete_testing_workflow() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Complete Testing Workflow Example ===");

    let compiler = BasicCompiler::new()?;
    let runner = BasicRunner::new()?;

    // Example: Simple algorithm problem - find sum of two numbers
    let source_code = r#"
#include <iostream>
using namespace std;

int main() {
    int a, b;
    cin >> a >> b;
    cout << a + b << endl;
    return 0;
}
"#;

    // Test cases
    let test_cases = vec![
        TestData {
            stdin: "5 3".to_string(),
            stdout: "8".to_string(),
            stderr: "".to_string(),
        },
        TestData {
            stdin: "10 -2".to_string(),
            stdout: "8".to_string(),
            stderr: "".to_string(),
        },
        TestData {
            stdin: "0 0".to_string(),
            stdout: "0".to_string(),
            stderr: "".to_string(),
        },
    ];

    let compilation_limits = CompilationLimits {
        time_ms: Some(10000),
        memory_bytes: Some(512 * 1024 * 1024),
    };

    let execution_limits = ExecutionLimits {
        time_ms: Some(2000),
        memory_bytes: Some(128 * 1024 * 1024),
        pids_count: Some(1),
        stdout_size_bytes: Some(1024),
        stderr_size_bytes: Some(1024),
    };

    // Step 1: Compile the code
    println!("Step 1: Compiling code...");
    let artifact = match compiler
        .compile(source_code, &Language::GnuCpp, &compilation_limits)
        .await
    {
        Ok(artifact) => {
            println!("✓ Compilation successful! Artifact ID: {}", artifact.id);
            artifact
        }
        Err(e) => {
            println!("✗ Compilation failed: {:?}", e);
            return Ok(());
        }
    };

    // Step 2: Run tests
    println!("\nStep 2: Running test cases...");
    let mut passed_tests = 0;
    let mut total_tests = test_cases.len();

    for (i, test_case) in test_cases.iter().enumerate() {
        println!("  Test case {}: Input = '{}'", i + 1, test_case.stdin);

        match runner
            .run(&artifact, &test_case.stdin, &execution_limits)
            .await
        {
            Ok(result) => {
                let actual_output = result.stdout.trim();
                let expected_output = test_case.stdout.trim();

                if result.status != 0 {
                    println!("    ✗ Program exited with code {}", result.status);
                    if !result.stderr.is_empty() {
                        println!("    stderr: {}", result.stderr);
                    }
                } else if actual_output == expected_output {
                    println!(
                        "    ✓ Passed! Output: '{}' ({}ms)",
                        actual_output, result.execution_time_ms
                    );
                    passed_tests += 1;
                } else {
                    println!(
                        "    ✗ Failed! Expected: '{}', Got: '{}'",
                        expected_output, actual_output
                    );
                }
            }
            Err(RunnerError::LimitsExceeded { result, limit_type }) => {
                println!("    ✗ Limits exceeded: {:?}", limit_type);
                match limit_type {
                    TestLimitType::Time => {
                        println!("      Execution time: {}ms", result.execution_time_ms)
                    }
                    TestLimitType::Ram => println!(
                        "      Memory usage: {} bytes",
                        result.peak_memory_usage_bytes
                    ),
                    _ => {}
                }
            }
            Err(RunnerError::Crash { result }) => {
                println!("    ✗ Program crashed with exit code {}", result.status);
                if !result.stderr.is_empty() {
                    println!("      stderr: {}", result.stderr);
                }
            }
            Err(RunnerError::FailedToLaunch { msg }) => {
                println!("    ✗ Failed to launch: {}", msg);
            }
        }
    }

    println!("\nResults: {}/{} tests passed", passed_tests, total_tests);

    if passed_tests == total_tests {
        println!("🎉 All tests passed!");
    } else {
        println!("❌ Some tests failed");
    }

    Ok(())
}

/// Example of handling different types of programs
pub async fn programming_contest_examples() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== Programming Contest Examples ===");

    let compiler = BasicCompiler::new()?;
    let runner = BasicRunner::new()?;

    let compilation_limits = CompilationLimits {
        time_ms: Some(15000),
        memory_bytes: Some(512 * 1024 * 1024),
    };

    let execution_limits = ExecutionLimits {
        time_ms: Some(1000),
        memory_bytes: Some(64 * 1024 * 1024),
        pids_count: Some(1),
        stdout_size_bytes: Some(10 * 1024),
        stderr_size_bytes: Some(1024),
    };

    // Example 1: Fibonacci sequence
    println!("\n--- Example 1: Fibonacci Sequence ---");
    let fibonacci_code = r#"
#include <iostream>
using namespace std;

int fibonacci(int n) {
    if (n <= 1) return n;
    return fibonacci(n-1) + fibonacci(n-2);
}

int main() {
    int n;
    cin >> n;
    cout << fibonacci(n) << endl;
    return 0;
}
"#;

    let artifact1 = compiler
        .compile(fibonacci_code, &Language::GnuCpp, &compilation_limits)
        .await?;
    let result1 = runner.run(&artifact1, "10", &execution_limits).await?;
    println!(
        "Fibonacci(10) = {} ({}ms)",
        result1.stdout.trim(),
        result1.execution_time_ms
    );

    // Example 2: Prime number check
    println!("\n--- Example 2: Prime Number Check ---");
    let prime_code = r#"
#include <iostream>
#include <cmath>
using namespace std;

bool isPrime(int n) {
    if (n <= 1) return false;
    if (n <= 3) return true;
    if (n % 2 == 0 || n % 3 == 0) return false;

    for (int i = 5; i * i <= n; i += 6) {
        if (n % i == 0 || n % (i + 2) == 0)
            return false;
    }
    return true;
}

int main() {
    int n;
    cin >> n;
    cout << (isPrime(n) ? "YES" : "NO") << endl;
    return 0;
}
"#;

    let artifact2 = compiler
        .compile(prime_code, &Language::GnuCpp, &compilation_limits)
        .await?;

    let test_numbers = vec![17, 25, 97, 100];
    for num in test_numbers {
        let result = runner
            .run(&artifact2, &num.to_string(), &execution_limits)
            .await?;
        println!(
            "Is {} prime? {} ({}ms)",
            num,
            result.stdout.trim(),
            result.execution_time_ms
        );
    }

    // Example 3: Sorting algorithm
    println!("\n--- Example 3: Sorting Algorithm ---");
    let sort_code = r#"
#include <iostream>
#include <vector>
#include <algorithm>
using namespace std;

int main() {
    int n;
    cin >> n;

    vector<int> arr(n);
    for (int i = 0; i < n; i++) {
        cin >> arr[i];
    }

    sort(arr.begin(), arr.end());

    for (int i = 0; i < n; i++) {
        cout << arr[i];
        if (i < n - 1) cout << " ";
    }
    cout << endl;

    return 0;
}
"#;

    let artifact3 = compiler
        .compile(sort_code, &Language::GnuCpp, &compilation_limits)
        .await?;
    let input = "5\n3 1 4 1 5";
    let result3 = runner.run(&artifact3, input, &execution_limits).await?;
    println!(
        "Sorted array: {} ({}ms)",
        result3.stdout.trim(),
        result3.execution_time_ms
    );

    Ok(())
}

/// Stress test example
pub async fn stress_test_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== Stress Test Example ===");

    let compiler = BasicCompiler::new()?;
    let runner = BasicRunner::new()?;

    let stress_code = r#"
#include <iostream>
#include <vector>
using namespace std;

int main() {
    int n;
    cin >> n;

    // Memory-intensive operation
    vector<vector<int>> matrix(n, vector<int>(n, 0));

    // CPU-intensive operation
    long long sum = 0;
    for (int i = 0; i < n; i++) {
        for (int j = 0; j < n; j++) {
            matrix[i][j] = i * j;
            sum += matrix[i][j];
        }
    }

    cout << sum << endl;
    return 0;
}
"#;

    let compilation_limits = CompilationLimits {
        time_ms: Some(10000),
        memory_bytes: Some(512 * 1024 * 1024),
    };

    // Test with different resource limits
    let test_cases = vec![
        (
            "Small input",
            "100",
            ExecutionLimits {
                time_ms: Some(1000),
                memory_bytes: Some(32 * 1024 * 1024),
                pids_count: Some(1),
                stdout_size_bytes: Some(1024),
                stderr_size_bytes: Some(1024),
            },
        ),
        (
            "Medium input",
            "1000",
            ExecutionLimits {
                time_ms: Some(2000),
                memory_bytes: Some(128 * 1024 * 1024),
                pids_count: Some(1),
                stdout_size_bytes: Some(1024),
                stderr_size_bytes: Some(1024),
            },
        ),
        (
            "Large input (should timeout)",
            "5000",
            ExecutionLimits {
                time_ms: Some(100), // Very short timeout
                memory_bytes: Some(256 * 1024 * 1024),
                pids_count: Some(1),
                stdout_size_bytes: Some(1024),
                stderr_size_bytes: Some(1024),
            },
        ),
    ];

    let artifact = compiler
        .compile(stress_code, &Language::GnuCpp, &compilation_limits)
        .await?;

    for (test_name, input, limits) in test_cases {
        println!("\n  Testing: {}", test_name);

        match runner.run(&artifact, input, &limits).await {
            Ok(result) => {
                println!(
                    "    ✓ Success: {} ({}ms, {} bytes peak memory)",
                    result.stdout.trim(),
                    result.execution_time_ms,
                    result.peak_memory_usage_bytes
                );
            }
            Err(RunnerError::LimitsExceeded { result, limit_type }) => {
                println!(
                    "    ⚠ Limits exceeded: {:?} ({}ms)",
                    limit_type, result.execution_time_ms
                );
            }
            Err(e) => {
                println!("    ✗ Error: {:?}", e);
            }
        }
    }

    Ok(())
}

/// Run all service examples
pub async fn run_all_service_examples() -> Result<(), Box<dyn std::error::Error>> {
    complete_testing_workflow().await?;
    programming_contest_examples().await?;
    stress_test_example().await?;

    println!("\n🎯 All service examples completed successfully!");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_service_creation() {
        let service = create_real_testing_service().await;
        assert!(service.is_ok());
    }

    #[tokio::test]
    async fn test_complete_workflow() {
        let result = complete_testing_workflow().await;
        assert!(result.is_ok());
    }
}
