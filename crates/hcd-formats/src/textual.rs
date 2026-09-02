use crate::common::{
    base_manifest, checked_export_state, collect_dirty_nodes, emit_failed, emit_started,
    escape_attribute, escape_text, finish_import, source_identity, source_identity_with_extensions,
    write_fidelity_report, ExportOptions, ImportOptions,
};
use hcd_core::{
    hash_bytes, stable_node_id, Bundle, BundleWriter, ChunkSourceMap, FidelityLevel,
    FidelityReport, FidelityWarning, HcdError, HcdManifest, ImportEvent, NodeMapEntry,
    SourceAnchor, DEFAULT_CHUNK_BLOCKS, HCD_SCHEMA_VERSION, MAX_CHUNK_BYTES,
};
use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event as MarkdownEvent, HeadingLevel,
    MetadataBlockKind, Options as MarkdownOptions, Parser as MarkdownParser, Tag as MarkdownTag,
    TagEnd as MarkdownTagEnd,
};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::ops::Range;
use std::path::Path;

const HTML_SOURCE_MAX_BYTES: u64 = 64 * 1024 * 1024;
const HTML_SOURCE_MAX_ELEMENTS: usize = 1_000_000;
const HTML_SOURCE_MAX_DEPTH: usize = 256;
const HTML_TABLE_MAX_ROWS: usize = 1_048_576;
const HTML_TABLE_MAX_COLUMNS: usize = 16_384;
const HTML_TABLE_MAX_CELLS: usize = 1_000_000;
const HTML_TABLE_ROWS_PER_FRAGMENT: usize = 128;
const MARKDOWN_TABLE_ROWS_PER_FRAGMENT: usize = 128;
const MARKDOWN_SOURCE_MAX_BYTES: u64 = 64 * 1024 * 1024;
const SOURCE_RANGE_PREFIX: &str = "bytes:";
const HTML_PART: &str = "html/document";
const MARKDOWN_PART: &str = "markdown/document";
const TEXT_PART: &str = "text/document";

struct FlatChunkWriter<'a, F>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    document_id: &'a str,
    source_part: &'a str,
    region: &'a str,
    writer: &'a mut BundleWriter,
    emit: &'a mut F,
    soft_bytes: usize,
    max_blocks: usize,
    chunk_ordinal: usize,
    blocks: usize,
    html: String,
    entries: Vec<NodeMapEntry>,
}

struct TextNode<'a> {
    text: &'a str,
    text_ordinal: u64,
    source_start: u64,
    source_end: u64,
    node_kind: &'a str,
    paragraph_id: Option<String>,
    wrapper: &'a str,
}

struct RenderedSourceBlock {
    html: String,
    entries: Vec<NodeMapEntry>,
}

impl<'a, F> FlatChunkWriter<'a, F>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    fn push(&mut self, node: TextNode<'_>) -> Result<(), HcdError> {
        let (text, entry) = render_source_text_node(
            self.document_id,
            self.source_part,
            node.text,
            node.text_ordinal,
            node.source_start,
            node.source_end,
            node.node_kind,
            node.paragraph_id,
        )?;
        let block = format!(
            "<{wrapper} class=\"hcd-source-block hcd-{kind}\">{text}</{wrapper}>",
            wrapper = node.wrapper,
            kind = node.node_kind,
            text = text,
        );
        self.push_rendered(
            RenderedSourceBlock {
                html: block,
                entries: vec![entry],
            },
            false,
        )
    }

    fn push_rendered(
        &mut self,
        block: RenderedSourceBlock,
        flush_after: bool,
    ) -> Result<(), HcdError> {
        if block.html.len() > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: structured {} block exceeds {MAX_CHUNK_BYTES} bytes",
                self.source_part
            )));
        }
        if self.blocks > 0
            && (self.html.len().saturating_add(block.html.len()) > self.soft_bytes
                || self.blocks >= self.max_blocks)
        {
            self.flush()?;
        }
        self.html.push_str(&block.html);
        self.entries.extend(block.entries);
        self.blocks += 1;
        if flush_after {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), HcdError> {
        if self.blocks == 0 {
            return Ok(());
        }
        let chunk_ordinal = self.chunk_ordinal.to_string();
        let chunk_id =
            stable_node_id(&[self.document_id, self.source_part, "chunk", &chunk_ordinal])
                .replacen("n_", "c_", 1);
        let html = format!(
            "<section class=\"hcd-source\" data-hcd-source-part=\"{}\">{}</section>",
            self.source_part,
            std::mem::take(&mut self.html)
        );
        let source_map = ChunkSourceMap {
            schema_version: HCD_SCHEMA_VERSION.to_string(),
            chunk_id: chunk_id.clone(),
            entries: std::mem::take(&mut self.entries),
        };
        let descriptor = self.writer.write_chunk(
            chunk_id,
            self.region.to_string(),
            html,
            source_map,
            self.blocks,
            self.chunk_ordinal > 0,
        )?;
        (self.emit)(&ImportEvent::ChunkReady { descriptor })?;
        self.chunk_ordinal += 1;
        self.blocks = 0;
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn render_source_text_node(
    document_id: &str,
    source_part: &str,
    text: &str,
    text_ordinal: u64,
    source_start: u64,
    source_end: u64,
    node_kind: &str,
    paragraph_id: Option<String>,
) -> Result<(String, NodeMapEntry), HcdError> {
    if text.len() > MAX_CHUNK_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "NODE_TOO_LARGE: {source_part} text node {text_ordinal} is {} bytes",
            text.len()
        )));
    }
    if text
        .chars()
        .any(|character| character < '\u{20}' && !matches!(character, '\t' | '\n' | '\r'))
    {
        return Err(HcdError::InvalidBundle(format!(
            "HTML text node {text_ordinal} contains a control character forbidden by HCD"
        )));
    }
    let ordinal = text_ordinal.to_string();
    let node_id = stable_node_id(&[document_id, source_part, node_kind, &ordinal]);
    let node_hash = hash_bytes(text.as_bytes());
    let html = format!(
        "<span data-hcd-id=\"{node_id}\" data-hcd-node-hash=\"{node_hash}\">{}</span>",
        escape_text(text)
    );
    Ok((
        html,
        NodeMapEntry {
            node_id,
            node_hash,
            source: SourceAnchor {
                part: source_part.to_string(),
                text_ordinal,
                paragraph_id,
                text_id: Some(format!("{SOURCE_RANGE_PREFIX}{source_start}:{source_end}")),
                node_kind: node_kind.to_string(),
                editable: true,
            },
        },
    ))
}

pub(crate) fn import_html<F>(
    source: &Path,
    output: &Path,
    options: &ImportOptions,
    mut emit: F,
) -> Result<HcdManifest, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let (source_hash, source_size) = source_identity_with_extensions(source, &["html", "htm"])?;
    if source_size > HTML_SOURCE_MAX_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "HTML source is {source_size} bytes; maximum is {HTML_SOURCE_MAX_BYTES}"
        )));
    }
    emit_started(&mut emit, options, &source_hash)?;
    let result = import_html_inner(source, output, options, source_hash, source_size, &mut emit);
    if let Err(error) = &result {
        emit_failed(&mut emit, options, error);
    }
    result
}

fn import_html_inner<F>(
    source: &Path,
    output: &Path,
    options: &ImportOptions,
    source_hash: String,
    source_size: u64,
    emit: &mut F,
) -> Result<HcdManifest, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let bytes = std::fs::read(source)?;
    let source_text = std::str::from_utf8(&bytes).map_err(|error| {
        HcdError::Unsupported(format!(
            "HTML HCD import currently requires UTF-8 input: {error}"
        ))
    })?;
    let mut writer = BundleWriter::create(output)?;
    writer.write_styles(
        ".hcd-source{display:block}.hcd-source-block{white-space:pre-wrap;margin:.25em 0}.hcd-heading{font-weight:bold}.hcd-pre{font-family:monospace;white-space:pre-wrap}.hcd-source-list{margin:.25em 0}.hcd-source-table{border-collapse:collapse}.hcd-source-table td,.hcd-source-table th{padding:.25em;border:1px solid #ccc}",
    )?;
    let mut chunks = FlatChunkWriter {
        document_id: &options.document_id,
        source_part: HTML_PART,
        region: "body",
        writer: &mut writer,
        emit,
        soft_bytes: options.chunk_soft_bytes.clamp(1, MAX_CHUNK_BYTES),
        max_blocks: options.chunk_blocks.clamp(1, DEFAULT_CHUNK_BLOCKS),
        chunk_ordinal: 0,
        blocks: 0,
        html: String::new(),
        entries: Vec::new(),
    };
    let table_plans = plan_html_tables(source_text)?;
    let mut canonicalizer =
        StructuredHtmlCanonicalizer::new(&options.document_id, &mut chunks, table_plans);
    scan_html(source_text, |event| canonicalizer.event(event))?;
    let mut ordinal = canonicalizer.finish()?;
    if ordinal == 0 {
        ordinal = 1;
        chunks.push(TextNode {
            text: "",
            text_ordinal: ordinal,
            source_start: 0,
            source_end: 0,
            node_kind: "html-empty",
            paragraph_id: Some("segment-1".to_string()),
            wrapper: "p",
        })?;
    }
    chunks.flush()?;

    let mut manifest = base_manifest(options, "html", "semantic-flow", source_hash, source_size);
    manifest.warnings.push(FidelityWarning {
        code: "HTML_ACTIVE_CONTENT_EXCLUDED".to_string(),
        message: "script, style, iframe, object, embed, template and noscript content is excluded from editable HCD chunks; the immutable source remains authoritative".to_string(),
        node_id: None,
        source_part: Some(HTML_PART.to_string()),
    });
    manifest.fidelity = Some(FidelityReport {
        schema_version: HCD_SCHEMA_VERSION.to_string(),
        level: FidelityLevel::Semantic,
        preserved: vec![
            "UTF-8 visible text in source order with stable source byte ranges".to_string(),
            "safe heading, paragraph, list and table semantics with bounded table fragments"
                .to_string(),
            "the immutable HTML source as the source-backed export boundary".to_string(),
        ],
        flattened: vec![
            "source CSS, arbitrary attributes and unsupported or nested markup are reduced to a canonical safe structure".to_string(),
        ],
        dropped: vec![
            "active and non-rendered element content from editable HCD chunks".to_string(),
        ],
        warnings: manifest.warnings.clone(),
    });
    finish_import(writer, manifest, emit)
}

pub(crate) fn import_text<F>(
    source: &Path,
    output: &Path,
    options: &ImportOptions,
    mut emit: F,
) -> Result<HcdManifest, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let (source_hash, source_size) = source_identity(source, "txt")?;
    emit_started(&mut emit, options, &source_hash)?;
    let result = import_text_inner(source, output, options, source_hash, source_size, &mut emit);
    if let Err(error) = &result {
        emit_failed(&mut emit, options, error);
    }
    result
}

pub(crate) fn import_markdown<F>(
    source: &Path,
    output: &Path,
    options: &ImportOptions,
    mut emit: F,
) -> Result<HcdManifest, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let (source_hash, source_size) = source_identity_with_extensions(source, &["md", "markdown"])?;
    emit_started(&mut emit, options, &source_hash)?;
    let result =
        import_markdown_inner(source, output, options, source_hash, source_size, &mut emit);
    if let Err(error) = &result {
        emit_failed(&mut emit, options, error);
    }
    result
}

fn import_markdown_inner<F>(
    source: &Path,
    output: &Path,
    options: &ImportOptions,
    source_hash: String,
    source_size: u64,
    emit: &mut F,
) -> Result<HcdManifest, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    if source_size > MARKDOWN_SOURCE_MAX_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "Markdown source is {source_size} bytes; maximum is {MARKDOWN_SOURCE_MAX_BYTES}"
        )));
    }
    let bytes = std::fs::read(source)?;
    let source_text = std::str::from_utf8(&bytes).map_err(|error| {
        HcdError::Unsupported(format!(
            "Markdown HCD import currently requires UTF-8 input: {error}"
        ))
    })?;
    let source_text = source_text.strip_prefix('\u{feff}').unwrap_or(source_text);
    let source_base = bytes.len().saturating_sub(source_text.len());

    let mut writer = BundleWriter::create(output)?;
    writer.write_styles(&format!(
        "{MARKDOWN_STYLES}{MARKDOWN_PRINT_FIDELITY_STYLES}"
    ))?;
    let mut chunks = FlatChunkWriter {
        document_id: &options.document_id,
        source_part: MARKDOWN_PART,
        region: "body",
        writer: &mut writer,
        emit,
        soft_bytes: options.chunk_soft_bytes.clamp(1, MAX_CHUNK_BYTES),
        max_blocks: options.chunk_blocks.clamp(1, DEFAULT_CHUNK_BLOCKS),
        chunk_ordinal: 0,
        blocks: 0,
        html: String::new(),
        entries: Vec::new(),
    };

    let mut markdown_options = MarkdownOptions::empty();
    markdown_options.insert(MarkdownOptions::ENABLE_TABLES);
    markdown_options.insert(MarkdownOptions::ENABLE_FOOTNOTES);
    markdown_options.insert(MarkdownOptions::ENABLE_STRIKETHROUGH);
    markdown_options.insert(MarkdownOptions::ENABLE_TASKLISTS);
    markdown_options.insert(MarkdownOptions::ENABLE_SMART_PUNCTUATION);
    markdown_options.insert(MarkdownOptions::ENABLE_HEADING_ATTRIBUTES);
    markdown_options.insert(MarkdownOptions::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    markdown_options.insert(MarkdownOptions::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS);
    markdown_options.insert(MarkdownOptions::ENABLE_MATH);
    markdown_options.insert(MarkdownOptions::ENABLE_GFM);
    markdown_options.insert(MarkdownOptions::ENABLE_DEFINITION_LIST);
    markdown_options.insert(MarkdownOptions::ENABLE_SUPERSCRIPT);
    markdown_options.insert(MarkdownOptions::ENABLE_SUBSCRIPT);
    markdown_options.insert(MarkdownOptions::ENABLE_WIKILINKS);

    let line_starts = markdown_line_starts(source_text);
    let mut renderer =
        MarkdownHcdRenderer::new(&options.document_id, source_text, source_base, &line_starts);
    let (parser_source, admonitions) = mask_markdown_admonitions(source_text);
    let mut next_admonition = 0usize;
    for (event, range) in
        MarkdownParser::new_ext(&parser_source, markdown_options).into_offset_iter()
    {
        while renderer.depth == 0
            && admonitions
                .get(next_admonition)
                .is_some_and(|admonition| admonition.range.start < range.start)
        {
            let block =
                renderer.render_admonition(&admonitions[next_admonition], markdown_options)?;
            chunks.push_rendered(block, false)?;
            next_admonition += 1;
        }
        if let Some(block) = renderer.event(event, range)? {
            chunks.push_rendered(block, false)?;
        }
    }
    while let Some(admonition) = admonitions.get(next_admonition) {
        let block = renderer.render_admonition(admonition, markdown_options)?;
        chunks.push_rendered(block, false)?;
        next_admonition += 1;
    }
    if let Some(block) = renderer.finish()? {
        chunks.push_rendered(block, false)?;
    }
    if chunks.blocks == 0 {
        chunks.push(TextNode {
            text: "",
            text_ordinal: 1,
            source_start: source_base as u64,
            source_end: source_base as u64,
            node_kind: "markdown-paragraph",
            paragraph_id: Some("line-1".to_string()),
            wrapper: "p",
        })?;
    }
    chunks.flush()?;

    let mut manifest = base_manifest(options, "md", "semantic-flow", source_hash, source_size);
    manifest.warnings.push(FidelityWarning {
        code: "MARKDOWN_SANITIZED_HTML".to_string(),
        message: "CommonMark, GFM and the enabled safe extensions are rendered semantically; active raw HTML and unsafe URL schemes are escaped or flattened at the HCD security boundary".to_string(),
        node_id: None,
        source_part: Some(MARKDOWN_PART.to_string()),
    });
    manifest.fidelity = Some(FidelityReport {
        schema_version: HCD_SCHEMA_VERSION.to_string(),
        level: FidelityLevel::Semantic,
        preserved: vec![
            "complete CommonMark block and inline structure, including Setext headings, nested containers, reference links and indented code".to_string(),
            "GFM tables, task lists, strikethrough, autolinks, alert blockquotes and footnotes".to_string(),
            "safe extensions for math, superscript, subscript, definition lists, wikilinks, heading attributes and metadata blocks".to_string(),
            "stable text/image node IDs with source byte ranges and source-backed editing".to_string(),
        ],
        flattened: vec![
            "raw HTML outside the safe inline allowlist is displayed as escaped source instead of executing".to_string(),
            "remote Markdown images are addressable semantic image nodes and are not fetched during import".to_string(),
            "editing a Markdown text node rewrites only its mapped source range as escaped Markdown text".to_string(),
        ],
        dropped: Vec::new(),
        warnings: manifest.warnings.clone(),
    });
    finish_import(writer, manifest, emit)
}

