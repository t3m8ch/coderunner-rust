use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{Duration, timeout};
use uuid::Uuid;

use crate::compiler::errors::CompilationError;
use crate::compiler::traits::Compiler;
use crate::domain::{Artifact, ArtifactKind, CompilationLimitType, CompilationLimits, Language};

#[derive(Debug)]
pub struct BasicCompiler {
    temp_dir: PathBuf,
}

impl BasicCompiler {
    pub fn new() -> std::io::Result<Self> {
        let base_temp_dir = std::env::temp_dir().join("coderunner");
        std::fs::create_dir_all(&base_temp_dir)?;

        // Create a unique temp directory for this compiler instance
        let unique_id = Uuid::new_v4();
        let temp_dir = base_temp_dir.join(format!("compiler_{}", unique_id));
        std::fs::create_dir_all(&temp_dir)?;

        Ok(Self { temp_dir })
    }
}

#[async_trait]
impl Compiler for BasicCompiler {
    async fn compile(
        &self,
        source: &str,
        language: &Language,
        limits: &CompilationLimits,
    ) -> Result<Artifact, CompilationError> {
        match language {
            Language::GnuCpp => self.compile_cpp(source, limits).await,
        }
    }
}

impl BasicCompiler {
    async fn compile_cpp(
        &self,
        source: &str,
        limits: &CompilationLimits,
    ) -> Result<Artifact, CompilationError> {
        let artifact_id = Uuid::new_v4();
        let source_file = self.temp_dir.join(format!("{}.cpp", artifact_id));
        let output_file = self.temp_dir.join(format!("{}", artifact_id));

        // Write source code to temporary file
        let mut file = fs::File::create(&source_file).await.map_err(|e| {
            CompilationError::CompilationFailed {
                msg: format!("Failed to create source file: {}", e),
            }
        })?;

        file.write_all(source.as_bytes()).await.map_err(|e| {
            CompilationError::CompilationFailed {
                msg: format!("Failed to write source code: {}", e),
            }
        })?;

        // Ensure the file is flushed to disk
        file.flush()
            .await
            .map_err(|e| CompilationError::CompilationFailed {
                msg: format!("Failed to flush source file: {}", e),
            })?;

        // Close the file to ensure it's fully written
        drop(file);

        // Prepare g++ command
        let mut cmd = Command::new("g++");
        cmd.arg("-o")
            .arg(&output_file)
            .arg(&source_file)
            .arg("-std=c++17")
            .arg("-O2")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Apply time limit if specified
        let compile_future = cmd.output();
        let result = if let Some(time_limit_ms) = limits.time_ms {
            match timeout(Duration::from_millis(time_limit_ms), compile_future).await {
                Ok(result) => result,
                Err(_) => {
                    // Clean up source file
                    let _ = fs::remove_file(&source_file).await;
                    return Err(CompilationError::CompilationLimitsExceeded(
                        CompilationLimitType::Time,
                    ));
                }
            }
        } else {
            compile_future.await
        };

        let output = result.map_err(|e| CompilationError::CompilationFailed {
            msg: format!("Failed to execute g++: {}", e),
        })?;

        // Clean up source file
        let _ = fs::remove_file(&source_file).await;

        // Check if compilation was successful
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(CompilationError::CompilationFailed {
                msg: format!("Compilation failed:\n{}", stderr),
            });
        }

        // Verify that the executable was created
        if !tokio::fs::try_exists(&output_file).await.unwrap_or(false) {
            return Err(CompilationError::CompilationFailed {
                msg: format!(
                    "Executable file was not created at: {}",
                    output_file.display()
                ),
            });
        }

        Ok(Artifact {
            id: artifact_id,
            kind: ArtifactKind::Executable,
        })
    }
}

impl Drop for BasicCompiler {
    fn drop(&mut self) {
        // Clean up this compiler's specific temp directory
        let _ = std::fs::remove_dir_all(&self.temp_dir);
    }
}
