use std::path::{Path, PathBuf};

use tokio::{fs, process::Command};
use uuid::Uuid;

use crate::core::{
    domain::{Artifact, ArtifactKind, CompilationLimits, ExecutionLimits, Language},
    traits::executor::{CompileError, Executor, RunError, RunResult},
};

#[derive(Clone, Debug)]
pub struct NativeExecutor {
    dir: PathBuf,
    gnucpp_path: PathBuf,
}

impl NativeExecutor {
    pub fn new<T, U>(dir: T, gnucpp_path: U) -> Self
    where
        T: AsRef<Path>,
        U: AsRef<Path>,
    {
        NativeExecutor {
            dir: dir.as_ref().into(),
            gnucpp_path: gnucpp_path.as_ref().into(),
        }
    }
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
        let artifact_path = self.dir.join(format!("{}.out", artifact_id));
        let source_path = self.dir.join(format!("{}.cpp", artifact_id));

        fs::create_dir_all(&self.dir)
            .await
            .map_err(|e| CompileError::Internal { msg: e.to_string() })?;
        fs::write(&source_path, source)
            .await
            .map_err(|e| CompileError::Internal { msg: e.to_string() })?;

        let out = Command::new(&self.gnucpp_path)
            .arg("-o")
            .arg(artifact_path)
            .arg(source_path)
            .spawn()
            .map_err(|e| CompileError::Internal { msg: e.to_string() })?
            .wait_with_output()
            .await
            .map_err(|e| CompileError::Internal { msg: e.to_string() })?;

        if !out.status.success() {
            return Err(CompileError::CompilationFailed {
                msg: String::from_utf8_lossy(&out.stderr).to_string(),
            });
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
        unimplemented!()
    }
}

// TODO: Write tests with compiling limits
// TODO: Create Dockerfile for executor testing with fixed g++ and filesystem

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tokio::{fs, process::Command};
    use uuid::Uuid;

    use crate::{
        core::{
            domain::{Artifact, ArtifactKind, CompilationLimitType, CompilationLimits, Language},
            traits::executor::{CompileError, Executor},
        },
        native::executor::NativeExecutor,
    };

    fn gnucpp_path() -> String {
        std::env::var("GNUCPP_PATH").unwrap_or_else(|_| "/usr/bin/g++".to_string())
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

    fn massive_cpp_code(
        num_functions: usize,
        num_variables: usize,
        num_structs: usize,
        usage_density: usize,
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

    #[tokio::test]
    async fn test_compile_success() {
        let executor_dir = format!("/tmp/coderunner_{}", Uuid::new_v4());
        let executor_dir = Path::new(&executor_dir);
        let executor = NativeExecutor::new(executor_dir, gnucpp_path());

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
        let executor_dir = format!("/tmp/coderunner_{}", Uuid::new_v4());
        let executor_dir = Path::new(&executor_dir);
        let executor = NativeExecutor::new(executor_dir, gnucpp_path());

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

        assert!(matches!(
            result,
            Err(CompileError::CompilationFailed { .. })
        ));
    }

    #[tokio::test]
    async fn test_compile_compiler_not_found() {
        let executor_dir = format!("/tmp/coderunner_{}", Uuid::new_v4());
        let executor_dir = Path::new(&executor_dir);
        let executor = NativeExecutor::new(executor_dir, "/aboba");

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
        // /proc is readonly dir
        let executor_dir = format!("/proc/coderunner_{}", Uuid::new_v4());
        let executor_dir = Path::new(&executor_dir);
        let executor = NativeExecutor::new(executor_dir, gnucpp_path());

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
}
