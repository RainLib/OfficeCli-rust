---
name: officecli-pdf
description: "Use this skill any time a .pdf file is involved -- as input, output, or both. This includes: reading, parsing, or extracting text from a PDF; viewing outlines, stats, or formatting issues; rendering PDF pages to HTML or SVG; applying highlights, text color changes, or modifications on specific pages; deleting pages; replacing content stream text. Trigger whenever the user mentions 'PDF', 'pdf file', or references a .pdf filename."
---

# OfficeCLI PDF Skill

## Setup

If `officecli` is missing:

- **macOS / Linux**: `curl -fsSL https://d.officecli.ai/install.sh | bash`
- **Windows (PowerShell)**: `irm https://d.officecli.ai/install.ps1 | iex`

Verify with `officecli --version` (open a new terminal if PATH hasn't picked up). If install fails, download a binary from https://github.com/iOfficeAI/OfficeCLI/releases.

## ⚠️ Help-First Rule

**This skill teaches how to interact with and modify PDF files, not every command flag. When a property name, path pattern, or action is uncertain, consult help BEFORE guessing.**

```bash
officecli help pdf                         # List all pdf elements and operations
officecli help pdf <element>               # Full element schema (e.g. page, text-block)
```

## Mental Model

**Mental model.** A PDF is a tree of page objects. Each page contains a `/Resources` dictionary (with `/Font` references) and a `/Contents` stream containing drawing operators (like `BT` for Begin Text, `ET` for End Text, `Tj`/`TJ` for text rendering, and `Tm` for text matrix transforms). 

`officecli` provides a semantic-path API over PDF files:
- Pages are addressed as `/page[N]` (1-based index).
- Text blocks within a page are addressed as `/page[N]/text[M]` (1-based index).
- Range paths can specify precise character ranges across blocks, such as `/page[1]/text[3][0:5]`.

## Shell & Execution Discipline

**Shell quoting (zsh / bash).** PDF paths contain `[]`. Both are shell metacharacters. Rules:
- ALWAYS quote element paths: `"/page[1]/text[3]"`, not `/page[1]/text[3]`.
- NEVER hand-write escape sequences unless necessary. The CLI does not interpret backslash escapes inside executable arguments.

**Incremental execution.** Run commands one at a time and read each exit code. `officecli` mutates the file on every call; a 50-command script that fails at command 3 will cascade silently. One command → check output → continue.

## Common Workflow

1. **Orient.** Run `view "$FILE" outline` or `view "$FILE" stats` first to understand page counts and layout.
2. **Retrieve.** Use `get "$FILE" "/page[N]"` or `query` to find the exact text blocks you want to inspect or modify.
3. **Edit incrementally.** Update text, apply styling, or highlight ranges.
4. **Close & Validate.** Run `officecli close "$FILE"` (if in resident mode) and `officecli validate "$FILE"` to verify PDF integrity.
5. **QA.** Run `view "$FILE" html` or `view "$FILE" svg` to visually verify your changes.

## Quick Start

```bash
FILE="document.pdf"
# Open PDF (optional resident mode)
officecli open "$FILE"

# View statistics
officecli view "$FILE" stats

# Inspect page 1 structure
officecli get "$FILE" "/page[1]" --depth 2

# Modify text in the first text block on page 1
officecli set "$FILE" "/page[1]/text[1]" --prop text="Updated Title"

# Close the file (writes to disk)
officecli close "$FILE"
```

## Reading & Analysis

Start wide, then narrow.

```bash
officecli view "$FILE" outline           # Show PDF page index/titles
officecli view "$FILE" stats             # Show file stats and page counts
officecli view "$FILE" text              # Extract plain text from the document
officecli view "$FILE" annotated         # Extract text with position boundaries and font details
officecli view "$FILE" issues            # Check for font conflicts or formatting issues
```

**Inspect one element.** XPath-style paths, 1-based. ALWAYS quote.
```bash
officecli get "$FILE" "/"                # Document root
officecli get "$FILE" "/page[1]"         # Page metadata and children text blocks
officecli get "$FILE" "/page[1]/text[1]" # Exact text block metadata, position, and styling
```

**Query across the PDF.** CSS-like selectors.
```bash
officecli query "$FILE" "text"                  # Retrieve all text blocks in the document
officecli query "$FILE" 'text-block[font=F1]'   # Query text blocks using font 'F1'
officecli query "$FILE" 'text:contains("Beat")' # Query text blocks containing "Beat"
```

**Visual Preview.**
```bash
officecli view "$FILE" html --page 1     # Render a specific page as static HTML (ideal for visual audits)
officecli view "$FILE" svg               # Render the first page as SVG
```

## Modifying & Annotating

PDF modification is supported via `set` and `remove` verbs. Adding new elements (`add`) is not supported, but text replacement and highlighting are first-class.

### 1. Simple Text Replacement
To change the text of a specific block:
```bash
officecli set "$FILE" "/page[1]/text[3]" --prop text="New Block Content"
```

### 2. Styling Text Blocks
You can modify font, size, spacing, and colors:
```bash
officecli set "$FILE" "/page[1]/text[3]" \
  --prop text="Styled Header" \
  --prop font="Helvetica-Bold" \
  --prop size=16 \
  --prop color="FF0000" \
  --prop bgColor="FFFF00"
```
*Note: Hex colors should drop the leading `#` (e.g. `FF0000` for red).*

### 3. Font Embedding & Fallback
If you insert characters that the existing document fonts cannot render, you can embed a new font file:
```bash
officecli set "$FILE" "/page[1]/text[3]" \
  --prop text="刑事技术" \
  --prop fontFile="/path/to/cjk-font.ttf"
```

### 4. Range-Based Highlighting & Coloring
Highlight or recolor ranges of text blocks using character indexes:
```bash
# Highlight character index 0 to 5 of text[3] on page 1 with default yellow background
officecli set "$FILE" / --prop range_paths="/page[1]/text[3][0:5]"

# Color characters red and highlight them in blue
officecli set "$FILE" / \
  --prop range_paths="/page[1]/text[3][0:5]" \
  --prop color="FF0000" \
  --prop bgColor="0000FF"
```

### 5. Deleting Pages
To delete a specific page:
```bash
officecli remove "$FILE" "/page[2]"
```

### 6. Raw Content Stream Modification (L3)
For advanced operators or raw content stream replacements:
```bash
officecli raw "$FILE" "/page[1]"                                                # View raw content stream
officecli raw-set "$FILE" "/page[1]" --action replace_content --content "BT ..." # Replace whole stream
```

## QA (Required)

1. **Verify structure.** Run `get "$FILE" "/"` to confirm page index structure is valid.
2. **Verify layout.** Run `view "$FILE" html --page 1` and open the HTML preview to verify that text replacements have correct dimensions, fonts, and colors, and do not overlap.
3. **Validate schema.** Run `officecli validate "$FILE"`.

## Common Pitfalls

| Pitfall | Correct Approach |
|---|---|
| Unquoted path bracket | Always wrap paths in quotes: `"/page[1]/text[3]"` |
| Using `add` | PDF does not support adding new elements; modify existing blocks instead |
| Forgetting hex color prefix | Hex colors should be hex strings without `#` or with it handled (e.g., `FF0000`) |
| Font missing character glyphs | If text rendering fails, specify a fallback `font` or `fontFile` |
