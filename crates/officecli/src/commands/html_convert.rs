use docx_handler::WordHandler;
use handler_common::{DocumentHandler, HandlerError, InsertPosition};
use pdf_handler::PdfHandler;
use pptx_handler::PptxHandler;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use xlsx_handler::ExcelHandler;

pub(crate) const MAX_HTML_BYTES: u64 = 64 * 1024 * 1024;
const MAX_HTML_BLOCKS: usize = 1_000_000;
const MAX_BLOCK_BYTES: usize = 2 * 1024 * 1024;
const MAX_EXCEL_ROWS: usize = 1_048_576;
const MAX_EXCEL_COLUMNS: usize = 16_384;
const MAX_SEMANTIC_TABLE_CELLS: usize = 1_000_000;
const MAX_HCD_TABLE_FRAGMENTS: usize = 1_000_000;
const MAX_PPT_TABLE_ROWS_PER_SLIDE: usize = 18;
const MAX_PPT_TABLE_COLUMNS_PER_SLIDE: usize = 12;
const MAX_SEMANTIC_IMAGE_EMU: i64 = 100_000_000;
const DEFAULT_IMAGE_WIDTH_EMU: i64 = 3_657_600;
const DEFAULT_IMAGE_HEIGHT_EMU: i64 = 2_743_200;
const EMU_PER_POINT: f32 = 12_700.0;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HtmlConversionSummary {
    pub fidelity: &'static str,
    pub block_count: usize,
    pub image_count: usize,
    pub embedded_image_count: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HtmlBlock {
    Heading {
        level: u8,
        text: String,
    },
    Paragraph(String),
    ListItem {
        ordered: bool,
        ordinal: usize,
        text: String,
    },
    Preformatted(String),
    Quote(String),
    Rule,
    Table(Vec<Vec<String>>),
    Image {
        source: String,
        alt: String,
        geometry: ImageGeometry,
    },
    SectionBreak,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ImageGeometry {
    x_emu: Option<i64>,
    y_emu: Option<i64>,
    width_emu: Option<i64>,
    height_emu: Option<i64>,
}

impl ImageGeometry {
    fn with_fallback(self, fallback: Self) -> Self {
        Self {
            x_emu: self.x_emu.or(fallback.x_emu),
            y_emu: self.y_emu.or(fallback.y_emu),
            width_emu: self.width_emu.or(fallback.width_emu),
            height_emu: self.height_emu.or(fallback.height_emu),
        }
    }

    fn width(self) -> i64 {
        self.width_emu.unwrap_or(DEFAULT_IMAGE_WIDTH_EMU)
    }

    fn height(self) -> i64 {
        self.height_emu.unwrap_or(DEFAULT_IMAGE_HEIGHT_EMU)
    }
}

#[derive(Default)]
struct SemanticSlide {
    lines: Vec<String>,
    images: Vec<(String, String, ImageGeometry)>,
    table: Option<SemanticTableWindow>,
}

impl SemanticSlide {
    fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.images.is_empty() && self.table.is_none()
    }
}

#[derive(Debug, Clone, Copy)]
struct SemanticTableWindow {
    block_index: usize,
    row_start: usize,
    row_end: usize,
    column_start: usize,
    column_end: usize,
    include_header: bool,
}

#[derive(Clone, Copy)]
enum ImageTarget {
    Office,
    Pdf,
}

#[derive(Clone)]
pub(crate) struct SemanticAsset {
    pub path: PathBuf,
    pub format: String,
}

fn resolved_image<'a>(
    document: &'a SemanticHtml,
    source: &str,
    target: ImageTarget,
) -> Option<&'a SemanticAsset> {
    let asset = document.assets.get(source)?;
    let supported = match target {
        ImageTarget::Office => matches!(
            asset.format.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "tif" | "tiff" | "webp" | "ico"
        ),
        ImageTarget::Pdf => matches!(asset.format.as_str(), "png" | "jpg" | "jpeg"),
    };
    supported.then_some(asset)
}

#[derive(Default)]
struct SemanticHtml {
    title: String,
    blocks: Vec<HtmlBlock>,
    assets: HashMap<String, SemanticAsset>,
    warnings: BTreeSet<String>,
}

#[derive(Debug, Clone)]
enum PendingKind {
    Paragraph,
    PdfText(PdfTextGeometry),
    Heading(u8),
    ListItem { ordered: bool, ordinal: usize },
    Preformatted,
    Quote,
}

#[derive(Debug, Clone, Copy)]
struct PdfTextGeometry {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

#[derive(Debug)]
struct PdfLineFragment {
    geometry: PdfTextGeometry,
    text: String,
}

#[derive(Debug)]
struct PdfLineBuilder {
    y: f32,
    height: f32,
    fragments: Vec<PdfLineFragment>,
}

struct PendingBlock {
    kind: PendingKind,
    text: String,
}

#[derive(Default)]
struct TableBuilder {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: Option<String>,
    cell_count: usize,
    fragment: Option<HcdTableFragment>,
}

#[derive(Debug, Clone)]
struct HcdTableFragment {
    node_id: String,
    ordinal: usize,
    row_start: usize,
    row_end: usize,
    fragment_row_count: usize,
    column_count: usize,
    final_fragment: bool,
    total_row_count: Option<usize>,
}

struct LogicalTableBuilder {
    node_id: String,
    next_fragment: usize,
    next_row: usize,
    column_count: usize,
    rows: Vec<Vec<String>>,
    cell_count: usize,
}

#[derive(Default)]
struct ListState {
    ordered: bool,
    next: usize,
}

#[derive(Default)]
struct HtmlParser {
    document: SemanticHtml,
    pending: Option<PendingBlock>,
    table: Option<TableBuilder>,
    logical_table: Option<LogicalTableBuilder>,
    lists: Vec<ListState>,
    links: Vec<String>,
    image_geometry_stack: Vec<Option<ImageGeometry>>,
    pdf_line: Option<PdfLineBuilder>,
    in_title: bool,
    suppressed_tag: Option<String>,
}

fn pdf_text_geometry(attributes: &HashMap<String, String>) -> Option<PdfTextGeometry> {
    let is_pdf_text = attributes.get("class").is_some_and(|classes| {
        classes
            .split_ascii_whitespace()
            .any(|class| class == "hcd-pdf-text")
    });
    if !is_pdf_text {
        return None;
    }
    let parse = |name: &str| {
        attributes
            .get(name)
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite() && value.abs() <= 10_000_000.0)
    };
    let geometry = PdfTextGeometry {
        x: parse("data-hcd-x")?,
        y: parse("data-hcd-y")?,
        width: parse("data-hcd-width")?.max(0.0),
        height: parse("data-hcd-height")?.max(0.0),
    };
    Some(geometry)
}

#[derive(Debug)]
struct Tag {
    name: String,
    attributes: HashMap<String, String>,
    closing: bool,
    self_closing: bool,
}

fn image_geometry(attributes: &HashMap<String, String>) -> ImageGeometry {
    ImageGeometry {
        x_emu: signed_emu_attribute(attributes, "data-hcd-x-emu"),
        y_emu: signed_emu_attribute(attributes, "data-hcd-y-emu"),
        width_emu: positive_emu_attribute(attributes, "data-hcd-width-emu"),
        height_emu: positive_emu_attribute(attributes, "data-hcd-height-emu"),
    }
}

fn signed_emu_attribute(attributes: &HashMap<String, String>, name: &str) -> Option<i64> {
    attributes
        .get(name)
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (-MAX_SEMANTIC_IMAGE_EMU..=MAX_SEMANTIC_IMAGE_EMU).contains(value))
}

fn positive_emu_attribute(attributes: &HashMap<String, String>, name: &str) -> Option<i64> {
    attributes
        .get(name)
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (1..=MAX_SEMANTIC_IMAGE_EMU).contains(value))
}