const MARKDOWN_STYLES: &str = ".hcd-source{display:block;line-height:1.6;color:#1f2328}.hcd-source-block{margin:.6em 0}.hcd-markdown-heading{font-weight:700;line-height:1.25;margin:1em 0 .45em}.hcd-markdown-code{font-family:ui-monospace,SFMono-Regular,Consolas,monospace;background:#f6f8fa;border:1px solid #d8dee4;border-radius:.35em;padding:.75em;overflow:auto;white-space:pre-wrap}.hcd-markdown-inline-code{font-family:ui-monospace,SFMono-Regular,Consolas,monospace;background:#eff1f3;border-radius:.25em;padding:.1em .3em}.hcd-markdown-quote{border-left:3px solid #b8c0cc;padding:.1em 0 .1em .85em;color:#57606a;margin:.65em 0}.hcd-markdown-alert{border:1px solid #b8c0cc;border-left-width:4px;border-radius:.35em;padding:.55em .8em}.hcd-markdown-alert[data-hcd-alert=note]{border-left-color:#0969da}.hcd-markdown-alert[data-hcd-alert=tip]{border-left-color:#1a7f37}.hcd-markdown-alert[data-hcd-alert=important]{border-left-color:#8250df}.hcd-markdown-alert[data-hcd-alert=warning]{border-left-color:#9a6700}.hcd-markdown-alert[data-hcd-alert=caution]{border-left-color:#cf222e}.hcd-markdown-list{margin:.35em 0;padding-left:1.8em}.hcd-markdown-task{list-style:none;margin-left:-1.3em}.hcd-markdown-task-marker{display:inline-flex;align-items:center;margin-right:.45em}.hcd-markdown-task-marker input{margin:0}.hcd-markdown-image{display:inline-flex;align-items:center;gap:.3em;border:1px dashed #8c959f;padding:.08em .4em;border-radius:.25em;color:#57606a}.hcd-markdown-image::before{content:'image';font-size:.72em;text-transform:uppercase;color:#6e7781}.hcd-markdown-table{border-collapse:collapse;margin:.7em 0;min-width:20em;max-width:100%}.hcd-markdown-table th,.hcd-markdown-table td{border:1px solid #b8c0cc;padding:.38em .65em}.hcd-markdown-table th{background:#eef2f7;font-weight:700}.hcd-markdown-footnotes{border-top:1px solid #d0d7de;margin-top:1.3em;padding-top:.5em}.hcd-markdown-footnote-ref{font-size:.75em;vertical-align:super}.hcd-markdown-definition-list dt{font-weight:700}.hcd-markdown-definition-list dd{margin:0 0 .5em 1.5em}.hcd-markdown-math{font-family:STIX Two Math,Cambria Math,serif;background:#f6f8fa;padding:.08em .25em}.hcd-markdown-display-math{display:block;text-align:center;margin:.7em 0;padding:.5em}.hcd-markdown-metadata,.hcd-markdown-raw-html{font-family:ui-monospace,SFMono-Regular,Consolas,monospace;background:#f6f8fa;color:#57606a}.hcd-markdown-admonition{border:1px solid #d0d7de;border-left:4px solid #9a6700;border-radius:.35em;padding:.6em .8em;margin:.7em 0}.hcd-markdown-rule{border:0;border-top:1px solid #d0d7de;margin:1em 0}.hcd-markdown-wikilink{text-decoration:underline;text-decoration-style:dotted}.hcd-mermaid{margin:1em 0}.hcd-mermaid-preview{overflow:hidden;border:1px solid #d0d7de;border-radius:.45em;padding:.75em;background:#fff;text-align:center}.hcd-mermaid-preview svg{display:block;max-width:100%;height:auto;margin:0 auto}.hcd-mermaid-source{margin-top:.4em;color:#57606a}.hcd-mermaid-source>summary{cursor:pointer;font-size:.85em}.hcd-mermaid-error{border-left:4px solid #cf222e;padding:.45em .7em;color:#82071e;background:#ffebe9}@media print{.hcd-mermaid-source{display:none}}body:not([data-hcd-image-hitboxes=\"off\"]) .hcd-markdown-image[data-hcd-id]:hover,body:not([data-hcd-image-hitboxes=\"off\"]) .hcd-mermaid[data-hcd-id]:hover{outline:2px solid rgba(255,59,48,.95)}body:not([data-hcd-text-hitboxes=\"off\"]) [data-hcd-node-hash]:hover{background:rgba(10,132,255,.12);outline:1px solid rgba(10,132,255,.8)}";

// Keep semantic-flow screen previews and A4 PDF export on the same 16 CSS px
// (12 pt) typography. Print-only fragmentation hints reduce avoidable splits;
// the print engine may still split an oversized block rather than overflow it.
const MARKDOWN_PRINT_FIDELITY_STYLES: &str = ".hcd-source{font-family:HCDSans,HCDEmoji,HCDFallback,\"Noto Sans SC\",\"PingFang SC\",\"Microsoft YaHei\",Arial,sans-serif;font-size:16px}.hcd-markdown-heading{break-after:avoid}.hcd-markdown-table,.hcd-mermaid{break-inside:avoid}@media print{.hcd-markdown-code,.hcd-markdown-quote,.hcd-markdown-alert,.hcd-markdown-admonition{break-inside:avoid}}";

struct RenderedMarkdownTag {
    close: String,
    node_kind: Option<&'static str>,
    is_link: bool,
    is_image: bool,
    content_range: Option<Range<usize>>,
}

struct MarkdownHcdRenderer<'a> {
    document_id: &'a str,
    source: &'a str,
    source_base: usize,
    line_starts: &'a [usize],
    html: String,
    entries: Vec<NodeMapEntry>,
    tags: Vec<RenderedMarkdownTag>,
    depth: usize,
    node_ordinal: u64,
    table_alignments: Vec<Alignment>,
    table_cell: usize,
    table_head: bool,
    table_rows: usize,
    table_fragment: usize,
}

impl<'a> MarkdownHcdRenderer<'a> {
    fn new(
        document_id: &'a str,
        source: &'a str,
        source_base: usize,
        line_starts: &'a [usize],
    ) -> Self {
        Self {
            document_id,
            source,
            source_base,
            line_starts,
            html: String::new(),
            entries: Vec::new(),
            tags: Vec::new(),
            depth: 0,
            node_ordinal: 0,
            table_alignments: Vec::new(),
            table_cell: 0,
            table_head: false,
            table_rows: 0,
            table_fragment: 0,
        }
    }

