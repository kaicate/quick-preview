# QuickPreview

QuickPreview is a Windows 11 x64 desktop preview/editor for CSV, TSV, Markdown,
and HTML. The native shell and delimited grid are deliberately independent of
the browser runtime. WebView2 is created only for Markdown and HTML documents.

## Current v1 behavior

- CSV/TSV: memory mapping, viewport-adjacent row indexing, 32-column field
  chunks, quoted records, sparse cell edits, and streaming atomic save.
- Markdown: CommonMark + GFM + MathML rendering (`$...$`, `$$...$$`, and
  ChatGPT-style `\\[...\\]`), source-block editing, and a self-contained
  preview document.
- HTML: source-preserving editable text nodes, active-content removal, and a
  restrictive preview security policy.
- UTF-8, UTF-8 BOM, and Windows Shift_JIS/CP932-compatible input and output.
- bounded undo/redo and external-file-change detection.
- Win32/Direct2D shell and a WebView2 host boundary on Windows.

## Build

Install Rust stable with the MSVC toolchain, Visual Studio Build Tools with the
Windows 11 SDK, and the Evergreen WebView2 Runtime.

```powershell
rustup default stable-x86_64-pc-windows-msvc
cargo test
cargo build --release
```

The application artwork is stored in `assets/QuickPreview.png`, and the Windows
executable embeds `assets/QuickPreview.ico` plus dedicated icons for each
registered file type.

## Keyboard shortcuts

- `Ctrl+O`: open
- `Ctrl+S`: save
- `Ctrl+Z` / `Ctrl+Y`: undo / redo
- Arrow keys / mouse wheel: navigate a grid
- Horizontal scrollbar, horizontal wheel, or `Shift` + mouse wheel: scroll columns
- `F2` or double click: edit a grid cell
- Markdown/HTML: click an editable block or text node; `Ctrl+Enter` commits
- `Esc`: discard the active cell or block edit and clear its selection

## Performance model

Delimited files are not copied into a matrix. Only checkpoints, record offsets
around the current viewport, sparse edits, and a small decoded viewport cache
are retained. The preview limit for
HTML and Markdown is 20 MiB; the delimited editing limit is 100 MiB and a
single record may not exceed 16 MiB. Files above the normal preview limits can
still be opened after an explicit warning.
