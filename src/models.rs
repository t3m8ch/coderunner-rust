#[derive(Debug, Clone)]
pub struct MsgToHandle {
    pub id: uuid::Uuid,
    pub text: String,
}

impl MsgToHandle {
    pub fn new(id: uuid::Uuid, text: &str) -> Self {
        MsgToHandle {
            id,
            text: text.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MsgToRes {
    pub id: uuid::Uuid,
    pub text: String,
}

impl MsgToRes {
    pub fn new(id: uuid::Uuid, text: &str) -> Self {
        MsgToRes {
            id,
            text: text.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileToCompile {
    pub id: uuid::Uuid,
    pub file_name: String,
}

impl FileToCompile {
    pub fn new(id: uuid::Uuid, file_name: &str) -> Self {
        FileToCompile {
            id,
            file_name: file_name.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileToRun {
    pub id: uuid::Uuid,
    pub file_name: String,
}

impl FileToRun {
    pub fn new(id: uuid::Uuid, file_name: &str) -> Self {
        FileToRun {
            id,
            file_name: file_name.to_string(),
        }
    }
}