    fn event<'event>(
        &mut self,
        event: MarkdownEvent<'event>,
        range: Range<usize>,
    ) -> Result<Option<RenderedSourceBlock>, HcdError> {
        if self.event_outside_admonition_content(&range) {
            return Ok(None);
        }
        let mut completed = None;
        match event {
            MarkdownEvent::Start(tag) => {
                if matches!(&tag, MarkdownTag::TableRow)
                    && !self.table_head
                    && self.table_rows > 0
                    && self
                        .table_rows
                        .is_multiple_of(MARKDOWN_TABLE_ROWS_PER_FRAGMENT)
                {
                    self.html.push_str("</tbody></table>");
                    completed = Some(self.take_block());
                    self.table_fragment = self.table_fragment.saturating_add(1);
                    self.html.push_str(&format!(
                        "<table class=\"hcd-markdown-table\" data-hcd-markdown-table-fragment=\"{}\" data-hcd-markdown-table-continuation=\"true\"><tbody>",
                        self.table_fragment
                    ));
                }
                self.start(tag, range)?;
            }
            MarkdownEvent::End(end) => self.end(end)?,
            MarkdownEvent::Text(text) => {
                let auto_link = !self.in_code() && !self.in_link() && !self.in_image();
                let inline_html = if auto_link {
                    markdown_text_with_autolinks(&text)
                } else {
                    escape_text(&text)
                };
                self.text_node(&text, &inline_html, range, self.context_node_kind(), false)?;
            }
            MarkdownEvent::Code(code) => {
                let html = format!(
                    "<code class=\"hcd-markdown-inline-code\">{}</code>",
                    escape_text(&code)
                );
                self.text_node(&code, &html, range, "markdown-inline-code", true)?;
            }
            MarkdownEvent::InlineMath(math) => {
                let html = format!(
                    "<span class=\"hcd-markdown-math\" data-hcd-math=\"inline\">{}</span>",
                    escape_text(&math)
                );
                self.text_node(&math, &html, range, "markdown-inline-math", true)?;
            }
            MarkdownEvent::DisplayMath(math) => {
                let html = format!(
                    "<span class=\"hcd-markdown-math hcd-markdown-display-math\" data-hcd-math=\"display\">{}</span>",
                    escape_text(&math)
                );
                self.text_node(&math, &html, range, "markdown-display-math", true)?;
            }
            MarkdownEvent::Html(html) => {
                let rendered = format!(
                    "<code class=\"hcd-markdown-raw-html\">{}</code>",
                    escape_text(&html)
                );
                self.text_node(&html, &rendered, range, "markdown-html", false)?;
            }
            MarkdownEvent::InlineHtml(html) => {
                if let Some(safe) = safe_markdown_inline_html(&html) {
                    self.html.push_str(safe);
                } else {
                    let rendered = format!(
                        "<code class=\"hcd-markdown-raw-html\">{}</code>",
                        escape_text(&html)
                    );
                    self.text_node(&html, &rendered, range, "markdown-inline-html", false)?;
                }
            }
            MarkdownEvent::FootnoteReference(label) => {
                let id = markdown_fragment_id(&label);
                let visible = format!("[{label}]");
                let html = format!(
                    "<sup class=\"hcd-markdown-footnote-ref\"><a href=\"#hcd-footnote-{id}\">{}</a></sup>",
                    escape_text(&visible)
                );
                self.text_node(&visible, &html, range, "markdown-footnote-reference", false)?;
            }
            MarkdownEvent::SoftBreak => self.html.push('\n'),
            MarkdownEvent::HardBreak => self.html.push_str("<br/>"),
            MarkdownEvent::Rule => self
                .html
                .push_str("<hr class=\"hcd-source-block hcd-markdown-rule\"/>"),
            MarkdownEvent::TaskListMarker(checked) => {
                let classes = if checked {
                    "hcd-markdown-task hcd-markdown-task-checked"
                } else {
                    "hcd-markdown-task"
                };
                let marker = if checked { "☑" } else { "☐" };
                self.html.push_str(&format!(
                    "<span class=\"{classes}\"><span class=\"hcd-markdown-task-marker\">{marker}</span></span>"
                ));
            }
        }
        if completed.is_some() {
            return Ok(completed);
        }
        if self.depth == 0 && !self.html.is_empty() {
            return Ok(Some(self.take_block()));
        }
        Ok(None)
    }

    fn start<'event>(
        &mut self,
        tag: MarkdownTag<'event>,
        range: Range<usize>,
    ) -> Result<(), HcdError> {
        let mut state = RenderedMarkdownTag {
            close: String::new(),
            node_kind: None,
            is_link: false,
            is_image: false,
            content_range: None,
        };
        match tag {
            MarkdownTag::Paragraph => {
                if let Some((kind, content_range)) = markdown_admonition(self.source, &range) {
                    self.html.push_str(&format!(
                        "<aside class=\"hcd-source-block hcd-markdown-admonition\" data-hcd-admonition=\"{}\"><strong class=\"hcd-markdown-admonition-title\">{}</strong>",
                        escape_attribute(kind),
                        escape_text(kind)
                    ));
                    state.close = "</aside>".to_string();
                    state.node_kind = Some("markdown-admonition");
                    state.content_range = Some(content_range);
                } else {
                    self.html
                        .push_str("<p class=\"hcd-source-block hcd-markdown-paragraph\">");
                    state.close = "</p>".to_string();
                    state.node_kind = Some("markdown-paragraph");
                }
            }
            MarkdownTag::Heading {
                level, id, classes, ..
            } => {
                let tag = heading_tag(level);
                let id = id
                    .map(|id| format!(" id=\"{}\"", escape_attribute(&id)))
                    .unwrap_or_default();
                let classes = if classes.is_empty() {
                    String::new()
                } else {
                    format!(
                        " data-hcd-markdown-classes=\"{}\"",
                        escape_attribute(&classes.join(" "))
                    )
                };
                self.html.push_str(&format!(
                    "<{tag} class=\"hcd-source-block hcd-markdown-heading\"{id}{classes}>"
                ));
                state.close = format!("</{tag}>");
                state.node_kind = Some("markdown-heading");
            }
            MarkdownTag::BlockQuote(kind) => {
                if let Some(kind) = kind {
                    let alert = blockquote_kind(kind);
                    self.html.push_str(&format!(
                        "<blockquote class=\"hcd-source-block hcd-markdown-quote hcd-markdown-alert\" data-hcd-alert=\"{alert}\">"
                    ));
                } else {
                    self.html
                        .push_str("<blockquote class=\"hcd-source-block hcd-markdown-quote\">");
                }
                state.close = "</blockquote>".to_string();
                state.node_kind = Some("markdown-quote");
            }
            MarkdownTag::CodeBlock(kind) => {
                let (language, fenced) = match kind {
                    CodeBlockKind::Indented => (String::new(), false),
                    CodeBlockKind::Fenced(info) => {
                        let language = info.split_whitespace().next().unwrap_or_default();
                        (markdown_css_token(language), true)
                    }
                };
                let class = if language.is_empty() {
                    String::new()
                } else {
                    format!(" class=\"language-{language}\"")
                };
                self.html.push_str(&format!(
                    "<pre class=\"hcd-source-block hcd-markdown-code\" data-hcd-fenced=\"{fenced}\"><code{class}>"
                ));
                state.close = "</code></pre>".to_string();
                state.node_kind = Some("markdown-code");
            }
            MarkdownTag::HtmlBlock => {
                self.html
                    .push_str("<pre class=\"hcd-source-block hcd-markdown-raw-html\">");
                state.close = "</pre>".to_string();
                state.node_kind = Some("markdown-html");
            }
            MarkdownTag::List(start) => {
                if let Some(start) = start {
                    let start = if start != 1 {
                        format!(" start=\"{start}\"")
                    } else {
                        String::new()
                    };
                    self.html
                        .push_str(&format!("<ol class=\"hcd-markdown-list\"{start}>"));
                    state.close = "</ol>".to_string();
                } else {
                    self.html.push_str("<ul class=\"hcd-markdown-list\">");
                    state.close = "</ul>".to_string();
                }
                state.node_kind = Some("markdown-list-item");
            }
            MarkdownTag::Item => {
                self.html.push_str("<li>");
                state.close = "</li>".to_string();
                state.node_kind = Some("markdown-list-item");
            }
            MarkdownTag::FootnoteDefinition(label) => {
                let id = markdown_fragment_id(&label);
                self.html.push_str(&format!(
                    "<section class=\"hcd-source-block hcd-markdown-footnotes\" id=\"hcd-footnote-{id}\" data-hcd-footnote=\"{}\"><sup>{}</sup>",
                    escape_attribute(&label),
                    escape_text(&label)
                ));
                state.close = "</section>".to_string();
                state.node_kind = Some("markdown-footnote-definition");
            }
            MarkdownTag::DefinitionList => {
                self.html
                    .push_str("<dl class=\"hcd-source-block hcd-markdown-definition-list\">");
                state.close = "</dl>".to_string();
            }
            MarkdownTag::DefinitionListTitle => {
                self.html.push_str("<dt>");
                state.close = "</dt>".to_string();
                state.node_kind = Some("markdown-definition-title");
            }
            MarkdownTag::DefinitionListDefinition => {
                self.html.push_str("<dd>");
                state.close = "</dd>".to_string();
                state.node_kind = Some("markdown-definition");
            }
            MarkdownTag::Table(alignments) => {
                self.table_alignments = alignments;
                self.table_rows = 0;
                self.table_fragment = 0;
                self.html.push_str(
                    "<table class=\"hcd-markdown-table\" data-hcd-markdown-table-fragment=\"0\">",
                );
                state.close = "</tbody></table>".to_string();
            }
            MarkdownTag::TableHead => {
                self.table_head = true;
                self.table_cell = 0;
                self.html.push_str("<thead><tr>");
                state.close = "</tr></thead><tbody>".to_string();
            }
            MarkdownTag::TableRow => {
                self.table_cell = 0;
                self.html.push_str("<tr>");
                state.close = "</tr>".to_string();
            }
            MarkdownTag::TableCell => {
                let tag = if self.table_head { "th" } else { "td" };
                let align = self
                    .table_alignments
                    .get(self.table_cell)
                    .copied()
                    .unwrap_or(Alignment::None);
                self.table_cell = self.table_cell.saturating_add(1);
                let align = match align {
                    Alignment::None => "",
                    Alignment::Left => " style=\"text-align:left\"",
                    Alignment::Center => " style=\"text-align:center\"",
                    Alignment::Right => " style=\"text-align:right\"",
                };
                self.html.push_str(&format!("<{tag}{align}>"));
                state.close = format!("</{tag}>");
                state.node_kind = Some(if self.table_head {
                    "markdown-table-header"
                } else {
                    "markdown-table-cell"
                });
            }
            MarkdownTag::Emphasis => self.simple_tag("<em>", "</em>", &mut state),
            MarkdownTag::Strong => self.simple_tag("<strong>", "</strong>", &mut state),
            MarkdownTag::Strikethrough => self.simple_tag("<del>", "</del>", &mut state),
            MarkdownTag::Superscript => self.simple_tag("<sup>", "</sup>", &mut state),
            MarkdownTag::Subscript => self.simple_tag("<sub>", "</sub>", &mut state),
            MarkdownTag::Link {
                link_type,
                dest_url,
                title,
                ..
            } => {
                state.is_link = true;
                let destination = match link_type {
                    pulldown_cmark::LinkType::WikiLink { .. } => {
                        format!("#hcd-wiki-{}", markdown_fragment_id(&dest_url))
                    }
                    pulldown_cmark::LinkType::Email => format!("mailto:{dest_url}"),
                    _ => dest_url.to_string(),
                };
                if safe_markdown_destination(&destination) {
                    let title = if title.is_empty() {
                        String::new()
                    } else {
                        format!(" title=\"{}\"", escape_attribute(&title))
                    };
                    let class = matches!(link_type, pulldown_cmark::LinkType::WikiLink { .. })
                        .then_some(" class=\"hcd-markdown-wikilink\"")
                        .unwrap_or_default();
                    self.html.push_str(&format!(
                        "<a{class} href=\"{}\"{title}>",
                        escape_attribute(&destination)
                    ));
                    state.close = "</a>".to_string();
                } else {
                    self.html
                        .push_str("<span class=\"hcd-markdown-unsafe-link\">");
                    state.close = "</span>".to_string();
                }
            }
            MarkdownTag::Image {
                dest_url, title, ..
            } => {
                state.is_image = true;
                let safe = safe_markdown_destination(&dest_url);
                let identity = format!(
                    "{}:{}:{}",
                    self.source_base.saturating_add(range.start),
                    self.source_base.saturating_add(range.end),
                    dest_url
                );
                let node_id =
                    stable_node_id(&[self.document_id, MARKDOWN_PART, "markdown-image", &identity]);
                let source = if safe {
                    format!(
                        " data-hcd-markdown-image-src=\"{}\"",
                        escape_attribute(&dest_url)
                    )
                } else {
                    String::new()
                };
                let title = if title.is_empty() {
                    String::new()
                } else {
                    format!(" title=\"{}\"", escape_attribute(&title))
                };
                self.html.push_str(&format!(
                    "<span class=\"hcd-markdown-image\" data-hcd-id=\"{node_id}\" data-hcd-node-kind=\"image\" data-hcd-editable=\"false\"{source}{title}>"
                ));
                state.close = "</span>".to_string();
                state.node_kind = Some("markdown-image-alt");
            }
            MarkdownTag::MetadataBlock(kind) => {
                let kind = match kind {
                    MetadataBlockKind::YamlStyle => "yaml",
                    MetadataBlockKind::PlusesStyle => "toml",
                };
                self.html.push_str(&format!(
                    "<pre class=\"hcd-source-block hcd-markdown-metadata\" data-hcd-metadata=\"{kind}\"><code>"
                ));
                state.close = "</code></pre>".to_string();
                state.node_kind = Some("markdown-metadata");
            }
        }
        self.tags.push(state);
        self.depth = self.depth.saturating_add(1);
        Ok(())
    }

    fn end(&mut self, end: MarkdownTagEnd) -> Result<(), HcdError> {
        let state = self.tags.pop().ok_or_else(|| {
            HcdError::InvalidBundle(format!("Markdown parser emitted unmatched end tag {end:?}"))
        })?;
        self.html.push_str(&state.close);
        self.depth = self.depth.saturating_sub(1);
        if matches!(end, MarkdownTagEnd::TableHead) {
            self.table_head = false;
        }
        if matches!(end, MarkdownTagEnd::TableRow) && !self.table_head {
            self.table_rows = self.table_rows.saturating_add(1);
        }
        if matches!(end, MarkdownTagEnd::Table) {
            self.table_alignments.clear();
            self.table_cell = 0;
            self.table_head = false;
            self.table_rows = 0;
            self.table_fragment = 0;
        }
        Ok(())
    }

    fn simple_tag(&mut self, open: &str, close: &str, state: &mut RenderedMarkdownTag) {
        self.html.push_str(open);
        state.close = close.to_string();
    }

    fn text_node(
        &mut self,
        visible: &str,
        inline_html: &str,
        range: Range<usize>,
        node_kind: &str,
        prefer_inner_range: bool,
    ) -> Result<(), HcdError> {
        if visible.is_empty() {
            return Ok(());
        }
        let range = markdown_editable_range(self.source, range, visible, prefer_inner_range);
        let (span, entry) = self.render_node(visible, inline_html, range, node_kind, true)?;
        self.html.push_str(&span);
        self.entries.push(entry);
        Ok(())
    }

    fn render_node(
        &mut self,
        visible: &str,
        inline_html: &str,
        range: Range<usize>,
        node_kind: &str,
        editable: bool,
    ) -> Result<(String, NodeMapEntry), HcdError> {
        if visible.len() > MAX_CHUNK_BYTES || inline_html.len() > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: Markdown {node_kind} node exceeds {MAX_CHUNK_BYTES} bytes"
            )));
        }
        self.node_ordinal = self.node_ordinal.saturating_add(1);
        let ordinal = self.node_ordinal.to_string();
        let node_id = stable_node_id(&[self.document_id, MARKDOWN_PART, node_kind, &ordinal]);
        let node_hash = hash_bytes(visible.as_bytes());
        let line = self.line_for_offset(range.start);
        let source_start = self.source_base.saturating_add(range.start) as u64;
        let source_end = self.source_base.saturating_add(range.end) as u64;
        let span = format!(
            "<span data-hcd-id=\"{node_id}\" data-hcd-node-hash=\"{node_hash}\">{inline_html}</span>"
        );
        Ok((
            span,
            NodeMapEntry {
                node_id,
                node_hash,
                source: SourceAnchor {
                    part: MARKDOWN_PART.to_string(),
                    text_ordinal: self.node_ordinal,
                    paragraph_id: Some(format!("line-{line}")),
                    text_id: Some(format!("{SOURCE_RANGE_PREFIX}{source_start}:{source_end}")),
                    node_kind: node_kind.to_string(),
                    editable,
                },
            },
        ))
    }

    fn context_node_kind(&self) -> &'static str {
        self.tags
            .iter()
            .rev()
            .find_map(|tag| tag.node_kind)
            .unwrap_or("markdown-text")
    }

    fn in_code(&self) -> bool {
        self.tags
            .iter()
            .any(|tag| tag.node_kind == Some("markdown-code"))
    }

    fn in_link(&self) -> bool {
        self.tags.iter().any(|tag| tag.is_link)
    }

    fn in_image(&self) -> bool {
        self.tags.iter().any(|tag| tag.is_image)
    }

    fn event_outside_admonition_content(&self, range: &Range<usize>) -> bool {
        self.tags
            .iter()
            .rev()
            .find_map(|tag| tag.content_range.as_ref())
            .is_some_and(|content| range.end <= content.start || range.start >= content.end)
    }

    fn line_for_offset(&self, offset: usize) -> usize {
        self.line_starts
            .partition_point(|start| *start <= offset)
            .max(1)
    }

    fn take_block(&mut self) -> RenderedSourceBlock {
        RenderedSourceBlock {
            html: std::mem::take(&mut self.html),
            entries: std::mem::take(&mut self.entries),
        }
    }

    fn render_admonition(
        &mut self,
        admonition: &MarkdownAdmonition,
        options: MarkdownOptions,
    ) -> Result<RenderedSourceBlock, HcdError> {
        if self.depth != 0 || !self.html.is_empty() {
            return Err(HcdError::InvalidBundle(
                "Markdown admonition cannot be inserted inside another open block".to_string(),
            ));
        }
        self.html.push_str(&format!(
            "<aside class=\"hcd-source-block hcd-markdown-admonition\" data-hcd-admonition=\"{}\"><strong class=\"hcd-markdown-admonition-title\">{}</strong>",
            escape_attribute(&admonition.kind),
            escape_text(&admonition.kind)
        ));
        self.tags.push(RenderedMarkdownTag {
            close: "</aside>".to_string(),
            node_kind: Some("markdown-admonition"),
            is_link: false,
            is_image: false,
            content_range: None,
        });
        self.depth = 1;
        let content = &self.source[admonition.content.clone()];
        for (event, range) in MarkdownParser::new_ext(content, options).into_offset_iter() {
            let range =
                admonition.content.start + range.start..admonition.content.start + range.end;
            if self.event(event, range)?.is_some() {
                return Err(HcdError::InvalidBundle(
                    "Markdown admonition content escaped its container".to_string(),
                ));
            }
        }
        let state = self.tags.pop().ok_or_else(|| {
            HcdError::InvalidBundle("Markdown admonition container was not closed".to_string())
        })?;
        self.html.push_str(&state.close);
        self.depth = 0;
        Ok(self.take_block())
    }

    fn finish(&mut self) -> Result<Option<RenderedSourceBlock>, HcdError> {
        if self.depth != 0 || !self.tags.is_empty() {
            return Err(HcdError::InvalidBundle(
                "Markdown parser ended with unclosed semantic containers".to_string(),
            ));
        }
        Ok((!self.html.is_empty()).then(|| self.take_block()))
    }
}

struct MarkdownAdmonition {
    kind: String,
    range: Range<usize>,
    content: Range<usize>,
}

fn mask_markdown_admonitions(source: &str) -> (String, Vec<MarkdownAdmonition>) {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    for line in source.split_inclusive('\n') {
        let end = offset + line.len();
        lines.push((offset, end, line.trim_end_matches(['\r', '\n'])));
        offset = end;
    }
    if offset < source.len() || source.is_empty() {
        lines.push((offset, source.len(), &source[offset..]));
    }

    let mut admonitions = Vec::new();
    let mut line_index = 0usize;
    while line_index < lines.len() {
        let (start, opener_end, opener) = lines[line_index];
        let Some(kind) = opener.trim().strip_prefix(":::").map(str::trim) else {
            line_index += 1;
            continue;
        };
        if kind.is_empty()
            || !kind
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            line_index += 1;
            continue;
        }
        let mut close_index = line_index + 1;
        while close_index < lines.len() && lines[close_index].2.trim() != ":::" {
            close_index += 1;
        }
        if close_index == lines.len() {
            line_index += 1;
            continue;
        }
        let (close_start, end, _) = lines[close_index];
        admonitions.push(MarkdownAdmonition {
            kind: kind.to_ascii_lowercase(),
            range: start..end,
            content: opener_end..close_start,
        });
        line_index = close_index + 1;
    }

    let mut masked = source.as_bytes().to_vec();
    for admonition in &admonitions {
        for byte in &mut masked[admonition.range.clone()] {
            if !matches!(*byte, b'\r' | b'\n') {
                *byte = b' ';
            }
        }
    }
    (
        String::from_utf8(masked).expect("masking UTF-8 with ASCII spaces remains UTF-8"),
        admonitions,
    )
}

fn markdown_line_starts(source: &str) -> Vec<usize> {
    let mut starts = Vec::with_capacity(source.len() / 40 + 1);
    starts.push(0);
    starts.extend(
        source
            .bytes()
            .enumerate()
            .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
    );
    starts
}

fn markdown_editable_range(
    source: &str,
    range: Range<usize>,
    visible: &str,
    prefer_inner: bool,
) -> Range<usize> {
    if range.start > range.end || range.end > source.len() {
        return range;
    }
    let raw = &source[range.clone()];
    if raw == visible {
        return range;
    }
    if prefer_inner {
        if let Some(offset) = raw.find(visible) {
            return range.start + offset..range.start + offset + visible.len();
        }
    }
    range
}

fn markdown_admonition<'a>(
    source: &'a str,
    range: &Range<usize>,
) -> Option<(&'a str, Range<usize>)> {
    let raw = source.get(range.clone())?;
    let first_end = raw.find('\n')?;
    let first = raw[..first_end].trim();
    let kind = first.strip_prefix(":::")?.trim();
    if kind.is_empty()
        || !kind
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    let trimmed = raw.trim_end();
    let close_start = trimmed.rfind('\n')? + 1;
    (trimmed[close_start..].trim() == ":::")
        .then_some((kind, range.start + first_end + 1..range.start + close_start))
}

fn heading_tag(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "h1",
        HeadingLevel::H2 => "h2",
        HeadingLevel::H3 => "h3",
        HeadingLevel::H4 => "h4",
        HeadingLevel::H5 => "h5",
        HeadingLevel::H6 => "h6",
    }
}

fn blockquote_kind(kind: BlockQuoteKind) -> &'static str {
    match kind {
        BlockQuoteKind::Note => "note",
        BlockQuoteKind::Tip => "tip",
        BlockQuoteKind::Important => "important",
        BlockQuoteKind::Warning => "warning",
        BlockQuoteKind::Caution => "caution",
    }
}

fn markdown_css_token(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .take(64)
        .collect()
}

fn markdown_fragment_id(value: &str) -> String {
    let token = markdown_css_token(value.trim());
    if token.is_empty() {
        hash_bytes(value.as_bytes())[..16].to_string()
    } else {
        token
    }
}

fn safe_markdown_destination(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || value.starts_with("//") {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:")
        || lower.starts_with('#')
        || lower.starts_with('/')
        || lower.starts_with("./")
        || lower.starts_with("../")
    {
        return true;
    }
    !value
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .contains(':')
}

fn safe_markdown_inline_html(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "<mark>"
            | "</mark>"
            | "<kbd>"
            | "</kbd>"
            | "<u>"
            | "</u>"
            | "<s>"
            | "</s>"
            | "<sub>"
            | "</sub>"
            | "<sup>"
            | "</sup>"
            | "<small>"
            | "</small>"
            | "<br>"
            | "<br/>"
            | "<br />"
    )
    .then_some(trimmed)
}

fn markdown_text_with_autolinks(value: &str) -> String {
    let mut html = String::new();
    let mut cursor = 0usize;
    while cursor < value.len() {
        let rest = &value[cursor..];
        let next = ["https://", "http://", "mailto:", "www."]
            .into_iter()
            .filter_map(|prefix| rest.find(prefix).map(|offset| (offset, prefix)))
            .min_by_key(|(offset, _)| *offset);
        let Some((offset, prefix)) = next else {
            html.push_str(&escape_text(rest));
            break;
        };
        html.push_str(&escape_text(&rest[..offset]));
        let candidate = &rest[offset..];
        let mut end = candidate
            .char_indices()
            .find_map(|(index, character)| {
                (index >= prefix.len()
                    && (character.is_whitespace() || matches!(character, '<' | '>' | '"')))
                .then_some(index)
            })
            .unwrap_or(candidate.len());
        while end > prefix.len()
            && candidate[..end]
                .chars()
                .next_back()
                .is_some_and(|character| matches!(character, ')' | ']' | '}' | ',' | '.' | ';'))
        {
            end -= candidate[..end].chars().next_back().unwrap().len_utf8();
        }
        if end == prefix.len() {
            html.push_str(&escape_text(prefix));
            cursor += offset + prefix.len();
            continue;
        }
        let label = &candidate[..end];
        let target = if prefix == "www." {
            format!("https://{label}")
        } else {
            label.to_string()
        };
        if safe_markdown_destination(&target) {
            html.push_str(&format!(
                "<a href=\"{}\">{}</a>",
                escape_attribute(&target),
                escape_text(label)
            ));
        } else {
            html.push_str(&escape_text(label));
        }
        cursor += offset + end;
    }
    html
}

