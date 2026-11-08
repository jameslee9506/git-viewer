use serde::{Serialize, Serializer};
use std::fmt;

/// Error type returned by every command. Serialized to the frontend as a plain string.
#[derive(Debug)]
pub enum AppError {
    /// A git invocation failed (non-allowed exit code).
    Git { args: String, code: Option<i32>, stderr: String },
    /// Spawning git or reading a file failed.
    Io(std::io::Error),
    /// The requested graph generation is no longer current. Serialized as exactly "stale".
    Stale,
    /// Generic failure with a message.
    Msg(String),
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn msg(m: impl Into<String>) -> Self {
        AppError::Msg(m.into())
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Git { args, code, stderr } => {
                let s = stderr.trim();
                if s.is_empty() {
                    write!(f, "git {} failed (exit {:?})", args, code)
                } else {
                    write!(f, "{}", s)
                }
            }
            AppError::Io(e) => write!(f, "{}", e),
            AppError::Stale => write!(f, "stale"),
            AppError::Msg(m) => write!(f, "{}", m),
        }
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e)
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
