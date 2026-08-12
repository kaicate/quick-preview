use std::{collections::BTreeMap, fs::File, io::Write, ops::Range, path::Path};

use memmap2::Mmap;

use crate::{
    atomic_save::AtomicWriter,
    encoding::{decode, detect, encode, EncodingInfo},
    PreviewError, Result, RECORD_LIMIT,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Newline {
    CrLf,
    Lf,
    Cr,
    None,
}

impl Newline {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::CrLf => b"\r\n",
            Self::Lf => b"\n",
            Self::Cr => b"\r",
            Self::None => b"",
        }
    }
}

#[derive(Debug, Clone)]
struct RecordSpan {
    content: Range<usize>,
    full: Range<usize>,
    newline: Newline,
}

pub struct DelimitedDocument {
    map: MappedFile,
    delimiter: u8,
    pub encoding: EncodingInfo,
    rows: Vec<RecordSpan>,
    edits: BTreeMap<(usize, usize), String>,
    max_columns: usize,
}

impl DelimitedDocument {
    pub fn open(path: &Path, delimiter: u8, _encoding_hint: EncodingInfo) -> Result<Self> {
        let file = File::open(path)?;
        let map = MappedFile::open(&file)?;
        let encoding = detect(&map);
        let data_start = if encoding.bom && map.starts_with(&[0xEF, 0xBB, 0xBF]) {
            3
        } else {
            0
        };
        let rows = index_records(&map, data_start)?;
        let mut result = Self {
            map,
            delimiter,
            encoding,
            rows,
            edits: BTreeMap::new(),
            max_columns: 0,
        };
        result.max_columns = result
            .rows
            .iter()
            .take(1_000)
            .enumerate()
            .map(|(row, _)| result.parse_row_bytes(row).map(|v| v.len()).unwrap_or(0))
            .max()
            .unwrap_or(0);
        Ok(result)
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }
    pub fn estimated_column_count(&self) -> usize {
        self.max_columns
    }
    pub fn is_dirty(&self) -> bool {
        !self.edits.is_empty()
    }

    pub fn row(&self, row: usize) -> Result<Vec<String>> {
        let fields = self.parse_row_bytes(row)?;
        Ok(fields
            .into_iter()
            .enumerate()
            .map(|(column, bytes)| {
                self.edits
                    .get(&(row, column))
                    .cloned()
                    .unwrap_or_else(|| decode(&bytes, self.encoding.encoding))
            })
            .collect())
    }

    pub fn cell(&self, row: usize, column: usize) -> Result<String> {
        if let Some(value) = self.edits.get(&(row, column)) {
            return Ok(value.clone());
        }
        Ok(self.row(row)?.get(column).cloned().unwrap_or_default())
    }

    pub fn edit_cell(&mut self, row: usize, column: usize, value: String) -> Result<String> {
        if row >= self.rows.len() {
            return Err(PreviewError::InvalidEditTarget(format!("row {row}")));
        }
        let before = self.cell(row, column)?;
        self.edits.insert((row, column), value);
        self.max_columns = self.max_columns.max(column + 1);
        Ok(before)
    }

    pub fn set_cell_without_history(
        &mut self,
        row: usize,
        column: usize,
        value: String,
    ) -> Result<()> {
        if row >= self.rows.len() {
            return Err(PreviewError::InvalidEditTarget(format!("row {row}")));
        }
        self.edits.insert((row, column), value);
        Ok(())
    }

    pub fn save(&mut self, path: &Path) -> Result<()> {
        if self.edits.is_empty() {
            return Ok(());
        }
        let mut writer = AtomicWriter::new(path)?;
        if self.encoding.bom {
            writer.write_all(&[0xEF, 0xBB, 0xBF])?;
        }
        for row in 0..self.rows.len() {
            if !self.edits.keys().any(|(r, _)| *r == row) {
                writer.write_all(&self.map[self.rows[row].full.clone()])?;
                continue;
            }
            let mut fields = self.row(row)?;
            let required = self
                .edits
                .range((row, 0)..=(row, usize::MAX))
                .map(|((_, column), _)| column + 1)
                .max()
                .unwrap_or(0)
                .max(fields.len());
            fields.resize(required, String::new());
            for (column, field) in fields.iter().enumerate() {
                if column > 0 {
                    writer.write_all(&[self.delimiter])?;
                }
                let encoded = encode(field, self.encoding.encoding)?;
                write_escaped(&mut writer, &encoded, self.delimiter)?;
            }
            writer.write_all(self.rows[row].newline.bytes())?;
        }
        writer.commit()?;
        self.edits.clear();
        let file = File::open(path)?;
        self.map = MappedFile::open(&file)?;
        let start = if self.encoding.bom && self.map.starts_with(&[0xEF, 0xBB, 0xBF]) {
            3
        } else {
            0
        };
        self.rows = index_records(&self.map, start)?;
        Ok(())
    }

    fn parse_row_bytes(&self, row: usize) -> Result<Vec<Vec<u8>>> {
        let span = self
            .rows
            .get(row)
            .ok_or_else(|| PreviewError::InvalidEditTarget(format!("row {row}")))?;
        parse_fields(
            &self.map[span.content.clone()],
            self.delimiter,
            span.content.start,
        )
    }
}

