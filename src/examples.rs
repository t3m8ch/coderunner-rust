use crate::compiler::basic::BasicCompiler;
use crate::compiler::traits::Compiler;
use crate::domain::{CompilationLimits, ExecutionLimits, Language, TestLimitType};
use crate::runner::basic::BasicRunner;
use crate::runner::traits::{Runner, RunnerError};

/// Simple example that compiles and runs a Hello World C++ program
pub async fn hello_world_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Hello World Example ===");

    let source_code = r#"
#include <iostream>
using namespace std;

int main() {
    cout << "Hello, World!" << endl;
    return 0;
}
"#;

    let compiler = BasicCompiler::new()?;
    let runner = BasicRunner::new()?;

    // Set compilation limits
    let compilation_limits = CompilationLimits {
        time_ms: Some(10000),                  // 10 seconds
        memory_bytes: Some(512 * 1024 * 1024), // 512 MB
    };

    // Set execution limits
    let execution_limits = ExecutionLimits {
        time_ms: Some(5000),                   // 5 seconds
        memory_bytes: Some(256 * 1024 * 1024), // 256 MB
        pids_count: Some(1),
        stdout_size_bytes: Some(1024 * 1024), // 1 MB
        stderr_size_bytes: Some(1024 * 1024), // 1 MB
    };

    // Compile the code
    println!("Compiling C++ code...");
    let artifact = compiler
        .compile(source_code, &Language::GnuCpp, &compilation_limits)
        .await?;

    println!("Compilation successful! Artifact ID: {}", artifact.id);

    // Run the compiled program
    println!("Running the program...");
    let result = runner.run(&artifact, "", &execution_limits).await?;

    println!("Program executed successfully!");
    println!("Exit code: {}", result.status);
    println!("Stdout: {}", result.stdout.trim());
    println!("Stderr: {}", result.stderr.trim());
    println!("Execution time: {} ms", result.execution_time_ms);
    println!(
        "Peak memory usage: {} bytes",
        result.peak_memory_usage_bytes
    );

    Ok(())
}

/// Example that demonstrates input/output handling
pub async fn echo_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== Echo Example ===");

    let source_code = r#"
#include <iostream>
#include <string>
using namespace std;

int main() {
    string line;
    while (getline(cin, line)) {
        cout << "Echo: " << line << endl;
    }
    return 0;
}
"#;

    let compiler = BasicCompiler::new()?;
    let runner = BasicRunner::new()?;

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

    println!("Compiling echo program...");
    let artifact = compiler
        .compile(source_code, &Language::GnuCpp, &compilation_limits)
        .await?;

    let input = "Hello\nWorld\nFrom Rust!";
    println!("Running with input:\n{}", input);
    let result = runner.run(&artifact, input, &execution_limits).await?;

    println!("Program output:");
    println!("Exit code: {}", result.status);
    println!("Stdout:\n{}", result.stdout);
    println!("Execution time: {} ms", result.execution_time_ms);

    Ok(())
}

/// Example demonstrating compilation error handling
pub async fn compilation_error_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== Compilation Error Example ===");

    let bad_source = r#"
#include <iostream>
using namespace std;

int main() {
    cout << "This won't compile because of syntax error" <<
    // Missing semicolon and closing quote
    return 0;
}
"#;

    let compiler = BasicCompiler::new()?;
    let compilation_limits = CompilationLimits {
        time_ms: Some(10000),
        memory_bytes: Some(512 * 1024 * 1024),
    };

    println!("Attempting to compile invalid C++ code...");
    match compiler
        .compile(bad_source, &Language::GnuCpp, &compilation_limits)
        .await
    {
        Ok(_) => println!("Unexpected: compilation succeeded!"),
        Err(e) => {
            println!("Expected compilation error occurred:");
            println!("{:?}", e);
        }
    }

    Ok(())
}

/// Example demonstrating runtime error handling
pub async fn runtime_error_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== Runtime Error Example ===");

    let source_code = r#"
#include <iostream>
using namespace std;

int main() {
    cout << "This program will crash!" << endl;
    int* p = nullptr;
    *p = 42; // Segmentation fault
    return 0;
}
"#;

    let compiler = BasicCompiler::new()?;
    let runner = BasicRunner::new()?;

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

    println!("Compiling program that will crash...");
    let artifact = compiler
        .compile(source_code, &Language::GnuCpp, &compilation_limits)
        .await?;

    println!("Running the program...");
    match runner.run(&artifact, "", &execution_limits).await {
        Ok(result) => {
            println!("Program completed:");
            println!("Exit code: {}", result.status);
            println!("Stdout: {}", result.stdout);
        }
        Err(RunnerError::Crash { result }) => {
            println!("Expected crash occurred:");
            println!("Exit code: {}", result.status);
            println!("Stdout: {}", result.stdout);
            println!("Stderr: {}", result.stderr);
        }
        Err(e) => {
            println!("Other error occurred: {:?}", e);
        }
    }

    Ok(())
}

/// Example demonstrating timeout handling
pub async fn timeout_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== Timeout Example ===");

    let source_code = r#"
#include <iostream>
#include <thread>
#include <chrono>
using namespace std;

int main() {
    cout << "Starting long operation..." << endl;
    this_thread::sleep_for(chrono::seconds(10)); // Sleep for 10 seconds
    cout << "Operation completed!" << endl;
    return 0;
}
"#;

    let compiler = BasicCompiler::new()?;
    let runner = BasicRunner::new()?;

    let compilation_limits = CompilationLimits {
        time_ms: Some(10000),
        memory_bytes: Some(512 * 1024 * 1024),
    };

    // Set a short timeout to trigger the limit
    let execution_limits = ExecutionLimits {
        time_ms: Some(2000), // 2 seconds - shorter than the sleep
        memory_bytes: Some(256 * 1024 * 1024),
        pids_count: Some(1),
        stdout_size_bytes: Some(1024 * 1024),
        stderr_size_bytes: Some(1024 * 1024),
    };

    println!("Compiling program with long operation...");
    let artifact = compiler
        .compile(source_code, &Language::GnuCpp, &compilation_limits)
        .await?;

    println!("Running with 2-second timeout...");
    match runner.run(&artifact, "", &execution_limits).await {
        Ok(result) => {
            println!("Unexpected: program completed within timeout:");
            println!("Stdout: {}", result.stdout);
        }
        Err(RunnerError::LimitsExceeded { result, limit_type }) => {
            println!("Expected timeout occurred:");
            println!("Limit type: {:?}", limit_type);
            println!("Execution time: {} ms", result.execution_time_ms);
            if limit_type == TestLimitType::Time {
                println!("Successfully caught time limit exceeded!");
            }
        }
        Err(e) => {
            println!("Other error occurred: {:?}", e);
        }
    }

    Ok(())
}

/// Run all examples
pub async fn run_all_examples() -> Result<(), Box<dyn std::error::Error>> {
    println!("Running all examples...\n");

    hello_world_example().await?;
    echo_example().await?;
    compilation_error_example().await?;
    runtime_error_example().await?;
    timeout_example().await?;

    println!("\n=== All examples completed! ===");
    Ok(())
}