#[allow(dead_code)]
fn import_markdown_inner_legacy<F>(
    source: &Path,
    output: &Path,
    options: &ImportOptions,
    source_hash: String,
    source_size: u64,
    emit: &mut F,
) -> Result<HcdManifest, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let mut writer = BundleWriter::create(output)?;
    writer.write_styles(
        ".hcd-source{display:block}.hcd-source-block{white-space:pre-wrap;margin:.35em 0}.hcd-markdown-heading{font-weight:700}.hcd-markdown-code{font-family:monospace;background:#f6f8fa;padding:.35em}.hcd-markdown-quote{border-left:3px solid #bbb;padding-left:.75em;color:#555}.hcd-markdown-list{margin:.2em 0;padding-left:1.5em}.hcd-markdown-task{list-style:none}.hcd-markdown-task-marker{display:inline-block;margin-right:.4em}.hcd-markdown-image{border:1px dashed #9aa4b2;padding:.1em .35em;border-radius:.25em}.hcd-markdown-table{border-collapse:collapse;margin:.6em 0;min-width:20em}.hcd-markdown-table th,.hcd-markdown-table td{border:1px solid #b8c0cc;padding:.35em .6em;text-align:left}.hcd-markdown-table th{background:#eef2f7;font-weight:700}",
    )?;
    let mut chunks = FlatChunkWriter {
        document_id: &options.document_id,
        source_part: MARKDOWN_PART,
        region: "body",
        writer: &mut writer,
        emit,
        soft_bytes: options.chunk_soft_bytes.clamp(1, MAX_CHUNK_BYTES),
        max_blocks: options.chunk_blocks.clamp(1, DEFAULT_CHUNK_BLOCKS),
        chunk_ordinal: 0,
        blocks: 0,
        html: String::new(),
        entries: Vec::new(),
    };
    let file = File::open(source)?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut source_offset = 0u64;
    let mut line_ordinal = 0u64;
    let mut node_ordinal = 0u64;
    let mut rendered_blocks = 0u64;
    let mut fenced = false;
    let mut table = None;
    while let Some(line) = read_bounded_line(&mut reader, source_offset)? {
        source_offset = line.next_offset;
        line_ordinal = line_ordinal.saturating_add(1);
        let mut text_start = line.content_start;
        let mut content = line.content;
        if line_ordinal == 1 && content.starts_with(&[0xef, 0xbb, 0xbf]) {
            content.drain(..3);
            text_start = text_start.saturating_add(3);
        }
        let raw = std::str::from_utf8(&content).map_err(|error| {
            HcdError::Unsupported(format!(
                "Markdown HCD import currently requires UTF-8 input at line {line_ordinal}: {error}"
            ))
        })?;
        let trimmed = raw.trim_start();
        let indentation = raw.len() - trimmed.len();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush_markdown_table(&mut table, &mut chunks)?;
            fenced = !fenced;
            continue;
        }
        if !fenced {
            if let Some(cells) = markdown_table_cells(trimmed) {
                if markdown_table_separator(&cells) {
                    continue;
                }
                let header = table
                    .as_ref()
                    .is_none_or(|table: &MarkdownTableBuilder| table.rows == 0);
                let mut row_html = String::from("<tr>");
                let mut row_entries = Vec::with_capacity(cells.len());
                for cell in cells {
                    node_ordinal = node_ordinal.saturating_add(1);
                    let (visible, inline_html) = markdown_inline(cell.text);
                    let source_start = text_start
                        .saturating_add(indentation as u64)
                        .saturating_add(cell.start as u64);
                    let source_end = text_start
                        .saturating_add(indentation as u64)
                        .saturating_add(cell.end as u64);
                    let node_kind = if header {
                        "markdown-table-header"
                    } else {
                        "markdown-table-cell"
                    };
                    let (span, entry) = render_markdown_text_node(
                        &options.document_id,
                        &visible,
                        &inline_html,
                        node_ordinal,
                        line_ordinal,
                        source_start,
                        source_end,
                        node_kind,
                    )?;
                    let tag = if header { "th" } else { "td" };
                    row_html.push_str(&format!("<{tag}>{span}</{tag}>"));
                    row_entries.push(entry);
                }
                row_html.push_str("</tr>");
                let must_flush = table.as_ref().is_some_and(|table| {
                    table.rows >= MARKDOWN_TABLE_ROWS_PER_FRAGMENT
                        || table
                            .html
                            .len()
                            .saturating_add(row_html.len())
                            .saturating_add(32)
                            > MAX_CHUNK_BYTES
                });
                if must_flush {
                    flush_markdown_table(&mut table, &mut chunks)?;
                }
                table
                    .get_or_insert_with(MarkdownTableBuilder::new)
                    .push_row(row_html, row_entries);
                rendered_blocks = rendered_blocks.saturating_add(1);
                continue;
            }
        }
        flush_markdown_table(&mut table, &mut chunks)?;
        let block = markdown_block(trimmed, fenced);
        if matches!(block.kind, MarkdownBlockKind::Rule) {
            chunks.push_rendered(
                RenderedSourceBlock {
                    html: "<hr class=\"hcd-source-block hcd-markdown-rule\"/>".to_string(),
                    entries: Vec::new(),
                },
                false,
            )?;
            rendered_blocks = rendered_blocks.saturating_add(1);
            continue;
        }
        let source_start = text_start
            .saturating_add(indentation as u64)
            .saturating_add(block.prefix_bytes as u64);
        let mut source_end = line.content_end;
        let source_text = &trimmed[block.prefix_bytes.min(trimmed.len())..];
        let (source_text, hard_break) = source_text
            .strip_suffix("  ")
            .map_or((source_text, false), |text| (text, true));
        if hard_break {
            source_end = source_end.saturating_sub(2);
        }
        let (visible, mut inline_html) = markdown_inline(source_text);
        if hard_break {
            inline_html.push_str("<br/>");
        }
        node_ordinal = node_ordinal.saturating_add(1);
        let (span, entry) = render_markdown_text_node(
            &options.document_id,
            &visible,
            &inline_html,
            node_ordinal,
            line_ordinal,
            source_start,
            source_end,
            block.node_kind,
        )?;
        let html = match block.kind {
            MarkdownBlockKind::ListItem {
                ordered,
                start,
                task,
            } => {
                let list = if ordered { "ol" } else { "ul" };
                let start = start
                    .filter(|start| ordered && *start != 1)
                    .map(|start| format!(" start=\"{start}\""))
                    .unwrap_or_default();
                let (item_class, marker) = match task {
                    Some(true) => (
                        " class=\"hcd-markdown-task hcd-markdown-task-checked\"",
                        "<span class=\"hcd-markdown-task-marker\">☑</span>",
                    ),
                    Some(false) => (
                        " class=\"hcd-markdown-task\"",
                        "<span class=\"hcd-markdown-task-marker\">☐</span>",
                    ),
                    None => ("", ""),
                };
                format!(
                    "<{list} class=\"hcd-markdown-list\"{start}><li{item_class}>{marker}{span}</li></{list}>"
                )
            }
            MarkdownBlockKind::Rule => unreachable!("rules are emitted before text mapping"),
            MarkdownBlockKind::Normal => format!(
                "<{wrapper} class=\"hcd-source-block hcd-{kind}\">{span}</{wrapper}>",
                wrapper = block.wrapper,
                kind = block.node_kind,
            ),
        };
        chunks.push_rendered(
            RenderedSourceBlock {
                html,
                entries: vec![entry],
            },
            false,
        )?;
        rendered_blocks = rendered_blocks.saturating_add(1);
    }
    flush_markdown_table(&mut table, &mut chunks)?;
    if rendered_blocks == 0 {
        chunks.push(TextNode {
            text: "",
            text_ordinal: 1,
            source_start: 0,
            source_end: 0,
            node_kind: "markdown-paragraph",
            paragraph_id: Some("line-1".to_string()),
            wrapper: "p",
        })?;
    }
    chunks.flush()?;

    let mut manifest = base_manifest(options, "md", "semantic-flow", source_hash, source_size);
    manifest.warnings.push(FidelityWarning {
        code: "MARKDOWN_CANONICAL_SUBSET".to_string(),
        message: "HCD preview recognizes ATX headings, blockquotes, ordered/unordered/task list items, fenced code, hard breaks, pipe-delimited GFM tables, emphasis, links, autolinks and safe image placeholders; unsupported Markdown constructs remain editable text and the immutable source remains authoritative".to_string(),
        node_id: None,
        source_part: Some(MARKDOWN_PART.to_string()),
    });
    manifest.fidelity = Some(FidelityReport {
        schema_version: HCD_SCHEMA_VERSION.to_string(),
        level: FidelityLevel::Semantic,
        preserved: vec![
            "UTF-8 Markdown text, empty lines, source order, BOM and original line terminators".to_string(),
            "ATX headings, blockquotes, ordered/unordered/task list items, fenced code, hard breaks, pipe-delimited GFM tables, emphasis, links, autolinks and safe image placeholders".to_string(),
            "stable node IDs and source byte ranges for source-backed editing".to_string(),
        ],
        flattened: vec![
            "nested and extension-specific Markdown layout is represented by a bounded canonical HTML subset".to_string(),
            "editing a Markdown node rewrites that node as escaped plain Markdown text while preserving all other source bytes".to_string(),
        ],
        dropped: Vec::new(),
        warnings: manifest.warnings.clone(),
    });
    finish_import(writer, manifest, emit)
}

#[derive(Clone, Copy)]
enum MarkdownBlockKind {
    Normal,
    ListItem {
        ordered: bool,
        start: Option<usize>,
        task: Option<bool>,
    },
    Rule,
}

struct MarkdownBlock {
    kind: MarkdownBlockKind,
    prefix_bytes: usize,
    wrapper: &'static str,
    node_kind: &'static str,
}

struct MarkdownTableBuilder {
    html: String,
    entries: Vec<NodeMapEntry>,
    rows: usize,
}

impl MarkdownTableBuilder {
    fn new() -> Self {
        Self {
            html: "<table class=\"hcd-markdown-table\"><tbody>".to_string(),
            entries: Vec::new(),
            rows: 0,
        }
    }

    fn push_row(&mut self, row_html: String, entries: Vec<NodeMapEntry>) {
        self.html.push_str(&row_html);
        self.entries.extend(entries);
        self.rows += 1;
    }

    fn finish(mut self) -> RenderedSourceBlock {
        self.html.push_str("</tbody></table>");
        RenderedSourceBlock {
            html: self.html,
            entries: self.entries,
        }
    }
}

fn flush_markdown_table<F>(
    table: &mut Option<MarkdownTableBuilder>,
    chunks: &mut FlatChunkWriter<'_, F>,
) -> Result<(), HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    if let Some(table) = table.take() {
        chunks.push_rendered(table.finish(), false)?;
    }
    Ok(())
}

struct MarkdownTableCell<'a> {
    start: usize,
    end: usize,
    text: &'a str,
}

fn markdown_table_cells(line: &str) -> Option<Vec<MarkdownTableCell<'_>>> {
    if !line.starts_with('|') || !line.ends_with('|') || line.len() < 3 {
        return None;
    }
    let mut separators = Vec::new();
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if character == '|' && !escaped {
            separators.push(index);
        }
        if character == '\\' {
            escaped = !escaped;
        } else {
            escaped = false;
        }
    }
    if separators.first() != Some(&0)
        || separators.last() != Some(&line.len().saturating_sub(1))
        || separators.len() < 3
    {
        return None;
    }
    let mut cells = Vec::with_capacity(separators.len() - 1);
    for pair in separators.windows(2) {
        let raw_start = pair[0] + 1;
        let raw_end = pair[1];
        let raw = &line[raw_start..raw_end];
        let text = raw.trim();
        let leading = raw.len() - raw.trim_start().len();
        let start = raw_start + leading;
        let end = start + text.len();
        cells.push(MarkdownTableCell { start, end, text });
    }
    Some(cells)
}

fn markdown_table_separator(cells: &[MarkdownTableCell<'_>]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let marker = cell.text.trim_matches(':');
            marker.len() >= 3 && marker.bytes().all(|byte| byte == b'-')
        })
}

fn markdown_block(line: &str, fenced: bool) -> MarkdownBlock {
    if fenced {
        return MarkdownBlock {
            kind: MarkdownBlockKind::Normal,
            prefix_bytes: 0,
            wrapper: "pre",
            node_kind: "markdown-code",
        };
    }
    let heading_marks = line.bytes().take_while(|byte| *byte == b'#').count();
    if (1..=6).contains(&heading_marks) && line.as_bytes().get(heading_marks) == Some(&b' ') {
        return MarkdownBlock {
            kind: MarkdownBlockKind::Normal,
            prefix_bytes: heading_marks + 1,
            wrapper: match heading_marks {
                1 => "h1",
                2 => "h2",
                3 => "h3",
                4 => "h4",
                5 => "h5",
                _ => "h6",
            },
            node_kind: "markdown-heading",
        };
    }
    if let Some(rest) = line.strip_prefix("> ") {
        return MarkdownBlock {
            kind: MarkdownBlockKind::Normal,
            prefix_bytes: line.len() - rest.len(),
            wrapper: "blockquote",
            node_kind: "markdown-quote",
        };
    }
    if line.starts_with("- ") || line.starts_with("* ") || line.starts_with("+ ") {
        let item = &line[2..];
        let task = markdown_task_prefix(item);
        return MarkdownBlock {
            kind: MarkdownBlockKind::ListItem {
                ordered: false,
                start: None,
                task: task.map(|(_, checked)| checked),
            },
            prefix_bytes: 2 + task.map(|(bytes, _)| bytes).unwrap_or(0),
            wrapper: "li",
            node_kind: if task.is_some() {
                "markdown-task-item"
            } else {
                "markdown-list-item"
            },
        };
    }
    if let Some(prefix) = ordered_list_prefix(line) {
        let start = line[..prefix.saturating_sub(2)].parse().ok();
        let item = &line[prefix..];
        let task = markdown_task_prefix(item);
        return MarkdownBlock {
            kind: MarkdownBlockKind::ListItem {
                ordered: true,
                start,
                task: task.map(|(_, checked)| checked),
            },
            prefix_bytes: prefix + task.map(|(bytes, _)| bytes).unwrap_or(0),
            wrapper: "li",
            node_kind: if task.is_some() {
                "markdown-task-item"
            } else {
                "markdown-list-item"
            },
        };
    }
    if matches!(line.trim(), "---" | "***" | "___") {
        return MarkdownBlock {
            kind: MarkdownBlockKind::Rule,
            prefix_bytes: 0,
            wrapper: "hr",
            node_kind: "markdown-rule",
        };
    }
    MarkdownBlock {
        kind: MarkdownBlockKind::Normal,
        prefix_bytes: 0,
        wrapper: "p",
        node_kind: "markdown-paragraph",
    }
}

fn markdown_task_prefix(item: &str) -> Option<(usize, bool)> {
    let marker = item.get(..4)?;
    match marker.as_bytes() {
        [b'[', b' ', b']', b' '] => Some((4, false)),
        [b'[', b'x' | b'X', b']', b' '] => Some((4, true)),
        _ => None,
    }
}

fn ordered_list_prefix(line: &str) -> Option<usize> {
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0
        && line.as_bytes().get(digits) == Some(&b'.')
        && line.as_bytes().get(digits + 1) == Some(&b' '))
    .then_some(digits + 2)
}

fn markdown_inline(source: &str) -> (String, String) {
    markdown_inline_inner(source, 0)
}