struct MappedFile(Option<Mmap>);

impl MappedFile {
    fn open(file: &File) -> Result<Self> {
        if file.metadata()?.len() == 0 {
            Ok(Self(None))
        } else {
            Ok(Self(Some(unsafe { Mmap::map(file)? })))
        }
    }
}

impl std::ops::Deref for MappedFile {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.0.as_deref().unwrap_or(&[])
    }
}

fn index_records(bytes: &[u8], start: usize) -> Result<Vec<RecordSpan>> {
    let mut records = Vec::new();
    let mut record_start = start;
    let mut quoted = false;
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                if quoted && bytes.get(index + 1) == Some(&b'"') {
                    index += 2;
                    continue;
                }
                quoted = !quoted;
                index += 1;
            }
            b'\r' | b'\n' if !quoted => {
                let (newline, end) =
                    if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                        (Newline::CrLf, index + 2)
                    } else if bytes[index] == b'\r' {
                        (Newline::Cr, index + 1)
                    } else {
                        (Newline::Lf, index + 1)
                    };
                if index - record_start > RECORD_LIMIT {
                    return Err(PreviewError::RecordTooLarge {
                        row: records.len(),
                        limit: RECORD_LIMIT,
                    });
                }
                records.push(RecordSpan {
                    content: record_start..index,
                    full: record_start..end,
                    newline,
                });
                record_start = end;
                index = end;
            }
            _ => index += 1,
        }
    }
    if quoted {
        return Err(PreviewError::MalformedDelimited {
            offset: bytes.len(),
            message: "unterminated quoted field".into(),
        });
    }
    if record_start < bytes.len() || bytes.is_empty() {
        if bytes.len().saturating_sub(record_start) > RECORD_LIMIT {
            return Err(PreviewError::RecordTooLarge {
                row: records.len(),
                limit: RECORD_LIMIT,
            });
        }
        records.push(RecordSpan {
            content: record_start..bytes.len(),
            full: record_start..bytes.len(),
            newline: Newline::None,
        });
    }
    Ok(records)
}

fn parse_fields(record: &[u8], delimiter: u8, source_offset: usize) -> Result<Vec<Vec<u8>>> {
    let mut fields = Vec::new();
    let mut field = Vec::new();
    let mut index = 0;
    let mut quoted = false;
    let mut at_start = true;
    while index < record.len() {
        let byte = record[index];
        if at_start && byte == b'"' {
            quoted = true;
            at_start = false;
            index += 1;
            continue;
        }
        if quoted && byte == b'"' {
            if record.get(index + 1) == Some(&b'"') {
                field.push(b'"');
                index += 2;
                continue;
            }
            quoted = false;
            index += 1;
            if index < record.len() && record[index] != delimiter {
                return Err(PreviewError::MalformedDelimited {
                    offset: source_offset + index,
                    message: "characters after closing quote".into(),
                });
            }
            continue;
        }
        if !quoted && byte == delimiter {
            fields.push(std::mem::take(&mut field));
            at_start = true;
            index += 1;
            continue;
        }
        field.push(byte);
        at_start = false;
        index += 1;
    }
    if quoted {
        return Err(PreviewError::MalformedDelimited {
            offset: source_offset + index,
            message: "unterminated quote".into(),
        });
    }
    fields.push(field);
    Ok(fields)
}

fn write_escaped(writer: &mut impl Write, field: &[u8], delimiter: u8) -> std::io::Result<()> {
    let quote = field
        .iter()
        .any(|b| *b == delimiter || matches!(*b, b'"' | b'\r' | b'\n'));
    if quote {
        writer.write_all(b"\"")?;
    }
    for &byte in field {
        if byte == b'"' {
            writer.write_all(b"\"\"")?;
        } else {
            writer.write_all(&[byte])?;
        }
    }
    if quote {
        writer.write_all(b"\"")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write};

    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("quick-preview-{}-{name}", std::process::id()));
        let mut file = File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        path
    }

    #[test]
    fn quoted_newline_and_trailing_empty_cell() {
        let path = temp_file("quoted.csv", b"a,\"b\nB\",\r\nc,d,e\n");
        let mut doc = DelimitedDocument::open(
            &path,
            b',',
            EncodingInfo {
                encoding: crate::encoding::TextEncoding::Utf8,
                bom: false,
            },
        )
        .unwrap();
        assert_eq!(doc.row_count(), 2);
        assert_eq!(doc.row(0).unwrap(), vec!["a", "b\nB", ""]);
        doc.edit_cell(0, 1, "x,y".into()).unwrap();
        doc.save(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"a,\"x,y\",\r\nc,d,e\n");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_unclosed_quote() {
        assert!(index_records(b"a,\"b", 0).is_err());
    }

    #[test]
    fn empty_file_has_one_editable_row() {
        let path = temp_file("empty.csv", b"");
        let mut doc = DelimitedDocument::open(
            &path,
            b',',
            EncodingInfo {
                encoding: crate::encoding::TextEncoding::Utf8,
                bom: false,
            },
        )
        .unwrap();
        assert_eq!(doc.row_count(), 1);
        doc.edit_cell(0, 0, "value".into()).unwrap();
        doc.save(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"value");
        let _ = fs::remove_file(path);
    }
}
