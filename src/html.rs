use std::{fs, io::Write, ops::Range, path::Path};

use crate::{
    atomic_save::AtomicWriter,
    encoding::{decode, detect, encode, payload, EncodingInfo},
    markdown::preview_shell,
    PreviewError, Result,
};

#[derive(Debug, Clone)]
pub struct HtmlTextNode {
    pub id: u64,
    pub range: Range<usize>,
}

pub struct HtmlDocument {
    source: String,
    pub encoding: EncodingInfo,
    pub nodes: Vec<HtmlTextNode>,
}

impl HtmlDocument {
    pub fn open(path: &Path, _encoding_hint: EncodingInfo) -> Result<Self> {
        let bytes = fs::read(path)?;
        let encoding = detect(&bytes);
        let source = decode(payload(&bytes, encoding), encoding.encoding);
        let nodes = visible_text_nodes(&source);
        Ok(Self {
            source,
            encoding,
            nodes,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn edit_text(&mut self, id: u64, text: &str) -> Result<String> {
        let range = self
            .nodes
            .iter()
            .find(|node| node.id == id)
            .map(|node| node.range.clone())
            .ok_or_else(|| PreviewError::InvalidEditTarget(format!("HTML node {id}")))?;
        let before = self.source[range.clone()].to_owned();
        self.source.replace_range(range, &escape_html_text(text));
        self.nodes = visible_text_nodes(&self.source);
        Ok(before)
    }

    pub fn replace_at(&mut self, start: usize, expected: &str, replacement: &str) -> Result<()> {
        let end = start
            .checked_add(expected.len())
            .ok_or_else(|| PreviewError::InvalidEditTarget("source range overflow".into()))?;
        if self.source.get(start..end) != Some(expected) {
            return Err(PreviewError::InvalidEditTarget(
                "HTML source changed since edit".into(),
            ));
        }
        self.source.replace_range(start..end, replacement);
        self.nodes = visible_text_nodes(&self.source);
        Ok(())
    }

    pub fn save(&mut self, path: &Path) -> Result<()> {
        let mut writer = AtomicWriter::new(path)?;
        if self.encoding.bom {
            writer.write_all(&[0xEF, 0xBB, 0xBF])?;
        }
        writer.write_all(&encode(&self.source, self.encoding.encoding)?)?;
        writer.commit()
    }

    pub fn preview_html(&self, revision: u64) -> String {
        let instrumented = instrument_and_sanitize(&self.source, &self.nodes);
        preview_shell("HTML preview", &instrumented, revision, "[]", false)
    }
}

fn visible_text_nodes(source: &str) -> Vec<HtmlTextNode> {
    let bytes = source.as_bytes();
    let mut nodes = Vec::new();
    let mut index = 0usize;
    let mut hidden: Vec<String> = Vec::new();
    while index < bytes.len() {
        if bytes[index] == b'<' {
            if source[index..].starts_with("<!--") {
                index = source[index + 4..]
                    .find("-->")
                    .map(|p| index + 4 + p + 3)
                    .unwrap_or(bytes.len());
                continue;
            }
            let end = find_tag_end(bytes, index).unwrap_or(bytes.len() - 1);
            let tag = &source[index + 1..end];
            let (closing, name) = tag_identity(tag);
            if matches!(
                name.as_str(),
                "script" | "style" | "textarea" | "title" | "iframe" | "object" | "embed"
            ) {
                if closing {
                    if hidden.last() == Some(&name) {
                        hidden.pop();
                    }
                } else if !tag.trim_end().ends_with('/') {
                    hidden.push(name);
                }
            }
            index = end + 1;
            continue;
        }
        let end = source[index..]
            .find('<')
            .map(|p| index + p)
            .unwrap_or(bytes.len());
        if hidden.is_empty() && !source[index..end].trim().is_empty() {
            nodes.push(HtmlTextNode {
                id: nodes.len() as u64 + 1,
                range: index..end,
            });
        }
        index = end;
    }
    nodes
}

fn instrument_and_sanitize(source: &str, nodes: &[HtmlTextNode]) -> String {
    let mut result = String::with_capacity(source.len() + nodes.len() * 64);
    let mut cursor = 0usize;
    let mut node_index = 0usize;
    let bytes = source.as_bytes();
    while cursor < source.len() {
        if node_index < nodes.len() && cursor == nodes[node_index].range.start {
            let node = &nodes[node_index];
            result.push_str(&format!("<span data-qp-edit data-node-id=\"{}\">", node.id));
            result.push_str(&source[node.range.clone()]);
            result.push_str("</span>");
            cursor = node.range.end;
            node_index += 1;
            continue;
        }
        if bytes[cursor] == b'<' {
            if source[cursor..].starts_with("<!--") {
                let end = source[cursor + 4..]
                    .find("-->")
                    .map(|p| cursor + 4 + p + 3)
                    .unwrap_or(source.len());
                result.push_str(&source[cursor..end]);
                cursor = end;
                continue;
            }
            let end = find_tag_end(bytes, cursor).unwrap_or(source.len() - 1);
            let raw = &source[cursor..=end];
            let (_, name) = tag_identity(&source[cursor + 1..end]);
            if matches!(name.as_str(), "script" | "iframe" | "object" | "embed") {
                let closing_tag = format!("</{name}>");
                if let Some(close) = find_ascii_case_insensitive(&source[end + 1..], &closing_tag) {
                    cursor = end + 1 + close + closing_tag.len();
                } else {
                    cursor = source.len();
                }
                continue;
            }
            result.push_str(&strip_event_attributes(raw));
            cursor = end + 1;
        } else {
            let next = source[cursor..]
                .find('<')
                .map(|p| cursor + p)
                .unwrap_or(source.len());
            result.push_str(&source[cursor..next]);
            cursor = next;
        }
    }
    result
}

pub(crate) fn sanitize_preview_fragment(source: &str) -> String {
    instrument_and_sanitize(source, &[])
}

fn find_tag_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, &byte) in bytes[start + 1..].iter().enumerate() {
        if let Some(q) = quote {
            if byte == q {
                quote = None;
            }
        } else if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        } else if byte == b'>' {
            return Some(start + 1 + offset);
        }
    }
    None
}

