use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::{
    delimited::DelimitedDocument,
    encoding::{detect, EncodingInfo},
    history::UndoStack,
    html::HtmlDocument,
    markdown::MarkdownDocument,
    PreviewError, Result, DELIMITED_FILE_LIMIT, WEB_PREVIEW_LIMIT,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Csv,
    Tsv,
    Markdown,
    Html,
}

impl DocumentKind {
    pub fn from_path(path: &Path) -> Result<Self> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match ext.as_str() {
            "csv" => Ok(Self::Csv),
            "tsv" => Ok(Self::Tsv),
            "md" | "markdown" => Ok(Self::Markdown),
            "html" | "htm" => Ok(Self::Html),
            _ => Err(PreviewError::UnsupportedType(path.to_owned())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFingerprint {
    pub size: u64,
    pub modified: Option<SystemTime>,
    #[cfg(windows)]
    pub file_index: Option<u64>,
}

impl FileFingerprint {
    pub fn read(path: &Path) -> Result<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            size: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(windows)]
            file_index: windows_file_index(path),
        })
    }
}

#[cfg(windows)]
fn windows_file_index(path: &Path) -> Option<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION},
    };
    let file = fs::File::open(path).ok()?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info).ok()?;
    }
    Some(((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64)
}

pub enum FormatDocument {
    Delimited(DelimitedDocument),
    Markdown(MarkdownDocument),
    Html(HtmlDocument),
}

pub struct DocumentSession {
    pub kind: DocumentKind,
    pub path: PathBuf,
    pub encoding: EncodingInfo,
    pub dirty: bool,
    pub revision: u64,
    pub fingerprint: FileFingerprint,
    pub undo_stack: UndoStack,
    pub document: FormatDocument,
}

impl DocumentSession {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_impl(path.as_ref(), true)
    }

    pub fn open_without_size_limit(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_impl(path.as_ref(), false)
    }

    pub fn size_limit_for(path: &Path) -> Result<u64> {
        let kind = DocumentKind::from_path(path)?;
        Ok(if matches!(kind, DocumentKind::Csv | DocumentKind::Tsv) {
            DELIMITED_FILE_LIMIT
        } else {
            WEB_PREVIEW_LIMIT
        })
    }

    fn open_impl(path: &Path, enforce_size_limit: bool) -> Result<Self> {
        let path = path.to_owned();
        let kind = DocumentKind::from_path(&path)?;
        let fingerprint = FileFingerprint::read(&path)?;
        let limit = Self::size_limit_for(&path)?;
        if enforce_size_limit && fingerprint.size > limit {
            return Err(PreviewError::FileTooLarge {
                actual: fingerprint.size,
                limit,
            });
        }

        let first = read_prefix(&path, 64 * 1024)?;
        let encoding = detect(&first);
        let document = match kind {
            DocumentKind::Csv => {
                FormatDocument::Delimited(DelimitedDocument::open(&path, b',', encoding)?)
            }
            DocumentKind::Tsv => {
                FormatDocument::Delimited(DelimitedDocument::open(&path, b'\t', encoding)?)
            }
            DocumentKind::Markdown => {
                FormatDocument::Markdown(MarkdownDocument::open(&path, encoding)?)
            }
            DocumentKind::Html => FormatDocument::Html(HtmlDocument::open(&path, encoding)?),
        };
        let encoding = match &document {
            FormatDocument::Delimited(document) => document.encoding,
            FormatDocument::Markdown(document) => document.encoding,
            FormatDocument::Html(document) => document.encoding,
        };
        Ok(Self {
            kind,
            path,
            encoding,
            dirty: false,
            revision: 0,
            fingerprint,
            undo_stack: UndoStack::default(),
            document,
        })
    }

    pub fn ensure_unchanged(&self) -> Result<()> {
        if FileFingerprint::read(&self.path)? == self.fingerprint {
            Ok(())
        } else {
            Err(PreviewError::ExternalChange)
        }
    }

    pub fn save(&mut self) -> Result<()> {
        self.ensure_unchanged()?;
        match &mut self.document {
            FormatDocument::Delimited(doc) => doc.save(&self.path)?,
            FormatDocument::Markdown(doc) => doc.save(&self.path)?,
            FormatDocument::Html(doc) => doc.save(&self.path)?,
        }
        self.fingerprint = FileFingerprint::read(&self.path)?;
        self.dirty = false;
        self.revision = self.revision.saturating_add(1);
        self.undo_stack.clear();
        Ok(())
    }

    pub fn mark_edited(&mut self) {
        self.dirty = true;
        self.revision = self.revision.saturating_add(1);
    }
}

fn read_prefix(path: &Path, length: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = fs::File::open(path)?;
    let mut bytes = Vec::with_capacity(length);
    file.take(length as u64).read_to_end(&mut bytes)?;
    Ok(bytes)
}
