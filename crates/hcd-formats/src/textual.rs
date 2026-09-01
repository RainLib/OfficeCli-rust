use crate::common::{
    base_manifest, checked_export_state, collect_dirty_nodes, emit_failed, emit_started,
    escape_text, finish_import, source_identity, source_identity_with_extensions,
    write_fidelity_report, ExportOptions, ImportOptions,
};
use hcd_core::{
    hash_bytes, stable_node_id, Bundle, BundleWriter, ChunkSourceMap, FidelityLevel,
    FidelityReport, FidelityWarning, HcdError, HcdManifest, ImportEvent, NodeMapEntry,
    SourceAnchor, DEFAULT_CHUNK_BLOCKS, HCD_SCHEMA_VERSION, MAX_CHUNK_BYTES,
};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

const HTML_SOURCE_MAX_BYTES: u64 = 64 * 1024 * 1024;
const HTML_SOURCE_MAX_ELEMENTS: usize = 1_000_000;
const HTML_SOURCE_MAX_DEPTH: usize = 256;
const HTML_TABLE_MAX_ROWS: usize = 1_048_576;
const HTML_TABLE_MAX_COLUMNS: usize = 16_384;
const HTML_TABLE_MAX_CELLS: usize = 1_000_000;
const HTML_TABLE_ROWS_PER_FRAGMENT: usize = 128;
const SOURCE_RANGE_PREFIX: &str = "bytes:";
const HTML_PART: &str = "html/document";
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
        ".hcd-source{display:block}.hcd-source-block{white-space:pre-wrap;margin:0;min-height:1em}",
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
        } else if format == "html" {
            vec!["edited HTML text ranges are serialized with safe entity escaping".to_string()]
        } else {
            Vec::new()
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
    let valid = if format == "html" {
        matches!(extension.as_str(), "html" | "htm")
    } else {
        extension == format
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
    fn bounded_line_rejects_a_node_larger_than_the_hard_limit() {
        let data = vec![b'x'; MAX_CHUNK_BYTES + 3];
        let error = read_bounded_line(&mut data.as_slice(), 0).unwrap_err();
        assert!(error.to_string().contains("NODE_TOO_LARGE"));
    }
}
