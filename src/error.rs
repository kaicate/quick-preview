use std::{io, path::PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error("unsupported file type: {0}")]
    UnsupportedType(PathBuf),
    #[error("file is too large ({actual} bytes; limit is {limit} bytes)")]
    FileTooLarge { actual: u64, limit: u64 },
    #[error("record {row} is larger than the {limit} byte limit")]
    RecordTooLarge { row: usize, limit: usize },
    #[error("malformed delimited data near byte {offset}: {message}")]
    MalformedDelimited { offset: usize, message: String },
    #[error("the file changed outside QuickPreview")]
    ExternalChange,
    #[error("text cannot be represented in Shift_JIS")]
    UnrepresentableShiftJis,
    #[error("invalid edit target: {0}")]
    InvalidEditTarget(String),
    #[error("preview message was rejected: {0}")]
    InvalidWebMessage(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, PreviewError>;