fn markdown_inline_inner(source: &str, depth: usize) -> (String, String) {
    if depth > 16 {
        return (source.to_string(), escape_text(source));
    }
    let mut visible = String::new();
    let mut html = String::new();
    let mut cursor = 0usize;
    while cursor < source.len() {
        let rest = &source[cursor..];
        let mut matched = false;
        if let Some(escaped) = rest.strip_prefix('\\') {
            if let Some(character) = escaped.chars().next() {
                visible.push(character);
                html.push_str(&escape_text(&character.to_string()));
                cursor += 1 + character.len_utf8();
                continue;
            }
        }
        for delimiter in ["***", "___"] {
            if let Some(after) = rest.strip_prefix(delimiter) {
                if let Some(end) = after.find(delimiter) {
                    let inner = &after[..end];
                    let (inner_text, inner_html) = markdown_inline_inner(inner, depth + 1);
                    visible.push_str(&inner_text);
                    html.push_str(&format!("<strong><em>{inner_html}</em></strong>"));
                    cursor += delimiter.len() + end + delimiter.len();
                    matched = true;
                    break;
                }
            }
        }
        if matched {
            continue;
        }
        for (delimiter, tag) in [("**", "strong"), ("__", "strong"), ("~~", "del")] {
            if let Some(after) = rest.strip_prefix(delimiter) {
                if let Some(end) = after.find(delimiter) {
                    let inner = &after[..end];
                    let (inner_text, inner_html) = markdown_inline_inner(inner, depth + 1);
                    visible.push_str(&inner_text);
                    html.push_str(&format!("<{tag}>{inner_html}</{tag}>"));
                    cursor += delimiter.len() + end + delimiter.len();
                    matched = true;
                    break;
                }
            }
        }
        if matched {
            continue;
        }
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                let inner = &after[..end];
                visible.push_str(inner);
                html.push_str("<code>");
                html.push_str(&escape_text(inner));
                html.push_str("</code>");
                cursor += end + 2;
                continue;
            }
        }
        if rest.starts_with("![") {
            if let Some(label_end) = rest.find("](") {
                let target_start = label_end + 2;
                if let Some(target_end) = markdown_destination_end(&rest[target_start..]) {
                    let label = &rest[2..label_end];
                    let raw_target = &rest[target_start..target_start + target_end];
                    let (label_text, label_html) = markdown_inline_inner(label, depth + 1);
                    visible.push_str(&label_text);
                    if let Some((target, title)) = markdown_link_target(raw_target) {
                        if safe_markdown_href(target) {
                            html.push_str(&format!(
                                "<span class=\"hcd-markdown-image\" data-hcd-markdown-image-src=\"{}\"{}>{label_html}</span>",
                                escape_attribute(target),
                                title
                                    .map(|title| format!(" title=\"{}\"", escape_attribute(title)))
                                    .unwrap_or_default()
                            ));
                        } else {
                            html.push_str(&label_html);
                        }
                    } else {
                        html.push_str(&label_html);
                    }
                    cursor += target_start + target_end + 1;
                    continue;
                }
            }
        }
        if rest.starts_with('[') {
            if let Some(label_end) = rest.find("](") {
                let target_start = label_end + 2;
                if let Some(target_end) = markdown_destination_end(&rest[target_start..]) {
                    let label = &rest[1..label_end];
                    let raw_target = &rest[target_start..target_start + target_end];
                    let (label_text, label_html) = markdown_inline_inner(label, depth + 1);
                    visible.push_str(&label_text);
                    if let Some((target, title)) = markdown_link_target(raw_target) {
                        if safe_markdown_href(target) {
                            html.push_str(&format!(
                                "<a href=\"{}\"{}>{label_html}</a>",
                                escape_attribute(target),
                                title
                                    .map(|title| format!(" title=\"{}\"", escape_attribute(title)))
                                    .unwrap_or_default()
                            ));
                        } else {
                            html.push_str(&label_html);
                        }
                    } else {
                        html.push_str(&label_html);
                    }
                    cursor += target_start + target_end + 1;
                    continue;
                }
            }
        }
        if let Some(after) = rest.strip_prefix('<') {
            if let Some(end) = after.find('>') {
                let candidate = &after[..end];
                let (target, label) = if safe_markdown_href(candidate) {
                    (candidate.to_string(), candidate)
                } else if markdown_email(candidate) {
                    (format!("mailto:{candidate}"), candidate)
                } else {
                    (String::new(), candidate)
                };
                if !target.is_empty() {
                    visible.push_str(label);
                    html.push_str(&format!(
                        "<a href=\"{}\">{}</a>",
                        escape_attribute(&target),
                        escape_text(label)
                    ));
                    cursor += end + 2;
                    continue;
                }
            }
        }
        if let Some(prefix) = ["https://", "http://", "mailto:"]
            .into_iter()
            .find(|prefix| rest.starts_with(prefix))
        {
            let mut end = rest
                .char_indices()
                .find_map(|(index, character)| {
                    (index >= prefix.len()
                        && (character.is_whitespace() || matches!(character, '<' | '>' | '"')))
                    .then_some(index)
                })
                .unwrap_or(rest.len());
            while end > prefix.len()
                && rest[..end]
                    .chars()
                    .next_back()
                    .is_some_and(|character| matches!(character, ')' | ']' | '}' | ',' | '.' | ';'))
            {
                end -= rest[..end].chars().next_back().unwrap().len_utf8();
            }
            let target = &rest[..end];
            if safe_markdown_href(target) {
                visible.push_str(target);
                html.push_str(&format!(
                    "<a href=\"{}\">{}</a>",
                    escape_attribute(target),
                    escape_text(target)
                ));
                cursor += end;
                continue;
            }
        }
        for (delimiter, tag) in [("*", "em"), ("_", "em")] {
            if let Some(after) = rest.strip_prefix(delimiter) {
                if let Some(end) = after.find(delimiter) {
                    let inner = &after[..end];
                    let (inner_text, inner_html) = markdown_inline_inner(inner, depth + 1);
                    visible.push_str(&inner_text);
                    html.push_str(&format!("<{tag}>{inner_html}</{tag}>"));
                    cursor += delimiter.len() + end + delimiter.len();
                    matched = true;
                    break;
                }
            }
        }
        if matched {
            continue;
        }
        let character = rest.chars().next().expect("cursor is within source");
        visible.push(character);
        html.push_str(&escape_text(&character.to_string()));
        cursor += character.len_utf8();
    }
    (visible, html)
}

fn markdown_link_target(value: &str) -> Option<(&str, Option<&str>)> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let (target, remainder) = if let Some(after) = value.strip_prefix('<') {
        let end = after.find('>')?;
        (&after[..end], after[end + 1..].trim())
    } else {
        let split = value.find(char::is_whitespace).unwrap_or(value.len());
        (&value[..split], value[split..].trim())
    };
    let title = (!remainder.is_empty())
        .then(|| {
            remainder
                .strip_prefix('"')
                .and_then(|title| title.strip_suffix('"'))
                .or_else(|| {
                    remainder
                        .strip_prefix('\'')
                        .and_then(|title| title.strip_suffix('\''))
                })
        })
        .flatten();
    (remainder.is_empty() || title.is_some()).then_some((target, title))
}

fn markdown_destination_end(value: &str) -> Option<usize> {
    let mut nested_parentheses = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            continue;
        }
        match character {
            '(' => nested_parentheses = nested_parentheses.saturating_add(1),
            ')' if nested_parentheses == 0 => return Some(index),
            ')' => nested_parentheses = nested_parentheses.saturating_sub(1),
            _ => {}
        }
    }
    None
}

fn markdown_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-' | b'@')
        })
}

fn safe_markdown_href(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
        || lower.starts_with('#')
}

#[allow(clippy::too_many_arguments)]
fn render_markdown_text_node(
    document_id: &str,
    visible: &str,
    inline_html: &str,
    text_ordinal: u64,
    line_ordinal: u64,
    source_start: u64,
    source_end: u64,
    node_kind: &str,
) -> Result<(String, NodeMapEntry), HcdError> {
    if visible.len() > MAX_CHUNK_BYTES || inline_html.len() > MAX_CHUNK_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "NODE_TOO_LARGE: Markdown node {text_ordinal} exceeds {MAX_CHUNK_BYTES} bytes"
        )));
    }
    if visible
        .chars()
        .any(|character| character < '\u{20}' && !matches!(character, '\t' | '\n' | '\r'))
    {
        return Err(HcdError::InvalidBundle(format!(
            "Markdown text node {text_ordinal} contains a control character forbidden by HCD"
        )));
    }
    let ordinal = text_ordinal.to_string();
    let node_id = stable_node_id(&[document_id, MARKDOWN_PART, node_kind, &ordinal]);
    let node_hash = hash_bytes(visible.as_bytes());
    let html = format!(
        "<span data-hcd-id=\"{node_id}\" data-hcd-node-hash=\"{node_hash}\">{inline_html}</span>"
    );
    Ok((
        html,
        NodeMapEntry {
            node_id,
            node_hash,
            source: SourceAnchor {
                part: MARKDOWN_PART.to_string(),
                text_ordinal,
                paragraph_id: Some(format!("line-{line_ordinal}")),
                text_id: Some(format!("{SOURCE_RANGE_PREFIX}{source_start}:{source_end}")),
                node_kind: node_kind.to_string(),
                editable: true,
            },
        },
    ))
}

fn import_text_inner<F>(
    source: &Path,
    output: &Path,
    options: &ImportOptions,
    source_hash: String,
    source_size: u64,
    emit: &mut F,
) -> Result<HcdManifest, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let mut writer = BundleWriter::create(output)?;
    writer.write_styles(
        ".hcd-source{display:block;font-family:HCDSans,HCDEmoji,HCDFallback,\"Noto Sans SC\",\"PingFang SC\",\"Microsoft YaHei\",Arial,sans-serif;font-size:16px;line-height:1.6}.hcd-source-block{white-space:pre-wrap;margin:0;min-height:1.6em}",
    )?;
    let mut chunks = FlatChunkWriter {
        document_id: &options.document_id,
        source_part: TEXT_PART,
        region: "body",
        writer: &mut writer,
        emit,
        soft_bytes: options.chunk_soft_bytes.clamp(1, MAX_CHUNK_BYTES),
        max_blocks: options.chunk_blocks.clamp(1, DEFAULT_CHUNK_BLOCKS),
        chunk_ordinal: 0,
        blocks: 0,
        html: String::new(),
        entries: Vec::new(),
    };
    let file = File::open(source)?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut source_offset = 0u64;
    let mut ordinal = 0u64;
    while let Some(line) = read_bounded_line(&mut reader, source_offset)? {
        source_offset = line.next_offset;
        ordinal = ordinal.saturating_add(1);
        let mut text_start = line.content_start;
        let mut content = line.content;
        if ordinal == 1 && content.starts_with(&[0xef, 0xbb, 0xbf]) {
            content.drain(..3);
            text_start = text_start.saturating_add(3);
        }
        let text = std::str::from_utf8(&content).map_err(|error| {
            HcdError::Unsupported(format!(
                "TXT HCD import currently requires UTF-8 input at line {ordinal}: {error}"
            ))
        })?;
        chunks.push(TextNode {
            text,
            text_ordinal: ordinal,
            source_start: text_start,
            source_end: line.content_end,
            node_kind: "text-line",
            paragraph_id: Some(format!("line-{ordinal}")),
            wrapper: "p",
        })?;
    }
    if ordinal == 0 {
        chunks.push(TextNode {
            text: "",
            text_ordinal: 1,
            source_start: 0,
            source_end: 0,
            node_kind: "text-line",
            paragraph_id: Some("line-1".to_string()),
            wrapper: "p",
        })?;
    }
    chunks.flush()?;

    let mut manifest = base_manifest(options, "txt", "semantic-flow", source_hash, source_size);
    manifest.fidelity = Some(FidelityReport {
        schema_version: HCD_SCHEMA_VERSION.to_string(),
        level: FidelityLevel::Semantic,
        preserved: vec![
            "UTF-8 line text, empty lines, source order, BOM and original line terminators"
                .to_string(),
            "the immutable TXT source as the source-backed export boundary".to_string(),
        ],
        flattened: vec![
            "plain text lines are represented as canonical HTML paragraphs".to_string(),
        ],
        dropped: Vec::new(),
        warnings: Vec::new(),
    });
    finish_import(writer, manifest, emit)
}

pub(crate) fn export_html(
    bundle: &Bundle,
    source: &Path,
    target: &Path,
    options: &ExportOptions,
) -> Result<FidelityReport, HcdError> {
    export_textual(bundle, source, target, options, "html", HTML_PART, |text| {
        escape_text(text).into_bytes()
    })
}

pub(crate) fn export_text(
    bundle: &Bundle,
    source: &Path,
    target: &Path,
    options: &ExportOptions,
) -> Result<FidelityReport, HcdError> {
    export_textual(bundle, source, target, options, "txt", TEXT_PART, |text| {
        text.as_bytes().to_vec()
    })
}

pub(crate) fn export_markdown(
    bundle: &Bundle,
    source: &Path,
    target: &Path,
    options: &ExportOptions,
) -> Result<FidelityReport, HcdError> {
    export_textual(
        bundle,
        source,
        target,
        options,
        "md",
        MARKDOWN_PART,
        |text| escape_markdown_text(text).into_bytes(),
    )
}

fn escape_markdown_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(
            character,
            '\\' | '*' | '_' | '~' | '`' | '[' | ']' | '<' | '>'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn export_textual(
    bundle: &Bundle,
    source: &Path,
    target: &Path,
    options: &ExportOptions,
    format: &str,
    source_part: &str,
    encode: impl Fn(&str) -> Vec<u8>,
) -> Result<FidelityReport, HcdError> {
    if target.exists() {
        return Err(HcdError::InvalidBundle(format!(
            "target already exists: {}",
            target.display()
        )));
    }
    validate_target_extension(target, format)?;
    let (manifest, _, dirty_parts, dirty_node_ids) = checked_export_state(bundle, source, options)?;
    if manifest.source.format != format {
        return Err(HcdError::InvalidBundle(format!(
            "bundle source format is {}, expected {format}",
            manifest.source.format
        )));
    }
    let nodes = collect_dirty_nodes(bundle, &manifest, &dirty_parts, &dirty_node_ids)?;
    let mut replacements = Vec::with_capacity(nodes.len());
    for node in nodes {
        if node.source.part != source_part {
            return Err(HcdError::InvalidBundle(format!(
                "{} node {} maps to unexpected source part {}",
                format, node.node_id, node.source.part
            )));
        }
        let (start, end) = parse_source_range(node.source.text_id.as_deref(), &node.node_id)?;
        replacements.push((start, end, encode(&node.text), node.node_id));
    }
    replacements.sort_by_key(|(start, end, _, _)| (*start, *end));
    validate_replacements(&replacements, manifest.source.size_bytes)?;

    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let suffix = format!(".{format}");
    let mut temporary = tempfile::Builder::new()
        .prefix(".officecli-hcd-text-")
        .suffix(&suffix)
        .tempfile_in(parent)?;
    {
        let source_file = File::open(source)?;
        let mut input = BufReader::with_capacity(64 * 1024, source_file);
        let mut output = BufWriter::with_capacity(64 * 1024, temporary.as_file_mut());
        let mut source_position = 0u64;
        for (start, end, replacement, _) in &replacements {
            copy_exact_bytes(&mut input, &mut output, start - source_position)?;
            input.seek(SeekFrom::Start(*end))?;
            output.write_all(replacement)?;
            source_position = *end;
        }
        std::io::copy(&mut input, &mut output)?;
        output.flush()?;
    }
    temporary.as_file().sync_all()?;

    let level = if replacements.is_empty() {
        FidelityLevel::Exact
    } else {
        FidelityLevel::High
    };
    let mut warnings = Vec::new();
    if format == "html" {
        warnings.push(FidelityWarning {
            code: "HTML_SOURCE_ACTIVE_CONTENT_PRESERVED".to_string(),
            message: "source-backed HTML export preserves original markup, CSS and active content outside edited text ranges; treat the exported file as untrusted HTML".to_string(),
            node_id: None,
            source_part: Some(source_part.to_string()),
        });
    }
    let report = FidelityReport {
        schema_version: HCD_SCHEMA_VERSION.to_string(),
        level,
        preserved: vec![
            "all immutable source bytes outside explicitly edited text ranges".to_string(),
            if format == "html" {
                "source markup, CSS, attributes and inactive/active opaque content".to_string()
            } else {
                "UTF-8 BOM and original line terminators".to_string()
            },
        ],
        flattened: if replacements.is_empty() {
            Vec::new()
        } else {
            match format {
                "html" => vec![
                    "edited HTML text ranges are serialized with safe entity escaping"
                        .to_string(),
                ],
                "md" => vec![
                    "edited Markdown nodes are serialized as escaped plain Markdown text; original inline delimiters inside that dirty range are not retained"
                        .to_string(),
                ],
                _ => Vec::new(),
            }
        },
        dropped: Vec::new(),
        warnings,
    };
    write_fidelity_report(options, &report)?;
    temporary
        .persist(target)
        .map_err(|error| HcdError::Io(error.error))?;
    Ok(report)
}

fn validate_target_extension(target: &Path, format: &str) -> Result<(), HcdError> {
    let extension = target
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let valid = match format {
        "html" => matches!(extension.as_str(), "html" | "htm"),
        "md" => matches!(extension.as_str(), "md" | "markdown"),
        _ => extension == format,
    };
    if valid {
        Ok(())
    } else {
        Err(HcdError::Unsupported(format!(
            "{format} HCD export requires a .{format} target, found .{extension}"
        )))
    }
}

fn parse_source_range(value: Option<&str>, node_id: &str) -> Result<(u64, u64), HcdError> {
    let value = value.ok_or_else(|| {
        HcdError::InvalidBundle(format!("node {node_id} has no source byte range"))
    })?;
    let range = value.strip_prefix(SOURCE_RANGE_PREFIX).ok_or_else(|| {
        HcdError::InvalidBundle(format!(
            "node {node_id} has invalid source byte range {value}"
        ))
    })?;
    let (start, end) = range.split_once(':').ok_or_else(|| {
        HcdError::InvalidBundle(format!(
            "node {node_id} has invalid source byte range {value}"
        ))
    })?;
    let start = start.parse::<u64>().map_err(|_| {
        HcdError::InvalidBundle(format!(
            "node {node_id} has invalid source byte range {value}"
        ))
    })?;
    let end = end.parse::<u64>().map_err(|_| {
        HcdError::InvalidBundle(format!(
            "node {node_id} has invalid source byte range {value}"
        ))
    })?;
    Ok((start, end))
}

fn validate_replacements(
    replacements: &[(u64, u64, Vec<u8>, String)],
    source_size: u64,
) -> Result<(), HcdError> {
    let mut previous_end = 0u64;
    for (start, end, _, node_id) in replacements {
        if start > end || *end > source_size || *start < previous_end {
            return Err(HcdError::InvalidBundle(format!(
                "node {node_id} has overlapping or out-of-bounds source byte range {start}:{end}"
            )));
        }
        previous_end = *end;
    }
    Ok(())
}

fn copy_exact_bytes(
    reader: &mut impl Read,
    writer: &mut impl Write,
    mut remaining: u64,
) -> Result<(), HcdError> {
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        let wanted = remaining.min(buffer.len() as u64) as usize;
        let count = reader.read(&mut buffer[..wanted])?;
        if count == 0 {
            return Err(HcdError::InvalidBundle(
                "source ended before an HCD source byte range".to_string(),
            ));
        }
        writer.write_all(&buffer[..count])?;
        remaining -= count as u64;
    }
    Ok(())
}