fn hcd_table_fragment(
    attributes: &HashMap<String, String>,
) -> Result<Option<HcdTableFragment>, HandlerError> {
    const MARKERS: [&str; 9] = [
        "data-hcd-table-node-id",
        "data-hcd-table-fragment",
        "data-hcd-row-start",
        "data-hcd-row-end",
        "data-hcd-fragment-row-count",
        "data-hcd-column-count",
        "data-hcd-table-continuation",
        "data-hcd-table-final",
        "data-hcd-row-count",
    ];
    if !MARKERS.iter().any(|name| attributes.contains_key(*name)) {
        return Ok(None);
    }
    let node_id = attributes
        .get("data-hcd-table-node-id")
        .filter(|value| canonical_node_id(value))
        .cloned()
        .ok_or_else(|| {
            HandlerError::InvalidArgument(
                "HCD table fragment requires a canonical data-hcd-table-node-id".to_string(),
            )
        })?;
    let ordinal = hcd_table_usize(
        attributes,
        "data-hcd-table-fragment",
        MAX_HCD_TABLE_FRAGMENTS.saturating_sub(1),
    )?;
    let row_start = hcd_table_usize(attributes, "data-hcd-row-start", MAX_EXCEL_ROWS)?;
    let row_end = hcd_table_usize(attributes, "data-hcd-row-end", MAX_EXCEL_ROWS)?;
    let fragment_row_count =
        hcd_table_usize(attributes, "data-hcd-fragment-row-count", MAX_EXCEL_ROWS)?;
    let column_count = hcd_table_usize(attributes, "data-hcd-column-count", MAX_EXCEL_COLUMNS)?;
    let continuation = hcd_true_attribute(attributes, "data-hcd-table-continuation")?;
    let final_fragment = hcd_true_attribute(attributes, "data-hcd-table-final")?;
    let total_row_count = attributes
        .contains_key("data-hcd-row-count")
        .then(|| hcd_table_usize(attributes, "data-hcd-row-count", MAX_EXCEL_ROWS))
        .transpose()?;

    if continuation != (ordinal > 0) {
        return Err(HandlerError::InvalidArgument(format!(
            "HCD table {node_id} fragment {ordinal} has inconsistent continuation metadata"
        )));
    }
    if final_fragment != total_row_count.is_some() {
        return Err(HandlerError::InvalidArgument(format!(
            "HCD table {node_id} final marker and row count must appear together"
        )));
    }
    let expected_fragment_rows = if row_start == 0 && row_end == 0 {
        0
    } else if row_start == 0 || row_end < row_start {
        return Err(HandlerError::InvalidArgument(format!(
            "HCD table {node_id} fragment {ordinal} has invalid row range {row_start}..={row_end}"
        )));
    } else {
        row_end - row_start + 1
    };
    if fragment_row_count != expected_fragment_rows {
        return Err(HandlerError::InvalidArgument(format!(
            "HCD table {node_id} fragment {ordinal} declares {fragment_row_count} rows for range {row_start}..={row_end}"
        )));
    }
    if fragment_row_count > 0 && column_count == 0 {
        return Err(HandlerError::InvalidArgument(format!(
            "HCD table {node_id} has rows but declares zero columns"
        )));
    }
    Ok(Some(HcdTableFragment {
        node_id,
        ordinal,
        row_start,
        row_end,
        fragment_row_count,
        column_count,
        final_fragment,
        total_row_count,
    }))
}

fn hcd_table_usize(
    attributes: &HashMap<String, String>,
    name: &str,
    maximum: usize,
) -> Result<usize, HandlerError> {
    let value = attributes.get(name).ok_or_else(|| {
        HandlerError::InvalidArgument(format!("HCD table fragment is missing {name}"))
    })?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(HandlerError::InvalidArgument(format!(
            "HCD table {name} must be a canonical non-negative integer"
        )));
    }
    let parsed = value
        .parse::<usize>()
        .map_err(|_| HandlerError::InvalidArgument(format!("HCD table {name} is too large")))?;
    if parsed > maximum {
        return Err(HandlerError::InvalidArgument(format!(
            "HCD table {name} exceeds {maximum}"
        )));
    }
    Ok(parsed)
}

fn hcd_true_attribute(
    attributes: &HashMap<String, String>,
    name: &str,
) -> Result<bool, HandlerError> {
    match attributes.get(name).map(String::as_str) {
        None => Ok(false),
        Some("true") => Ok(true),
        Some(value) => Err(HandlerError::InvalidArgument(format!(
            "HCD table {name} must be omitted or true, got {value}"
        ))),
    }
}

