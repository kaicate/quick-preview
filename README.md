# QuickPreview

QuickPreview is a Windows 11 x64 desktop preview/editor for CSV, TSV, Markdown,
and HTML. The native shell and delimited grid are deliberately independent of
the browser runtime. WebView2 is created only for Markdown and HTML documents.

## Current v1 behavior

- CSV/TSV: memory-mapped indexing, quoted records, viewport-only decoding,
  sparse cell edits, and streaming atomic save.
- Markdown: CommonMark + GFM + math parsing, source-block editing, and a
  self-contained preview document.
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

The MSIX packaging script expects `makeappx.exe` and `signtool.exe` from the
Windows SDK:

```powershell
./packaging/build-msix.ps1 `
  -Publisher 'CN=Your Publisher' `
  -CertificatePath cert.pfx `
  -AppInstallerUri 'https://github.com/kaicate/quick-preview/releases/latest/download/QuickPreview.appinstaller' `
  -MsixUri 'https://github.com/kaicate/quick-preview/releases/download/v0.1.0/QuickPreview-0.1.0.0-x64.msix'
```

The package never contains a signing private key. For development, pass
`-SkipSign` and sign the resulting package separately before installation.

## Releases

Version tags in the form `vMAJOR.MINOR.PATCH` trigger
`.github/workflows/release.yml`. The workflow verifies that the tag matches the
version in `Cargo.toml`, tests the Windows build, creates and signs the MSIX,
and publishes these GitHub Release assets:

- `QuickPreview-MAJOR.MINOR.PATCH.0-x64.msix`
- `QuickPreview.appinstaller`
- `QuickPreview-checksums.txt`

The stable installation URL is:

```text
https://github.com/kaicate/quick-preview/releases/latest/download/QuickPreview.appinstaller
```

Create a `release` environment in the GitHub repository with this configuration:

- Variable `WINDOWS_PUBLISHER`: the exact Subject of the code-signing
  certificate, such as `CN=Your Publisher`
- Secret `WINDOWS_CERTIFICATE_BASE64`: the PFX file encoded as base64
- Secret `WINDOWS_CERTIFICATE_PASSWORD`: the PFX password

With GitHub CLI installed and authenticated, the repository setup can be done
from PowerShell without writing an encoded certificate to disk:

```powershell
./packaging/configure-github-release.ps1 `
  -CertificatePath cert.pfx `
  -CertificatePassword (Read-Host 'PFX password' -AsSecureString) `
  -Repository kaicate/quick-preview
```

Do not commit the PFX, certificate password, generated MSIX, or generated App
Installer file. Production packages must use a code-signing certificate trusted
by the target Windows devices. Once the environment is configured and the
`Cargo.toml` version is ready, publish a release with:

```powershell
git tag v0.1.0
git push origin v0.1.0
```

## Keyboard shortcuts

- `Ctrl+O`: open
- `Ctrl+S`: save
- `Ctrl+Z` / `Ctrl+Y`: undo / redo
- Arrow keys / mouse wheel: navigate a grid
- `F2` or double click: edit a grid cell
- Markdown/HTML: click an editable block or text node; `Ctrl+Enter` commits

## Performance model

Delimited files are not copied into a matrix. Only record byte offsets, sparse
edits, and a small decoded viewport cache are retained. The preview limit for
HTML and Markdown is 20 MiB; the delimited editing limit is 100 MiB and a
single record may not exceed 16 MiB.
