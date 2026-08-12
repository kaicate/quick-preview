use std::{fs, io::Write, ops::Range, path::Path};

use pulldown_cmark::{html, Event, Options, Parser};

use crate::{
    atomic_save::AtomicWriter,
    encoding::{decode, detect, encode, payload, EncodingInfo},
    html::sanitize_preview_fragment,
    PreviewError, Result,
};

#[derive(Debug, Clone)]
pub struct MarkdownBlock {
    pub id: u64,
    pub range: Range<usize>,
}

pub struct MarkdownDocument {
    source: String,
    pub encoding: EncodingInfo,
    pub blocks: Vec<MarkdownBlock>,
}

impl MarkdownDocument {
    pub fn open(path: &Path, _encoding_hint: EncodingInfo) -> Result<Self> {
        let bytes = fs::read(path)?;
        let encoding = detect(&bytes);
        let source = decode(payload(&bytes, encoding), encoding.encoding);
        let blocks = source_blocks(&source);
        Ok(Self {
            source,
            encoding,
            blocks,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn edit_block(&mut self, id: u64, replacement: &str) -> Result<String> {
        let range = self
            .blocks
            .iter()
            .find(|block| block.id == id)
            .map(|block| block.range.clone())
            .ok_or_else(|| PreviewError::InvalidEditTarget(format!("Markdown block {id}")))?;
        let before = self.source[range.clone()].to_owned();
        self.source.replace_range(range, replacement);
        self.blocks = source_blocks(&self.source);
        Ok(before)
    }

    pub fn replace_at(&mut self, start: usize, expected: &str, replacement: &str) -> Result<()> {
        let end = start
            .checked_add(expected.len())
            .ok_or_else(|| PreviewError::InvalidEditTarget("source range overflow".into()))?;
        if self.source.get(start..end) != Some(expected) {
            return Err(PreviewError::InvalidEditTarget(
                "Markdown source changed since edit".into(),
            ));
        }
        self.source.replace_range(start..end, replacement);
        self.blocks = source_blocks(&self.source);
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
        let mut body = String::new();
        let mut sources = Vec::with_capacity(self.blocks.len());
        for block in &self.blocks {
            let source = &self.source[block.range.clone()];
            let mut rendered = String::new();
            html::push_html(&mut rendered, Parser::new_ext(source, markdown_options()));
            body.push_str(&format!(
                "<section class=\"qp-block\" data-node-id=\"{}\">{}</section>",
                block.id, rendered
            ));
            sources.push((block.id, source));
        }
        let sources_json = serde_json::to_string(&sources)
            .unwrap_or_else(|_| "[]".into())
            .replace('<', "\\u003c");
        let body = sanitize_preview_fragment(&body);
        preview_shell("Markdown preview", &body, revision, &sources_json, true)
    }
}

pub fn markdown_options() -> Options {
    Options::ENABLE_GFM
        | Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
}

fn source_blocks(source: &str) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    let mut last_end = 0usize;
    for (event, range) in Parser::new_ext(source, markdown_options()).into_offset_iter() {
        match event {
            Event::Start(_) => {
                if depth == 0 {
                    start = Some(range.start);
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let from = start.take().unwrap_or(range.start);
                    blocks.push(from..range.end);
                }
            }
            _ if depth == 0 && range.end > range.start => blocks.push(range.clone()),
            _ => {}
        }
        last_end = last_end.max(range.end);
    }
    if blocks.is_empty() && !source.is_empty() {
        blocks.push(0..source.len());
    }
    if last_end < source.len() && !source[last_end..].trim().is_empty() {
        blocks.push(last_end..source.len());
    }
    blocks.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in blocks {
        if let Some(previous) = merged.last_mut() {
            if range.start <= previous.end {
                previous.end = previous.end.max(range.end);
                continue;
            }
        }
        merged.push(range);
    }
    merged
        .into_iter()
        .enumerate()
        .map(|(index, range)| MarkdownBlock {
            id: index as u64 + 1,
            range,
        })
        .collect()
}

pub(crate) fn preview_shell(
    title: &str,
    body: &str,
    revision: u64,
    sources_json: &str,
    markdown: bool,
) -> String {
    let mode = if markdown { "markdown" } else { "html" };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'">
<title>{title}</title><style>
:root{{color-scheme:light dark;font:15px/1.55 system-ui,sans-serif}}body{{max-width:980px;margin:0 auto;padding:24px}}img{{max-width:100%}}
.qp-block{{border:1px solid transparent;border-radius:5px;padding:2px 7px;margin:1px -8px}}.qp-block:hover{{border-color:#6aa9ff}}
textarea.qp-editor{{box-sizing:border-box;width:100%;min-height:7em;resize:vertical;font:14px/1.45 Consolas,monospace}}
table{{border-collapse:collapse}}th,td{{border:1px solid #888;padding:.3em .6em}}pre{{overflow:auto;padding:12px;background:#8882}}
.math{{font-family:'Cambria Math',serif}}a{{color:#2785d8}}</style></head><body data-mode="{mode}">{body}<script>
(()=>{{'use strict';const revision={revision};const sources=new Map({sources_json});
const send=(id,text)=>chrome.webview.postMessage({{type:'edit',documentRevision:revision,nodeId:Number(id),text}});
document.addEventListener('click',e=>{{const link=e.target.closest('a');if(link){{if(e.ctrlKey){{e.preventDefault();chrome.webview.postMessage({{type:'openLink',documentRevision:revision,nodeId:0,text:link.href}})}}else e.preventDefault();return}}
if(document.body.dataset.mode==='markdown'){{const block=e.target.closest('.qp-block');if(!block||block.querySelector('textarea'))return;const id=Number(block.dataset.nodeId);const area=document.createElement('textarea');let committed=false;area.className='qp-editor';area.value=sources.get(id)||'';block.replaceChildren(area);area.focus();area.addEventListener('keydown',x=>{{if(x.ctrlKey&&x.key==='Enter'){{x.preventDefault();committed=true;send(id,area.value)}}}});area.addEventListener('blur',()=>{{if(!committed)send(id,area.value)}},{{once:true}})}}else{{const node=e.target.closest('[data-qp-edit]');if(!node)return;node.contentEditable='true';node.focus()}}}});
document.addEventListener('focusout',e=>{{const node=e.target.closest?.('[data-qp-edit]');if(node&&node.contentEditable==='true'){{node.contentEditable='false';send(node.dataset.nodeId,node.textContent)}}}});
document.addEventListener('keydown',e=>{{const node=e.target.closest?.('[data-qp-edit]');if(node&&e.ctrlKey&&e.key==='Enter'){{e.preventDefault();node.blur()}}}});
}})();</script></body></html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::TextEncoding;

    #[test]
    fn finds_and_edits_top_level_blocks() {
        let mut doc = MarkdownDocument {
            source: "# A\n\nText with $x$.\n".into(),
            encoding: EncodingInfo {
                encoding: TextEncoding::Utf8,
                bom: false,
            },
            blocks: vec![],
        };
        doc.blocks = source_blocks(&doc.source);
        assert_eq!(doc.blocks.len(), 2);
        let id = doc.blocks[1].id;
        doc.edit_block(id, "Changed\n").unwrap();
        assert!(doc.source.contains("Changed"));
        assert!(doc.preview_html(1).contains("data-node-id"));
    }

    #[test]
    fn raw_markdown_html_cannot_execute_script() {
        let source = "Text\n\n<script>alert(1)</script>".to_string();
        let doc = MarkdownDocument {
            blocks: source_blocks(&source),
            source,
            encoding: EncodingInfo {
                encoding: TextEncoding::Utf8,
                bom: false,
            },
        };
        let preview = doc.preview_html(0);
        assert!(!preview.contains("<script>alert(1)</script>"));
    }
}
