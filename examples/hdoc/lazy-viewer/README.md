# HCD lazy viewer

This dependency-free viewer demonstrates the generic HCD random-access contract for DOCX, PPTX,
PDF, HTML, Markdown and TXT bundles. It fetches index pages progressively, downloads only chunks
near the viewport, preserves canonical `data-hcd-id` attributes, rewrites content-addressed assets,
and evicts off-screen chunks after the configurable resident limit is reached.
Mapped image nodes are also verified against their `visualHash`; historical revisions resolve their
own immutable asset index rather than borrowing the current head's images.

XLSX should use `examples/hdoc/xlsx-univer-viewer`, which maps grid windows to Univer's Canvas
renderer and supports cell editing. This generic viewer is read-only and virtualizes HCD chunks;
DOCX chunks are semantic blocks rather than exact Word pages.

From the repository root, first create a bundle under a path served by the same HTTP server:

```bash
target/debug/officecli hdoc import examples/word/numbering-showcase.docx \
  --output examples/hdoc/lazy-viewer/demo.hcd \
  --events ndjson

python3 -m http.server 4175
```

Then open:

```text
http://127.0.0.1:4175/examples/hdoc/lazy-viewer/?bundle=/examples/hdoc/lazy-viewer/demo.hcd/
```

The viewer must run over HTTP because browsers do not allow a `file://` page to fetch neighboring
JSON and HTML fragments reliably. The server is only for local static-file preview; all document
conversion remains inside OfficeCLI's Rust implementation.

Query parameters:

- `bundle`: URL prefix of an HCD directory.
- `revision`: optional immutable revision number; omitted means head.
- `cache`: maximum resident chunks, clamped to 2-64.

The viewer intentionally does not execute scripts found in chunks. Canonical HCD validation already
rejects active content, but production Java/OSS delivery must still publish only validated bundles.