#[derive(Debug)]
struct BoundedLine {
    content_start: u64,
    content_end: u64,
    next_offset: u64,
    content: Vec<u8>,
}

fn read_bounded_line(
    reader: &mut impl BufRead,
    start_offset: u64,
) -> Result<Option<BoundedLine>, HcdError> {
    let mut raw = Vec::with_capacity(8 * 1024);
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if raw.is_empty() {
                return Ok(None);
            }
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(available.len(), |position| position + 1);
        if raw.len().saturating_add(count) > MAX_CHUNK_BYTES.saturating_add(2) {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: TXT line starting at byte {start_offset} exceeds {MAX_CHUNK_BYTES} bytes"
            )));
        }
        raw.extend_from_slice(&available[..count]);
        reader.consume(count);
        if newline.is_some() {
            break;
        }
    }
    let next_offset = start_offset.saturating_add(raw.len() as u64);
    let mut content_length = raw.len();
    if raw.last() == Some(&b'\n') {
        content_length -= 1;
        if content_length > 0 && raw[content_length - 1] == b'\r' {
            content_length -= 1;
        }
    }
    raw.truncate(content_length);
    Ok(Some(BoundedLine {
        content_start: start_offset,
        content_end: start_offset.saturating_add(content_length as u64),
        next_offset,
        content: raw,
    }))
}

struct HtmlTextSegment {
    start: usize,
    end: usize,
    text: String,
}

struct HtmlTag {
    name: String,
    attributes: HashMap<String, String>,
    closing: bool,
    self_closing: bool,
}

enum HtmlScanEvent {
    Tag(HtmlTag),
    Text(HtmlTextSegment),
}

#[derive(Clone, Copy)]
struct HtmlTablePlan {
    rows: usize,
    columns: usize,
}

struct HtmlTablePlanBuilder {
    rows: usize,
    columns: usize,
    current_columns: usize,
    row_open: bool,
    nested_depth: usize,
    cells: usize,
}

impl HtmlTablePlanBuilder {
    fn new() -> Self {
        Self {
            rows: 0,
            columns: 0,
            current_columns: 0,
            row_open: false,
            nested_depth: 0,
            cells: 0,
        }
    }

    fn start_row(&mut self) -> Result<(), HcdError> {
        self.finish_row();
        if self.rows >= HTML_TABLE_MAX_ROWS {
            return Err(HcdError::ResourceLimit(format!(
                "HTML table exceeds {HTML_TABLE_MAX_ROWS} rows"
            )));
        }
        self.rows += 1;
        self.current_columns = 0;
        self.row_open = true;
        Ok(())
    }

    fn add_cell(&mut self, tag: &HtmlTag) -> Result<(), HcdError> {
        if !self.row_open {
            self.start_row()?;
        }
        let colspan = bounded_html_span(tag, "colspan", HTML_TABLE_MAX_COLUMNS)?;
        self.current_columns = self.current_columns.checked_add(colspan).ok_or_else(|| {
            HcdError::ResourceLimit("HTML table column count overflowed".to_string())
        })?;
        if self.current_columns > HTML_TABLE_MAX_COLUMNS {
            return Err(HcdError::ResourceLimit(format!(
                "HTML table exceeds {HTML_TABLE_MAX_COLUMNS} columns"
            )));
        }
        self.cells = self.cells.checked_add(1).ok_or_else(|| {
            HcdError::ResourceLimit("HTML table cell count overflowed".to_string())
        })?;
        if self.cells > HTML_TABLE_MAX_CELLS {
            return Err(HcdError::ResourceLimit(format!(
                "HTML table exceeds {HTML_TABLE_MAX_CELLS} cells"
            )));
        }
        Ok(())
    }

    fn finish_row(&mut self) {
        if self.row_open {
            self.columns = self.columns.max(self.current_columns.max(1));
            self.row_open = false;
        }
    }

    fn finish(mut self) -> HtmlTablePlan {
        self.finish_row();
        HtmlTablePlan {
            rows: self.rows,
            columns: self.columns,
        }
    }
}

fn plan_html_tables(source: &str) -> Result<Vec<HtmlTablePlan>, HcdError> {
    let mut plans = Vec::new();
    let mut current: Option<HtmlTablePlanBuilder> = None;
    scan_html(source, |event| {
        let HtmlScanEvent::Tag(tag) = event else {
            return Ok(());
        };
        if tag.name == "table" {
            if tag.closing {
                if let Some(table) = current.as_mut() {
                    if table.nested_depth > 0 {
                        table.nested_depth -= 1;
                    } else {
                        let table = current.take().expect("checked table");
                        plans.push(table.finish());
                    }
                }
            } else if let Some(table) = current.as_mut() {
                table.nested_depth = table.nested_depth.saturating_add(1);
            } else {
                current = Some(HtmlTablePlanBuilder::new());
            }
            return Ok(());
        }
        let Some(table) = current.as_mut().filter(|table| table.nested_depth == 0) else {
            return Ok(());
        };
        match (tag.closing, tag.name.as_str()) {
            (false, "tr") => table.start_row()?,
            (true, "tr") => table.finish_row(),
            (false, "td" | "th") => table.add_cell(&tag)?,
            _ => {}
        }
        Ok(())
    })?;
    if let Some(table) = current {
        plans.push(table.finish());
    }
    Ok(plans)
}

struct HtmlSemanticBlock {
    source_tag: String,
    closing_html: String,
    node_kind: &'static str,
    paragraph_id: String,
    preserve_whitespace: bool,
    html: String,
    entries: Vec<NodeMapEntry>,
}

struct HtmlListContext {
    ordered: bool,
    next: usize,
}

struct HtmlTableRow {
    html: String,
    entries: Vec<NodeMapEntry>,
    columns: usize,
    cells: usize,
}

struct HtmlTableCell {
    closing_tag: &'static str,
    html: String,
    entries: Vec<NodeMapEntry>,
}

struct StructuredHtmlTable {
    plan: HtmlTablePlan,
    table_node_id: String,
    table_ordinal: usize,
    fragment_ordinal: usize,
    fragment_start_row: usize,
    fragment_rows: usize,
    fragment_html: String,
    fragment_entries: Vec<NodeMapEntry>,
    total_rows: usize,
    total_cells: usize,
    max_columns: usize,
    row: Option<HtmlTableRow>,
    cell: Option<HtmlTableCell>,
    nested_depth: usize,
}

impl StructuredHtmlTable {
    fn new(document_id: &str, table_ordinal: usize, plan: HtmlTablePlan) -> Self {
        let ordinal = table_ordinal.to_string();
        Self {
            plan,
            table_node_id: stable_node_id(&[document_id, HTML_PART, "html-table", &ordinal]),
            table_ordinal,
            fragment_ordinal: 0,
            fragment_start_row: 1,
            fragment_rows: 0,
            fragment_html: String::new(),
            fragment_entries: Vec::new(),
            total_rows: 0,
            total_cells: 0,
            max_columns: 0,
            row: None,
            cell: None,
            nested_depth: 0,
        }
    }

    fn start_row<F>(&mut self, chunks: &mut FlatChunkWriter<'_, F>) -> Result<(), HcdError>
    where
        F: FnMut(&ImportEvent) -> Result<(), HcdError>,
    {
        self.finish_row(chunks)?;
        self.row = Some(HtmlTableRow {
            html: "<tr>".to_string(),
            entries: Vec::new(),
            columns: 0,
            cells: 0,
        });
        Ok(())
    }

    fn start_cell(&mut self, tag: &HtmlTag) -> Result<(), HcdError> {
        self.finish_cell();
        if self.row.is_none() {
            self.row = Some(HtmlTableRow {
                html: "<tr>".to_string(),
                entries: Vec::new(),
                columns: 0,
                cells: 0,
            });
        }
        let colspan = bounded_html_span(tag, "colspan", HTML_TABLE_MAX_COLUMNS)?;
        let rowspan = bounded_html_span(tag, "rowspan", HTML_TABLE_MAX_ROWS)?;
        let row = self.row.as_mut().expect("created row");
        row.columns = row.columns.checked_add(colspan).ok_or_else(|| {
            HcdError::ResourceLimit("HTML table column count overflowed".to_string())
        })?;
        row.cells += 1;
        let canonical_tag = if tag.name == "th" { "th" } else { "td" };
        let mut html = format!("<{canonical_tag}");
        if colspan > 1 {
            html.push_str(&format!(" colspan=\"{colspan}\""));
        }
        if rowspan > 1 {
            html.push_str(&format!(" rowspan=\"{rowspan}\""));
        }
        html.push('>');
        self.cell = Some(HtmlTableCell {
            closing_tag: if canonical_tag == "th" {
                "</th>"
            } else {
                "</td>"
            },
            html,
            entries: Vec::new(),
        });
        Ok(())
    }

    fn push_text(&mut self, html: String, entry: NodeMapEntry) {
        if let Some(cell) = &mut self.cell {
            cell.html.push_str(&html);
            cell.entries.push(entry);
        }
    }

    fn push_break(&mut self) {
        if let Some(cell) = &mut self.cell {
            cell.html.push_str("<br/>");
        }
    }

    fn finish_cell(&mut self) {
        let Some(mut cell) = self.cell.take() else {
            return;
        };
        cell.html.push_str(cell.closing_tag);
        if let Some(row) = &mut self.row {
            row.html.push_str(&cell.html);
            row.entries.extend(cell.entries);
        }
    }

    fn finish_row<F>(&mut self, chunks: &mut FlatChunkWriter<'_, F>) -> Result<(), HcdError>
    where
        F: FnMut(&ImportEvent) -> Result<(), HcdError>,
    {
        self.finish_cell();
        let Some(mut row) = self.row.take() else {
            return Ok(());
        };
        if row.cells == 0 {
            row.html.push_str("<td></td>");
            row.columns = 1;
            row.cells = 1;
        }
        row.html.push_str("</tr>");
        if row.html.len() > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: HTML table {} row {} exceeds {MAX_CHUNK_BYTES} bytes",
                self.table_ordinal,
                self.total_rows + 1
            )));
        }
        if self.fragment_rows > 0
            && (self.fragment_rows >= HTML_TABLE_ROWS_PER_FRAGMENT
                || self.fragment_html.len().saturating_add(row.html.len()) > chunks.soft_bytes)
        {
            self.flush_fragment(chunks, false)?;
        }
        self.total_rows += 1;
        self.total_cells = self.total_cells.saturating_add(row.cells);
        self.max_columns = self.max_columns.max(row.columns);
        if self.total_rows > HTML_TABLE_MAX_ROWS
            || self.total_cells > HTML_TABLE_MAX_CELLS
            || self.max_columns > HTML_TABLE_MAX_COLUMNS
        {
            return Err(HcdError::ResourceLimit(
                "HTML table exceeds its row, column, or cell budget".to_string(),
            ));
        }
        self.fragment_rows += 1;
        self.fragment_html.push_str(&row.html);
        self.fragment_entries.extend(row.entries);
        if self.fragment_html.len() > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: HTML table {} fragment exceeds {MAX_CHUNK_BYTES} bytes",
                self.table_ordinal
            )));
        }
        if self.fragment_rows >= HTML_TABLE_ROWS_PER_FRAGMENT && self.total_rows < self.plan.rows {
            self.flush_fragment(chunks, false)?;
        }
        Ok(())
    }

    fn finish<F>(mut self, chunks: &mut FlatChunkWriter<'_, F>) -> Result<(), HcdError>
    where
        F: FnMut(&ImportEvent) -> Result<(), HcdError>,
    {
        self.finish_row(chunks)?;
        if self.total_rows != self.plan.rows || self.max_columns != self.plan.columns {
            return Err(HcdError::InvalidBundle(format!(
                "HTML table {} changed between planning and canonicalization",
                self.table_ordinal
            )));
        }
        self.flush_fragment(chunks, true)
    }

    fn flush_fragment<F>(
        &mut self,
        chunks: &mut FlatChunkWriter<'_, F>,
        final_fragment: bool,
    ) -> Result<(), HcdError>
    where
        F: FnMut(&ImportEvent) -> Result<(), HcdError>,
    {
        if self.fragment_rows == 0 && !(final_fragment && self.fragment_ordinal == 0) {
            return Ok(());
        }
        let row_start = if self.fragment_rows == 0 {
            0
        } else {
            self.fragment_start_row
        };
        let row_end = if self.fragment_rows == 0 {
            0
        } else {
            row_start + self.fragment_rows - 1
        };
        let mut attributes = format!(
            " data-hcd-table-node-id=\"{}\" data-hcd-table-fragment=\"{}\" data-hcd-row-start=\"{row_start}\" data-hcd-row-end=\"{row_end}\" data-hcd-fragment-row-count=\"{}\" data-hcd-column-count=\"{}\"",
            self.table_node_id, self.fragment_ordinal, self.fragment_rows, self.plan.columns
        );
        if self.fragment_ordinal > 0 {
            attributes.push_str(" data-hcd-table-continuation=\"true\"");
        }
        if final_fragment {
            attributes.push_str(&format!(
                " data-hcd-table-final=\"true\" data-hcd-row-count=\"{}\"",
                self.plan.rows
            ));
        }
        let html = format!(
            "<table class=\"hcd-source-table\"{attributes}><tbody>{}</tbody></table>",
            std::mem::take(&mut self.fragment_html)
        );
        let entries = std::mem::take(&mut self.fragment_entries);
        chunks.push_rendered(RenderedSourceBlock { html, entries }, true)?;
        self.fragment_ordinal += 1;
        self.fragment_start_row = row_end.saturating_add(1);
        self.fragment_rows = 0;
        Ok(())
    }
}

