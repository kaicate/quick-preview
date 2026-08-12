use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

use crate::Result;

pub struct AtomicWriter {
    destination: PathBuf,
    temporary: PathBuf,
    writer: Option<BufWriter<File>>,
    committed: bool,
}

impl AtomicWriter {
    pub fn new(destination: &Path) -> Result<Self> {
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        let name = destination
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("document");
        for nonce in 0..1_000u32 {
            let temporary = parent.join(format!(".{name}.quickpreview-{nonce}.tmp"));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(file) => {
                    return Ok(Self {
                        destination: destination.to_owned(),
                        temporary,
                        writer: Some(BufWriter::with_capacity(1024 * 1024, file)),
                        committed: false,
                    })
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a temporary save file",
        )
        .into())
    }

    pub fn commit(mut self) -> Result<()> {
        let mut writer = self
            .writer
            .take()
            .expect("writer is available before commit");
        writer.flush()?;
        writer.get_ref().sync_all()?;
        // Windows cannot replace a file while the source handle is still open.
        drop(writer);
        replace_file(&self.temporary, &self.destination)?;
        self.committed = true;
        Ok(())
    }
}

impl Write for AtomicWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.writer
            .as_mut()
            .expect("writer is available before commit")
            .write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.writer
            .as_mut()
            .expect("writer is available before commit")
            .flush()
    }
}

impl Drop for AtomicWriter {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::GetLastError,
            Storage::FileSystem::{
                MoveFileExW, ReplaceFileW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
                REPLACE_FILE_FLAGS,
            },
        },
    };
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let src = wide(source);
    let dst = wide(destination);
    let result = unsafe {
        if destination.exists() {
            ReplaceFileW(
                PCWSTR(dst.as_ptr()),
                PCWSTR(src.as_ptr()),
                PCWSTR::null(),
                REPLACE_FILE_FLAGS(0),
                None,
                None,
            )
        } else {
            MoveFileExW(
                PCWSTR(src.as_ptr()),
                PCWSTR(dst.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    result.map_err(|_| std::io::Error::from_raw_os_error(unsafe { GetLastError().0 as i32 }))
}
