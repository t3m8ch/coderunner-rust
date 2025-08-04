use std::{path::PathBuf, process::Stdio};

use bon::Builder;
use tokio::{fs, io::AsyncWriteExt, process::Command};
use uuid::Uuid;

use crate::core::{
    domain::{
        Artifact, ArtifactKind, CompilationLimitType, CompilationLimits, ExecutionLimits, Language,
    },
    traits::executor::{CompileError, Executor, RunError, RunResult},
};

#[derive(Clone, Debug, Builder)]
#[builder(on(PathBuf, into))]
pub struct NativeExecutor {
    dir: PathBuf,
    gnucpp_path: PathBuf,
    systemd_run_path: PathBuf,
    journalctl_path: PathBuf,
}

#[async_trait::async_trait]
impl Executor for NativeExecutor {
    async fn compile(
        &self,
        source: &str,
        language: &Language,
        limits: &CompilationLimits,
    ) -> Result<Artifact, CompileError> {
        let artifact_id = Uuid::new_v4();
        let artifact_path = self.artifact_path(&artifact_id);
        let source_path = self.dir.join(format!("{}.cpp", artifact_id));
        let scope_name = format!("coderunner-compile-{}", artifact_id);

        fs::create_dir_all(&self.dir)
            .await
            .map_err(|e| CompileError::Internal { msg: e.to_string() })?;
        fs::write(&source_path, source)
            .await
            .map_err(|e| CompileError::Internal { msg: e.to_string() })?;

        let cmd = if let Some(executable_size_bytes) = limits.executable_size_bytes {
            vec![
                "sh".to_string(),
                "-c".to_string(),
                format!(
                    "ulimit -Sf {}; {} -o {} {}",
                    executable_size_bytes / 512,
                    &self.gnucpp_path.to_str().unwrap(),
                    &artifact_path.to_str().unwrap(),
                    &source_path.to_str().unwrap()
                ),
            ]
        } else {
            vec![
                self.gnucpp_path.to_string_lossy().to_string(),
                "-o".to_string(),
                artifact_path.to_string_lossy().to_string(),
                source_path.to_string_lossy().to_string(),
            ]
        };

        let out = Command::new(&self.systemd_run_path)
            .arg("--user")
            .arg("--scope")
            .arg("-u")
            .arg(&scope_name)
            .args({
                let mut args = Vec::new();
                if let Some(memory_bytes) = limits.memory_bytes {
                    args.push("-p".to_string());
                    args.push("MemorySwapMax=0".to_string());
                    args.push("-p".to_string());
                    args.push(format!("MemoryMax={}", memory_bytes));
                }
                if let Some(time_ms) = limits.time_ms {
                    args.push("-p".to_string());
                    args.push(format!("RuntimeMaxSec={}ms", time_ms));
                }
                args
            })
            .arg("--")
            .args(cmd)
            .output()
            .await
            .map_err(|e| CompileError::Internal { msg: e.to_string() })?;

        if !out.status.success() {
            let journal_out = Command::new(&self.journalctl_path)
                .arg("--user")
                .arg("--unit")
                .arg(&format!("{}.scope", scope_name))
                .arg("--no-pager")
                .output()
                .await
                .map_err(|e| CompileError::Internal { msg: e.to_string() })?;

            let journal_stdout = String::from_utf8_lossy(&journal_out.stdout);
            println!("{}", journal_stdout);

            if journal_stdout.contains("Failed with result 'timeout'") {
                return Err(CompileError::CompilationLimitsExceeded(
                    CompilationLimitType::Time,
                ));
            }

            if journal_stdout.contains("Failed with result 'oom-kill'") {
                return Err(CompileError::CompilationLimitsExceeded(
                    CompilationLimitType::Ram,
                ));
            }

            if journal_stdout.contains("dumped core") {
                return Err(CompileError::CompilationLimitsExceeded(
                    CompilationLimitType::ExecutableSize,
                ));
            }

            let msg = format!(
                "Journal: {}, stdout: {}, stderr: {}",
                journal_stdout,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );

            if journal_stdout.contains("Started") {
                return Err(CompileError::CompilationFailed { msg });
            }

            return Err(CompileError::Internal { msg });
        }

        Ok(Artifact {
            id: artifact_id,
            kind: ArtifactKind::Executable,
        })
    }