struct StructuredHtmlCanonicalizer<'a, 'w, F>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    document_id: &'a str,
    chunks: &'a mut FlatChunkWriter<'w, F>,
    table_plans: std::vec::IntoIter<HtmlTablePlan>,
    block: Option<HtmlSemanticBlock>,
    table: Option<StructuredHtmlTable>,
    lists: Vec<HtmlListContext>,
    text_ordinal: u64,
    block_ordinal: usize,
    table_ordinal: usize,
}

impl<'a, 'w, F> StructuredHtmlCanonicalizer<'a, 'w, F>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    fn new(
        document_id: &'a str,
        chunks: &'a mut FlatChunkWriter<'w, F>,
        plans: Vec<HtmlTablePlan>,
    ) -> Self {
        Self {
            document_id,
            chunks,
            table_plans: plans.into_iter(),
            block: None,
            table: None,
            lists: Vec::new(),
            text_ordinal: 0,
            block_ordinal: 0,
            table_ordinal: 0,
        }
    }

    fn event(&mut self, event: HtmlScanEvent) -> Result<(), HcdError> {
        match event {
            HtmlScanEvent::Text(segment) => self.text(segment),
            HtmlScanEvent::Tag(tag) => self.tag(tag),
        }
    }

    fn text(&mut self, segment: HtmlTextSegment) -> Result<(), HcdError> {
        if segment.text.is_empty() {
            return Ok(());
        }
        if self.table.is_some() {
            if self
                .table
                .as_ref()
                .is_some_and(|table| table.cell.is_none())
            {
                return Ok(());
            }
            self.text_ordinal = self.text_ordinal.saturating_add(1);
            let paragraph_id = self.table.as_ref().and_then(|table| {
                table.row.as_ref().map(|row| {
                    format!(
                        "table-{}-row-{}-cell-{}",
                        table.table_ordinal,
                        table.total_rows + 1,
                        row.cells
                    )
                })
            });
            let (html, entry) = render_source_text_node(
                self.document_id,
                HTML_PART,
                &segment.text,
                self.text_ordinal,
                segment.start as u64,
                segment.end as u64,
                "html-table-cell-text",
                paragraph_id,
            )?;
            self.table
                .as_mut()
                .expect("checked table")
                .push_text(html, entry);
            return Ok(());
        }
        if self.block.is_none() {
            if !segment
                .text
                .chars()
                .any(|character| !character.is_whitespace())
            {
                return Ok(());
            }
            self.start_block(
                "__implicit",
                "<p class=\"hcd-source-block hcd-html-text\">".to_string(),
                "</p>".to_string(),
                "html-text",
                false,
            )?;
        }
        let block = self.block.as_mut().expect("created block");
        if !block.preserve_whitespace
            && !segment
                .text
                .chars()
                .any(|character| !character.is_whitespace())
            && block.entries.is_empty()
        {
            return Ok(());
        }
        self.text_ordinal = self.text_ordinal.saturating_add(1);
        let (html, entry) = render_source_text_node(
            self.document_id,
            HTML_PART,
            &segment.text,
            self.text_ordinal,
            segment.start as u64,
            segment.end as u64,
            block.node_kind,
            Some(block.paragraph_id.clone()),
        )?;
        block.html.push_str(&html);
        block.entries.push(entry);
        if block.html.len() > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: structured HTML {} block exceeds {MAX_CHUNK_BYTES} bytes",
                block.source_tag
            )));
        }
        Ok(())
    }

    fn tag(&mut self, tag: HtmlTag) -> Result<(), HcdError> {
        if self.table.is_some() {
            return self.table_tag(tag);
        }
        if tag.name == "table" && !tag.closing {
            self.finish_block()?;
            self.table_ordinal += 1;
            let plan = self
                .table_plans
                .next()
                .ok_or_else(|| HcdError::InvalidBundle("HTML table plan is missing".to_string()))?;
            self.table = Some(StructuredHtmlTable::new(
                self.document_id,
                self.table_ordinal,
                plan,
            ));
            return Ok(());
        }
        match (tag.closing, tag.name.as_str()) {
            (false, "title") => self.start_simple_block("title", "p", "title", false),
            (false, name @ ("h1" | "h2" | "h3" | "h4" | "h5" | "h6")) => {
                self.start_simple_block(name, name, "heading", false)
            }
            (false, "p") => self.start_simple_block("p", "p", "paragraph", false),
            (false, "pre") => self.start_simple_block("pre", "pre", "pre", true),
            (false, "blockquote") => {
                self.start_simple_block("blockquote", "blockquote", "quote", false)
            }
            (false, "ul") => {
                self.finish_implicit_block()?;
                self.lists.push(HtmlListContext {
                    ordered: false,
                    next: 1,
                });
                Ok(())
            }
            (false, "ol") => {
                self.finish_implicit_block()?;
                let start = optional_bounded_positive_attribute(
                    &tag.attributes,
                    "start",
                    HTML_TABLE_MAX_ROWS,
                )?
                .unwrap_or(1);
                self.lists.push(HtmlListContext {
                    ordered: true,
                    next: start,
                });
                Ok(())
            }
            (false, "li") => self.start_list_item(),
            (false, "br") => {
                if let Some(block) = &mut self.block {
                    block.html.push_str("<br/>");
                }
                Ok(())
            }
            (true, "ul" | "ol") => {
                self.finish_block()?;
                self.lists.pop();
                Ok(())
            }
            (
                true,
                "title" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "pre" | "blockquote"
                | "li",
            ) => self.finish_block(),
            (
                _,
                "body" | "main" | "section" | "article" | "header" | "footer" | "address"
                | "figure" | "figcaption",
            ) => self.finish_implicit_block(),
            _ => Ok(()),
        }
    }

    fn table_tag(&mut self, tag: HtmlTag) -> Result<(), HcdError> {
        let table = self.table.as_mut().expect("checked table");
        if tag.name == "table" {
            if tag.closing {
                if table.nested_depth > 0 {
                    table.nested_depth -= 1;
                    return Ok(());
                }
                let table = self.table.take().expect("checked table");
                return table.finish(self.chunks);
            }
            table.nested_depth = table.nested_depth.saturating_add(1);
            return Ok(());
        }
        if table.nested_depth > 0 {
            return Ok(());
        }
        match (tag.closing, tag.name.as_str()) {
            (false, "tr") => table.start_row(self.chunks),
            (true, "tr") => table.finish_row(self.chunks),
            (false, "td" | "th") => table.start_cell(&tag),
            (true, "td" | "th") => {
                table.finish_cell();
                Ok(())
            }
            (false, "br") => {
                table.push_break();
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn start_simple_block(
        &mut self,
        source_tag: &str,
        canonical_tag: &str,
        node_kind: &'static str,
        preserve_whitespace: bool,
    ) -> Result<(), HcdError> {
        self.start_block(
            source_tag,
            format!("<{canonical_tag} class=\"hcd-source-block hcd-{node_kind}\">"),
            format!("</{canonical_tag}>"),
            node_kind,
            preserve_whitespace,
        )
    }

    fn start_list_item(&mut self) -> Result<(), HcdError> {
        self.finish_block()?;
        let (ordered, ordinal) = self
            .lists
            .last_mut()
            .map(|list| {
                let ordinal = list.next;
                list.next = list.next.saturating_add(1);
                (list.ordered, ordinal)
            })
            .unwrap_or((false, 1));
        let (opening, closing) = if ordered {
            (
                format!("<ol start=\"{ordinal}\" class=\"hcd-source-list\"><li>"),
                "</li></ol>".to_string(),
            )
        } else {
            (
                "<ul class=\"hcd-source-list\"><li>".to_string(),
                "</li></ul>".to_string(),
            )
        };
        self.start_block("li", opening, closing, "list-item", false)
    }

    fn start_block(
        &mut self,
        source_tag: &str,
        opening_html: String,
        closing_html: String,
        node_kind: &'static str,
        preserve_whitespace: bool,
    ) -> Result<(), HcdError> {
        self.finish_block()?;
        self.block_ordinal += 1;
        self.block = Some(HtmlSemanticBlock {
            source_tag: source_tag.to_string(),
            closing_html,
            node_kind,
            paragraph_id: format!("html-block-{}", self.block_ordinal),
            preserve_whitespace,
            html: opening_html,
            entries: Vec::new(),
        });
        Ok(())
    }

    fn finish_implicit_block(&mut self) -> Result<(), HcdError> {
        if self
            .block
            .as_ref()
            .is_some_and(|block| block.source_tag == "__implicit")
        {
            self.finish_block()?;
        }
        Ok(())
    }

    fn finish_block(&mut self) -> Result<(), HcdError> {
        let Some(mut block) = self.block.take() else {
            return Ok(());
        };
        if block.entries.is_empty() {
            return Ok(());
        }
        block.html.push_str(&block.closing_html);
        self.chunks.push_rendered(
            RenderedSourceBlock {
                html: block.html,
                entries: block.entries,
            },
            false,
        )
    }

    fn finish(mut self) -> Result<u64, HcdError> {
        if let Some(table) = self.table.take() {
            table.finish(self.chunks)?;
        }
        self.finish_block()?;
        if self.table_plans.next().is_some() {
            return Err(HcdError::InvalidBundle(
                "HTML table plan was not consumed".to_string(),
            ));
        }
        Ok(self.text_ordinal)
    }
}

fn scan_html(
    source: &str,
    mut emit: impl FnMut(HtmlScanEvent) -> Result<(), HcdError>,
) -> Result<(), HcdError> {
    let mut cursor = 0usize;
    let mut suppressed: Option<String> = None;
    let mut element_stack: Vec<String> = Vec::new();
    let mut element_count = 0usize;
    while cursor < source.len() {
        if let Some(tag) = suppressed.clone() {
            let closing = format!("</{tag}");
            let Some(relative) =
                find_ascii_case_insensitive(&source.as_bytes()[cursor..], closing.as_bytes())
            else {
                break;
            };
            let start = cursor + relative;
            let Some(end) = find_tag_end(source.as_bytes(), start + 1) else {
                break;
            };
            suppressed = None;
            cursor = end + 1;
            continue;
        }
        let Some(relative) = source[cursor..].find('<') else {
            emit_html_text(source, cursor, source.len(), &mut emit)?;
            break;
        };
        let start = cursor + relative;
        emit_html_text(source, cursor, start, &mut emit)?;
        if source[start..].starts_with("<!--") {
            if let Some(end) = source[start + 4..].find("-->") {
                cursor = start + 4 + end + 3;
                continue;
            }
            break;
        }
        let Some(end) = find_tag_end(source.as_bytes(), start + 1) else {
            emit_html_text(source, start, source.len(), &mut emit)?;
            break;
        };
        if end.saturating_sub(start) > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "HTML tag at byte {start} exceeds {MAX_CHUNK_BYTES} bytes"
            )));
        }
        if let Some(tag) = parse_html_tag(&source[start + 1..end]) {
            element_count = element_count.saturating_add(1);
            if element_count > HTML_SOURCE_MAX_ELEMENTS {
                return Err(HcdError::ResourceLimit(format!(
                    "HTML source exceeds {HTML_SOURCE_MAX_ELEMENTS} elements"
                )));
            }
            if is_suppressed_html_element(&tag.name) && !tag.closing && !tag.self_closing {
                suppressed = Some(tag.name);
                cursor = end + 1;
                continue;
            }
            if tag.closing {
                if let Some(position) = element_stack.iter().rposition(|name| name == &tag.name) {
                    element_stack.truncate(position);
                }
            } else if !tag.self_closing && !is_void_html_element(&tag.name) {
                if element_stack.len() >= HTML_SOURCE_MAX_DEPTH {
                    return Err(HcdError::ResourceLimit(format!(
                        "HTML source exceeds nesting depth {HTML_SOURCE_MAX_DEPTH}"
                    )));
                }
                element_stack.push(tag.name.clone());
            }
            emit(HtmlScanEvent::Tag(tag))?;
        }
        cursor = end + 1;
    }
    Ok(())
}

fn emit_html_text(
    source: &str,
    start: usize,
    end: usize,
    emit: &mut impl FnMut(HtmlScanEvent) -> Result<(), HcdError>,
) -> Result<(), HcdError> {
    if start < end {
        emit(HtmlScanEvent::Text(HtmlTextSegment {
            start,
            end,
            text: decode_entities(&source[start..end]),
        }))?;
    }
    Ok(())
}

fn parse_html_tag(raw: &str) -> Option<HtmlTag> {
    let mut content = raw.trim();
    if content.is_empty() || content.starts_with('!') || content.starts_with('?') {
        return None;
    }
    let closing = content.starts_with('/');
    if closing {
        content = content[1..].trim_start();
    }
    let self_closing = content.trim_end().ends_with('/');
    let name_end = content
        .find(|character: char| !(character.is_ascii_alphanumeric() || character == '-'))
        .unwrap_or(content.len());
    let name = content[..name_end].to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    Some(HtmlTag {
        name,
        attributes: if closing {
            HashMap::new()
        } else {
            parse_html_attributes(&content[name_end..])
        },
        closing,
        self_closing,
    })
}

fn parse_html_attributes(source: &str) -> HashMap<String, String> {
    let bytes = source.as_bytes();
    let mut attributes = HashMap::new();
    let mut index = 0usize;
    while index < bytes.len() {
        while index < bytes.len() && (bytes[index].is_ascii_whitespace() || bytes[index] == b'/') {
            index += 1;
        }
        let start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && !matches!(bytes[index], b'=' | b'/')
        {
            index += 1;
        }
        if start == index {
            break;
        }
        let name = source[start..index].to_ascii_lowercase();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let mut value = String::new();
        if index < bytes.len() && bytes[index] == b'=' {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if index < bytes.len() && matches!(bytes[index], b'\'' | b'"') {
                let quote = bytes[index];
                index += 1;
                let value_start = index;
                while index < bytes.len() && bytes[index] != quote {
                    index += 1;
                }
                value = decode_entities(&source[value_start..index]);
                index = (index + 1).min(bytes.len());
            } else {
                let value_start = index;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && bytes[index] != b'/'
                {
                    index += 1;
                }
                value = decode_entities(&source[value_start..index]);
            }
        }
        attributes.insert(name, value);
    }
    attributes
}

fn bounded_html_span(tag: &HtmlTag, name: &str, maximum: usize) -> Result<usize, HcdError> {
    optional_bounded_positive_attribute(&tag.attributes, name, maximum)
        .map(|value| value.unwrap_or(1))
}

fn optional_bounded_positive_attribute(
    attributes: &HashMap<String, String>,
    name: &str,
    maximum: usize,
) -> Result<Option<usize>, HcdError> {
    let Some(value) = attributes.get(name) else {
        return Ok(None);
    };
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(HcdError::InvalidBundle(format!(
            "HTML {name} must be a positive canonical integer"
        )));
    }
    let parsed = value
        .parse::<usize>()
        .ok()
        .filter(|value| (1..=maximum).contains(value))
        .ok_or_else(|| HcdError::ResourceLimit(format!("HTML {name} must be in 1..={maximum}")))?;
    Ok(Some(parsed))
}

fn is_suppressed_html_element(name: &str) -> bool {
    matches!(
        name,
        "script" | "style" | "iframe" | "object" | "embed" | "template" | "noscript"
    )
}

fn is_void_html_element(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn decode_entities(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(position) = rest.find('&') {
        output.push_str(&rest[..position]);
        let after = &rest[position + 1..];
        let Some(end) = after.find(';').filter(|end| *end <= 32) else {
            output.push('&');
            rest = after;
            continue;
        };
        let entity = &after[..end];
        if let Some(character) = decode_entity(entity) {
            output.push(character);
        } else {
            output.push('&');
            output.push_str(entity);
            output.push(';');
        }
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    output
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" | "#39" => Some('\''),
        "nbsp" => Some('\u{00a0}'),
        _ if entity.starts_with("#x") || entity.starts_with("#X") => {
            u32::from_str_radix(&entity[2..], 16)
                .ok()
                .and_then(char::from_u32)
        }
        _ if entity.starts_with('#') => entity[1..].parse().ok().and_then(char::from_u32),
        _ => None,
    }
}

fn find_tag_end(bytes: &[u8], mut index: usize) -> Option<usize> {
    let mut quote = None;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' if quote.is_none() => quote = Some(bytes[index]),
            value if quote == Some(value) => quote = None,
            b'>' if quote.is_none() => return Some(index),
            _ => {}
        }
        index += 1;
    }
    None
}

