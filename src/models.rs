#[derive(Debug, Clone)]
pub struct FileToCompile {
    pub file_name: String,
}

impl FileToCompile {
    pub fn new(file_name: &str) -> Self {
        FileToCompile {
            file_name: file_name.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileToRun {
    pub file_name: String,
}

impl FileToRun {
    pub fn new(file_name: &str) -> Self {
        FileToRun {
            file_name: file_name.to_string(),
        }
    }
}
