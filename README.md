# OfficeCLI (Rust)

> **A pure-Rust CLI for AI agents to create, read, modify, and render Office documents and PDFs.**

**Give any AI agent structured control over Word, Excel, PowerPoint, and PDF — in one line of code.**

Open-source. Single binary. No Office installation. No runtime dependency. Works on macOS, Linux, and Windows.

[![GitHub Release](https://img.shields.io/github/v/release/RainLib/OfficeCli-rust)](https://github.com/RainLib/OfficeCli-rust/releases)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/language-Rust-orange.svg)](https://www.rust-lang.org/)

**English** | [中文](README_zh.md) | [日本語](README_ja.md) | [한국어](README_ko.md)

## About This Repository

This is **[RainLib/OfficeCli-rust](https://github.com/RainLib/OfficeCli-rust)** — a **Rust rewrite** of [OfficeCLI](https://github.com/iOfficeAI/OfficeCLI), the open-source Office automation CLI originally built in C#/.NET by [iOfficeAI](https://github.com/iOfficeAI).

| | **This repo (Rust)** | **[Upstream (C#)](https://github.com/iOfficeAI/OfficeCLI)** |
|---|---|---|
| Repository | [RainLib/OfficeCli-rust](https://github.com/RainLib/OfficeCli-rust) | [iOfficeAI/OfficeCLI](https://github.com/iOfficeAI/OfficeCLI) |
| Language | Pure Rust | C# / .NET (self-contained binary) |
| Version | v0.1.x (command parity) | v1.0.x (mature, 6k+ stars) |
| Runtime | None — native binary | .NET embedded in binary |
| PDF support | ✅ read / modify / preview | Via plugins |
| Goal | Lightweight, auditable, embeddable Rust core | Full-featured production CLI + ecosystem |

The Rust edition shares the same **CLI philosophy** — path-based DOM operations, JSON output, TextOffsetMap, three-layer architecture, MCP server, and live HTML preview — and has reached **command-level parity** with the C# upstream. Use upstream for maximum ecosystem integration (AionUi, plugins marketplace); use this repo when you need a **dependency-free Rust binary** or want to contribute to the Rust implementation.

## Supported Formats

| Format             | Read | Modify                         | Create | Text/Offset Mapping | Convert Legacy  |
| ------------------ | ---- | ------------------------------ | ------ | ------------------- | --------------- |
| Word (.docx)       | ✅   | ✅                             | ✅     | ✅                  | ✅ .doc → .docx |
| Excel (.xlsx)      | ✅   | ✅                             | ✅     | ✅                  | ✅ .xls → .xlsx |
| PowerPoint (.pptx) | ✅   | ✅                             | ✅     | ✅                  | ✅ .ppt → .pptx |
| PDF (.pdf)         | ✅   | ✅ (text replace, page delete) | ✅     | ✅                  | —               |

## For AI Agents — Text/Offset → Path Mapping

Every supported format can emit a **TextOffsetMap** — full text plus a character-offset→path mapping. An agent reads the map, finds the text to change, gets the exact document path (e.g. `/body/p[3]/r[1]`), and calls `set` precisely. No regex guessing.

```bash
officecli extract-text report.docx --with-offsets --json
```

```json
{
  "full_text": "Hello World\nSecond paragraph",
  "spans": [
    { "start": 0, "end": 5, "path": "/body/p[1]/r[1]", "text": "Hello", "element_type": "run" },
    { "start": 6, "end": 11, "path": "/body/p[1]/r[2]", "text": "World", "element_type": "run" },
    { "start": 12, "end": 28, "path": "/body/p[2]/r[1]", "text": "Second paragraph", "element_type": "run" }
  ],
  "meta": { "format": "docx", "total_chars": 28, "total_spans": 3 }
}
```

**Agent setup** — feed the skill file to your coding agent:

```bash
curl -fsSL https://raw.githubusercontent.com/RainLib/OfficeCli-rust/main/SKILL.md
```

Or install the binary + skill in one step (see [Installation](#installation)).

## Quick Start

```bash
# 1. Install (macOS / Linux)
curl -fsSL https://raw.githubusercontent.com/RainLib/OfficeCli-rust/main/install.sh | bash
# Windows (PowerShell):
#   irm https://raw.githubusercontent.com/RainLib/OfficeCli-rust/main/install.ps1 | iex

# 2. Create a blank PowerPoint
officecli create deck.pptx

# 3. Add a slide
officecli add deck.pptx / --type slide --prop title="Hello, World!"

# 4. Preview as HTML
officecli view deck.pptx --mode html

# 5. Live preview — auto-refresh on every edit
officecli watch deck.pptx
```

In another terminal, every `add` / `set` / `remove` refreshes the browser at `http://localhost:26315`.

## Why OfficeCLI?

What used to take 50 lines of Python and three separate libraries:

```python
from pptx import Presentation
prs = Presentation()
slide = prs.slides.add_slide(prs.slide_layouts[0])
slide.shapes.title.text = "Q4 Report"
# ... dozens more lines ...
prs.save("deck.pptx")
```

Becomes one command:

```bash
officecli add deck.pptx / --type slide --prop title="Q4 Report"
```

**Core capabilities in this Rust build:**

- **Create** blank documents or add structured content
- **Read** text, outline, stats, and annotated views — plain text or `--json`
- **Modify** elements via path-based `set` / `add` / `remove` / `move`
- **Validate** document structure and surface issues
- **Extract** text with offset→path mapping for agent positioning
- **Render** documents to HTML/SVG for visual feedback
- **Convert** legacy `.doc` / `.xls` / `.ppt` to modern formats
- **PDF** — read, preview, replace text, delete pages
- **Batch** — run multiple operations in one open/save cycle
- **MCP** — expose all operations as AI tools over JSON-RPC

## Installation

Ships as a single native binary. Pure Rust — no .NET, no Python, no Office.

**One-line install:**

```bash
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/RainLib/OfficeCli-rust/main/install.sh | bash

# Windows (PowerShell)
irm https://raw.githubusercontent.com/RainLib/OfficeCli-rust/main/install.ps1 | iex
```

Pin a specific release:

```bash
OFFICECLI_VERSION=v0.1.2 curl -fsSL https://raw.githubusercontent.com/RainLib/OfficeCli-rust/main/install.sh | bash
```

**Manual download** from [GitHub Releases](https://github.com/RainLib/OfficeCli-rust/releases):

| Platform            | Binary                       |
| ------------------- | ---------------------------- |
| macOS Apple Silicon | `officecli-mac-arm64`        |
| macOS Intel         | `officecli-mac-x64`          |
| Linux x64           | `officecli-linux-x64`        |
| Linux ARM64         | `officecli-linux-arm64`      |
| Linux Alpine x64    | `officecli-linux-alpine-x64` |
| Windows x64         | `officecli-win-x64.exe`      |
| Windows ARM64       | `officecli-win-arm64.exe`    |

```bash
# Download script — current platform, latest published release
./scripts/download.sh

# Specific version, all platforms
./scripts/download.sh v0.1.2 all

# GitHub CLI
gh release download v0.1.2 --repo RainLib/OfficeCli-rust --pattern 'officecli-*'
```

> **Release note:** CI builds binaries on every `v*` tag push and uploads them to a **draft** GitHub Release. Publish the draft on the [Releases](https://github.com/RainLib/OfficeCli-rust/releases) page before `latest` download URLs work. Push tags to the `github` remote (`git push github v0.1.2`), not only the internal `origin` remote.

Verify: `officecli --version`

## Key Features

### Three-Layer Architecture

Start simple, go deep only when needed.

| Layer           | Purpose                                  | Commands                                                         |
| --------------- | ---------------------------------------- | ---------------------------------------------------------------- |
| **L1: Read**    | Semantic views of content                | `view` (text, annotated, outline, stats, issues, html, svg, screenshot, pdf, forms) |
| **L2: DOM**     | Structured element operations            | `get`, `query`, `set`, `add`, `add-part`, `remove`, `move`, `swap`  |
| **L3: Raw**     | Direct XML/XPath access — universal fallback | `raw`, `raw-set`, `validate`                                 |

```bash
# L1 — high-level views
officecli view report.docx --mode annotated
officecli view report.docx --mode forms      # list form fields (SDT)
officecli view budget.xlsx --mode stats
officecli view report.pdf --mode text
officecli view report.pdf --mode pdf        # export as PDF via headless browser

# L2 — element-level operations
officecli query report.docx paragraph
officecli add budget.xlsx / --type sheet --prop name="Q2 Report"
officecli remove report.pptx '/slide[3]'

# L3 — raw XML when L2 isn't enough
officecli raw deck.pptx 'ppt/slides/slide1.xml'
officecli raw-set report.docx document --xpath "//w:p[1]" --action append \
  --xml '<w:r><w:t>Injected</w:t></w:r>'
```

### Live Preview & Rendering

Built-in HTML/SVG rendering closes the **render → look → fix** loop without Office installed:

```bash
officecli view deck.pptx --mode html     # standalone HTML preview
officecli view deck.pptx --mode svg      # SVG output
officecli watch deck.pptx                # live server at :26315
```

### Format Conversion

Multiple engines for legacy format conversion, PDF to DOCX, and standalone semantic HTML import:

```bash
officecli convert old.doc              # .doc -> .docx (LibreOffice, default)
officecli convert old.xls -o new.xlsx  # .xls -> .xlsx
officecli convert old.ppt --engine oxide  # pure-Rust engine, no external deps
officecli convert input.pdf --engine pdf2docx  # PDF -> DOCX via Python pdf2docx
officecli convert input.html -o report.docx    # headings/paragraphs/lists/tables -> Word
officecli convert input.html -o report.xlsx    # tables/content -> worksheet cells
officecli convert input.html -o report.pptx    # sections/H1 headings -> slides
officecli convert input.html -o report.pdf     # semantic paginated PDF
```

HTML conversion is always performed in-process by OfficeCLI's Rust handlers and reports
`engine=rust-html-semantic` with `fidelity=semantic`. It never invokes LibreOffice, WPS,
`pdf2docx`, or a browser renderer, even when one of those engines is supplied on the command line.
It does not require an original Office file. CSS layout, scripts, active content, and exact browser
pagination are not carried into the output. Standalone HTML never fetches local or remote image
URLs and represents them as safe alt text. Source-free HCD export can instead embed validated,
content-addressed raster assets natively in DOCX/XLSX/PPTX and as PNG/JPEG PDF image XObjects;
bounded source dimensions are retained and direct PPTX picture coordinates are reused when present.
Cropping, wrapping and page collision still remain semantic rather than source-layout exact. Input
is bounded to 64 MiB and each semantic text block to 2 MiB.

HTML/HCD tables are emitted as native Word/Excel structures and editable DrawingML tables in PPTX.
PPTX tables wider than 12 columns or taller than 18 rows are split into bounded table slides with
the first row repeated. Logical PPTX tables split across HCD chunks are validated and reassembled by
stable table ID and contiguous row ranges before this layout step; PDF currently keeps the table as
aligned semantic row text.

For incremental editing, `officecli hdoc import` accepts `.docx`, `.xlsx`, `.pptx`, `.pdf`,
`.html`/`.htm`, and UTF-8 `.txt`. It emits immutable canonical HTML chunks and source maps;
HTML headings, paragraphs, lists, and tables remain safe canonical structures, while each editable
text span retains its exact UTF-8 source byte range. Large HTML tables use stable IDs and contiguous
128-row fragments for frontend virtualization and source-free native Office table reconstruction.
When `--document-id` is omitted, an immutable source SHA-256-derived ID makes byte-identical imports
repeat the same document and node IDs; production systems should pass their persistent business ID
to preserve lineage when source bytes change. `hdoc get-node <bundle> <nodeId>` resolves one current
node through chunk bloom filters and source maps without building a full-text buffer. `hdoc apply`
then appends nodeId-addressed text/annotation revisions. With `--source`, `hdoc export` writes the selected
revision back against the SHA-256-verified immutable source; without a source (or with a different
target), it can rebuild DOCX/XLSX/PPTX/PDF at explicitly reported `SEMANTIC` fidelity through the
same in-process Rust HTML handlers. HTML and TXT source-backed export rewrites only mapped text byte
ranges, preserving all other source bytes. These HCD commands are implemented entirely in Rust and
do not invoke an installed office suite. The PPTX adapter materializes direct text/picture
geometry and DrawingML table grids, merges, direct cell formatting, and editable cell text while
preserving the original presentation as the source-backed export authority. Large DrawingML tables
stream as bounded row-group fragments (at most 128 rows per normal fragment); each fragment repeats
its frame geometry and column grid, and row-spanning merge groups are never split.
PDF HCD import uses a pinned pure-Rust Hayro renderer to composite each CropBox page into a
content-addressed 96-DPI PNG visual authority layer. Extractable nodeId text remains in a separate
transparent HTML interaction layer, and pages after the first are lazy-loaded. This preserves the
source page layout far better than XObject/image reconstruction, but `VISUAL` does not mean that two
different PDF engines produce byte- or pixel-identical color management and antialiasing. Edited PDF
text requires dirty-page recomposition to retain the same visual guarantee. The current lopdf/Hayro
backend also does not yet meet the 512 MiB RSS target on high-resolution JPEG2000-heavy files.

The engine table below applies to non-HTML conversion paths:

| Engine                  | Fidelity                                | Speed                  | Dependency           |
| ----------------------- | --------------------------------------- | ---------------------- | -------------------- |
| `libreoffice` (default) | ~1:1                                    | Slower (process spawn) | LibreOffice (~700MB) |
| `oxide`                 | Lower (may lose styles/headers/objects) | Fast (sub-second)      | None (pure Rust)     |
| `pdf-text`              | Text only                               | Fastest                | None (pure Rust)     |
| `pdf2docx`              | Layout parser, PDF-only                 | Usually faster than LO | Python `pdf2docx` CLI |

### Resident Mode & Batch

For multi-step workflows, resident mode (Unix) keeps the document in memory. Batch runs multiple operations in one cycle.

```bash
# Resident mode — near-zero latency via Unix Domain Socket
officecli open report.docx
officecli set report.docx /body/p[1]/r[1] --prop text="Updated"
officecli save report.docx
officecli close report.docx

# Batch mode — atomic multi-command execution
echo '[{"command":"set","path":"/slide[1]/shape[1]","props":{"text":"Hello"}}]' \
  | officecli batch deck.pptx --stdin --json
officecli batch deck.pptx --commands-file batch.json --json
```

### PDF Support

```bash
officecli view report.pdf --mode text
officecli get report.pdf '/page[1]'
officecli extract-text report.pdf --with-offsets --json
officecli set report.pdf '/page[1]' --prop text="New content"
officecli remove report.pdf '/page[3]'
officecli save report.pdf
```

## AI Integration

### MCP Server

```bash
officecli mcp    # Start MCP stdio server (JSON-RPC 2.0)
```

Exposes document operations as tools — no shell access required.

### Built-in Help

```bash
officecli --help
officecli help docx paragraph
officecli help xlsx cell --json
```

When unsure about property names, use `officecli help <format> <element>` — it reflects the installed binary version.

## Comparison

### vs. Traditional Tools

|                                 | OfficeCLI (Rust) | [OfficeCLI (C#)](https://github.com/iOfficeAI/OfficeCLI) | Microsoft Office | python-docx / openpyxl |
| ------------------------------- | ---------------- | -------------------------------------------------------- | ---------------- | ---------------------- |
| Open source & free              | ✅ Apache 2.0    | ✅ Apache 2.0                                            | ✗                | ✅                     |
| AI-native CLI + JSON            | ✅               | ✅                                                       | ✗                | ✗                      |
| Zero runtime (single binary)    | ✅ (Rust)        | ✅ (.NET embedded)                                       | ✗                | ✗ (Python + pip)       |
| Word + Excel + PowerPoint + PDF | ✅               | ✅ (+ plugins)                                           | ✅               | Separate libs          |
| Text/offset → path mapping      | ✅               | ✅                                                       | ✗                | ✗                      |
| Path-based element access       | ✅               | ✅                                                       | ✗                | ✗                      |
| Live HTML preview (`watch`)     | ✅               | ✅                                                       | ✗                | ✗                      |
| MCP server                      | ✅               | ✅ (+ auto-register)                                       | ✗                | ✗                      |
| Headless / CI / Docker          | ✅               | ✅                                                       | ✗                | ✅                     |

### vs. Upstream OfficeCLI (C#)

This Rust port is **API-compatible** (same command names, path syntax, `--prop` conventions) and has reached **command-level parity** with the C# upstream. Remaining gaps are in edge-case fidelity and ecosystem tooling, not in command coverage.

| Feature | Upstream (C#) | This repo (Rust) |
| ------- | ------------- | ---------------- |
| Template `merge` (`{{key}}`) | ✅ | ✅ |
| `view screenshot` (PNG) | ✅ | ✅ (headless Chrome/Edge/Firefox) |
| `view pdf` (PDF export) | ✅ | ✅ (headless Chromium `--print-to-pdf`) |
| `view forms` (SDT form fields) | ✅ | ✅ (docx SDT parsing) |
| `swap`, `refresh`, `plugins` | ✅ | ✅ |
| `add-part` (chart/header/footer) | ✅ | ✅ |
| `import` (CSV/TSV → xlsx) | ✅ | ✅ |
| `mark/unmark/marks/goto` (watch) | ✅ | ✅ (watch server routes) |
| `officecli install` self-setup | ✅ | ✅ (binary + skills + MCP) |
| Formula engine (150+ functions) | ✅ | ✅ (80+ functions) |
| Pivot tables (listing) | ✅ | ✅ (listing + source range) |
| Morph transitions (reporting) | ✅ | ✅ (detection + candidate count) |
| 3D models | ✅ | ✅ (HTML preview) |
| Python SDK (`officecli-sdk`) | ✅ | ✅ (Unix domain socket IPC) |
| CLI smoke & integration tests | ✅ | ✅ (39 CLI + 32 unit tests) |
| `cargo clippy -D warnings` clean | N/A | ✅ |
| AionUi GUI integration | ✅ | N/A (upstream ecosystem) |
| Wiki & 4000+ commits of polish | ✅ | Early stage |

Track upstream for the full command reference and wiki: [iOfficeAI/OfficeCLI Wiki](https://github.com/iOfficeAI/OfficeCLI/wiki).

## Command Reference

| Command        | Description                                                                   |
| -------------- | ----------------------------------------------------------------------------- |
| `create`       | Create a blank `.docx`, `.xlsx`, `.pptx`, or `.pdf`                           |
| `view`         | View content (`text`, `annotated`, `outline`, `stats`, `issues`, `html`, `svg`, `screenshot`, `pdf`, `forms`) |
| `get`          | Get element and children (`--depth N`, `--json`)                                |
| `query`        | CSS-like element query                                                        |
| `set`          | Modify element properties                                                     |
| `add`          | Add element                                                                   |
| `add-part`     | Create a new document part (chart, header, footer) and return its rel ID      |
| `remove`       | Remove an element                                                             |
| `move`         | Move element                                                                  |
| `swap`         | Swap two elements (paragraphs, slides, cells)                                  |
| `save`         | Save changes back to file                                                     |
| `validate`     | Validate document structure                                                   |
| `extract-text` | Extract text with offset→path mapping (`--with-offsets`, `--json`)            |
| `convert`      | Convert legacy formats and PDF (`--engine libreoffice\|oxide\|pdf-text\|pdf2docx`) |
| `batch`        | Multiple operations in one cycle                                              |
| `dump`         | Serialize document structure to replayable JSON                               |
| `raw`          | View raw XML of a document part                                               |
| `raw-set`      | Modify raw XML via XPath (`setattr`, `remove`)                                |
| `import`       | Import CSV/TSV data into an Excel sheet                                       |
| `merge`        | Merge template placeholders (`{{key}}`) with JSON data                        |
| `refresh`      | Refresh derived fields (TOC, cross-references)                                 |
| `watch`        | Live HTML preview with auto-refresh                                           |
| `unwatch`      | Stop a running watch server                                                   |
| `open`         | Start resident mode (Unix)                                                    |
| `close`        | Save and close resident mode                                                   |
| `plugins`      | List, inspect, and lint installed plugins (`list`, `info`, `lint`)              |
| `install`      | Install binary, skills, and MCP configuration (`--dry-run`, `--prefix`)        |
| `info`         | Show info about the tool or document topics                                   |
| `mcp`          | Start MCP server for AI tool integration                                      |

Global flag: `--json` on any command for structured output.

## Use Cases

**Developers**
- Automate report generation in CI/CD pipelines
- Headless document processing in Docker (Alpine musl build available)
- Embed a small Rust binary without .NET or Python runtimes

**AI Agents**
- Precise text edits via TextOffsetMap → path → `set`
- Visual feedback loop with `watch` and `view html`
- Tool integration via MCP server

**Teams**
- Internal document automation with auditable open-source Rust code
- Gradual migration path from upstream OfficeCLI with compatible CLI syntax

## Build from Source

Requires [Rust](https://rustup.rs/) 1.75+ (CI pins 1.90.0).

```bash
git clone https://github.com/RainLib/OfficeCli-rust.git
cd OfficeCli-rust
cargo build --release
# Binary at target/release/officecli
```

Cross-compile:

```bash
cargo build --release --target aarch64-apple-darwin    # macOS ARM
cargo build --release --target x86_64-unknown-linux-gnu
cargo build --release --target x86_64-pc-windows-msvc
```

Local distribution:

```bash
make dist          # build + copy to dist/ with SHA256
make download VERSION=v0.1.2 PLATFORM=all  # fetch release binaries
make smoke         # quick sanity check
```

## Project Structure

```
OfficeCli-rust/
├── Cargo.toml                 # Workspace root (v0.1.x)
├── install.sh / install.ps1   # One-line installers
├── scripts/download.sh        # Platform binary downloader
├── SKILL.md                   # AI agent skill file
├── crates/
│   ├── officecli/              # CLI entry + commands
│   ├── handler-common/         # DocumentHandler trait + shared types
│   ├── oxml/                   # OOXML ZIP/XML package handling
│   ├── docx-handler/           # Word handler
│   ├── xlsx-handler/           # Excel handler
│   ├── pptx-handler/           # PowerPoint handler
│   └── pdf-handler/            # PDF handler (lopdf + custom parser)
├── examples/                   # Runnable examples (.sh / .md)
└── skills/                     # Specialized agent skills
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Every PR should be atomic and include a verifiable validation method (command sequence showing before/after).

Bug reports and feature requests: [GitHub Issues](https://github.com/RainLib/OfficeCli-rust/issues)

Upstream reference implementation: [iOfficeAI/OfficeCLI](https://github.com/iOfficeAI/OfficeCLI)

## License

[Apache License 2.0](LICENSE)

---

If you find this project useful, please [star it on GitHub](https://github.com/RainLib/OfficeCli-rust) — and consider starring [upstream OfficeCLI](https://github.com/iOfficeAI/OfficeCLI) too.

[GitHub — RainLib/OfficeCli-rust](https://github.com/RainLib/OfficeCli-rust) | [Upstream — iOfficeAI/OfficeCLI](https://github.com/iOfficeAI/OfficeCLI) | [Releases](https://github.com/RainLib/OfficeCli-rust/releases)

<!-- LLM/agent discovery metadata
tool: officecli
repo: RainLib/OfficeCli-rust
upstream: iOfficeAI/OfficeCLI
type: cli
language: rust
formats: docx, xlsx, pptx, pdf
capabilities: create, read, modify, validate, batch, resident-mode, mcp-server, live-preview, text-offset-mapping, format-conversion
platforms: macos, linux, windows
license: Apache-2.0
skill-file: SKILL.md
-->