fn find_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hcd_core::{
        apply_patch, extract_text_page, validate_bundle, NodePrecondition, PatchBatch,
        PatchOperation, HCD_PATCH_SCHEMA_VERSION,
    };
    use std::collections::BTreeMap;

    fn options(document_id: &str) -> ImportOptions {
        let mut options = ImportOptions::new(document_id);
        options.chunk_soft_bytes = 256;
        options.chunk_blocks = 2;
        options
    }

    fn chunk_html(bundle: &Bundle) -> Vec<String> {
        let manifest = bundle.manifest().unwrap();
        let mut html = Vec::new();
        for page in 0..manifest.index_page_count {
            let index = bundle.read_index_page(&manifest, page).unwrap();
            html.extend(
                index
                    .chunks
                    .iter()
                    .map(|descriptor| bundle.read_chunk(descriptor).unwrap()),
            );
        }
        html
    }

    #[test]
    fn html_hcd_patch_preserves_markup_and_escapes_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.html");
        let bundle_path = temp.path().join("bundle");
        let target = temp.path().join("target.html");
        std::fs::write(
            &source,
            "<!doctype html><style>.x{color:red}</style><h1>Heading</h1><p class='x'>Secret &amp; 中文</p><ul><li>First</li><li>Second</li></ul><table data-private='drop'><tr><th>Name</th><th>Value</th></tr><tr><td>Account</td><td>6222</td></tr></table><script>alert('kept')</script>",
        )
        .unwrap();
        let mut events = Vec::new();
        import_html(&source, &bundle_path, &options("html-doc"), |event| {
            events.push(event.clone());
            Ok(())
        })
        .unwrap();
        assert!(events
            .iter()
            .any(|event| matches!(event, ImportEvent::ChunkReady { .. })));
        let bundle = Bundle::open(&bundle_path).unwrap();
        let validation = validate_bundle(&bundle).unwrap();
        assert!(validation.valid, "{:?}", validation.issues);
        let canonical_html = chunk_html(&bundle).join("");
        assert!(canonical_html.contains("<h1 class=\"hcd-source-block hcd-heading\">"));
        assert!(canonical_html.contains("<p class=\"hcd-source-block hcd-paragraph\">"));
        assert!(canonical_html.contains("<ul class=\"hcd-source-list\"><li>"));
        assert!(canonical_html.contains("<table class=\"hcd-source-table\""));
        assert!(canonical_html.contains("<th>"));
        assert!(!canonical_html.contains("<style"));
        assert!(!canonical_html.contains("<script"));
        assert!(!canonical_html.contains("alert('kept')"));
        assert!(!canonical_html.contains("data-private"));
        let page = extract_text_page(&bundle, None, 100).unwrap();
        let entry = page
            .entries
            .iter()
            .find(|entry| entry.text == "Secret & 中文")
            .unwrap();
        apply_patch(
            &bundle,
            &PatchBatch {
                schema_version: HCD_PATCH_SCHEMA_VERSION.to_string(),
                document_id: "html-doc".to_string(),
                patch_id: "patch-1".to_string(),
                base_revision: 0,
                actor: BTreeMap::new(),
                operations: vec![PatchOperation::TextSplice {
                    node_id: entry.node_id.clone(),
                    start: 7,
                    delete_count: 1,
                    insert_text: "<masked>".to_string(),
                    precondition: NodePrecondition {
                        node_hash: entry.node_hash.clone(),
                    },
                }],
                metadata: BTreeMap::new(),
            },
            0,
        )
        .unwrap();
        let report = export_html(
            &bundle,
            &source,
            &target,
            &ExportOptions {
                revision: Some(1),
                fidelity_report: None,
            },
        )
        .unwrap();
        assert_eq!(report.level, FidelityLevel::High);
        let output = std::fs::read_to_string(target).unwrap();
        assert!(output.contains("class='x'"));
        assert!(output.contains("Secret &lt;masked&gt; 中文"));
        assert!(output.contains("<script>alert('kept')</script>"));
    }

    #[test]
    fn large_html_table_is_stably_split_into_contiguous_fragments() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("large.html");
        let first_bundle_path = temp.path().join("first-bundle");
        let second_bundle_path = temp.path().join("second-bundle");
        let mut source_html = String::from("<!doctype html><table><tbody>");
        for row in 1..=300 {
            source_html.push_str(&format!("<tr><td>Row {row}</td><td>Value {row}</td></tr>"));
        }
        source_html.push_str("</tbody></table>");
        std::fs::write(&source, source_html).unwrap();
        let mut import_options = ImportOptions::new("large-html-table");
        import_options.chunk_soft_bytes = 512 * 1024;
        import_options.chunk_blocks = 256;
        import_html(&source, &first_bundle_path, &import_options, |_| Ok(())).unwrap();
        import_html(&source, &second_bundle_path, &import_options, |_| Ok(())).unwrap();

        let first = Bundle::open(&first_bundle_path).unwrap();
        let second = Bundle::open(&second_bundle_path).unwrap();
        let validation = validate_bundle(&first).unwrap();
        assert!(validation.valid, "{:?}", validation.issues);
        let fragments = chunk_html(&first);
        assert_eq!(fragments.len(), 3);
        assert!(fragments[0].contains("data-hcd-table-fragment=\"0\""));
        assert!(fragments[0].contains("data-hcd-row-start=\"1\""));
        assert!(fragments[0].contains("data-hcd-row-end=\"128\""));
        assert!(fragments[1].contains("data-hcd-table-fragment=\"1\""));
        assert!(fragments[1].contains("data-hcd-row-start=\"129\""));
        assert!(fragments[1].contains("data-hcd-row-end=\"256\""));
        assert!(fragments[1].contains("data-hcd-table-continuation=\"true\""));
        assert!(fragments[2].contains("data-hcd-table-fragment=\"2\""));
        assert!(fragments[2].contains("data-hcd-row-start=\"257\""));
        assert!(fragments[2].contains("data-hcd-row-end=\"300\""));
        assert!(fragments[2].contains("data-hcd-table-final=\"true\""));
        assert!(fragments[2].contains("data-hcd-row-count=\"300\""));
        assert_eq!(fragments, chunk_html(&second));
    }

    #[test]
    fn txt_hcd_patch_preserves_bom_and_crlf() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.txt");
        let bundle_path = temp.path().join("bundle");
        let target = temp.path().join("target.txt");
        std::fs::write(&source, b"\xef\xbb\xbfSecret 123\r\nSecond\n").unwrap();
        import_text(&source, &bundle_path, &options("txt-doc"), |_| Ok(())).unwrap();
        let bundle = Bundle::open(&bundle_path).unwrap();
        let validation = validate_bundle(&bundle).unwrap();
        assert!(validation.valid, "{:?}", validation.issues);
        let page = extract_text_page(&bundle, None, 100).unwrap();
        let entry = &page.entries[0];
        apply_patch(
            &bundle,
            &PatchBatch {
                schema_version: HCD_PATCH_SCHEMA_VERSION.to_string(),
                document_id: "txt-doc".to_string(),
                patch_id: "patch-1".to_string(),
                base_revision: 0,
                actor: BTreeMap::new(),
                operations: vec![PatchOperation::TextSplice {
                    node_id: entry.node_id.clone(),
                    start: 7,
                    delete_count: 3,
                    insert_text: "***".to_string(),
                    precondition: NodePrecondition {
                        node_hash: entry.node_hash.clone(),
                    },
                }],
                metadata: BTreeMap::new(),
            },
            0,
        )
        .unwrap();
        export_text(
            &bundle,
            &source,
            &target,
            &ExportOptions {
                revision: Some(1),
                fidelity_report: None,
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read(target).unwrap(),
            b"\xef\xbb\xbfSecret ***\r\nSecond\n"
        );
    }

    #[test]
    fn markdown_hcd_renders_semantics_and_source_backed_patch_preserves_other_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.markdown");
        let bundle_path = temp.path().join("bundle");
        let exact_target = temp.path().join("exact.md");
        let patched_target = temp.path().join("patched.md");
        let original = b"\xef\xbb\xbf# Secret **123**\r\n\r\n- First item\n- [ ] Pending task\n- [x] Completed task\n3. Ordered from three\n> [Safe link](https://example.com \"Docs\")\n\n***Bold italic*** and <https://example.org> and <test@example.org>  \n![Diagram](https://example.com/diagram.png \"Diagram title\")\n[Unsafe](javascript:alert(1))\n| Name | Value |\n| --- | --- |\n| User | Zhang |\n```text\ncode <value>\n```\n";
        std::fs::write(&source, original).unwrap();
        import_markdown(&source, &bundle_path, &options("markdown-doc"), |_| Ok(())).unwrap();
        let bundle = Bundle::open(&bundle_path).unwrap();
        let validation = validate_bundle(&bundle).unwrap();
        assert!(validation.valid, "{:?}", validation.issues);
        let html = chunk_html(&bundle).join("");
        assert!(html.contains("<h1 class=\"hcd-source-block hcd-markdown-heading\">"));
        assert!(html.contains("<strong><span"));
        assert!(html.contains(">123</span></strong>"));
        assert!(html.contains("<ul class=\"hcd-markdown-list\">"));
        assert!(html.contains("class=\"hcd-markdown-task\""));
        assert!(html.contains("class=\"hcd-markdown-task hcd-markdown-task-checked\""));
        assert!(html.contains("<ol class=\"hcd-markdown-list\" start=\"3\">"));
        assert!(html.contains("<blockquote class=\"hcd-source-block hcd-markdown-quote\">"));
        assert!(html.contains("href=\"https://example.com\""));
        assert!(html.contains("title=\"Docs\""));
        assert!(html.contains("<strong>"));
        assert!(html.contains("<em>"));
        assert!(html.contains(">Bold italic</span>"));
        assert!(html.contains("href=\"https://example.org\""));
        assert!(html.contains("href=\"mailto:test@example.org\""));
        assert!(html.contains("<br/>"));
        assert!(html.contains("class=\"hcd-markdown-image\""));
        assert!(html.contains("data-hcd-node-kind=\"image\""));
        assert!(html.contains("data-hcd-markdown-image-src=\"https://example.com/diagram.png\""));
        assert!(!html.contains("href=\"javascript:"));
        assert!(html.contains(">Unsafe</span>"));
        assert!(
            html.contains("<table class=\"hcd-markdown-table\""),
            "{html}"
        );
        assert!(html.contains("<th>"));
        assert!(html.contains("<td>"));
        assert!(!html.contains("| --- | --- |"));
        assert!(html.contains("<pre class=\"hcd-source-block hcd-markdown-code\""));

        let exact = export_markdown(
            &bundle,
            &source,
            &exact_target,
            &ExportOptions {
                revision: Some(0),
                fidelity_report: None,
            },
        )
        .unwrap();
        assert_eq!(exact.level, FidelityLevel::Exact);
        assert_eq!(std::fs::read(&exact_target).unwrap(), original);

        let page = extract_text_page(&bundle, None, 100).unwrap();
        assert!(page.entries.iter().any(|entry| entry.text == "Name"));
        assert!(page.entries.iter().any(|entry| entry.text == "Zhang"));
        let entry = page
            .entries
            .iter()
            .find(|entry| entry.text == "123")
            .unwrap();
        apply_patch(
            &bundle,
            &PatchBatch {
                schema_version: HCD_PATCH_SCHEMA_VERSION.to_string(),
                document_id: "markdown-doc".to_string(),
                patch_id: "patch-md-1".to_string(),
                base_revision: 0,
                actor: BTreeMap::new(),
                operations: vec![PatchOperation::TextSplice {
                    node_id: entry.node_id.clone(),
                    start: 0,
                    delete_count: 3,
                    insert_text: "[MASKED]".to_string(),
                    precondition: NodePrecondition {
                        node_hash: entry.node_hash.clone(),
                    },
                }],
                metadata: BTreeMap::new(),
            },
            0,
        )
        .unwrap();
        let patched = export_markdown(
            &bundle,
            &source,
            &patched_target,
            &ExportOptions {
                revision: Some(1),
                fidelity_report: None,
            },
        )
        .unwrap();
        assert_eq!(patched.level, FidelityLevel::High);
        let output = std::fs::read(&patched_target).unwrap();
        assert!(output.starts_with(b"\xef\xbb\xbf# Secret **\\[MASKED\\]**\r\n"));
        assert!(output.ends_with(b"```text\ncode <value>\n```\n"));
    }

    #[test]
    fn markdown_hcd_covers_commonmark_gfm_and_safe_extensions() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("all.md");
        let bundle_path = temp.path().join("bundle");
        let markdown = r#"---
title: Full syntax
---

Setext heading {#setext .hero}
=============================

[reference link][docs] and footnote[^note], H ~2~ O, x ^2^, $a+b$, [[Wiki Page|wiki label]].

[docs]: https://example.com/reference "Reference"

> [!WARNING]
> Alert with **strong** text.
>
> - nested item
>   1. nested ordered item

Term
  : Definition value

| Left | Center | Right |
| :--- | :----: | ----: |
| A | B | C |

    indented code

```mermaid
graph TD; A-->B
```

$$E=mc^2$$

Inline <mark>safe HTML</mark> and <script>unsafe HTML</script>.

::: warning
Colon admonition.
:::

[^note]: Footnote **definition**.
"#;
        std::fs::write(&source, markdown).unwrap();
        import_markdown(&source, &bundle_path, &options("markdown-all"), |_| Ok(())).unwrap();
        let bundle = Bundle::open(&bundle_path).unwrap();
        let validation = validate_bundle(&bundle).unwrap();
        assert!(validation.valid, "{:?}", validation.issues);
        let html = chunk_html(&bundle).join("");

        for marker in [
            "data-hcd-metadata=\"yaml\"",
            "id=\"setext\"",
            "data-hcd-markdown-classes=\"hero\"",
            "href=\"https://example.com/reference\"",
            "hcd-markdown-footnote-ref",
            "id=\"hcd-footnote-note\"",
            "<sub>",
            "<sup>",
            "data-hcd-math=\"inline\"",
            "data-hcd-math=\"display\"",
            "hcd-markdown-wikilink",
            "data-hcd-alert=\"warning\"",
            "data-hcd-admonition=\"warning\"",
            "hcd-markdown-definition-list",
            "style=\"text-align:center\"",
            "style=\"text-align:right\"",
            "data-hcd-fenced=\"false\"",
            "class=\"language-mermaid\"",
            "<mark>",
            "hcd-markdown-raw-html",
        ] {
            assert!(html.contains(marker), "missing {marker} in {html}");
        }
        assert!(!html.contains("<script>unsafe"));
    }

    #[test]
    fn large_markdown_table_is_stably_split_into_row_groups() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("large.md");
        let first_bundle_path = temp.path().join("first-bundle");
        let second_bundle_path = temp.path().join("second-bundle");
        let mut markdown = String::from("| Row | Value |\n| ---: | :--- |\n");
        for row in 1..=300 {
            markdown.push_str(&format!("| {row} | Value {row} |\n"));
        }
        std::fs::write(&source, markdown).unwrap();
        let mut import_options = ImportOptions::new("large-markdown-table");
        import_options.chunk_soft_bytes = 512 * 1024;
        import_options.chunk_blocks = 256;
        import_markdown(&source, &first_bundle_path, &import_options, |_| Ok(())).unwrap();
        import_markdown(&source, &second_bundle_path, &import_options, |_| Ok(())).unwrap();

        let first = Bundle::open(&first_bundle_path).unwrap();
        let second = Bundle::open(&second_bundle_path).unwrap();
        let validation = validate_bundle(&first).unwrap();
        assert!(validation.valid, "{:?}", validation.issues);
        let html = chunk_html(&first).join("");
        assert_eq!(
            html.matches("<table class=\"hcd-markdown-table\"").count(),
            3
        );
        assert!(html.contains("data-hcd-markdown-table-fragment=\"0\""));
        assert!(html.contains("data-hcd-markdown-table-fragment=\"1\""));
        assert!(html.contains("data-hcd-markdown-table-fragment=\"2\""));
        assert_eq!(
            html.matches("data-hcd-markdown-table-continuation=\"true\"")
                .count(),
            2
        );
        assert_eq!(chunk_html(&first), chunk_html(&second));
    }

    #[test]
    fn bounded_line_rejects_a_node_larger_than_the_hard_limit() {
        let data = vec![b'x'; MAX_CHUNK_BYTES + 3];
        let error = read_bounded_line(&mut data.as_slice(), 0).unwrap_err();
        assert!(error.to_string().contains("NODE_TOO_LARGE"));
    }
}
