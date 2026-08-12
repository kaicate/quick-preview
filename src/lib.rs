pub mod atomic_save;
pub mod delimited;
pub mod document;
pub mod encoding;
pub mod error;
pub mod history;
pub mod html;
pub mod markdown;
pub mod web_message;

pub use document::{DocumentKind, DocumentSession, FormatDocument};
pub use error::{PreviewError, Result};

pub const DELIMITED_FILE_LIMIT: u64 = 100 * 1024 * 1024;
pub const WEB_PREVIEW_LIMIT: u64 = 20 * 1024 * 1024;
pub const RECORD_LIMIT: usize = 16 * 1024 * 1024;
