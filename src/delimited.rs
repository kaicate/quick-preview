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

#[derive(Debug, Clone, Copy)]
struct IndexCheckpoint {
    row: usize,
    offset: usize,
}

const INITIAL_CACHE_ROWS: usize = 64;
const CACHE_PADDING_ROWS: usize = 128;
const CHECKPOINT_INTERVAL_ROWS: usize = 128;
pub const COLUMN_CHUNK_SIZE: usize = 32;

pub struct DelimitedDocument {
    map: MappedFile,
    delimiter: u8,
    pub encoding: EncodingInfo,
    data_start: usize,
    cache_start_row: usize,
    rows: Vec<RecordSpan>,
    checkpoints: Vec<IndexCheckpoint>,
    known_row_count: Option<usize>,
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
        let mut result = Self {
            map,
            delimiter,
            encoding,
            data_start,
            cache_start_row: 0,
            rows: Vec::new(),
            checkpoints: vec![IndexCheckpoint {
                row: 0,
                offset: data_start,
            }],
            known_row_count: None,
            edits: BTreeMap::new(),
            max_columns: 0,
        };
        result.ensure_rows_around(0, INITIAL_CACHE_ROWS)?;
        result.ensure_columns_around(0, 1)?;
        Ok(result)
    }

    pub fn row_count(&self) -> usize {
        self.known_row_count
            .unwrap_or_else(|| self.cache_start_row + self.rows.len() + 1)
    }
    pub fn known_row_count(&self) -> Option<usize> {
        self.known_row_count
    }
    pub fn estimated_column_count(&self) -> usize {
        self.max_columns
    }
    pub fn is_dirty(&self) -> bool {
        !self.edits.is_empty()
    }

    pub fn ensure_rows_around(&mut self, first_row: usize, visible_rows: usize) -> Result<()> {
        if self.map.is_empty() {
            self.cache_start_row = 0;
            self.rows = vec![RecordSpan {
                content: self.data_start..self.data_start,
                full: self.data_start..self.data_start,
                newline: Newline::None,
            }];
            self.known_row_count = Some(1);
            self.max_columns = self.max_columns.max(1);
            return Ok(());
        }

        let window_start = first_row.saturating_sub(CACHE_PADDING_ROWS);
        let desired_end = first_row
            .saturating_add(visible_rows.max(1))
            .saturating_add(CACHE_PADDING_ROWS);
        let cached_end = self.cache_start_row.saturating_add(self.rows.len());
        let required_end = self
            .known_row_count
            .map_or(desired_end, |row_count| desired_end.min(row_count));
        if window_start >= self.cache_start_row && required_end <= cached_end {
            return Ok(());
        }

        let checkpoint = self
            .checkpoints
            .iter()
            .rev()
            .find(|checkpoint| checkpoint.row <= window_start)
            .copied()
            .unwrap_or(IndexCheckpoint {
                row: 0,
                offset: self.data_start,
            });
        let mut row = checkpoint.row;
        let mut offset = checkpoint.offset;
        let mut rows = Vec::with_capacity(
            desired_end
                .saturating_sub(window_start)
                .min(visible_rows.saturating_add(CACHE_PADDING_ROWS * 2)),
        );
        let mut checkpoints = Vec::new();
        let mut known_row_count = self.known_row_count;

        while row < desired_end {
            if row % CHECKPOINT_INTERVAL_ROWS == 0 {
                checkpoints.push(IndexCheckpoint { row, offset });
            }
            let Some(span) = scan_record(&self.map, offset, row)? else {
                known_row_count = Some(row);
                break;
            };
            offset = span.full.end;
            if row >= window_start {
                rows.push(span);
            }
            row = row.saturating_add(1);
        }

        self.checkpoints.extend(checkpoints);
        self.checkpoints.sort_by_key(|checkpoint| checkpoint.row);
        self.checkpoints.dedup_by_key(|checkpoint| checkpoint.row);
        self.cache_start_row = if rows.is_empty() {
            known_row_count.unwrap_or(window_start)
        } else {
            window_start
        };
        self.rows = rows;
        self.known_row_count = known_row_count;
        Ok(())
    }

    pub fn ensure_columns_around(
        &mut self,
        first_column: usize,
        visible_columns: usize,
    ) -> Result<()> {
        let right_edge = first_column.saturating_add(visible_columns.saturating_sub(1));
        let chunk_start = (right_edge / COLUMN_CHUNK_SIZE) * COLUMN_CHUNK_SIZE;
        for span in &self.rows {
            let slice = parse_fields_range(
                &self.map[span.content.clone()],
                self.delimiter,
                span.content.start,
                chunk_start,
                COLUMN_CHUNK_SIZE,
            )?;
            self.max_columns = self.max_columns.max(slice.total_columns_at_least);
        }
        self.max_columns = self.max_columns.max(
            self.edits
                .keys()
                .map(|(_, column)| column + 1)
                .max()
                .unwrap_or(0),
        );
        Ok(())
    }

    pub fn row_range(&self, row: usize, start: usize, count: usize) -> Result<Vec<String>> {
        let span = self.span_for_row(row)?;
        let slice = parse_fields_range(
            &self.map[span.content.clone()],
            self.delimiter,
            span.content.start,
            start,
            count,
        )?;
        let mut fields = slice
            .fields
            .into_iter()
            .map(|bytes| decode(&bytes, self.encoding.encoding))
            .collect::<Vec<_>>();
        fields.resize(count, String::new());
        if count > 0 {
            let end = start.saturating_add(count).saturating_sub(1);
            for ((_, column), value) in self.edits.range((row, start)..=(row, end)) {
                fields[*column - start].clone_from(value);
            }
        }
        Ok(fields)
    }

    pub fn row(&self, row: usize) -> Result<Vec<String>> {
        self.row_from_span(row, self.span_for_row(row)?)
    }

    pub fn cell(&self, row: usize, column: usize) -> Result<String> {
        if let Some(value) = self.edits.get(&(row, column)) {
            return Ok(value.clone());
        }
        Ok(self
            .row_range(row, column, 1)?
            .into_iter()
            .next()
            .unwrap_or_default())
    }

    pub fn edit_cell(&mut self, row: usize, column: usize, value: String) -> Result<String> {
        self.span_for_row(row)?;
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
        self.span_for_row(row)?;
        self.edits.insert((row, column), value);
        self.max_columns = self.max_columns.max(column + 1);
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
        let mut row = 0usize;
        let mut offset = self.data_start;
        loop {
            let span = if self.map.is_empty() && row == 0 {
                Some(RecordSpan {
                    content: self.data_start..self.data_start,
                    full: self.data_start..self.data_start,
                    newline: Newline::None,
                })
            } else {
                scan_record(&self.map, offset, row)?
            };
            let Some(span) = span else { break };
            offset = span.full.end;
            if self
                .edits
                .range((row, 0)..=(row, usize::MAX))
                .next()
                .is_none()
            {
                writer.write_all(&self.map[span.full.clone()])?;
                row = row.saturating_add(1);
                continue;
            }
            let mut fields = self.row_from_span(row, &span)?;
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
            writer.write_all(span.newline.bytes())?;
            row = row.saturating_add(1);
        }
        // The mapped destination must be released before ReplaceFileW can
        // atomically replace it on Windows. All source bytes have already
        // been copied to the temporary file at this point.
        self.map = MappedFile(None);
        if let Err(error) = writer.commit() {
            if let Ok(file) = File::open(path) {
                if let Ok(map) = MappedFile::open(&file) {
                    self.map = map;
                }
            }
            return Err(error);
        }

        let file = File::open(path)?;
        self.map = MappedFile::open(&file)?;
        self.data_start = if self.encoding.bom && self.map.starts_with(&[0xEF, 0xBB, 0xBF]) {
            3
        } else {
            0
        };
        self.edits.clear();
        self.reset_cache()?;
        Ok(())
    }

    fn span_for_row(&self, row: usize) -> Result<&RecordSpan> {
        row.checked_sub(self.cache_start_row)
            .and_then(|index| self.rows.get(index))
            .ok_or_else(|| PreviewError::InvalidEditTarget(format!("row {row}")))
    }

    fn row_from_span(&self, row: usize, span: &RecordSpan) -> Result<Vec<String>> {
        let fields = parse_fields(
            &self.map[span.content.clone()],
            self.delimiter,
            span.content.start,
        )?;
        let mut fields = fields
            .into_iter()
            .map(|bytes| decode(&bytes, self.encoding.encoding))
            .collect::<Vec<_>>();
        let required = self
            .edits
            .range((row, 0)..=(row, usize::MAX))
            .map(|((_, column), _)| column + 1)
            .max()
            .unwrap_or(fields.len())
            .max(fields.len());
        fields.resize(required, String::new());
        for ((_, column), value) in self.edits.range((row, 0)..=(row, usize::MAX)) {
            fields[*column].clone_from(value);
        }
        Ok(fields)
    }

    fn reset_cache(&mut self) -> Result<()> {
        self.cache_start_row = 0;
        self.rows.clear();
        self.checkpoints = vec![IndexCheckpoint {
            row: 0,
            offset: self.data_start,
        }];
        self.known_row_count = None;
        self.max_columns = 0;
        self.ensure_rows_around(0, INITIAL_CACHE_ROWS)?;
        self.ensure_columns_around(0, 1)
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

fn scan_record(bytes: &[u8], start: usize, row: usize) -> Result<Option<RecordSpan>> {
    if start >= bytes.len() {
        return Ok(None);
    }
    let record_start = start;
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
                        row,
                        limit: RECORD_LIMIT,
                    });
                }
                return Ok(Some(RecordSpan {
                    content: record_start..index,
                    full: record_start..end,
                    newline,
                }));
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
    if record_start < bytes.len() {
        if bytes.len().saturating_sub(record_start) > RECORD_LIMIT {
            return Err(PreviewError::RecordTooLarge {
                row,
                limit: RECORD_LIMIT,
            });
        }
        return Ok(Some(RecordSpan {
            content: record_start..bytes.len(),
            full: record_start..bytes.len(),
            newline: Newline::None,
        }));
    }
    Ok(None)
}