    async fn run(
        &self,
        artifact: &Artifact,
        stdin: &str,
        limits: &ExecutionLimits,
    ) -> Result<RunResult, RunError> {
        let mut child = Command::new(self.artifact_path(&artifact.id))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let mut stdin_stream = child.stdin.take().unwrap();
        stdin_stream.write_all(stdin.as_bytes()).await.unwrap();
        drop(stdin_stream);

        let out = child.wait_with_output().await.unwrap();
        Ok(RunResult {
            status: out.status.code().unwrap(),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            execution_time_ms: 100,
            peak_memory_usage_bytes: 1024 * 1024,
        })
    }
}

impl NativeExecutor {
    fn artifact_path(&self, artifact_id: &Uuid) -> PathBuf {
        self.dir.join(format!("{}.out", artifact_id))
    }
}

// TODO: Write tests with compiling limits
// TODO: Create Dockerfile for executor testing with fixed g++ and filesystem

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use bon::builder;
    use futures::future::join_all;
    use tokio::process::Command;
    use uuid::Uuid;

    use crate::{
        core::{
            domain::{
                Artifact, ArtifactKind, CompilationLimitType, CompilationLimits, ExecutionLimits,
                Language,
            },
            traits::executor::{CompileError, Executor, RunResult},
        },
        native::executor::NativeExecutor,
    };

    #[tokio::test]
    async fn test_compile_success() {
        let (executor, executor_dir) = executor().create().await;

        let result = executor
            .compile(
                CORRECT_CODE,
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: None,
                    executable_size_bytes: None,
                },
            )
            .await;

        assert!(matches!(
            result,
            Ok(Artifact {
                kind: ArtifactKind::Executable,
                ..
            })
        ));

        let executable_path = executor_dir.join(format!("{}.out", result.unwrap().id));
        println!("Executable path: {}", executable_path.display());
        let out = Command::new(executable_path)
            .output()
            .await
            .expect("Failed to run command");

        println!("{:#?}", out);

        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        assert_eq!(stdout, "Hello, World!\n");
    }

    #[tokio::test]
    async fn test_compile_code_error() {
        let (executor, _) = executor().create().await;

        let result = executor
            .compile(
                INCORRECT_CODE,
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: None,
                    executable_size_bytes: None,
                },
            )
            .await;

        println!("result: {:#?}", result);
        assert!(matches!(
            result,
            Err(CompileError::CompilationFailed { .. })
        ));
    }

    #[tokio::test]
    async fn test_compile_compiler_not_found() {
        let (executor, _) = executor().with_wrong_gnucpp(true).create().await;

        let result = executor
            .compile(
                CORRECT_CODE,
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: None,
                    executable_size_bytes: None,
                },
            )
            .await;

        println!("{:#?}", result);

        assert!(matches!(result, Err(CompileError::Internal { .. })));
    }

    #[tokio::test]
    async fn test_compile_filesystem_error() {
        let (executor, _) = executor().with_readonly_dir(true).create().await;

        let result = executor
            .compile(
                CORRECT_CODE,
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: None,
                    executable_size_bytes: None,
                },
            )
            .await;

        assert!(matches!(result, Err(CompileError::Internal { .. })));
    }

    #[tokio::test]
    async fn test_compile_ram_limit_exceeded() {
        let (executor, _) = executor().create().await;

        let result = executor
            .compile(
                &massive_cpp_code().generate(),
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: Some(1024 * 1024 * 5), // 5 MB
                    executable_size_bytes: None,
                },
            )
            .await;

        println!("result: {:#?}", result);

        assert!(matches!(
            result,
            Err(CompileError::CompilationLimitsExceeded(
                CompilationLimitType::Ram
            ))
        ));
    }

    #[tokio::test]
    async fn test_compile_time_limit_exceeded() {
        let (executor, _) = executor().create().await;

        let result = executor
            .compile(
                &massive_cpp_code().generate(),
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: Some(100),
                    memory_bytes: None,
                    executable_size_bytes: None,
                },
            )
            .await;

        println!("result: {:#?}", result);

        assert!(matches!(
            result,
            Err(CompileError::CompilationLimitsExceeded(
                CompilationLimitType::Time
            ))
        ));
    }

    #[tokio::test]
    async fn test_compile_executable_size_limit_exceeded() {
        let (executor, _) = executor().create().await;

        let result = executor
            .compile(
                &massive_cpp_code().generate(),
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: None,
                    executable_size_bytes: Some(1024 * 512), // 512 KB,
                },
            )
            .await;

        println!("result: {:#?}", result);

        assert!(matches!(
            result,
            Err(CompileError::CompilationLimitsExceeded(
                CompilationLimitType::ExecutableSize
            ))
        ));
    }

    #[tokio::test]
    async fn test_run_success() {
        let (executor, _) = executor().create().await;
        let artifact = vec![
            cpp_code()
                .status(0)
                .stdout("Hello, world!")
                .stderr("")
                .compile(&executor)
                .await,
            cpp_code()
                .status(1)
                .stdout("Aboba")
                .stderr("Aboba")
                .compile(&executor)
                .await,
        ];

        let result = join_all(artifact.iter().map(|a| {
            executor.run(
                a,
                "",
                &ExecutionLimits {
                    time_ms: None,
                    memory_bytes: None,
                    pids_count: None,
                    stdout_size_bytes: None,
                    stderr_size_bytes: None,
                },
            )
        }))
        .await;

        println!("result: {:#?}", result);

        assert!(matches!(
            result[0],
            Ok(RunResult { status: 0, ref stdout, ref stderr, .. })
            if stdout == "Hello, world!" && stderr == ""
        ));

        assert!(matches!(
            result[1],
            Ok(RunResult { status: 1, ref stdout, ref stderr, .. })
            if stdout == "Aboba" && stderr == "Aboba"
        ));
    }

    #[tokio::test]
    async fn test_run_stdin() {
        let (executor, _) = executor().create().await;
        let artifact = cpp_stdin_repeater().count(3).compile(&executor).await;

        let result = executor
            .run(
                &artifact,
                "Aboba",
                &ExecutionLimits {
                    time_ms: None,
                    memory_bytes: None,
                    pids_count: None,
                    stdout_size_bytes: None,
                    stderr_size_bytes: None,
                },
            )
            .await;

        println!("result: {:#?}", result);

        assert!(matches!(
            result,
            Ok(RunResult { ref stdout, .. })
            if stdout == "Aboba\nAboba\nAboba\n"
        ))
    }

    const CORRECT_CODE: &str = "
            #include <iostream>
            int main() {
                std::cout << \"Hello, World!\" << std::endl;
                return 0;
            }";

    const INCORRECT_CODE: &str = "
            #include <iostream>
            int main() {
                std::cout << \"Hello, World!\" << std::endl
                return 0;
            }";

    #[builder(finish_fn = generate)]
    fn massive_cpp_code(
        #[builder(default = 15000)] num_functions: usize,
        #[builder(default = 12000)] num_variables: usize,
        #[builder(default = 8000)] num_structs: usize,
        #[builder(default = 200)] usage_density: usize,
    ) -> String {
        let mut code = String::new();

        code.push_str("#include <iostream>\n");
        code.push_str("#include <vector>\n\n");

        code.push_str("// Макрос для создания простых функций\n");
        code.push_str("#define GENERATE_FUNCTION(n) \\\n");
        code.push_str("    int function_##n() { \\\n");
        code.push_str("        return n * 2 + 1; \\\n");
        code.push_str("    }\n\n");

        code.push_str("#define GENERATE_VARIABLE(n) \\\n");
        code.push_str("    const int var_##n = n * 3;\n\n");

        code.push_str("#define GENERATE_STRUCT(n) \\\n");
        code.push_str("    struct Struct_##n { \\\n");
        code.push_str("        int value = n; \\\n");
        code.push_str("        int getValue() const { return value; } \\\n");
        code.push_str("    };\n\n");

        if num_functions > 0 {
            code.push_str(&format!("// Генерация {} функций\n", num_functions));
            for i in 0..num_functions {
                code.push_str(&format!("GENERATE_FUNCTION({})\n", i));
            }
        }

        if num_variables > 0 {
            code.push_str(&format!("\n// Генерация {} переменных\n", num_variables));
            for i in 0..num_variables {
                code.push_str(&format!("GENERATE_VARIABLE({})\n", i));
            }
        }

        if num_structs > 0 {
            code.push_str(&format!("\n// Генерация {} структур\n", num_structs));
            for i in 0..num_structs {
                code.push_str(&format!("GENERATE_STRUCT({})\n", i));
            }
        }

        code.push_str("\nint main() {\n");
        code.push_str("    std::cout << \"Starting massive code execution...\\n\";\n");

        if num_functions > 0 && usage_density > 0 {
            code.push_str("    \n    // Массовые вызовы функций\n");
            for i in (0..num_functions).step_by(usage_density) {
                code.push_str(&format!("    volatile int result_{} = ", i));
                let calls_per_line = std::cmp::min(10, usage_density);
                for j in 0..calls_per_line {
                    if i + j < num_functions {
                        code.push_str(&format!("function_{}() + ", i + j));
                    }
                }
                code.push_str("0;\n");
            }
        }

        if num_variables > 0 && usage_density > 0 {
            code.push_str("    \n    // Использование переменных\n");
            code.push_str("    volatile int sum = ");
            for i in (0..num_variables).step_by(usage_density) {
                code.push_str(&format!("var_{} + ", i));
                if i > 0 && i % 20 == 0 {
                    code.push_str("\n        ");
                }
            }
            code.push_str("0;\n");
        }

        if num_structs > 0 && usage_density > 0 {
            code.push_str("    \n    // Создание экземпляров структур\n");
            for i in (0..num_structs).step_by(usage_density) {
                code.push_str(&format!("    Struct_{} obj_{};\n", i, i));
            }
        }

        code.push_str("    \n    std::cout << \"Code execution completed!\\n\";\n");
        code.push_str("    return 0;\n");
        code.push_str("}\n");

        code
    }

    #[builder(finish_fn = compile)]
    async fn cpp_code(
        #[builder(finish_fn)] executor: &NativeExecutor,
        status: i32,
        stdout: &str,
        stderr: &str,
    ) -> Artifact {
        let mut code = String::new();
        code.push_str("#include <iostream>\n");
        code.push_str("int main() {\n");
        code.push_str(&format!("  std::cout << \"{}\";\n", stdout));
        code.push_str(&format!("  std::cerr << \"{}\";\n", stderr));
        code.push_str(&format!("  return {};\n", status));
        code.push_str("}\n");

        println!("code:\n{}", code);

        executor
            .compile(
                &code,
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: None,
                    executable_size_bytes: None,
                },
            )
            .await
            .unwrap()
    }

    #[builder(finish_fn = compile)]
    async fn cpp_stdin_repeater(
        #[builder(finish_fn)] executor: &NativeExecutor,
        count: u32,
    ) -> Artifact {
        let mut code = String::new();
        code.push_str("#include <iostream>\n");
        code.push_str("#include <string>\n");
        code.push_str("int main() {\n");
        code.push_str("  std::string input;\n");
        code.push_str("  std::getline(std::cin, input);\n");
        code.push_str(&format!(
            "  for (int i = 0; i < {}; i++) {{ std::cout << input << std::endl; }}",
            count
        ));
        code.push_str("}\n");

        println!("code:\n{}", code);

        executor
            .compile(
                &code,
                &Language::GnuCpp,
                &CompilationLimits {
                    time_ms: None,
                    memory_bytes: None,
                    executable_size_bytes: None,
                },
            )
            .await
            .unwrap()
    }

    #[builder(finish_fn = create)]
    async fn executor(
        #[builder(default = false)] with_readonly_dir: bool,
        #[builder(default = false)] with_wrong_gnucpp: bool,
    ) -> (NativeExecutor, PathBuf) {
        let executor_dir = if with_readonly_dir {
            format!("/proc/coderunner_{}", Uuid::new_v4())
        } else {
            std::env::var("EXECUTOR_DIR")
                .unwrap_or_else(|_| format!("/tmp/coderunner_{}", Uuid::new_v4()))
        };

        let executor = NativeExecutor::builder()
            .dir(executor_dir.clone())
            .gnucpp_path(if with_wrong_gnucpp {
                "/aboba".to_string()
            } else {
                std::env::var("GNUCPP_PATH").unwrap_or_else(|_| "/usr/bin/g++".to_string())
            })
            .systemd_run_path(
                std::env::var("SYSTEMD_RUN_PATH")
                    .unwrap_or_else(|_| "/usr/bin/systemd-run".to_string()),
            )
            .journalctl_path(
                std::env::var("JOURNALCTL_PATH")
                    .unwrap_or_else(|_| "/usr/bin/journalctl".to_string()),
            )
            .build();

        (executor, executor_dir.into())
    }
}