fn tag_identity(tag: &str) -> (bool, String) {
    let trimmed = tag.trim_start();
    let closing = trimmed.starts_with('/');
    let name = trimmed
        .trim_start_matches('/')
        .split(|c: char| c.is_ascii_whitespace() || matches!(c, '/' | '>'))
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    (closing, name)
}

fn strip_event_attributes(tag: &str) -> String {
    let mut output = String::with_capacity(tag.len());
    let bytes = tag.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            let whitespace = i;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let name_start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'-' | b':' | b'_'))
            {
                i += 1;
            }
            let name = tag[name_start..i].to_ascii_lowercase();
            if name.starts_with("on") {
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                if bytes.get(i) == Some(&b'=') {
                    i += 1;
                    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    if let Some(&q @ (b'\'' | b'"')) = bytes.get(i) {
                        i += 1;
                        while i < bytes.len() && bytes[i] != q {
                            i += 1;
                        }
                        i = (i + 1).min(bytes.len());
                    } else {
                        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>'
                        {
                            i += 1;
                        }
                    }
                }
                continue;
            }
            output.push_str(&tag[whitespace..i]);
        } else {
            output.push(bytes[i] as char);
            i += 1;
        }
    }
    output
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn escape_html_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::TextEncoding;

    #[test]
    fn only_visible_text_is_editable_and_active_content_is_removed() {
        let source = "<div onclick=\"bad()\">Hello <b>world</b></div><script>alert(1)</script>";
        let nodes = visible_text_nodes(source);
        assert_eq!(nodes.len(), 2);
        let rendered = instrument_and_sanitize(source, &nodes);
        assert!(!rendered.contains("onclick"));
        assert!(!rendered.contains("alert(1)"));
        assert_eq!(rendered.matches("data-qp-edit").count(), 2);
    }

    #[test]
    fn removes_embedded_document_contents() {
        let source = "before<iframe>untrusted</iframe>after";
        let rendered = instrument_and_sanitize(source, &visible_text_nodes(source));
        assert!(!rendered.contains("untrusted"));
        assert!(rendered.contains("before"));
        assert!(rendered.contains("after"));
    }

    #[test]
    fn edit_escapes_markup() {
        let source = "<p>Hello</p>".to_string();
        let mut doc = HtmlDocument {
            nodes: visible_text_nodes(&source),
            source,
            encoding: EncodingInfo {
                encoding: TextEncoding::Utf8,
                bom: false,
            },
        };
        doc.edit_text(1, "a < b & c").unwrap();
        assert_eq!(doc.source(), "<p>a &lt; b &amp; c</p>");
    }
}