#[cfg(test)]
fn index_records(bytes: &[u8], start: usize) -> Result<Vec<RecordSpan>> {
    if bytes.is_empty() {
        return Ok(vec![RecordSpan {
            content: start..start,
            full: start..start,
            newline: Newline::None,
        }]);
    }
    let mut records = Vec::new();
    let mut offset = start;
    while let Some(record) = scan_record(bytes, offset, records.len())? {
        offset = record.full.end;
        records.push(record);
    }
    Ok(records)
}

struct FieldSlice {
    fields: Vec<Vec<u8>>,
    total_columns_at_least: usize,
}

fn parse_fields_range(
    record: &[u8],
    delimiter: u8,
    source_offset: usize,
    start: usize,
    count: usize,
) -> Result<FieldSlice> {
    if count == 0 {
        return Ok(FieldSlice {
            fields: Vec::new(),
            total_columns_at_least: 0,
        });
    }
    let end = start.saturating_add(count);
    let mut fields = Vec::with_capacity(count);
    let mut field = Vec::new();
    let mut column = 0usize;
    let mut index = 0usize;
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
                if column >= start && column < end {
                    field.push(b'"');
                }
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
            if column >= start && column < end {
                fields.push(std::mem::take(&mut field));
            }
            column = column.saturating_add(1);
            if column >= end {
                return Ok(FieldSlice {
                    fields,
                    total_columns_at_least: column.saturating_add(1),
                });
            }
            at_start = true;
            index += 1;
            continue;
        }
        if column >= start && column < end {
            field.push(byte);
        }
        at_start = false;
        index += 1;
    }
    if quoted {
        return Err(PreviewError::MalformedDelimited {
            offset: source_offset + index,
            message: "unterminated quote".into(),
        });
    }
    if column >= start && column < end {
        fields.push(field);
    }
    Ok(FieldSlice {
        fields,
        total_columns_at_least: column.saturating_add(1),
    })
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
        assert_eq!(doc.estimated_column_count(), 3);
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

    #[test]
    fn indexes_only_the_viewport_neighborhood_and_streams_evicted_edits() {
        let mut bytes = Vec::new();
        for row in 0..10_000 {
            writeln!(bytes, "{row},value").unwrap();
        }
        let path = temp_file("large.csv", &bytes);
        let mut doc = DelimitedDocument::open(
            &path,
            b',',
            EncodingInfo {
                encoding: crate::encoding::TextEncoding::Utf8,
                bom: false,
            },
        )
        .unwrap();

        assert_eq!(doc.known_row_count(), None);
        assert!(doc.rows.len() <= INITIAL_CACHE_ROWS + CACHE_PADDING_ROWS);
        doc.ensure_rows_around(5_000, 40).unwrap();
        assert_eq!(doc.row(5_000).unwrap(), vec!["5000", "value"]);
        assert!(doc.rows.len() <= 40 + CACHE_PADDING_ROWS * 2);
        doc.edit_cell(5_000, 1, "changed".into()).unwrap();
        doc.edit_cell(5_000, 3, "tail".into()).unwrap();

        doc.ensure_rows_around(9_000, 40).unwrap();
        assert!(doc.row(5_000).is_err());
        doc.save(&path).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("5000,changed,,tail\n"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn parses_wide_rows_in_32_column_chunks() {
        let source = (0..100)
            .map(|column| format!("c{column}"))
            .collect::<Vec<_>>()
            .join(",");
        let path = temp_file("wide.csv", source.as_bytes());
        let mut doc = DelimitedDocument::open(
            &path,
            b',',
            EncodingInfo {
                encoding: crate::encoding::TextEncoding::Utf8,
                bom: false,
            },
        )
        .unwrap();

        assert_eq!(doc.estimated_column_count(), COLUMN_CHUNK_SIZE + 1);
        let middle = doc.row_range(0, 32, COLUMN_CHUNK_SIZE).unwrap();
        assert_eq!(middle.first().map(String::as_str), Some("c32"));
        assert_eq!(middle.last().map(String::as_str), Some("c63"));
        doc.ensure_columns_around(32, 1).unwrap();
        assert_eq!(doc.estimated_column_count(), 65);
        doc.ensure_columns_around(96, 1).unwrap();
        assert_eq!(doc.estimated_column_count(), 100);
        let _ = fs::remove_file(path);
    }
}