fn canonical_node_id(value: &str) -> bool {
    value.strip_prefix("n_").is_some_and(|hex| {
        hex.len() == 32
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

pub(crate) fn convert_html(
    input: &Path,
    output: &Path,
    output_format: &str,
) -> Result<HtmlConversionSummary, HandlerError> {
    convert_html_with_assets(input, output, output_format, HashMap::new())
}

pub(crate) fn convert_html_with_assets(
    input: &Path,
    output: &Path,
    output_format: &str,
    assets: HashMap<String, SemanticAsset>,
) -> Result<HtmlConversionSummary, HandlerError> {
    let metadata = std::fs::metadata(input).map_err(HandlerError::IoError)?;
    if metadata.len() > MAX_HTML_BYTES {
        return Err(HandlerError::InvalidArgument(format!(
            "HTML input is {} bytes; maximum is {MAX_HTML_BYTES}",
            metadata.len()
        )));
    }
    let bytes = std::fs::read(input).map_err(HandlerError::IoError)?;
    let source = String::from_utf8_lossy(&bytes);
    let mut document = parse_html(&source)?;
    document.assets = assets;
    if document.blocks.is_empty() {
        document
            .blocks
            .push(HtmlBlock::Paragraph(document.title.clone()));
    }
    if output_format == "pptx"
        && document.blocks.iter().any(|block| {
            matches!(
                block,
                HtmlBlock::Table(rows)
                    if rows.len() > MAX_PPT_TABLE_ROWS_PER_SLIDE
                        || rows.iter().map(Vec::len).max().unwrap_or(0)
                            > MAX_PPT_TABLE_COLUMNS_PER_SLIDE
            )
        })
    {
        document.warnings.insert(format!(
            "PPTX tables exceeding {MAX_PPT_TABLE_ROWS_PER_SLIDE} rows or {MAX_PPT_TABLE_COLUMNS_PER_SLIDE} columns were split into bounded native table slides with the first row repeated"
        ));
    }

    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(HandlerError::IoError)?;
    let temporary = temporary_output_path(parent, output_format);
    let conversion = match output_format {
        "docx" => export_docx(&document, &temporary),
        "xlsx" => export_xlsx(&document, &temporary),
        "pptx" => export_pptx(&document, &temporary),
        "pdf" => export_pdf(&mut document, &temporary),
        other => Err(HandlerError::UnsupportedMode(format!(
            "HTML semantic conversion does not support .{other}"
        ))),
    };
    let embedded_image_count = match conversion {
        Ok(count) => count,
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
    };
    if let Err(error) = validate_output(&temporary, output_format) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    publish_output(&temporary, output)?;

    let image_count = document
        .blocks
        .iter()
        .filter(|block| matches!(block, HtmlBlock::Image { .. }))
        .count();
    if embedded_image_count < image_count {
        document.warnings.insert(
            format!(
                "{} of {image_count} images were represented by alt text because they were not trusted content-addressed assets or the target did not support their encoding",
                image_count - embedded_image_count
            ),
        );
    }
    document
        .warnings
        .insert("CSS layout is not reproduced; output uses semantic document defaults".to_string());
    Ok(HtmlConversionSummary {
        fidelity: "semantic",
        block_count: document.blocks.len(),
        image_count,
        embedded_image_count,
        warnings: document.warnings.into_iter().collect(),
    })
}

fn parse_html(source: &str) -> Result<SemanticHtml, HandlerError> {
    let mut parser = HtmlParser::default();
    let mut cursor = 0usize;
    while cursor < source.len() {
        if let Some(tag) = parser.suppressed_tag.clone() {
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
            parser.suppressed_tag = None;
            cursor = end + 1;
            continue;
        }

        let Some(relative) = source[cursor..].find('<') else {
            parser.text(&source[cursor..])?;
            break;
        };
        let start = cursor + relative;
        parser.text(&source[cursor..start])?;
        if source[start..].starts_with("<!--") {
            if let Some(end) = source[start + 4..].find("-->") {
                cursor = start + 4 + end + 3;
                continue;
            }
            break;
        }
        let Some(end) = find_tag_end(source.as_bytes(), start + 1) else {
            parser.text(&source[start..])?;
            break;
        };
        if let Some(tag) = parse_tag(&source[start + 1..end]) {
            parser.tag(tag)?;
        }
        cursor = end + 1;
    }
    parser.finish()
}

impl HtmlParser {
    fn text(&mut self, raw: &str) -> Result<(), HandlerError> {
        if raw.is_empty() || self.suppressed_tag.is_some() {
            return Ok(());
        }
        let decoded = decode_entities(raw);
        if self.in_title {
            append_normalized(&mut self.document.title, &decoded, false);
            return Ok(());
        }
        if let Some(table) = &mut self.table {
            if let Some(cell) = &mut table.cell {
                append_normalized(cell, &decoded, false);
            }
            return Ok(());
        }
        if self.pending.is_none() && decoded.chars().any(|character| !character.is_whitespace()) {
            self.pending = Some(PendingBlock {
                kind: PendingKind::Paragraph,
                text: String::new(),
            });
        }
        if let Some(pending) = &mut self.pending {
            let preserve = matches!(pending.kind, PendingKind::Preformatted);
            append_normalized(&mut pending.text, &decoded, preserve);
            ensure_text_limit(&pending.text)?;
        }
        Ok(())
    }

    fn tag(&mut self, tag: Tag) -> Result<(), HandlerError> {
        if matches!(
            tag.name.as_str(),
            "script" | "style" | "noscript" | "iframe" | "object" | "embed"
        ) {
            if !tag.closing && !tag.self_closing {
                self.suppressed_tag = Some(tag.name.clone());
            }
            self.document.warnings.insert(format!(
                "ignored unsafe or non-semantic <{}> content",
                tag.name
            ));
            return Ok(());
        }
        if tag.attributes.contains_key("style") {
            self.document
                .warnings
                .insert("inline CSS was ignored".to_string());
        }
        if tag.closing {
            self.end_tag(&tag.name)
        } else {
            self.start_tag(&tag)?;
            if tag.self_closing || is_void_element(&tag.name) {
                self.end_tag(&tag.name)?;
            }
            Ok(())
        }
    }

    fn start_tag(&mut self, tag: &Tag) -> Result<(), HandlerError> {
        match tag.name.as_str() {
            "title" => self.in_title = true,
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = tag.name[1..].parse().unwrap_or(1);
                self.start_pending(PendingKind::Heading(level))?;
            }
            "p" => match pdf_text_geometry(&tag.attributes) {
                Some(geometry) => self.start_pending(PendingKind::PdfText(geometry))?,
                None => self.start_pending(PendingKind::Paragraph)?,
            },
            "article" | "header" | "footer" | "address" | "figure" | "figcaption" => {
                self.start_pending(PendingKind::Paragraph)?
            }
            "div" => {
                let geometry = tag
                    .attributes
                    .get("class")
                    .is_some_and(|classes| {
                        classes
                            .split_ascii_whitespace()
                            .any(|class| class == "hcd-slide-picture")
                    })
                    .then(|| image_geometry(&tag.attributes));
                self.image_geometry_stack.push(geometry);
                if geometry.is_some() {
                    self.flush_pending()?;
                } else {
                    self.start_pending(PendingKind::Paragraph)?;
                }
            }
            "blockquote" => self.start_pending(PendingKind::Quote)?,
            "pre" => self.start_pending(PendingKind::Preformatted)?,
            "section" | "main" => {
                self.flush_pending()?;
                self.flush_pdf_line()?;
                self.push_section_break()?;
            }
            "ul" => self.lists.push(ListState {
                ordered: false,
                next: 1,
            }),
            "ol" => self.lists.push(ListState {
                ordered: true,
                next: tag
                    .attributes
                    .get("start")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(1),
            }),
            "li" => {
                let (ordered, ordinal) = self
                    .lists
                    .last_mut()
                    .map(|state| {
                        let ordinal = state.next;
                        state.next = state.next.saturating_add(1);
                        (state.ordered, ordinal)
                    })
                    .unwrap_or((false, 1));
                self.start_pending(PendingKind::ListItem { ordered, ordinal })?;
            }
            "br" => self.append_active("\n", true)?,
            "hr" => {
                self.flush_pending()?;
                self.push_block(HtmlBlock::Rule)?;
            }
            "table" => {
                self.flush_pending()?;
                if self.table.is_some() {
                    self.document
                        .warnings
                        .insert("nested HTML tables were flattened".to_string());
                } else {
                    self.table = Some(TableBuilder {
                        fragment: hcd_table_fragment(&tag.attributes)?,
                        ..TableBuilder::default()
                    });
                }
            }
            "tr" => {
                if let Some(table) = &mut self.table {
                    finish_table_row(table)?;
                }
            }
            "td" | "th" => {
                if let Some(table) = &mut self.table {
                    finish_table_cell(table);
                    if table.cell_count >= MAX_SEMANTIC_TABLE_CELLS {
                        return Err(HandlerError::InvalidArgument(format!(
                            "HTML table exceeds {MAX_SEMANTIC_TABLE_CELLS} cells"
                        )));
                    }
                    table.cell_count += 1;
                    table.cell = Some(String::new());
                }
            }
            "a" => {
                let target = tag
                    .attributes
                    .get("href")
                    .filter(|target| safe_link(target))
                    .cloned()
                    .unwrap_or_default();
                self.links.push(target);
            }
            "img" => {
                let source = tag.attributes.get("src").cloned().unwrap_or_default();
                let alt = tag
                    .attributes
                    .get("alt")
                    .cloned()
                    .unwrap_or_else(|| "image".to_string());
                if self
                    .table
                    .as_ref()
                    .is_some_and(|table| table.cell.is_some())
                {
                    self.append_active(&format!("[Image: {alt}]"), false)?;
                } else {
                    if self.pending.is_some() {
                        self.flush_pending()?;
                        self.document.warnings.insert(
                            "inline images were reflowed as standalone semantic blocks".to_string(),
                        );
                    }
                    let inherited = self
                        .image_geometry_stack
                        .iter()
                        .rev()
                        .flatten()
                        .copied()
                        .next()
                        .unwrap_or_default();
                    let geometry = image_geometry(&tag.attributes).with_fallback(inherited);
                    self.push_block(HtmlBlock::Image {
                        source,
                        alt,
                        geometry,
                    })?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn end_tag(&mut self, name: &str) -> Result<(), HandlerError> {
        match name {
            "title" => self.in_title = false,
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "div" | "article" | "header"
            | "footer" | "address" | "figure" | "figcaption" | "blockquote" | "pre" | "li" => {
                self.flush_pending()?
            }
            "ul" | "ol" => {
                self.flush_pending()?;
                self.lists.pop();
            }
            "td" | "th" => {
                if let Some(table) = &mut self.table {
                    finish_table_cell(table);
                }
            }
            "tr" => {
                if let Some(table) = &mut self.table {
                    finish_table_row(table)?;
                }
            }
            "table" => {
                if let Some(mut table) = self.table.take() {
                    finish_table_cell(&mut table);
                    finish_table_row(&mut table)?;
                    self.finish_table(table)?;
                }
            }
            "a" => {
                if let Some(target) = self.links.pop() {
                    if !target.is_empty() {
                        self.append_active(&format!(" ({target})"), false)?;
                    }
                }
            }
            "section" | "main" => {
                self.flush_pending()?;
                self.flush_pdf_line()?;
            }
            _ => {}
        }
        if name == "div" {
            self.image_geometry_stack.pop();
        }
        Ok(())
    }

    fn finish_table(&mut self, table: TableBuilder) -> Result<(), HandlerError> {
        let TableBuilder {
            rows,
            cell_count,
            fragment,
            ..
        } = table;
        let Some(fragment) = fragment else {
            if let Some(active) = &self.logical_table {
                return Err(HandlerError::InvalidArgument(format!(
                    "ordinary HTML table interrupted HCD table {} before its final fragment",
                    active.node_id
                )));
            }
            if !rows.is_empty() {
                self.push_block(HtmlBlock::Table(rows))?;
            }
            return Ok(());
        };
        if rows.len() != fragment.fragment_row_count {
            return Err(HandlerError::InvalidArgument(format!(
                "HCD table {} fragment {} declares {} rows but contains {}",
                fragment.node_id,
                fragment.ordinal,
                fragment.fragment_row_count,
                rows.len()
            )));
        }
        if rows.iter().any(|row| row.len() > fragment.column_count) {
            return Err(HandlerError::InvalidArgument(format!(
                "HCD table {} fragment {} contains more than {} declared columns",
                fragment.node_id, fragment.ordinal, fragment.column_count
            )));
        }

        if fragment.ordinal == 0 {
            if let Some(active) = &self.logical_table {
                return Err(HandlerError::InvalidArgument(format!(
                    "HCD table {} started before table {} reached its final fragment",
                    fragment.node_id, active.node_id
                )));
            }
            if fragment.row_start != usize::from(fragment.fragment_row_count > 0) {
                return Err(HandlerError::InvalidArgument(format!(
                    "HCD table {} first fragment must start at row 1 (or 0 for an empty table)",
                    fragment.node_id
                )));
            }
            self.logical_table = Some(LogicalTableBuilder {
                node_id: fragment.node_id.clone(),
                next_fragment: 0,
                next_row: fragment.row_start,
                column_count: fragment.column_count,
                rows: Vec::new(),
                cell_count: 0,
            });
        }

        let logical = self.logical_table.as_mut().ok_or_else(|| {
            HandlerError::InvalidArgument(format!(
                "HCD table {} continuation has no first fragment",
                fragment.node_id
            ))
        })?;
        if logical.node_id != fragment.node_id
            || logical.next_fragment != fragment.ordinal
            || logical.next_row != fragment.row_start
            || logical.column_count != fragment.column_count
        {
            return Err(HandlerError::InvalidArgument(format!(
                "HCD table {} fragment {} is missing, duplicated, out of order, or inconsistent",
                fragment.node_id, fragment.ordinal
            )));
        }
        logical.cell_count = logical.cell_count.checked_add(cell_count).ok_or_else(|| {
            HandlerError::InvalidArgument("HCD table cell count overflowed".to_string())
        })?;
        if logical.cell_count > MAX_SEMANTIC_TABLE_CELLS {
            return Err(HandlerError::InvalidArgument(format!(
                "HCD table {} exceeds {MAX_SEMANTIC_TABLE_CELLS} cells",
                logical.node_id
            )));
        }
        if logical.rows.len().saturating_add(rows.len()) > MAX_EXCEL_ROWS {
            return Err(HandlerError::InvalidArgument(format!(
                "HCD table {} exceeds {MAX_EXCEL_ROWS} rows",
                logical.node_id
            )));
        }
        logical.rows.extend(rows);
        logical.next_fragment += 1;
        logical.next_row = fragment.row_end.saturating_add(1);

        if fragment.final_fragment {
            let expected_rows = fragment.total_row_count.unwrap_or(0);
            if expected_rows != logical.rows.len() || expected_rows != fragment.row_end {
                return Err(HandlerError::InvalidArgument(format!(
                    "HCD table {} final row count {expected_rows} does not match its assembled rows",
                    fragment.node_id
                )));
            }
            let logical = self.logical_table.take().expect("logical table exists");
            if logical.next_fragment > 1 {
                self.document.warnings.insert(format!(
                    "reassembled HCD table {} from {} contiguous fragments",
                    logical.node_id, logical.next_fragment
                ));
            }
            if !logical.rows.is_empty() {
                self.push_block(HtmlBlock::Table(logical.rows))?;
            }
        }
        Ok(())
    }

    fn start_pending(&mut self, kind: PendingKind) -> Result<(), HandlerError> {
        self.flush_pending()?;
        if !matches!(kind, PendingKind::PdfText(_)) {
            self.flush_pdf_line()?;
        }
        self.pending = Some(PendingBlock {
            kind,
            text: String::new(),
        });
        Ok(())
    }

    fn append_active(&mut self, value: &str, preserve: bool) -> Result<(), HandlerError> {
        if let Some(table) = &mut self.table {
            if let Some(cell) = &mut table.cell {
                append_normalized(cell, value, preserve);
                ensure_text_limit(cell)?;
                return Ok(());
            }
        }
        if self.pending.is_none() {
            self.pending = Some(PendingBlock {
                kind: PendingKind::Paragraph,
                text: String::new(),
            });
        }
        let pending = self.pending.as_mut().expect("created pending block");
        append_normalized(&mut pending.text, value, preserve);
        ensure_text_limit(&pending.text)
    }

    fn flush_pending(&mut self) -> Result<(), HandlerError> {
        let Some(pending) = self.pending.take() else {
            return Ok(());
        };
        let text = if matches!(pending.kind, PendingKind::Preformatted) {
            pending.text.trim_matches('\n').to_string()
        } else {
            pending.text.trim().to_string()
        };
        if text.is_empty() {
            return Ok(());
        }
        let block = match pending.kind {
            PendingKind::Paragraph => HtmlBlock::Paragraph(text),
            PendingKind::PdfText(geometry) => return self.push_pdf_line_fragment(geometry, text),
            PendingKind::Heading(level) => HtmlBlock::Heading { level, text },
            PendingKind::ListItem { ordered, ordinal } => HtmlBlock::ListItem {
                ordered,
                ordinal,
                text,
            },
            PendingKind::Preformatted => HtmlBlock::Preformatted(text),
            PendingKind::Quote => HtmlBlock::Quote(text),
        };
        self.push_block(block)
    }

    fn push_pdf_line_fragment(
        &mut self,
        geometry: PdfTextGeometry,
        text: String,
    ) -> Result<(), HandlerError> {
        let same_line = self.pdf_line.as_ref().is_some_and(|line| {
            let tolerance = line.height.max(geometry.height).max(1.0) * 0.35;
            (line.y - geometry.y).abs() <= tolerance
        });
        if !same_line {
            self.flush_pdf_line()?;
            self.pdf_line = Some(PdfLineBuilder {
                y: geometry.y,
                height: geometry.height,
                fragments: Vec::new(),
            });
        }
        let line = self.pdf_line.as_mut().expect("PDF line initialized");
        if line.fragments.len() >= MAX_HTML_BLOCKS {
            return Err(HandlerError::InvalidArgument(
                "PDF semantic line exceeds the fragment limit".to_string(),
            ));
        }
        line.fragments.push(PdfLineFragment { geometry, text });
        Ok(())
    }

    fn flush_pdf_line(&mut self) -> Result<(), HandlerError> {
        let Some(mut line) = self.pdf_line.take() else {
            return Ok(());
        };
        line.fragments.sort_by(|left, right| {
            left.geometry
                .x
                .partial_cmp(&right.geometry.x)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut text = String::new();
        let mut previous_end = None;
        for fragment in line.fragments {
            let gap = previous_end
                .map(|end| fragment.geometry.x - end)
                .unwrap_or(0.0);
            let needs_ascii_space = gap > 1.0
                && text.chars().next_back().is_some_and(|character| {
                    character.is_ascii_alphanumeric() || character.is_ascii_punctuation()
                })
                && fragment.text.chars().next().is_some_and(|character| {
                    character.is_ascii_alphanumeric() || character.is_ascii_punctuation()
                });
            if needs_ascii_space && !text.chars().next_back().is_some_and(char::is_whitespace) {
                text.push(' ');
            }
            text.push_str(&fragment.text);
            previous_end = Some(fragment.geometry.x + fragment.geometry.width);
            ensure_text_limit(&text)?;
        }
        let text = text.trim().to_string();
        if !text.is_empty() {
            self.push_block(HtmlBlock::Paragraph(text))?;
        }
        Ok(())
    }

    fn push_section_break(&mut self) -> Result<(), HandlerError> {
        if self.logical_table.is_some() {
            // Every content-addressed HCD chunk is a self-contained section. A
            // logical table is allowed to span those wrapper sections, so the
            // wrapper boundary is not a document/slide boundary.
            return Ok(());
        }
        if !self.document.blocks.is_empty()
            && !matches!(self.document.blocks.last(), Some(HtmlBlock::SectionBreak))
        {
            self.push_block(HtmlBlock::SectionBreak)?;
        }
        Ok(())
    }

    fn push_block(&mut self, block: HtmlBlock) -> Result<(), HandlerError> {
        if self.document.blocks.len() >= MAX_HTML_BLOCKS {
            return Err(HandlerError::InvalidArgument(format!(
                "HTML exceeds {MAX_HTML_BLOCKS} semantic blocks"
            )));
        }
        self.document.blocks.push(block);
        Ok(())
    }

    fn finish(mut self) -> Result<SemanticHtml, HandlerError> {
        self.flush_pending()?;
        self.flush_pdf_line()?;
        if let Some(mut table) = self.table.take() {
            finish_table_cell(&mut table);
            finish_table_row(&mut table)?;
            self.finish_table(table)?;
        }
        if let Some(table) = self.logical_table {
            return Err(HandlerError::InvalidArgument(format!(
                "HCD table {} ended before its final fragment",
                table.node_id
            )));
        }
        while matches!(self.document.blocks.last(), Some(HtmlBlock::SectionBreak)) {
            self.document.blocks.pop();
        }
        self.document.title = self.document.title.trim().to_string();
        Ok(self.document)
    }
}

fn export_docx(document: &SemanticHtml, output: &Path) -> Result<usize, HandlerError> {
    let output = output.to_string_lossy();
    super::create::create_blank_docx(&output)?;
    let handler = WordHandler::open(&output, true)?;
    let _ = handler.remove("/body/p[1]")?;
    let mut pending = Vec::new();
    let mut embedded = 0usize;
    for block in &document.blocks {
        if let HtmlBlock::Image {
            source,
            alt,
            geometry,
        } = block
        {
            if let Some(asset) = resolved_image(document, source, ImageTarget::Office) {
                flush_docx_markdown(&handler, &mut pending)?;
                let mut properties = HashMap::new();
                properties.insert("src".to_string(), asset.path.to_string_lossy().into_owned());
                properties.insert("format".to_string(), asset.format.clone());
                properties.insert("alt".to_string(), alt.clone());
                properties.insert("width".to_string(), geometry.width().to_string());
                properties.insert("height".to_string(), geometry.height().to_string());
                handler.add("/body", "image", InsertPosition::Append, &properties, None)?;
                embedded += 1;
                continue;
            }
        }
        pending.push(block.clone());
    }
    flush_docx_markdown(&handler, &mut pending)?;
    handler.save()?;
    Ok(embedded)
}

fn flush_docx_markdown(
    handler: &WordHandler,
    pending: &mut Vec<HtmlBlock>,
) -> Result<(), HandlerError> {
    if pending.is_empty() {
        return Ok(());
    }
    let mut properties = HashMap::new();
    properties.insert("markdown".to_string(), to_markdown_blocks(pending));
    handler.add(
        "/body",
        "markdown",
        InsertPosition::Append,
        &properties,
        None,
    )?;
    pending.clear();
    Ok(())
}

fn export_xlsx(document: &SemanticHtml, output: &Path) -> Result<usize, HandlerError> {
    let output = output.to_string_lossy();
    super::create::create_blank_xlsx(&output)?;
    let mut worksheet = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>",
    );
    let mut row = 1usize;
    let mut images = Vec::new();
    for block in &document.blocks {
        match block {
            HtmlBlock::Table(rows) => {
                for cells in rows {
                    if cells.len() > MAX_EXCEL_COLUMNS {
                        return Err(HandlerError::InvalidArgument(format!(
                            "HTML table has {} columns; XLSX maximum is {MAX_EXCEL_COLUMNS}",
                            cells.len()
                        )));
                    }
                    append_xlsx_row(&mut worksheet, row, cells)?;
                    row = row.saturating_add(1);
                }
            }
            HtmlBlock::SectionBreak => row = row.saturating_add(1),
            HtmlBlock::Image {
                source,
                alt,
                geometry,
            } => {
                if let Some(asset) = resolved_image(document, source, ImageTarget::Office) {
                    images.push((row, asset.clone(), alt.clone(), *geometry));
                    let row_height = ((geometry.height() as f64 / 914_400.0) * 5.0)
                        .ceil()
                        .clamp(1.0, 100.0) as usize;
                    row = row.saturating_add(row_height);
                } else {
                    for value in block_lines(block) {
                        append_xlsx_row(&mut worksheet, row, &[value])?;
                        row = row.saturating_add(1);
                    }
                }
            }
            other => {
                for value in block_lines(other) {
                    append_xlsx_row(&mut worksheet, row, &[value])?;
                    row = row.saturating_add(1);
                }
            }
        }
        if row > MAX_EXCEL_ROWS + 1 {
            return Err(HandlerError::InvalidArgument(format!(
                "HTML content exceeds the XLSX row limit {MAX_EXCEL_ROWS}"
            )));
        }
    }
    worksheet.push_str("</sheetData></worksheet>");
    let mut package = oxml::OxmlPackage::open(&output, true)
        .map_err(|error| HandlerError::OpenError(error.to_string()))?;
    package
        .write_part_xml("xl/worksheets/sheet1.xml", &worksheet)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    package
        .save()
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    drop(package);

    if !images.is_empty() {
        let handler = ExcelHandler::open(&output, true)?;
        for (row, asset, alt, geometry) in &images {
            let mut properties = HashMap::new();
            properties.insert("src".to_string(), asset.path.to_string_lossy().into_owned());
            properties.insert("format".to_string(), asset.format.clone());
            properties.insert("anchor".to_string(), format!("A{row}"));
            properties.insert("alt".to_string(), alt.clone());
            properties.insert("width".to_string(), geometry.width().to_string());
            properties.insert("height".to_string(), geometry.height().to_string());
            handler.add(
                "/Sheet1",
                "image",
                InsertPosition::Append,
                &properties,
                None,
            )?;
        }
        handler.save()?;
    }
    Ok(images.len())
}

fn append_xlsx_row(
    worksheet: &mut String,
    row: usize,
    cells: &[String],
) -> Result<(), HandlerError> {
    worksheet.push_str(&format!("<row r=\"{row}\">"));
    for (index, value) in cells.iter().enumerate() {
        let column = index + 1;
        if value.chars().count() > 32_767 {
            return Err(HandlerError::InvalidArgument(format!(
                "HTML text for cell {}{} exceeds the XLSX 32,767 character limit",
                column_letters(column),
                row
            )));
        }
        let reference = format!("{}{}", column_letters(column), row);
        let preserve =
            value.starts_with(char::is_whitespace) || value.ends_with(char::is_whitespace);
        worksheet.push_str(&format!(
            "<c r=\"{reference}\" t=\"inlineStr\"><is><t{}>{}</t></is></c>",
            if preserve {
                " xml:space=\"preserve\""
            } else {
                ""
            },
            escape_xml_text(value)?
        ));
    }
    worksheet.push_str("</row>");
    Ok(())
}

fn export_pptx(document: &SemanticHtml, output: &Path) -> Result<usize, HandlerError> {
    let output = output.to_string_lossy();
    super::create::create_blank_pptx(&output)?;
    let handler = PptxHandler::open(&output, true)?;
    let slides = semantic_slides(document);
    let mut embedded = 0usize;
    for (index, slide) in slides.iter().enumerate() {
        if index > 0 {
            handler.add("/", "slide", InsertPosition::Append, &HashMap::new(), None)?;
        }
        let parent = format!("/slide[{}]", index + 1);
        if !slide.lines.is_empty() {
            let mut properties = HashMap::new();
            properties.insert("name".to_string(), format!("HTML Content {}", index + 1));
            properties.insert("text".to_string(), slide.lines.join("\n"));
            handler.add(&parent, "shape", InsertPosition::Append, &properties, None)?;
        }
        if let Some(window) = slide.table {
            let rows = match document.blocks.get(window.block_index) {
                Some(HtmlBlock::Table(rows)) => rows,
                _ => {
                    return Err(HandlerError::OperationFailed(
                        "semantic PPTX table window no longer references a table block".to_string(),
                    ))
                }
            };
            let mut source_rows = Vec::with_capacity(MAX_PPT_TABLE_ROWS_PER_SLIDE);
            if window.include_header && !rows.is_empty() {
                source_rows.push(0usize);
            }
            source_rows.extend(window.row_start..window.row_end);
            let column_count = window.column_end.saturating_sub(window.column_start);
            if source_rows.is_empty() || column_count == 0 {
                return Err(HandlerError::OperationFailed(
                    "semantic PPTX table window is empty".to_string(),
                ));
            }
            let mut properties = HashMap::new();
            properties.insert("rows".to_string(), source_rows.len().to_string());
            properties.insert("cols".to_string(), column_count.to_string());
            properties.insert(
                "name".to_string(),
                format!(
                    "HCD Table rows {}-{} columns {}-{}",
                    window.row_start + 1,
                    window.row_end,
                    window.column_start + 1,
                    window.column_end
                ),
            );
            properties.insert("x".to_string(), "457200".to_string());
            properties.insert("y".to_string(), "685800".to_string());
            properties.insert("width".to_string(), "8229600".to_string());
            let height = (source_rows.len() as i64 * 320_040).clamp(457_200, 5_486_400);
            properties.insert("height".to_string(), height.to_string());
            for (target_row, source_row) in source_rows.into_iter().enumerate() {
                let cells = rows.get(source_row).map(Vec::as_slice).unwrap_or(&[]);
                for (target_column, source_column) in
                    (window.column_start..window.column_end).enumerate()
                {
                    properties.insert(
                        format!("r{}c{}", target_row + 1, target_column + 1),
                        cells.get(source_column).cloned().unwrap_or_default(),
                    );
                }
            }
            handler.add(&parent, "table", InsertPosition::Append, &properties, None)?;
        }
        for (image_index, (source, alt, geometry)) in slide.images.iter().enumerate() {
            let Some(asset) = resolved_image(document, source, ImageTarget::Office) else {
                continue;
            };
            let mut properties = HashMap::new();
            properties.insert("src".to_string(), asset.path.to_string_lossy().into_owned());
            properties.insert("format".to_string(), asset.format.clone());
            properties.insert("alt".to_string(), alt.clone());
            properties.insert("name".to_string(), format!("HCD Image {}", embedded + 1));
            let default_x = if image_index % 2 == 0 {
                685_800
            } else {
                4_572_000
            };
            properties.insert(
                "x".to_string(),
                geometry.x_emu.unwrap_or(default_x).to_string(),
            );
            properties.insert(
                "y".to_string(),
                geometry.y_emu.unwrap_or(2_057_400).to_string(),
            );
            properties.insert("width".to_string(), geometry.width().to_string());
            properties.insert("height".to_string(), geometry.height().to_string());
            handler.add(&parent, "image", InsertPosition::Append, &properties, None)?;
            embedded += 1;
        }
    }
    handler.save()?;
    Ok(embedded)
}

fn export_pdf(document: &mut SemanticHtml, output: &Path) -> Result<usize, HandlerError> {
    let output = output.to_string_lossy();
    super::create::create_blank_pdf(&output)?;
    let handler = PdfHandler::open(&output, true)?;
    let font_characters: String = document
        .blocks
        .iter()
        .filter(|block| {
            !matches!(
                block,
                HtmlBlock::Image { source, .. }
                    if resolved_image(document, source, ImageTarget::Pdf).is_some()
            )
        })
        .flat_map(block_lines)
        .flat_map(|line| line.chars().collect::<Vec<_>>())
        .filter(|character| !character.is_control())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let font_selection = semantic_pdf_font_selection(&font_characters);
    let font_characters = sanitize_semantic_pdf_text(&font_characters, font_selection.as_ref());
    let font_character_set: HashSet<char> = font_characters.chars().collect();
    let font_file = font_selection
        .as_ref()
        .map(|selection| selection.path.to_string_lossy().into_owned());
    if let Some(selection) = &font_selection {
        if !selection.missing.is_empty() {
            document.warnings.insert(format!(
                "PDF font fallback replaced {} distinct unsupported extracted characters with {}",
                selection.missing.len(),
                selection.replacement
            ));
        }
    }
    let mut page = 1usize;
    let mut line_on_page = 0usize;
    let mut embedded = 0usize;
    let mut page_text = Vec::new();
    for block in &document.blocks {
        if matches!(block, HtmlBlock::SectionBreak) && line_on_page > 0 {
            flush_semantic_pdf_page(
                &handler,
                page,
                &mut page_text,
                &font_character_set,
                font_file.as_deref(),
            )?;
            page += 1;
            line_on_page = 0;
            handler.add("/", "page", InsertPosition::Append, &HashMap::new(), None)?;
            continue;
        }
        if let HtmlBlock::Image {
            source, geometry, ..
        } = block
        {
            if let Some(asset) = resolved_image(document, source, ImageTarget::Pdf) {
                let (width, height) = pdf_image_dimensions(*geometry);
                let image_lines = ((height / 15.0).ceil() as usize + 1).clamp(1, 44);
                if line_on_page.saturating_add(image_lines) > 48 {
                    flush_semantic_pdf_page(
                        &handler,
                        page,
                        &mut page_text,
                        &font_character_set,
                        font_file.as_deref(),
                    )?;
                    page += 1;
                    line_on_page = 0;
                    handler.add("/", "page", InsertPosition::Append, &HashMap::new(), None)?;
                }
                let y = 790.0 - line_on_page as f32 * 15.0 - height;
                let mut properties = HashMap::new();
                properties.insert("src".to_string(), asset.path.to_string_lossy().into_owned());
                properties.insert("x".to_string(), "54".to_string());
                properties.insert("y".to_string(), format!("{y:.2}"));
                properties.insert("width".to_string(), format!("{width:.2}"));
                properties.insert("height".to_string(), format!("{height:.2}"));
                handler.add(
                    &format!("/page[{page}]"),
                    "image",
                    InsertPosition::Append,
                    &properties,
                    None,
                )?;
                embedded += 1;
                line_on_page += image_lines;
                continue;
            }
        }
        for logical in block_lines(block) {
            for line in wrap_text(&logical, 82) {
                if line_on_page >= 48 {
                    flush_semantic_pdf_page(
                        &handler,
                        page,
                        &mut page_text,
                        &font_character_set,
                        font_file.as_deref(),
                    )?;
                    page += 1;
                    line_on_page = 0;
                    handler.add("/", "page", InsertPosition::Append, &HashMap::new(), None)?;
                }
                let heading = matches!(block, HtmlBlock::Heading { .. });
                let size = if heading { 16.0 } else { 11.0 };
                let y = 790.0 - line_on_page as f32 * 15.0;
                let line = line.replace('\t', " ");
                page_text.push(pdf_handler::modifier::ReadyTextBlock {
                    text: sanitize_semantic_pdf_text(&line, font_selection.as_ref()),
                    x: 54.0,
                    y,
                    size,
                });
                line_on_page += 1;
            }
        }
    }
    flush_semantic_pdf_page(
        &handler,
        page,
        &mut page_text,
        &font_character_set,
        font_file.as_deref(),
    )?;
    handler.save()?;
    Ok(embedded)
}

fn flush_semantic_pdf_page(
    handler: &PdfHandler,
    page: usize,
    blocks: &mut Vec<pdf_handler::modifier::ReadyTextBlock>,
    font_characters: &HashSet<char>,
    font_file: Option<&str>,
) -> Result<(), HandlerError> {
    if blocks.is_empty() {
        return Ok(());
    }
    let font_name =
        handler.ensure_font_for_chars(page, font_characters, "HCDSemantic", font_file)?;
    handler.add_ready_text_blocks(page, blocks, &font_name)?;
    blocks.clear();
    Ok(())
}

struct SemanticPdfFontSelection {
    path: PathBuf,
    missing: HashSet<char>,
    replacement: char,
}

fn semantic_pdf_font_selection(characters: &str) -> Option<SemanticPdfFontSelection> {
    let required: HashSet<char> = characters.chars().collect();
    let configured = std::env::var_os("OFFICECLI_SEMANTIC_PDF_FONT_FILE").map(PathBuf::from);
    let candidates = configured.into_iter().chain(
        [
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
            "/Library/Fonts/Arial Unicode.ttf",
            "C:\\Windows\\Fonts\\arialuni.ttf",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    candidates
        .into_iter()
        .filter(|candidate| candidate.is_file())
        .filter_map(|path| {
            pdf_handler::font_embedder::font_file_missing_chars(&path, &required)
                .ok()
                .map(|missing| (path, missing))
        })
        .min_by_key(|(_, missing)| missing.len())
        .map(|(path, missing)| {
            let replacement_chars = HashSet::from(['\u{fffd}']);
            let replacement =
                if pdf_handler::font_embedder::font_file_covers_chars(&path, &replacement_chars)
                    .unwrap_or(false)
                {
                    '\u{fffd}'
                } else {
                    '?'
                };
            SemanticPdfFontSelection {
                path,
                missing,
                replacement,
            }
        })
}

fn sanitize_semantic_pdf_text(text: &str, selection: Option<&SemanticPdfFontSelection>) -> String {
    let Some(selection) = selection else {
        return text.to_string();
    };
    text.chars()
        .map(|character| {
            if selection.missing.contains(&character) {
                selection.replacement
            } else {
                character
            }
        })
        .collect()
}

fn pdf_image_dimensions(geometry: ImageGeometry) -> (f32, f32) {
    let mut width = geometry.width() as f32 / EMU_PER_POINT;
    let mut height = geometry.height() as f32 / EMU_PER_POINT;
    let scale = 1.0_f32.min(504.0 / width).min(650.0 / height);
    width *= scale;
    height *= scale;
    (width.max(1.0), height.max(1.0))
}

fn validate_output(path: &Path, format: &str) -> Result<(), HandlerError> {
    let path = path.to_string_lossy();
    let issues = match format {
        "docx" => WordHandler::open(&path, false)?.validate()?,
        "xlsx" => ExcelHandler::open(&path, false)?.validate()?,
        "pptx" => PptxHandler::open(&path, false)?.validate()?,
        "pdf" => PdfHandler::open(&path, false)?.validate()?,
        _ => Vec::new(),
    };
    if !issues.is_empty() {
        return Err(HandlerError::ValidationError(format!(
            "generated .{format} failed validation: {issues:?}"
        )));
    }
    Ok(())
}

fn to_markdown_blocks(blocks: &[HtmlBlock]) -> String {
    let mut output = String::new();
    for block in blocks {
        match block {
            HtmlBlock::Heading { level, text } => {
                output.push_str(&"#".repeat(*level as usize));
                output.push(' ');
                output.push_str(text);
            }
            HtmlBlock::Paragraph(text) => output.push_str(text),
            HtmlBlock::ListItem {
                ordered,
                ordinal,
                text,
            } => {
                if *ordered {
                    output.push_str(&format!("{ordinal}. {text}"));
                } else {
                    output.push_str(&format!("- {text}"));
                }
            }
            HtmlBlock::Preformatted(text) => {
                output.push_str("```\n");
                output.push_str(text);
                output.push_str("\n```");
            }
            HtmlBlock::Quote(text) => output.push_str(&format!("> {text}")),
            HtmlBlock::Rule => output.push_str("---"),
            HtmlBlock::Table(rows) => append_markdown_table(&mut output, rows),
            HtmlBlock::Image { source, alt, .. } => {
                output.push_str(&format!("[Image: {alt}]"));
                if !source.is_empty() {
                    output.push_str(&format!(" ({source})"));
                }
            }
            HtmlBlock::SectionBreak => {}
        }
        output.push_str("\n\n");
    }
    if output.trim().is_empty() {
        " ".to_string()
    } else {
        output
    }
}

fn append_markdown_table(output: &mut String, rows: &[Vec<String>]) {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 {
        return;
    }
    append_markdown_row(
        output,
        rows.first().map(Vec::as_slice).unwrap_or(&[]),
        columns,
    );
    output.push('\n');
    output.push('|');
    for _ in 0..columns {
        output.push_str(" --- |");
    }
    for row in rows.iter().skip(1) {
        output.push('\n');
        append_markdown_row(output, row, columns);
    }
}

fn append_markdown_row(output: &mut String, row: &[String], columns: usize) {
    output.push('|');
    for column in 0..columns {
        output.push(' ');
        output.push_str(
            &row.get(column)
                .map(String::as_str)
                .unwrap_or("")
                .replace('|', "\\|"),
        );
        output.push_str(" |");
    }
}

fn semantic_slides(document: &SemanticHtml) -> Vec<SemanticSlide> {
    let mut slides = Vec::new();
    let mut current = SemanticSlide::default();
    let mut current_chars = 0usize;
    for (block_index, block) in document.blocks.iter().enumerate() {
        let starts_slide = matches!(block, HtmlBlock::SectionBreak)
            || matches!(block, HtmlBlock::Heading { level: 1, .. }) && !current.is_empty();
        if starts_slide && !current.is_empty() {
            slides.push(std::mem::take(&mut current));
            current_chars = 0;
        }
        if matches!(block, HtmlBlock::SectionBreak) {
            continue;
        }
        if let HtmlBlock::Table(rows) = block {
            if !current.is_empty() {
                slides.push(std::mem::take(&mut current));
            }
            append_semantic_table_slides(&mut slides, block_index, rows);
            current_chars = 0;
            continue;
        }
        if let HtmlBlock::Image {
            source,
            alt,
            geometry,
        } = block
        {
            if resolved_image(document, source, ImageTarget::Office).is_some() {
                if current.images.len() >= 2 && !current.is_empty() {
                    slides.push(std::mem::take(&mut current));
                    current_chars = 0;
                }
                current
                    .images
                    .push((source.clone(), alt.clone(), *geometry));
                continue;
            }
        }
        for line in block_lines(block) {
            let line_chars = line.chars().count();
            if current_chars + line_chars > 1_600 && !current.is_empty() {
                slides.push(std::mem::take(&mut current));
                current_chars = 0;
            }
            current_chars += line_chars + 1;
            current.lines.push(line);
        }
    }
    if !current.is_empty() {
        slides.push(current);
    }
    if slides.is_empty() {
        slides.push(SemanticSlide {
            lines: vec![document.title.clone()],
            images: Vec::new(),
            table: None,
        });
    }
    slides
}

fn append_semantic_table_slides(
    slides: &mut Vec<SemanticSlide>,
    block_index: usize,
    rows: &[Vec<String>],
) {
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    if rows.is_empty() || column_count == 0 {
        return;
    }
    let body_rows_per_slide = MAX_PPT_TABLE_ROWS_PER_SLIDE.saturating_sub(1).max(1);
    for column_start in (0..column_count).step_by(MAX_PPT_TABLE_COLUMNS_PER_SLIDE) {
        let column_end = (column_start + MAX_PPT_TABLE_COLUMNS_PER_SLIDE).min(column_count);
        if rows.len() == 1 {
            slides.push(SemanticSlide {
                table: Some(SemanticTableWindow {
                    block_index,
                    row_start: 1,
                    row_end: 1,
                    column_start,
                    column_end,
                    include_header: true,
                }),
                ..SemanticSlide::default()
            });
            continue;
        }
        for row_start in (1..rows.len()).step_by(body_rows_per_slide) {
            slides.push(SemanticSlide {
                table: Some(SemanticTableWindow {
                    block_index,
                    row_start,
                    row_end: (row_start + body_rows_per_slide).min(rows.len()),
                    column_start,
                    column_end,
                    include_header: true,
                }),
                ..SemanticSlide::default()
            });
        }
    }
}

fn block_lines(block: &HtmlBlock) -> Vec<String> {
    match block {
        HtmlBlock::Heading { text, .. }
        | HtmlBlock::Paragraph(text)
        | HtmlBlock::Preformatted(text)
        | HtmlBlock::Quote(text) => text.lines().map(str::to_string).collect(),
        HtmlBlock::ListItem {
            ordered,
            ordinal,
            text,
        } => vec![if *ordered {
            format!("{ordinal}. {text}")
        } else {
            format!("• {text}")
        }],
        HtmlBlock::Rule => vec!["────────────────────".to_string()],
        HtmlBlock::Table(rows) => rows.iter().map(|row| row.join(" | ")).collect(),
        HtmlBlock::Image { source, alt, .. } => vec![if source.is_empty() {
            format!("[Image: {alt}]")
        } else {
            format!("[Image: {alt}] ({source})")
        }],
        HtmlBlock::SectionBreak => Vec::new(),
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for character in text.chars() {
        let character_width = semantic_pdf_character_width(character);
        if character == '\n'
            || (!current.is_empty() && current_width.saturating_add(character_width) > width)
        {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
            if character == '\n' {
                continue;
            }
        }
        current.push(character);
        current_width = current_width.saturating_add(character_width);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn semantic_pdf_character_width(character: char) -> usize {
    match character as u32 {
        0x1100..=0x115f
        | 0x2329..=0x232a
        | 0x2e80..=0xa4cf
        | 0xac00..=0xd7a3
        | 0xf900..=0xfaff
        | 0xfe10..=0xfe19
        | 0xfe30..=0xfe6f
        | 0xff00..=0xff60
        | 0xffe0..=0xffe6
        | 0x1f300..=0x1faff
        | 0x20000..=0x3fffd => 2,
        _ => 1,
    }
}

fn column_letters(mut column: usize) -> String {
    let mut letters = Vec::new();
    while column > 0 {
        column -= 1;
        letters.push((b'A' + (column % 26) as u8) as char);
        column /= 26;
    }
    letters.iter().rev().collect()
}

fn finish_table_cell(table: &mut TableBuilder) {
    if let Some(cell) = table.cell.take() {
        table.row.push(cell.trim().to_string());
    }
}

fn finish_table_row(table: &mut TableBuilder) -> Result<(), HandlerError> {
    finish_table_cell(table);
    if !table.row.is_empty() {
        if table.rows.len() >= MAX_EXCEL_ROWS {
            return Err(HandlerError::InvalidArgument(format!(
                "HTML table exceeds {MAX_EXCEL_ROWS} rows"
            )));
        }
        table.rows.push(std::mem::take(&mut table.row));
    }
    Ok(())
}

fn ensure_text_limit(text: &str) -> Result<(), HandlerError> {
    if text.len() > MAX_BLOCK_BYTES {
        return Err(HandlerError::InvalidArgument(format!(
            "HTML text node exceeds {MAX_BLOCK_BYTES} bytes"
        )));
    }
    if text
        .chars()
        .any(|character| character < '\u{20}' && !matches!(character, '\t' | '\n' | '\r'))
    {
        return Err(HandlerError::InvalidArgument(
            "HTML contains a control character that XML document formats cannot represent"
                .to_string(),
        ));
    }
    Ok(())
}

fn escape_xml_text(value: &str) -> Result<String, HandlerError> {
    ensure_text_limit(value)?;
    Ok(value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;"))
}

fn append_normalized(target: &mut String, value: &str, preserve: bool) {
    if preserve {
        target.push_str(value);
        return;
    }
    let leading_space = value.chars().next().is_some_and(char::is_whitespace);
    let trailing_space = value.chars().next_back().is_some_and(char::is_whitespace);
    for (index, piece) in value.split_whitespace().enumerate() {
        if !target.is_empty()
            && (leading_space || index > 0)
            && !target.chars().next_back().is_some_and(char::is_whitespace)
        {
            target.push(' ');
        }
        target.push_str(piece);
    }
    if trailing_space
        && !target.is_empty()
        && !target.chars().next_back().is_some_and(char::is_whitespace)
    {
        target.push(' ');
    }
}

fn parse_tag(raw: &str) -> Option<Tag> {
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
        .find(|character: char| character.is_whitespace() || character == '/')
        .unwrap_or(content.len());
    let name = content[..name_end].to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    Some(Tag {
        name,
        attributes: if closing {
            HashMap::new()
        } else {
            parse_attributes(&content[name_end..])
        },
        closing,
        self_closing,
    })
}

fn parse_attributes(source: &str) -> HashMap<String, String> {
    let bytes = source.as_bytes();
    let mut attributes = HashMap::new();
    let mut index = 0usize;
    while index < bytes.len() {
        while index < bytes.len() && (bytes[index].is_ascii_whitespace() || bytes[index] == b'/') {
            index += 1;
        }
        let key_start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && !matches!(bytes[index], b'=' | b'/')
        {
            index += 1;
        }
        if key_start == index {
            break;
        }
        let key = source[key_start..index].to_ascii_lowercase();
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
                let start = index;
                while index < bytes.len() && bytes[index] != quote {
                    index += 1;
                }
                value = decode_entities(&source[start..index]);
                index = (index + 1).min(bytes.len());
            } else {
                let start = index;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && bytes[index] != b'/'
                {
                    index += 1;
                }
                value = decode_entities(&source[start..index]);
            }
        }
        attributes.insert(key, value);
    }
    attributes
}

fn decode_entities(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '&' {
            output.push(character);
            continue;
        }
        let mut entity = String::new();
        let mut terminated = false;
        while entity.len() <= 16 {
            match chars.peek().copied() {
                Some(';') => {
                    chars.next();
                    terminated = true;
                    break;
                }
                Some(next) if next.is_ascii_alphanumeric() || matches!(next, '#' | 'x' | 'X') => {
                    entity.push(next);
                    chars.next();
                }
                _ => break,
            }
        }
        if terminated {
            if let Some(decoded) = decode_entity(&entity) {
                output.push(decoded);
            } else {
                output.push('&');
                output.push_str(&entity);
                output.push(';');
            }
        } else {
            output.push('&');
            output.push_str(&entity);
        }
    }
    output
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" | "ensp" | "emsp" => Some(' '),
        "ndash" => Some('–'),
        "mdash" => Some('—'),
        "hellip" => Some('…'),
        "copy" => Some('©'),
        "reg" => Some('®'),
        value if value.starts_with("#x") || value.starts_with("#X") => {
            u32::from_str_radix(&value[2..], 16)
                .ok()
                .and_then(char::from_u32)
        }
        value if value.starts_with('#') => value[1..].parse().ok().and_then(char::from_u32),
        _ => None,
    }
}

fn find_tag_end(bytes: &[u8], mut index: usize) -> Option<usize> {
    let mut quote = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if matches!(byte, b'\'' | b'"') {
            if quote == Some(byte) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(byte);
            }
        } else if byte == b'>' && quote.is_none() {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn find_ascii_case_insensitive(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
}

fn safe_link(target: &str) -> bool {
    let normalized = target.trim().to_ascii_lowercase();
    normalized.starts_with("http://")
        || normalized.starts_with("https://")
        || normalized.starts_with("mailto:")
        || normalized.starts_with('#')
        || (!normalized.contains(':') && !normalized.starts_with("//"))
}

fn is_void_element(name: &str) -> bool {
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

fn temporary_output_path(parent: &Path, extension: &str) -> PathBuf {
    parent.join(format!(
        ".officecli-html-{}-{}.{}",
        std::process::id(),
        uuid::Uuid::new_v4(),
        extension
    ))
}

fn publish_output(temporary: &Path, output: &Path) -> Result<(), HandlerError> {
    match std::fs::rename(temporary, output) {
        Ok(()) => return Ok(()),
        Err(error) if !output.exists() => {
            let _ = std::fs::remove_file(temporary);
            return Err(HandlerError::IoError(error));
        }
        Err(_) => {}
    }

    let backup = output.with_file_name(format!(
        ".officecli-html-backup-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::rename(output, &backup).map_err(HandlerError::IoError)?;
    match std::fs::rename(temporary, output) {
        Ok(()) => {
            let _ = std::fs::remove_file(backup);
            Ok(())
        }
        Err(error) => {
            let _ = std::fs::rename(&backup, output);
            let _ = std::fs::remove_file(temporary);
            Err(HandlerError::IoError(error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_pdf_wrap_counts_east_asian_characters_as_wide() {
        let text = "中".repeat(42);
        let lines = wrap_text(&text, 82);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].chars().count(), 41);
        assert_eq!(lines[1], "中");
    }

    #[test]
    fn hcd_pdf_fragments_on_the_same_visual_line_are_coalesced() {
        let html = r#"<section class="hcd-pdf-page" data-hcd-page="1"><p class="hcd-pdf-text" data-hcd-x="90" data-hcd-y="700" data-hcd-width="12" data-hcd-height="12"><span>整</span></p><p class="hcd-pdf-text" data-hcd-x="150" data-hcd-y="700" data-hcd-width="12" data-hcd-height="12"><span>改</span></p><p class="hcd-pdf-text" data-hcd-x="90" data-hcd-y="680" data-hcd-width="30" data-hcd-height="12"><span>Hello</span></p><p class="hcd-pdf-text" data-hcd-x="140" data-hcd-y="680" data-hcd-width="30" data-hcd-height="12"><span>world</span></p></section>"#;
        let document = parse_html(html).unwrap();
        assert_eq!(
            document.blocks,
            vec![
                HtmlBlock::Paragraph("整改".to_string()),
                HtmlBlock::Paragraph("Hello world".to_string()),
            ]
        );
    }

    #[test]
    fn semantic_pdf_font_replaces_only_missing_characters() {
        let selection = SemanticPdfFontSelection {
            path: PathBuf::from("unused.ttf"),
            missing: HashSet::from(['ə', 'ʊ']),
            replacement: '?',
        };
        assert_eq!(
            sanitize_semantic_pdf_text("中文 əʊ text", Some(&selection)),
            "中文 ?? text"
        );
    }

    #[test]
    fn parses_semantic_blocks_tables_entities_and_safe_links() {
        let html = r#"<!doctype html><html><head><title>Demo &amp; Test</title><style>bad</style></head><body><section><h1>标题 😀</h1><p>Hello <b>World</b> &lt;ok&gt; <a href="https://example.com">site</a></p><ul><li>One</li><li>Two</li></ul><table><tr><th>Name</th><th>Value</th></tr><tr><td>A</td><td>123</td></tr></table><script>alert(1)</script></section></body></html>"#;
        let document = parse_html(html).unwrap();
        assert_eq!(document.title, "Demo & Test");
        assert!(document.blocks.contains(&HtmlBlock::Heading {
            level: 1,
            text: "标题 😀".to_string()
        }));
        assert!(document.blocks.contains(&HtmlBlock::Paragraph(
            "Hello World <ok> site (https://example.com)".to_string()
        )));
        assert!(document.blocks.contains(&HtmlBlock::Table(vec![
            vec!["Name".to_string(), "Value".to_string()],
            vec!["A".to_string(), "123".to_string()]
        ])));
        assert!(document
            .warnings
            .iter()
            .any(|warning| warning.contains("script")));
    }

    #[test]
    fn rejects_dangerous_links_and_decodes_numeric_entities() {
        let document =
            parse_html(r#"<p><a href="javascript:alert(1)">safe label</a> &#x1F600; &#20013;</p>"#)
                .unwrap();
        assert_eq!(
            document.blocks,
            vec![HtmlBlock::Paragraph("safe label 😀 中".to_string())]
        );
    }

    #[test]
    fn inline_images_become_standalone_blocks_without_fetching() {
        let document = parse_html(
            r#"<p>Before<img src="https://example.invalid/private.png" alt="safe alt">After</p>"#,
        )
        .unwrap();
        assert_eq!(
            document.blocks,
            vec![
                HtmlBlock::Paragraph("Before".to_string()),
                HtmlBlock::Image {
                    source: "https://example.invalid/private.png".to_string(),
                    alt: "safe alt".to_string(),
                    geometry: ImageGeometry::default(),
                },
                HtmlBlock::Paragraph("After".to_string()),
            ]
        );
        assert!(document
            .warnings
            .contains("inline images were reflowed as standalone semantic blocks"));
        assert!(document.assets.is_empty());
    }

    #[test]
    fn hcd_picture_geometry_is_inherited_and_bounded() {
        let document = parse_html(
            r#"<div class="hcd-slide-picture" data-hcd-x-emu="914400" data-hcd-y-emu="1828800" data-hcd-width-emu="2743200" data-hcd-height-emu="1371600"><img src="asset://sha256/abc" alt="pixel"/></div><img src="asset://sha256/def" data-hcd-width-emu="100000001" data-hcd-height-emu="0"/>"#,
        )
        .unwrap();
        assert_eq!(
            document.blocks[0],
            HtmlBlock::Image {
                source: "asset://sha256/abc".to_string(),
                alt: "pixel".to_string(),
                geometry: ImageGeometry {
                    x_emu: Some(914_400),
                    y_emu: Some(1_828_800),
                    width_emu: Some(2_743_200),
                    height_emu: Some(1_371_600),
                },
            }
        );
        assert!(matches!(
            document.blocks[1],
            HtmlBlock::Image {
                geometry: ImageGeometry {
                    width_emu: None,
                    height_emu: None,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn large_tables_are_windowed_without_cloning_into_text_lines() {
        let rows = (0..40)
            .map(|row| {
                (0..14)
                    .map(|column| format!("R{row}C{column}"))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let document = SemanticHtml {
            blocks: vec![HtmlBlock::Table(rows)],
            ..SemanticHtml::default()
        };
        let slides = semantic_slides(&document);
        assert_eq!(slides.len(), 6);
        assert!(slides.iter().all(|slide| slide.lines.is_empty()));
        for slide in slides {
            let window = slide.table.unwrap();
            assert!(window.row_end - window.row_start + usize::from(window.include_header) <= 18);
            assert!(window.column_end - window.column_start <= 12);
        }
    }

    #[test]
    fn contiguous_hcd_table_fragments_reassemble_before_semantic_layout() {
        let table_id = "n_0123456789abcdef0123456789abcdef";
        let html = format!(
            r#"<section data-hcd-source-part="ppt/slides/slide1.xml"><p>Before</p><table data-hcd-table-node-id="{table_id}" data-hcd-table-fragment="0" data-hcd-row-start="1" data-hcd-row-end="2" data-hcd-fragment-row-count="2" data-hcd-column-count="2"><tr><td>H1</td><td>H2</td></tr><tr><td>A1</td><td>A2</td></tr></table></section><section data-hcd-source-part="ppt/slides/slide1.xml"><table data-hcd-table-node-id="{table_id}" data-hcd-table-fragment="1" data-hcd-row-start="3" data-hcd-row-end="3" data-hcd-fragment-row-count="1" data-hcd-column-count="2" data-hcd-table-continuation="true" data-hcd-table-final="true" data-hcd-row-count="3"><tr><td>B1</td><td>B2</td></tr></table><p>After</p></section>"#
        );
        let document = parse_html(&html).unwrap();
        assert_eq!(
            document.blocks,
            vec![
                HtmlBlock::Paragraph("Before".to_string()),
                HtmlBlock::Table(vec![
                    vec!["H1".to_string(), "H2".to_string()],
                    vec!["A1".to_string(), "A2".to_string()],
                    vec!["B1".to_string(), "B2".to_string()],
                ]),
                HtmlBlock::Paragraph("After".to_string()),
            ]
        );
        assert!(document.warnings.contains(&format!(
            "reassembled HCD table {table_id} from 2 contiguous fragments"
        )));
    }

    #[test]
    fn malformed_hcd_table_fragment_sequence_fails_before_export() {
        let table_id = "n_0123456789abcdef0123456789abcdef";
        let html = format!(
            r#"<table data-hcd-table-node-id="{table_id}" data-hcd-table-fragment="0" data-hcd-row-start="1" data-hcd-row-end="1" data-hcd-fragment-row-count="1" data-hcd-column-count="1"><tr><td>A</td></tr></table><table data-hcd-table-node-id="{table_id}" data-hcd-table-fragment="2" data-hcd-row-start="2" data-hcd-row-end="2" data-hcd-fragment-row-count="1" data-hcd-column-count="1" data-hcd-table-continuation="true" data-hcd-table-final="true" data-hcd-row-count="2"><tr><td>B</td></tr></table>"#
        );
        let error = parse_html(&html)
            .err()
            .expect("fragment sequence must fail");
        assert!(error
            .to_string()
            .contains("missing, duplicated, out of order, or inconsistent"));
    }

    #[test]
    fn column_names_cover_excel_boundaries() {
        assert_eq!(column_letters(1), "A");
        assert_eq!(column_letters(26), "Z");
        assert_eq!(column_letters(27), "AA");
        assert_eq!(column_letters(16_384), "XFD");
    }
}
