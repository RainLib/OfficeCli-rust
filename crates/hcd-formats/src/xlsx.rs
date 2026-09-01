use crate::common::{
    base_manifest, checked_export_state, collect_dirty_nodes, emit_failed, emit_started,
    escape_attribute, escape_text, finish_import, source_identity, write_fidelity_report,
    ExportOptions, ImportOptions, XmlBudget,
};
use hcd_core::{
    hash_bytes, stable_node_id, Bundle, BundleWriter, ChunkSourceMap, FidelityLevel,
    FidelityReport, FidelityWarning, HcdError, HcdManifest, ImportEvent, NodeMapEntry,
    SourceAnchor, HCD_SCHEMA_VERSION, MAX_CHUNK_BYTES,
};
use oxml::{PackageError, StreamingOxmlArchive, StreamingOxmlRewriter};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use serde::Serialize;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use time::{Date, Duration, Month};

const MAX_CONTROL_BYTES: u64 = 16 * 1024 * 1024;
const ROWS_PER_WINDOW: usize = 128;
const MAX_MERGED_RANGES: usize = 1_000_000;
const MAX_FORMAT_CODE_BYTES: usize = 1_024;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssetRecord {
    source_part: String,
    hash: String,
    href: String,
    byte_length: u64,
}

#[derive(Debug)]
struct SheetPart {
    name: String,
    part: String,
}

struct WorkbookInfo {
    sheets: Vec<SheetPart>,
    date_1904: bool,
}

#[derive(Default)]
struct CellBuilder {
    reference: String,
    value_type: String,
    style_index: Option<usize>,
    formula: bool,
    value: String,
    inline_text: String,
    capture: Capture,
}

#[derive(Default, PartialEq, Eq)]
enum Capture {
    #[default]
    None,
    Value,
    InlineText,
}

#[derive(Default)]
struct RenderedRow {
    number: u64,
    html: String,
    entries: Vec<NodeMapEntry>,
    merge_end_row: Option<u32>,
    cells: Vec<(u32, String)>,
    merge_anchors: Vec<MergeRange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MergeRange {
    start_row: u32,
    end_row: u32,
    start_col: u32,
    end_col: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MergePosition {
    Anchor(MergeRange),
    Covered(MergeRange),
}

struct MergeCursor {
    ranges: Vec<MergeRange>,
    next_range: usize,
    current_row: Option<u32>,
    active_by_column: BTreeMap<u32, usize>,
    expirations: BinaryHeap<Reverse<(u32, usize)>>,
}

struct WorksheetScan {
    merged_ranges: MergeCursor,
    view: WorksheetViewMetadata,
}

#[derive(Clone, Default)]
struct WorksheetViewMetadata {
    workbook_view_id: Option<u32>,
    view: Option<&'static str>,
    top_left_cell: Option<String>,
    right_to_left: Option<bool>,
    show_grid_lines: Option<bool>,
    show_row_column_headers: Option<bool>,
    show_zeros: Option<bool>,
    show_formulas: Option<bool>,
    zoom_scale: Option<u16>,
    pane: Option<WorksheetPaneMetadata>,
}

#[derive(Clone)]
struct WorksheetPaneMetadata {
    state: &'static str,
    x_split: Option<f64>,
    y_split: Option<f64>,
    top_left_cell: Option<String>,
    active_pane: Option<&'static str>,
}

impl WorksheetViewMetadata {
    fn html_attributes(&self) -> String {
        let mut attributes = String::new();
        push_data_attribute(&mut attributes, "data-hcd-sheet-view", self.view);
        push_data_attribute(
            &mut attributes,
            "data-hcd-view-top-left-cell",
            self.top_left_cell.as_deref(),
        );
        push_data_bool(
            &mut attributes,
            "data-hcd-right-to-left",
            self.right_to_left,
        );
        push_data_bool(
            &mut attributes,
            "data-hcd-show-grid-lines",
            self.show_grid_lines,
        );
        push_data_bool(
            &mut attributes,
            "data-hcd-show-row-column-headers",
            self.show_row_column_headers,
        );
        push_data_bool(&mut attributes, "data-hcd-show-zeros", self.show_zeros);
        push_data_bool(
            &mut attributes,
            "data-hcd-show-formulas",
            self.show_formulas,
        );
        push_data_number(&mut attributes, "data-hcd-zoom-percent", self.zoom_scale);
        if self.right_to_left == Some(true) {
            attributes.push_str(" style=\"direction:rtl\"");
        }
        if let Some(pane) = &self.pane {
            push_data_attribute(&mut attributes, "data-hcd-pane-state", Some(pane.state));
            push_data_attribute(
                &mut attributes,
                "data-hcd-pane-top-left-cell",
                pane.top_left_cell.as_deref(),
            );
            push_data_attribute(&mut attributes, "data-hcd-active-pane", pane.active_pane);
            if matches!(pane.state, "frozen" | "frozen-split") {
                push_data_number(
                    &mut attributes,
                    "data-hcd-frozen-columns",
                    pane.x_split
                        .and_then(|value| frozen_split_count(value, 16_384)),
                );
                push_data_number(
                    &mut attributes,
                    "data-hcd-frozen-rows",
                    pane.y_split
                        .and_then(|value| frozen_split_count(value, 1_048_576)),
                );
            } else {
                push_data_decimal(&mut attributes, "data-hcd-split-x-twips", pane.x_split);
                push_data_decimal(&mut attributes, "data-hcd-split-y-twips", pane.y_split);
            }
        }
        attributes
    }
}

fn push_data_attribute(output: &mut String, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        output.push(' ');
        output.push_str(name);
        output.push_str("=\"");
        output.push_str(&escape_attribute(value));
        output.push('"');
    }
}

fn push_data_bool(output: &mut String, name: &str, value: Option<bool>) {
    push_data_attribute(
        output,
        name,
        value.map(|value| if value { "true" } else { "false" }),
    );
}

fn push_data_number<T: std::fmt::Display>(output: &mut String, name: &str, value: Option<T>) {
    if let Some(value) = value {
        let value = value.to_string();
        push_data_attribute(output, name, Some(&value));
    }
}

fn push_data_decimal(output: &mut String, name: &str, value: Option<f64>) {
    if let Some(value) = value {
        let value = format!("{value:.2}");
        push_data_attribute(output, name, Some(&value));
    }
}

impl MergeCursor {
    fn new(mut ranges: Vec<MergeRange>) -> Self {
        ranges.sort_unstable_by_key(|range| {
            (
                range.start_row,
                range.start_col,
                range.end_row,
                range.end_col,
            )
        });
        Self {
            ranges,
            next_range: 0,
            current_row: None,
            active_by_column: BTreeMap::new(),
            expirations: BinaryHeap::new(),
        }
    }

    fn begin_row(&mut self, row: u32) -> Result<(), HcdError> {
        if self.current_row.is_some_and(|current| row <= current) {
            return Err(HcdError::InvalidBundle(format!(
                "worksheet rows are duplicate or out of order: {row} follows {}",
                self.current_row.unwrap_or_default()
            )));
        }
        while let Some(Reverse((end_row, index))) = self.expirations.peek().copied() {
            if end_row >= row {
                break;
            }
            self.expirations.pop();
            let start_col = self.ranges[index].start_col;
            if self.active_by_column.get(&start_col) == Some(&index) {
                self.active_by_column.remove(&start_col);
            }
        }
        while let Some(range) = self.ranges.get(self.next_range).copied() {
            if range.start_row > row {
                break;
            }
            let index = self.next_range;
            self.next_range += 1;
            if range.end_row < row {
                continue;
            }
            if let Some((_, active_index)) =
                self.active_by_column.range(..=range.end_col).next_back()
            {
                let active = self.ranges[*active_index];
                if active.end_col >= range.start_col {
                    return Err(HcdError::InvalidBundle(format!(
                        "overlapping XLSX merged ranges {} and {}",
                        merge_reference(active),
                        merge_reference(range)
                    )));
                }
            }
            self.active_by_column.insert(range.start_col, index);
            self.expirations.push(Reverse((range.end_row, index)));
        }
        self.current_row = Some(row);
        Ok(())
    }

    fn classify(&self, row: u32, col: u32) -> Option<MergePosition> {
        if self.current_row != Some(row) {
            return None;
        }
        let (_, index) = self.active_by_column.range(..=col).next_back()?;
        let range = self.ranges[*index];
        if col > range.end_col {
            return None;
        }
        if row == range.start_row && col == range.start_col {
            Some(MergePosition::Anchor(range))
        } else {
            Some(MergePosition::Covered(range))
        }
    }

    fn current_row_anchors(&self) -> Vec<MergeRange> {
        let Some(row) = self.current_row else {
            return Vec::new();
        };
        self.active_by_column
            .values()
            .map(|index| self.ranges[*index])
            .filter(|range| range.start_row == row)
            .collect()
    }
}

#[derive(Default)]
struct XlsxStyleCatalog {
    fonts: Vec<XlsxFont>,
    fills: Vec<XlsxFill>,
    borders: Vec<XlsxBorder>,
    cell_formats: Vec<XlsxCellFormat>,
    number_formats: HashMap<u32, String>,
}

#[derive(Default)]
struct XlsxFormatStats {
    formatted_cells: u64,
    approximate_cells: u64,
}

struct FormattedCell {
    text: String,
    kind: &'static str,
    num_fmt_id: Option<u32>,
    approximate: bool,
}

#[derive(Default)]
struct XlsxFont {
    name: Option<String>,
    size_points: Option<f64>,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    color: Option<String>,
}

#[derive(Default)]
struct XlsxFill {
    solid: bool,
    color: Option<String>,
}

#[derive(Default)]
struct XlsxBorder {
    left: Option<XlsxBorderSide>,
    right: Option<XlsxBorderSide>,
    top: Option<XlsxBorderSide>,
    bottom: Option<XlsxBorderSide>,
}

#[derive(Default)]
struct XlsxBorderSide {
    style: String,
    color: Option<String>,
}

#[derive(Default)]
struct XlsxCellFormat {
    font_id: Option<usize>,
    fill_id: Option<usize>,
    border_id: Option<usize>,
    num_fmt_id: Option<u32>,
    horizontal: Option<String>,
    vertical: Option<String>,
    wrap_text: bool,
}

struct SheetChunkWriter<'a, F>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    document_id: &'a str,
    sheet_name: &'a str,
    part: &'a str,
    writer: &'a mut BundleWriter,
    emit: &'a mut F,
    soft_bytes: usize,
    max_rows: usize,
    ordinal: usize,
    rows: usize,
    first_row: Option<u64>,
    last_row: Option<u64>,
    html: String,
    entries: Vec<NodeMapEntry>,
    column_markup: String,
    default_column_width: Option<f64>,
    view_attributes: String,
    hold_until_row: Option<u32>,
}

impl<'a, F> SheetChunkWriter<'a, F>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    fn new(
        document_id: &'a str,
        sheet_name: &'a str,
        part: &'a str,
        options: &ImportOptions,
        view: &WorksheetViewMetadata,
        writer: &'a mut BundleWriter,
        emit: &'a mut F,
    ) -> Self {
        Self {
            document_id,
            sheet_name,
            part,
            writer,
            emit,
            soft_bytes: options.chunk_soft_bytes.min(MAX_CHUNK_BYTES),
            max_rows: options.chunk_blocks.clamp(1, ROWS_PER_WINDOW),
            ordinal: 0,
            rows: 0,
            first_row: None,
            last_row: None,
            html: String::new(),
            entries: Vec::new(),
            column_markup: String::new(),
            default_column_width: None,
            view_attributes: view.html_attributes(),
            hold_until_row: None,
        }
    }

    fn push(&mut self, row: RenderedRow) -> Result<(), HcdError> {
        if row.html.len() > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: XLSX row {} in {} is {} bytes",
                row.number,
                self.part,
                row.html.len()
            )));
        }
        if self
            .hold_until_row
            .is_some_and(|end_row| row.number > u64::from(end_row))
        {
            self.hold_until_row = None;
        }
        let inside_merge_group = self
            .hold_until_row
            .is_some_and(|end_row| row.number <= u64::from(end_row));
        if self.rows > 0
            && !inside_merge_group
            && (self.html.len() + row.html.len() > self.soft_bytes || self.rows >= self.max_rows)
        {
            self.flush()?;
        }
        if self.html.len() + row.html.len() > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: XLSX merged row group ending at row {} in {} exceeds 2 MiB",
                self.hold_until_row
                    .unwrap_or_else(|| u32::try_from(row.number).unwrap_or(u32::MAX)),
                self.part
            )));
        }
        self.first_row.get_or_insert(row.number);
        self.last_row = Some(row.number);
        if let Some(end_row) = row.merge_end_row {
            self.hold_until_row = Some(self.hold_until_row.unwrap_or(0).max(end_row));
        }
        self.rows += 1;
        self.html.push_str(&row.html);
        self.entries.extend(row.entries);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), HcdError> {
        if self.rows == 0 {
            return Ok(());
        }
        let first = self.first_row.unwrap_or(0);
        let last = self.last_row.unwrap_or(first);
        let chunk_id = stable_node_id(&[
            self.document_id,
            self.part,
            "sheet-window",
            &self.ordinal.to_string(),
        ])
        .replacen("n_", "c_", 1);
        let default_width = self
            .default_column_width
            .map(|width| format!(" data-hcd-default-column-width=\"{width:.2}\""))
            .unwrap_or_default();
        let html = format!(
            "<section class=\"hcd-sheet\" data-hcd-sheet=\"{}\" data-hcd-row-start=\"{}\" data-hcd-row-end=\"{}\"{}{}><table class=\"hcd-grid\"><colgroup>{}</colgroup><tbody>{}</tbody></table></section>",
            escape_attribute(self.sheet_name),
            first,
            last,
            default_width,
            self.view_attributes,
            self.column_markup,
            self.html
        );
        let map = ChunkSourceMap {
            schema_version: HCD_SCHEMA_VERSION.to_string(),
            chunk_id: chunk_id.clone(),
            entries: std::mem::take(&mut self.entries),
        };
        let descriptor = self.writer.write_chunk(
            chunk_id,
            "sheet".to_string(),
            html,
            map,
            self.rows,
            self.ordinal > 0,
        )?;
        (self.emit)(&ImportEvent::ChunkReady { descriptor })?;
        self.ordinal += 1;
        self.rows = 0;
        self.first_row = None;
        self.last_row = None;
        self.hold_until_row = None;
        self.html.clear();
        Ok(())
    }

    fn finish(&mut self) -> Result<(), HcdError> {
        if self.rows > 0 {
            return self.flush();
        }
        if self.ordinal > 0 {
            return Ok(());
        }
        let chunk_id = stable_node_id(&[
            self.document_id,
            self.part,
            "sheet-window",
            &self.ordinal.to_string(),
        ])
        .replacen("n_", "c_", 1);
        let default_width = self
            .default_column_width
            .map(|width| format!(" data-hcd-default-column-width=\"{width:.2}\""))
            .unwrap_or_default();
        let html = format!(
            "<section class=\"hcd-sheet\" data-hcd-sheet=\"{}\"{}{}><table class=\"hcd-grid\"><colgroup>{}</colgroup><tbody></tbody></table></section>",
            escape_attribute(self.sheet_name),
            default_width,
            self.view_attributes,
            self.column_markup
        );
        let map = ChunkSourceMap {
            schema_version: HCD_SCHEMA_VERSION.to_string(),
            chunk_id: chunk_id.clone(),
            entries: Vec::new(),
        };
        let descriptor =
            self.writer
                .write_chunk(chunk_id, "sheet".to_string(), html, map, 1, false)?;
        (self.emit)(&ImportEvent::ChunkReady { descriptor })?;
        self.ordinal += 1;
        Ok(())
    }
}

struct SharedStringStore {
    values: File,
    offsets: File,
    count: u64,
}

impl SharedStringStore {
    fn empty(directory: &Path) -> Result<Self, HcdError> {
        Ok(Self {
            values: OpenOptions::new()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(directory.join("shared-values.bin"))?,
            offsets: OpenOptions::new()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(directory.join("shared-offsets.bin"))?,
            count: 0,
        })
    }

    fn build(archive: &mut StreamingOxmlArchive, directory: &Path) -> Result<Self, HcdError> {
        let mut store = Self::empty(directory)?;
        if !archive.contains("xl/sharedStrings.xml") {
            return Ok(store);
        }
        archive
            .with_part("xl/sharedStrings.xml", |source| {
                let mut reader = Reader::from_reader(BufReader::with_capacity(64 * 1024, source));
                reader.config_mut().check_end_names = true;
                let mut buffer = Vec::with_capacity(64 * 1024);
                let mut current: Option<String> = None;
                let mut in_text = false;
                let mut budget = XmlBudget::default();
                loop {
                    let event = reader.read_event_into(&mut buffer).map_err(|error| {
                        PackageError::ReadPartError(format!("sharedStrings XML: {error}"))
                    })?;
                    budget
                        .observe(&event, "xl/sharedStrings.xml")
                        .map_err(|error| PackageError::ReadPartError(error.to_string()))?;
                    match event {
                        Event::Start(ref start) if local_name(start.name().as_ref()) == "si" => {
                            current = Some(String::new());
                        }
                        Event::Start(ref start) if local_name(start.name().as_ref()) == "t" => {
                            in_text = current.is_some();
                        }
                        Event::Text(text) if in_text => {
                            let decoded = text.unescape().map_err(|error| {
                                PackageError::ReadPartError(format!("shared string text: {error}"))
                            })?;
                            let value = current.as_mut().expect("in_text requires a string");
                            value.push_str(&decoded);
                            if value.len() > MAX_CHUNK_BYTES {
                                return Err(PackageError::ReadPartError(
                                    "NODE_TOO_LARGE: shared string exceeds 2 MiB".to_string(),
                                ));
                            }
                        }
                        Event::End(ref end) if local_name(end.name().as_ref()) == "t" => {
                            in_text = false;
                        }
                        Event::End(ref end) if local_name(end.name().as_ref()) == "si" => {
                            let value = current.take().unwrap_or_default();
                            store
                                .push(&value)
                                .map_err(|error| PackageError::ReadPartError(error.to_string()))?;
                        }
                        Event::Eof => {
                            budget
                                .finish("xl/sharedStrings.xml")
                                .map_err(|error| PackageError::ReadPartError(error.to_string()))?;
                            break;
                        }
                        _ => {}
                    }
                    buffer.clear();
                }
                Ok(())
            })
            .map_err(package_error)?;
        Ok(store)
    }

    fn push(&mut self, value: &str) -> Result<(), HcdError> {
        let offset = self.values.seek(SeekFrom::End(0))?;
        self.offsets.write_all(&offset.to_le_bytes())?;
        self.values.write_all(&(value.len() as u64).to_le_bytes())?;
        self.values.write_all(value.as_bytes())?;
        self.count += 1;
        Ok(())
    }

    fn get(&mut self, index: u64) -> Result<String, HcdError> {
        if index >= self.count {
            return Err(HcdError::InvalidBundle(format!(
                "shared string index {index} exceeds {} entries",
                self.count
            )));
        }
        self.offsets.seek(SeekFrom::Start(index * 8))?;
        let mut encoded = [0u8; 8];
        self.offsets.read_exact(&mut encoded)?;
        let offset = u64::from_le_bytes(encoded);
        self.values.seek(SeekFrom::Start(offset))?;
        self.values.read_exact(&mut encoded)?;
        let length = u64::from_le_bytes(encoded);
        if length > MAX_CHUNK_BYTES as u64 {
            return Err(HcdError::ResourceLimit(
                "NODE_TOO_LARGE: shared string exceeds 2 MiB".to_string(),
            ));
        }
        let mut bytes = vec![0u8; length as usize];
        self.values.read_exact(&mut bytes)?;
        String::from_utf8(bytes)
            .map_err(|error| HcdError::InvalidBundle(format!("shared string UTF-8: {error}")))
    }
}

fn render_xlsx_styles(
    archive: &mut StreamingOxmlArchive,
) -> Result<(String, XlsxStyleCatalog), HcdError> {
    let mut css = String::from(
        ".hcd-grid{border-collapse:collapse}.hcd-grid td{border:1px solid #ddd;padding:.2em .35em}.hcd-grid span{white-space:pre-wrap}.hcd-sheet[data-hcd-show-grid-lines=\"false\"] .hcd-grid td{border-color:transparent}",
    );
    if !archive.contains("xl/styles.xml") {
        return Ok((css, XlsxStyleCatalog::default()));
    }
    let xml = archive
        .read_control_part("xl/styles.xml", MAX_CONTROL_BYTES)
        .map_err(package_error)?;
    let catalog = parse_xlsx_styles(&xml)?;
    for (index, format) in catalog.cell_formats.iter().enumerate() {
        let declarations = xlsx_format_declarations(format, &catalog);
        if !declarations.is_empty() {
            css.push_str(&format!(".hcd-xs-{index}{{{}}}", declarations.join(";")));
        }
    }
    hcd_core::validate_css_text(&css)?;
    Ok((css, catalog))
}

fn parse_xlsx_styles(xml: &[u8]) -> Result<XlsxStyleCatalog, HcdError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut section: Option<(&'static str, usize)> = None;
    let mut catalog = XlsxStyleCatalog::default();
    let mut font: Option<XlsxFont> = None;
    let mut fill: Option<XlsxFill> = None;
    let mut border: Option<XlsxBorder> = None;
    let mut border_edge: Option<&'static str> = None;
    let mut cell_format: Option<XlsxCellFormat> = None;

    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| HcdError::InvalidBundle(format!("invalid xl/styles.xml: {error}")))?;
        match event {
            Event::Start(ref element) => {
                depth += 1;
                let qualified_name = element.name();
                let name = local_name(qualified_name.as_ref());
                match name {
                    "numFmts" | "fonts" | "fills" | "borders" | "cellXfs" => {
                        section = Some((
                            match name {
                                "numFmts" => "numFmts",
                                "fonts" => "fonts",
                                "fills" => "fills",
                                "borders" => "borders",
                                _ => "cellXfs",
                            },
                            depth,
                        ));
                    }
                    "numFmt" if section.is_some_and(|(name, _)| name == "numFmts") => {
                        capture_number_format(element, &mut catalog)?;
                    }
                    "font" if section.is_some_and(|(name, _)| name == "fonts") => {
                        font = Some(XlsxFont::default())
                    }
                    "fill" if section.is_some_and(|(name, _)| name == "fills") => {
                        fill = Some(XlsxFill::default())
                    }
                    "border" if section.is_some_and(|(name, _)| name == "borders") => {
                        border = Some(XlsxBorder::default())
                    }
                    "xf" if section.is_some_and(|(name, _)| name == "cellXfs") => {
                        cell_format = Some(parse_cell_format(element));
                    }
                    "left" | "right" | "top" | "bottom" if border.is_some() => {
                        border_edge = Some(match name {
                            "left" => "left",
                            "right" => "right",
                            "top" => "top",
                            _ => "bottom",
                        });
                        capture_border_side(element, border.as_mut(), border_edge);
                    }
                    _ => capture_xlsx_style_property(
                        element,
                        font.as_mut(),
                        fill.as_mut(),
                        border.as_mut(),
                        border_edge,
                        cell_format.as_mut(),
                    ),
                }
            }
            Event::Empty(ref element) => {
                let qualified_name = element.name();
                let name = local_name(qualified_name.as_ref());
                if name == "numFmt" && section.is_some_and(|(name, _)| name == "numFmts") {
                    capture_number_format(element, &mut catalog)?;
                } else if name == "font" && section.is_some_and(|(name, _)| name == "fonts") {
                    catalog.fonts.push(XlsxFont::default());
                } else if name == "fill" && section.is_some_and(|(name, _)| name == "fills") {
                    catalog.fills.push(XlsxFill::default());
                } else if name == "border" && section.is_some_and(|(name, _)| name == "borders") {
                    catalog.borders.push(XlsxBorder::default());
                } else if name == "xf" && section.is_some_and(|(name, _)| name == "cellXfs") {
                    catalog.cell_formats.push(parse_cell_format(element));
                } else if matches!(name, "left" | "right" | "top" | "bottom") && border.is_some() {
                    let edge = Some(match name {
                        "left" => "left",
                        "right" => "right",
                        "top" => "top",
                        _ => "bottom",
                    });
                    capture_border_side(element, border.as_mut(), edge);
                } else {
                    capture_xlsx_style_property(
                        element,
                        font.as_mut(),
                        fill.as_mut(),
                        border.as_mut(),
                        border_edge,
                        cell_format.as_mut(),
                    );
                }
            }
            Event::End(ref element) => {
                let qualified_name = element.name();
                let name = local_name(qualified_name.as_ref());
                match name {
                    "font" => {
                        if let Some(font) = font.take() {
                            catalog.fonts.push(font);
                        }
                    }
                    "fill" => {
                        if let Some(fill) = fill.take() {
                            catalog.fills.push(fill);
                        }
                    }
                    "border" => {
                        if let Some(border) = border.take() {
                            catalog.borders.push(border);
                        }
                    }
                    "xf" => {
                        if let Some(format) = cell_format.take() {
                            catalog.cell_formats.push(format);
                        }
                    }
                    "left" | "right" | "top" | "bottom" => border_edge = None,
                    _ => {}
                }
                if section.is_some_and(|(_, section_depth)| section_depth == depth) {
                    section = None;
                }
                depth = depth.checked_sub(1).ok_or_else(|| {
                    HcdError::InvalidBundle("unbalanced xl/styles.xml".to_string())
                })?;
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(catalog)
}

fn capture_number_format(
    element: &BytesStart<'_>,
    catalog: &mut XlsxStyleCatalog,
) -> Result<(), HcdError> {
    let Some(id) = attribute(element, "numFmtId").and_then(|value| value.parse::<u32>().ok())
    else {
        return Ok(());
    };
    let Some(code) = attribute(element, "formatCode") else {
        return Ok(());
    };
    if code.len() > MAX_FORMAT_CODE_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "XLSX number format {id} exceeds {MAX_FORMAT_CODE_BYTES} bytes"
        )));
    }
    catalog.number_formats.insert(id, code);
    Ok(())
}

fn parse_cell_format(element: &BytesStart<'_>) -> XlsxCellFormat {
    XlsxCellFormat {
        font_id: attribute(element, "fontId").and_then(|value| value.parse().ok()),
        fill_id: attribute(element, "fillId").and_then(|value| value.parse().ok()),
        border_id: attribute(element, "borderId").and_then(|value| value.parse().ok()),
        num_fmt_id: attribute(element, "numFmtId").and_then(|value| value.parse().ok()),
        ..Default::default()
    }
}

fn capture_xlsx_style_property(
    element: &BytesStart<'_>,
    font: Option<&mut XlsxFont>,
    fill: Option<&mut XlsxFill>,
    border: Option<&mut XlsxBorder>,
    border_edge: Option<&str>,
    cell_format: Option<&mut XlsxCellFormat>,
) {
    let qualified_name = element.name();
    let name = local_name(qualified_name.as_ref());
    if let Some(font) = font {
        match name {
            "name" => font.name = attribute(element, "val"),
            "sz" => font.size_points = attribute(element, "val").and_then(|v| v.parse().ok()),
            "b" => font.bold = xlsx_on_off(element),
            "i" => font.italic = xlsx_on_off(element),
            "u" => font.underline = xlsx_on_off(element),
            "strike" => font.strike = xlsx_on_off(element),
            "color" => font.color = xlsx_rgb(element),
            _ => {}
        }
    }
    if let Some(fill) = fill {
        match name {
            "patternFill" => {
                fill.solid = attribute(element, "patternType").as_deref() == Some("solid")
            }
            "fgColor" if fill.solid => fill.color = xlsx_rgb(element),
            _ => {}
        }
    }
    if name == "color" {
        if let (Some(border), Some(edge)) = (border, border_edge) {
            if let Some(side) = border_side_mut(border, edge) {
                side.color = xlsx_rgb(element);
            }
        }
    }
    if name == "alignment" {
        if let Some(format) = cell_format {
            format.horizontal = attribute(element, "horizontal");
            format.vertical = attribute(element, "vertical");
            format.wrap_text = attribute(element, "wrapText")
                .is_some_and(|value| matches!(value.as_str(), "1" | "true"));
        }
    }
}

fn capture_border_side(
    element: &BytesStart<'_>,
    border: Option<&mut XlsxBorder>,
    edge: Option<&str>,
) {
    let (Some(border), Some(edge), Some(style)) = (border, edge, attribute(element, "style"))
    else {
        return;
    };
    *border_side_slot(border, edge) = Some(XlsxBorderSide { style, color: None });
}

fn border_side_slot<'a>(border: &'a mut XlsxBorder, edge: &str) -> &'a mut Option<XlsxBorderSide> {
    match edge {
        "left" => &mut border.left,
        "right" => &mut border.right,
        "top" => &mut border.top,
        _ => &mut border.bottom,
    }
}

fn border_side_mut<'a>(border: &'a mut XlsxBorder, edge: &str) -> Option<&'a mut XlsxBorderSide> {
    border_side_slot(border, edge).as_mut()
}

fn xlsx_format_declarations(format: &XlsxCellFormat, catalog: &XlsxStyleCatalog) -> Vec<String> {
    let mut css = Vec::new();
    if let Some(font) = format.font_id.and_then(|id| catalog.fonts.get(id)) {
        if let Some(name) = font.name.as_deref().and_then(safe_css_font) {
            css.push(format!("font-family:'{}'", name.replace('\'', "")));
        }
        if let Some(size) = font.size_points.filter(|size| (1.0..=409.0).contains(size)) {
            css.push(format!("font-size:{size:.1}pt"));
        }
        if font.bold {
            css.push("font-weight:700".to_string());
        }
        if font.italic {
            css.push("font-style:italic".to_string());
        }
        let mut decorations = Vec::new();
        if font.underline {
            decorations.push("underline");
        }
        if font.strike {
            decorations.push("line-through");
        }
        if !decorations.is_empty() {
            css.push(format!("text-decoration:{}", decorations.join(" ")));
        }
        if let Some(color) = &font.color {
            css.push(format!("color:#{color}"));
        }
    }
    if let Some(fill) = format.fill_id.and_then(|id| catalog.fills.get(id)) {
        if fill.solid {
            if let Some(color) = &fill.color {
                css.push(format!("background-color:#{color}"));
            }
        }
    }
    if let Some(border) = format.border_id.and_then(|id| catalog.borders.get(id)) {
        for (property, side) in [
            ("border-left", &border.left),
            ("border-right", &border.right),
            ("border-top", &border.top),
            ("border-bottom", &border.bottom),
        ] {
            if let Some(side) = side {
                let (width, line) = xlsx_border_css(&side.style);
                let color = side.color.as_deref().unwrap_or("000000");
                css.push(format!("{property}:{width}px {line} #{color}"));
            }
        }
    }
    if let Some(horizontal) = format.horizontal.as_deref().and_then(xlsx_horizontal) {
        css.push(format!("text-align:{horizontal}"));
    }
    if let Some(vertical) = format.vertical.as_deref().and_then(xlsx_vertical) {
        css.push(format!("vertical-align:{vertical}"));
    }
    if format.wrap_text {
        css.push("white-space:pre-wrap".to_string());
        css.push("overflow-wrap:anywhere".to_string());
    }
    if let Some(num_fmt_id) = format.num_fmt_id {
        css.push(format!("--hcd-num-fmt-id:{num_fmt_id}"));
    }
    css
}

fn format_xlsx_cell(
    raw: &str,
    style_index: Option<usize>,
    is_numeric: bool,
    catalog: &XlsxStyleCatalog,
    date_1904: bool,
) -> FormattedCell {
    let num_fmt_id = style_index
        .and_then(|index| catalog.cell_formats.get(index))
        .and_then(|format| format.num_fmt_id);
    let Some(id) = num_fmt_id else {
        return FormattedCell {
            text: raw.to_string(),
            kind: "general",
            num_fmt_id: None,
            approximate: false,
        };
    };
    if !is_numeric || id == 0 {
        return FormattedCell {
            text: raw.to_string(),
            kind: "general",
            num_fmt_id: Some(id),
            approximate: false,
        };
    }
    let Ok(value) = raw.parse::<f64>() else {
        return FormattedCell {
            text: raw.to_string(),
            kind: "general",
            num_fmt_id: Some(id),
            approximate: true,
        };
    };
    if !value.is_finite() {
        return FormattedCell {
            text: raw.to_string(),
            kind: "general",
            num_fmt_id: Some(id),
            approximate: true,
        };
    }
    let (code, built_in_approximate) = catalog
        .number_formats
        .get(&id)
        .map(|code| (code.as_str(), false))
        .or_else(|| built_in_number_format(id))
        .unwrap_or(("General", true));
    let (section, use_absolute, section_approximate) = select_number_format_section(code, value);
    let value = if use_absolute { value.abs() } else { value };
    let (has_date, has_time) = date_time_format_kind(id, section);
    if has_date || has_time {
        let text =
            format_excel_date_time(value, section, date_1904).unwrap_or_else(|| raw.to_string());
        return FormattedCell {
            text,
            kind: match (has_date, has_time) {
                (true, true) => "datetime",
                (true, false) => "date",
                _ => "time",
            },
            num_fmt_id: Some(id),
            approximate: built_in_approximate
                || section_approximate
                || contains_locale_directive(section),
        };
    }
    let (text, kind, formatter_approximate) = format_excel_number(value, section, raw);
    FormattedCell {
        text,
        kind,
        num_fmt_id: Some(id),
        approximate: built_in_approximate || section_approximate || formatter_approximate,
    }
}

fn built_in_number_format(id: u32) -> Option<(&'static str, bool)> {
    let format = match id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        5 | 6 => r#"$#,##0;($#,##0)"#,
        7 | 8 => r#"$#,##0.00;($#,##0.00)"#,
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "m/d/yy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yy h:mm",
        27..=36 | 50..=58 => "yyyy-mm-dd",
        37 | 38 => "#,##0;(#,##0)",
        39 | 40 => "#,##0.00;(#,##0.00)",
        41 | 42 => r#"$#,##0;($#,##0);$-"#,
        43 | 44 => r#"$#,##0.00;($#,##0.00);$-"#,
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mmss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    };
    Some((format, matches!(id, 27..=36 | 50..=58)))
}

fn split_number_format_sections(code: &str) -> Vec<&str> {
    let mut sections = Vec::new();
    let mut start = 0usize;
    let mut quoted = false;
    let mut bracket_depth = 0usize;
    let mut escaped = false;
    for (index, character) in code.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' if bracket_depth == 0 => quoted = !quoted,
            '[' if !quoted => bracket_depth += 1,
            ']' if !quoted => bracket_depth = bracket_depth.saturating_sub(1),
            ';' if !quoted && bracket_depth == 0 => {
                sections.push(&code[start..index]);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    sections.push(&code[start..]);
    sections
}

fn select_number_format_section(code: &str, value: f64) -> (&str, bool, bool) {
    let sections = split_number_format_sections(code);
    let contains_conditions = sections
        .iter()
        .any(|section| section.contains("[>") || section.contains("[<") || section.contains("[="));
    if value < 0.0 && sections.get(1).is_some_and(|section| !section.is_empty()) {
        (sections[1], true, contains_conditions)
    } else if value == 0.0 && sections.get(2).is_some_and(|section| !section.is_empty()) {
        (sections[2], false, contains_conditions)
    } else {
        (
            sections.first().copied().unwrap_or("General"),
            false,
            contains_conditions,
        )
    }
}

fn contains_locale_directive(code: &str) -> bool {
    code.as_bytes().windows(2).any(|window| window == b"[$")
}

fn format_symbols(code: &str) -> String {
    let mut output = String::new();
    let mut chars = code.chars().peekable();
    let mut quoted = false;
    while let Some(character) = chars.next() {
        if character == '"' {
            quoted = !quoted;
            continue;
        }
        if character == '\\' {
            chars.next();
            continue;
        }
        if quoted {
            continue;
        }
        if character == '[' {
            let mut bracket = String::new();
            for nested in chars.by_ref() {
                if nested == ']' {
                    break;
                }
                bracket.push(nested);
            }
            if matches!(
                bracket.to_ascii_lowercase().as_str(),
                "h" | "hh" | "m" | "mm" | "s" | "ss"
            ) {
                output.push_str(&bracket.to_ascii_lowercase());
            }
            continue;
        }
        output.extend(character.to_lowercase());
    }
    output
}

fn date_time_format_kind(id: u32, code: &str) -> (bool, bool) {
    let built_in_date = matches!(id, 14..=17 | 22 | 27..=36 | 50..=58);
    let built_in_time = matches!(id, 18..=22 | 45..=47);
    let symbols = format_symbols(code);
    let has_time = built_in_time
        || symbols.contains('h')
        || symbols.contains('s')
        || symbols.contains("am/pm");
    let has_date = built_in_date
        || symbols.contains('y')
        || symbols.contains('d')
        || (symbols.contains('m') && !has_time);
    (has_date, has_time)
}

struct ExcelDateTimeParts {
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    millisecond: u16,
    weekday_from_monday: Option<u8>,
    elapsed_seconds: i64,
}

fn excel_serial_date_time(value: f64, date_1904: bool) -> Option<ExcelDateTimeParts> {
    if !(0.0..=2_958_465.999_999).contains(&value) || !value.is_finite() {
        return None;
    }
    let mut serial_days = value.floor() as i64;
    let mut day_milliseconds = ((value - value.floor()) * 86_400_000.0).round() as i64;
    if day_milliseconds >= 86_400_000 {
        serial_days = serial_days.checked_add(1)?;
        day_milliseconds -= 86_400_000;
    }
    let elapsed_seconds = (value * 86_400.0).round() as i64;
    let (year, month, day, weekday_from_monday) = if !date_1904 && serial_days == 60 {
        (1900, 2, 29, None)
    } else {
        let (base, offset) = if date_1904 {
            (
                Date::from_calendar_date(1904, Month::January, 1).ok()?,
                serial_days,
            )
        } else {
            (
                Date::from_calendar_date(1899, Month::December, 31).ok()?,
                if serial_days > 60 {
                    serial_days - 1
                } else {
                    serial_days
                },
            )
        };
        let date = base.checked_add(Duration::days(offset))?;
        (
            date.year(),
            date.month() as u8,
            date.day(),
            Some(date.weekday().number_days_from_monday()),
        )
    };
    let total_seconds = day_milliseconds / 1_000;
    Some(ExcelDateTimeParts {
        year,
        month,
        day,
        hour: (total_seconds / 3_600) as u8,
        minute: ((total_seconds % 3_600) / 60) as u8,
        second: (total_seconds % 60) as u8,
        millisecond: (day_milliseconds % 1_000) as u16,
        weekday_from_monday,
        elapsed_seconds,
    })
}

fn format_excel_date_time(value: f64, code: &str, date_1904: bool) -> Option<String> {
    let parts = excel_serial_date_time(value, date_1904)?;
    let chars: Vec<char> = code.chars().collect();
    let symbols = format_symbols(code);
    let has_date = symbols.contains('y') || symbols.contains('d');
    let has_time = symbols.contains('h') || symbols.contains('s') || symbols.contains("am/pm");
    let twelve_hour = symbols.contains("am/pm");
    let mut output = String::new();
    let mut index = 0usize;
    while index < chars.len() {
        if starts_with_ascii_case_insensitive(&chars, index, "AM/PM") {
            output.push_str(if parts.hour < 12 { "AM" } else { "PM" });
            index += 5;
            continue;
        }
        match chars[index] {
            '"' => {
                index += 1;
                while index < chars.len() && chars[index] != '"' {
                    output.push(chars[index]);
                    index += 1;
                }
                index += usize::from(index < chars.len());
            }
            '\\' => {
                index += 1;
                if let Some(character) = chars.get(index) {
                    output.push(*character);
                    index += 1;
                }
            }
            '_' => {
                output.push(' ');
                index = (index + 2).min(chars.len());
            }
            '*' => index = (index + 2).min(chars.len()),
            '[' => {
                let end = chars[index + 1..]
                    .iter()
                    .position(|character| *character == ']')
                    .map(|offset| index + 1 + offset)
                    .unwrap_or(chars.len());
                let directive: String = chars[index + 1..end].iter().collect();
                match directive.to_ascii_lowercase().as_str() {
                    "h" => output.push_str(&(parts.elapsed_seconds / 3_600).to_string()),
                    "hh" => output.push_str(&format!("{:02}", parts.elapsed_seconds / 3_600)),
                    "m" => output.push_str(&(parts.elapsed_seconds / 60).to_string()),
                    "mm" => output.push_str(&format!("{:02}", parts.elapsed_seconds / 60)),
                    "s" => output.push_str(&parts.elapsed_seconds.to_string()),
                    "ss" => output.push_str(&format!("{:02}", parts.elapsed_seconds)),
                    _ => {
                        if let Some(currency) = currency_from_directive(&directive) {
                            output.push_str(currency);
                        }
                    }
                }
                index = (end + 1).min(chars.len());
            }
            character if matches!(character.to_ascii_lowercase(), 'y' | 'm' | 'd' | 'h' | 's') => {
                let token = character.to_ascii_lowercase();
                let start = index;
                while index < chars.len() && chars[index].to_ascii_lowercase() == token {
                    index += 1;
                }
                let width = index - start;
                match token {
                    'y' if width == 2 => output.push_str(&format!("{:02}", parts.year % 100)),
                    'y' => output.push_str(&format!("{:04}", parts.year)),
                    'd' if width == 1 => output.push_str(&parts.day.to_string()),
                    'd' if width == 2 => output.push_str(&format!("{:02}", parts.day)),
                    'd' if width == 3 => {
                        output.push_str(weekday_name(parts.weekday_from_monday, false))
                    }
                    'd' => output.push_str(weekday_name(parts.weekday_from_monday, true)),
                    'm' if is_minute_token(&chars, start, index, has_date, has_time) => {
                        if width == 1 {
                            output.push_str(&parts.minute.to_string());
                        } else {
                            output.push_str(&format!("{:02}", parts.minute));
                        }
                    }
                    'm' if width == 1 => output.push_str(&parts.month.to_string()),
                    'm' if width == 2 => output.push_str(&format!("{:02}", parts.month)),
                    'm' if width == 3 => output.push_str(month_name(parts.month, false)),
                    'm' => output.push_str(month_name(parts.month, true)),
                    'h' => {
                        let hour = if twelve_hour {
                            let hour = parts.hour % 12;
                            if hour == 0 {
                                12
                            } else {
                                hour
                            }
                        } else {
                            parts.hour
                        };
                        if width == 1 {
                            output.push_str(&hour.to_string());
                        } else {
                            output.push_str(&format!("{hour:02}"));
                        }
                    }
                    's' if width == 1 => output.push_str(&parts.second.to_string()),
                    's' => output.push_str(&format!("{:02}", parts.second)),
                    _ => {}
                }
            }
            '0' if index > 0 && chars[index - 1] == '.' && has_time => {
                let start = index;
                while index < chars.len() && chars[index] == '0' {
                    index += 1;
                }
                let width = (index - start).min(3);
                let milliseconds = format!("{:03}", parts.millisecond);
                output.push_str(&milliseconds[..width]);
            }
            character => {
                output.push(character);
                index += 1;
            }
        }
    }
    Some(output.trim().to_string())
}

fn starts_with_ascii_case_insensitive(chars: &[char], start: usize, expected: &str) -> bool {
    let expected: Vec<char> = expected.chars().collect();
    chars
        .get(start..start + expected.len())
        .is_some_and(|actual| {
            actual
                .iter()
                .zip(expected.iter())
                .all(|(left, right)| left.eq_ignore_ascii_case(right))
        })
}

fn is_minute_token(
    chars: &[char],
    start: usize,
    end: usize,
    has_date: bool,
    has_time: bool,
) -> bool {
    if has_time && !has_date {
        return true;
    }
    let previous = chars[..start]
        .iter()
        .rev()
        .find(|character| !character.is_whitespace());
    let next = chars[end..]
        .iter()
        .find(|character| !character.is_whitespace());
    previous == Some(&':') || next == Some(&':')
}

fn month_name(month: u8, long: bool) -> &'static str {
    const SHORT: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    const LONG: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let index = usize::from(month.saturating_sub(1)).min(11);
    if long {
        LONG[index]
    } else {
        SHORT[index]
    }
}

fn weekday_name(day: Option<u8>, long: bool) -> &'static str {
    const SHORT: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const LONG: [&str; 7] = [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ];
    let index = usize::from(day.unwrap_or(0)).min(6);
    if long {
        LONG[index]
    } else {
        SHORT[index]
    }
}

fn currency_from_directive(directive: &str) -> Option<&str> {
    let value = directive.strip_prefix('$')?;
    let symbol = value.split('-').next().unwrap_or("");
    (!symbol.is_empty()).then_some(symbol)
}

fn format_excel_number(value: f64, section: &str, raw: &str) -> (String, &'static str, bool) {
    if section.trim().eq_ignore_ascii_case("general") || section.trim() == "@" {
        return (raw.to_string(), "general", false);
    }
    let chars: Vec<char> = section.chars().collect();
    let Some((first_placeholder, last_placeholder)) = placeholder_bounds(&chars) else {
        let (literal, approximate) = decode_number_format_literal(&chars);
        return (literal.trim().to_string(), "literal", approximate);
    };
    let symbols = format_symbols(section);
    if symbols.contains('/') && symbols.contains('?') {
        return format_excel_fraction(value, section, &chars, first_placeholder, last_placeholder);
    }
    if symbols.contains("e+") || symbols.contains("e-") {
        return format_excel_scientific(value, section);
    }

    let percent_count = unquoted_character_count(section, '%');
    let mut numeric_value = value.abs() * 100f64.powi(percent_count as i32);
    let mut suffix_start = last_placeholder + 1;
    let mut scale_commas = 0usize;
    while chars.get(suffix_start) == Some(&',') {
        scale_commas += 1;
        suffix_start += 1;
    }
    numeric_value /= 1_000f64.powi(scale_commas as i32);

    let numeric_pattern = &chars[first_placeholder..=last_placeholder];
    let decimal_index = numeric_pattern
        .iter()
        .position(|character| *character == '.');
    let integer_pattern = &numeric_pattern[..decimal_index.unwrap_or(numeric_pattern.len())];
    let decimal_pattern = decimal_index
        .map(|index| &numeric_pattern[index + 1..])
        .unwrap_or(&[]);
    let mandatory_integer_digits = integer_pattern
        .iter()
        .filter(|character| **character == '0')
        .count();
    let minimum_decimals = decimal_pattern
        .iter()
        .filter(|character| **character == '0')
        .count();
    let maximum_decimals = decimal_pattern
        .iter()
        .filter(|character| matches!(**character, '0' | '#' | '?'))
        .count()
        .min(15);
    let grouping = integer_pattern.contains(&',');

    let rounded = format!("{numeric_value:.maximum_decimals$}");
    let (mut integer, mut decimals) = rounded
        .split_once('.')
        .map(|(integer, decimals)| (integer.to_string(), decimals.to_string()))
        .unwrap_or((rounded, String::new()));
    while decimals.len() > minimum_decimals && decimals.ends_with('0') {
        decimals.pop();
    }
    if mandatory_integer_digits == 0
        && integer == "0"
        && decimals.is_empty()
        && integer_pattern
            .iter()
            .all(|character| matches!(*character, '#' | '?' | ','))
    {
        integer.clear();
    } else if integer.len() < mandatory_integer_digits {
        integer = format!(
            "{}{}",
            "0".repeat(mandatory_integer_digits - integer.len()),
            integer
        );
    }
    if grouping && !integer.is_empty() {
        integer = group_decimal_digits(&integer);
    }
    let mut core = integer;
    if !decimals.is_empty() {
        core.push('.');
        core.push_str(&decimals);
    }

    let (prefix, prefix_approximate) = decode_number_format_literal(&chars[..first_placeholder]);
    let (suffix, suffix_approximate) = decode_number_format_literal(&chars[suffix_start..]);
    let contains_explicit_negative = prefix.contains('-')
        || suffix.contains('-')
        || (prefix.contains('(') && suffix.contains(')'));
    let sign = if value < 0.0 && !contains_explicit_negative {
        "-"
    } else {
        ""
    };
    let text = format!("{sign}{prefix}{core}{suffix}").trim().to_string();
    let kind = if percent_count > 0 || prefix.contains('%') || suffix.contains('%') {
        "percent"
    } else if contains_currency(&prefix) || contains_currency(&suffix) {
        "currency"
    } else {
        "number"
    };
    (
        text,
        kind,
        prefix_approximate || suffix_approximate || scale_commas > 0,
    )
}

fn placeholder_bounds(chars: &[char]) -> Option<(usize, usize)> {
    let mut quoted = false;
    let mut bracketed = false;
    let mut escaped = false;
    let mut first = None;
    let mut last = None;
    for (index, character) in chars.iter().copied().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' if !bracketed => quoted = !quoted,
            '[' if !quoted => bracketed = true,
            ']' if !quoted => bracketed = false,
            '0' | '#' | '?' if !quoted && !bracketed => {
                first.get_or_insert(index);
                last = Some(index);
            }
            _ => {}
        }
    }
    first.zip(last)
}

fn decode_number_format_literal(chars: &[char]) -> (String, bool) {
    let mut output = String::new();
    let mut approximate = false;
    let mut index = 0usize;
    while index < chars.len() {
        match chars[index] {
            '"' => {
                index += 1;
                while index < chars.len() && chars[index] != '"' {
                    output.push(chars[index]);
                    index += 1;
                }
                index += usize::from(index < chars.len());
            }
            '\\' => {
                index += 1;
                if let Some(character) = chars.get(index) {
                    output.push(*character);
                    index += 1;
                }
            }
            '_' => {
                output.push(' ');
                index = (index + 2).min(chars.len());
            }
            '*' => {
                approximate = true;
                index = (index + 2).min(chars.len());
            }
            '[' => {
                let end = chars[index + 1..]
                    .iter()
                    .position(|character| *character == ']')
                    .map(|offset| index + 1 + offset)
                    .unwrap_or(chars.len());
                let directive: String = chars[index + 1..end].iter().collect();
                if let Some(currency) = currency_from_directive(&directive) {
                    output.push_str(currency);
                } else {
                    approximate = true;
                }
                index = (end + 1).min(chars.len());
            }
            '0' | '#' | '?' | ',' | '.' => index += 1,
            character => {
                output.push(character);
                index += 1;
            }
        }
    }
    (output, approximate)
}

fn unquoted_character_count(code: &str, wanted: char) -> usize {
    let mut count = 0usize;
    let mut quoted = false;
    let mut bracketed = false;
    let mut escaped = false;
    for character in code.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' if !bracketed => quoted = !quoted,
            '[' if !quoted => bracketed = true,
            ']' if !quoted => bracketed = false,
            _ if character == wanted && !quoted && !bracketed => count += 1,
            _ => {}
        }
    }
    count
}

fn group_decimal_digits(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + value.len() / 3);
    for (index, character) in value.chars().enumerate() {
        if index > 0 && (value.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(character);
    }
    output
}

fn contains_currency(value: &str) -> bool {
    value
        .chars()
        .any(|character| matches!(character, '$' | '€' | '£' | '¥' | '₩' | '₹' | '₽'))
}

fn format_excel_scientific(value: f64, section: &str) -> (String, &'static str, bool) {
    let lower = section.to_ascii_lowercase();
    let exponent_index = lower.find('e').unwrap_or(section.len());
    let mantissa = &section[..exponent_index];
    let decimals = mantissa
        .split_once('.')
        .map(|(_, fraction)| {
            fraction
                .chars()
                .filter(|character| matches!(character, '0' | '#' | '?'))
                .count()
        })
        .unwrap_or(0)
        .min(15);
    let exponent_digits = section[exponent_index.saturating_add(1)..]
        .chars()
        .filter(|character| *character == '0')
        .count()
        .max(1);
    let rendered = format!("{:.*E}", decimals, value);
    let (mantissa, exponent) = rendered.split_once('E').unwrap_or((&rendered, "0"));
    let exponent = exponent.parse::<i32>().unwrap_or(0);
    (
        format!(
            "{mantissa}E{}{:0width$}",
            if exponent < 0 { '-' } else { '+' },
            exponent.unsigned_abs(),
            width = exponent_digits
        ),
        "scientific",
        false,
    )
}

fn format_excel_fraction(
    value: f64,
    section: &str,
    chars: &[char],
    first_placeholder: usize,
    last_placeholder: usize,
) -> (String, &'static str, bool) {
    let slash = chars
        .iter()
        .position(|character| *character == '/')
        .unwrap_or(last_placeholder);
    let denominator_digits = chars[slash + 1..=last_placeholder]
        .iter()
        .filter(|character| matches!(**character, '0' | '#' | '?'))
        .count()
        .clamp(1, 3);
    let maximum_denominator = 10i64.pow(denominator_digits as u32) - 1;
    let absolute = value.abs();
    let whole = absolute.floor() as i64;
    let fraction = absolute - whole as f64;
    let mut best_numerator = 0i64;
    let mut best_denominator = 1i64;
    let mut best_error = f64::MAX;
    for denominator in 1..=maximum_denominator {
        let numerator = (fraction * denominator as f64).round() as i64;
        let error = (fraction - numerator as f64 / denominator as f64).abs();
        if error < best_error {
            best_error = error;
            best_numerator = numerator;
            best_denominator = denominator;
        }
    }
    let (prefix, prefix_approximate) = decode_number_format_literal(&chars[..first_placeholder]);
    let (suffix, suffix_approximate) = decode_number_format_literal(&chars[last_placeholder + 1..]);
    let sign = if value < 0.0 { "-" } else { "" };
    let core = if best_numerator == 0 {
        whole.to_string()
    } else if section[..section.find('/').unwrap_or(0)].contains(' ') {
        format!("{whole} {best_numerator}/{best_denominator}")
    } else {
        format!("{best_numerator}/{best_denominator}")
    };
    (
        format!("{sign}{prefix}{core}{suffix}").trim().to_string(),
        "fraction",
        prefix_approximate || suffix_approximate,
    )
}

fn xlsx_border_css(style: &str) -> (u8, &'static str) {
    match style {
        "medium" | "mediumDashed" | "mediumDashDot" | "mediumDashDotDot" => (2, "solid"),
        "thick" => (3, "solid"),
        "double" => (3, "double"),
        "dashed" | "dashDot" | "dashDotDot" | "slantDashDot" => (1, "dashed"),
        "dotted" | "hair" => (1, "dotted"),
        _ => (1, "solid"),
    }
}

fn xlsx_rgb(element: &BytesStart<'_>) -> Option<String> {
    let rgb = attribute(element, "rgb")?;
    let value = if rgb.len() == 8 { &rgb[2..] } else { &rgb };
    (value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

fn xlsx_on_off(element: &BytesStart<'_>) -> bool {
    !attribute(element, "val").is_some_and(|value| matches!(value.as_str(), "0" | "false"))
}

fn safe_css_font(value: &str) -> Option<&str> {
    (!value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|character| character.is_alphanumeric() || " -_,.'".contains(character)))
    .then_some(value)
}

fn xlsx_horizontal(value: &str) -> Option<&'static str> {
    match value {
        "left" | "general" | "fill" => Some("left"),
        "center" | "centerContinuous" => Some("center"),
        "right" => Some("right"),
        "justify" | "distributed" => Some("justify"),
        _ => None,
    }
}

fn xlsx_vertical(value: &str) -> Option<&'static str> {
    match value {
        "top" => Some("top"),
        "center" => Some("middle"),
        "bottom" | "justify" | "distributed" => Some("bottom"),
        _ => None,
    }
}

pub(crate) fn import_xlsx<F>(
    source: &Path,
    output: &Path,
    options: &ImportOptions,
    mut emit: F,
) -> Result<HcdManifest, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let (source_hash, source_size) = source_identity(source, "xlsx")?;
    emit_started(&mut emit, options, &source_hash)?;
    let result = import_xlsx_inner(source, output, options, source_hash, source_size, &mut emit);
    if let Err(error) = &result {
        emit_failed(&mut emit, options, error);
    }
    result
}

fn import_xlsx_inner<F>(
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
    let mut archive = StreamingOxmlArchive::open(source).map_err(package_error)?;
    let workbook = workbook_info(&mut archive)?;
    let scratch = tempfile::tempdir()?;
    let mut shared_strings = SharedStringStore::build(&mut archive, scratch.path())?;
    let (rendered_styles, style_catalog) = render_xlsx_styles(&mut archive)?;
    let mut writer = BundleWriter::create(output)?;
    writer.write_styles(&rendered_styles)?;
    let mut format_stats = XlsxFormatStats::default();

    for sheet in workbook.sheets {
        let WorksheetScan {
            mut merged_ranges,
            view,
        } = archive
            .with_part(&sheet.part, |source| {
                scan_worksheet_metadata(source, &sheet.part)
                    .map_err(|error| PackageError::ReadPartError(error.to_string()))
            })
            .map_err(package_error)?;
        let mut chunks = SheetChunkWriter::new(
            &options.document_id,
            &sheet.name,
            &sheet.part,
            options,
            &view,
            &mut writer,
            emit,
        );
        archive
            .with_part(&sheet.part, |source| {
                parse_worksheet(
                    source,
                    &options.document_id,
                    &sheet,
                    &mut shared_strings,
                    &mut merged_ranges,
                    &style_catalog,
                    workbook.date_1904,
                    &mut format_stats,
                    &mut chunks,
                )
                .map_err(|error| PackageError::ReadPartError(error.to_string()))
            })
            .map_err(package_error)?;
        chunks.finish()?;
    }

    // Worksheet text is the progressive primary representation. Media is
    // content-addressed afterwards so a workbook with large drawings does not
    // delay every sheet chunk even though drawings remain read-only in hcd/1.
    let assets = import_assets(&mut archive, &writer, emit)?;
    std::fs::write(
        writer.root().join("assets/index.json"),
        serde_json::to_vec(&assets)?,
    )?;

    let mut manifest = base_manifest(options, "xlsx", "grid", source_hash, source_size);
    manifest.warnings.push(FidelityWarning {
        code: "XLSX_ADVANCED_VISUALS_EXTERNAL".to_string(),
        message: "HCD materializes worksheet view/frozen-pane metadata, merged ranges, direct cell styles, row/column dimensions and common numeric display formats; conditional formatting, charts and drawings remain authoritative in the immutable source".to_string(),
        node_id: None,
        source_part: Some("xl/styles.xml".to_string()),
    });
    if format_stats.approximate_cells > 0 {
        manifest.warnings.push(FidelityWarning {
            code: "XLSX_NUMFMT_PARTIAL".to_string(),
            message: format!(
                "{} cells use locale-dependent, conditional or advanced number-format tokens and were rendered best-effort",
                format_stats.approximate_cells
            ),
            node_id: None,
            source_part: Some("xl/styles.xml".to_string()),
        });
    }
    manifest.fidelity = Some(FidelityReport {
        schema_version: HCD_SCHEMA_VERSION.to_string(),
        level: FidelityLevel::Semantic,
        preserved: vec![
            "worksheet order, cell addresses, stored values and formulas".to_string(),
            "direct cell fonts, fills, borders, alignment, row height and column width"
                .to_string(),
            "merged cell ranges as bounded HTML rowspans and colspans".to_string(),
            "worksheet view metadata including frozen/split panes, RTL, grid/header/zero/formula flags, zoom and initial visible cells"
                .to_string(),
            format!(
                "{} numeric cells rendered with common built-in or custom number/date/percent/currency formats, including the workbook 1900/1904 date system",
                format_stats.formatted_cells
            ),
            "opaque workbook parts and media in the immutable source".to_string(),
        ],
        flattened: vec![
            "locale-dependent, conditional and advanced number formats are best-effort; conditional formatting, charts and drawing layout are not fully materialized in HTML".to_string(),
            "showFormulas is preserved as view metadata, while HCD text continues to display cached cell values and formula expressions remain read-only"
                .to_string(),
        ],
        dropped: Vec::new(),
        warnings: manifest.warnings.clone(),
    });
    finish_import(writer, manifest, emit)
}

fn import_assets<F>(
    archive: &mut StreamingOxmlArchive,
    writer: &BundleWriter,
    emit: &mut F,
) -> Result<Vec<AssetRecord>, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let parts: Vec<String> = archive
        .entries()
        .iter()
        .filter(|entry| !entry.is_dir && entry.name.starts_with("xl/media/"))
        .map(|entry| entry.name.clone())
        .collect();
    let mut assets = Vec::new();
    for part in parts {
        let extension = Path::new(&part)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let (href, hash, byte_length) = archive
            .with_part(&part, |source| {
                writer
                    .write_asset_from_reader(extension, source)
                    .map_err(|error| PackageError::ReadPartError(error.to_string()))
            })
            .map_err(package_error)?;
        emit(&ImportEvent::AssetReady {
            hash: hash.clone(),
            href: href.clone(),
            byte_length,
        })?;
        assets.push(AssetRecord {
            source_part: part,
            hash,
            href,
            byte_length,
        });
    }
    Ok(assets)
}

fn scan_worksheet_metadata(source: &mut dyn Read, part: &str) -> Result<WorksheetScan, HcdError> {
    let mut reader = Reader::from_reader(BufReader::with_capacity(64 * 1024, source));
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::with_capacity(64 * 1024);
    let mut ranges = Vec::new();
    let mut selected_view: Option<WorksheetViewMetadata> = None;
    let mut current_view: Option<WorksheetViewMetadata> = None;
    let mut budget = XmlBudget::default();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| HcdError::InvalidBundle(format!("worksheet {part} XML: {error}")))?;
        budget.observe(&event, part)?;
        match event {
            Event::Start(ref element) if local_name(element.name().as_ref()) == "sheetView" => {
                if current_view.is_some() {
                    return Err(HcdError::InvalidBundle(format!(
                        "worksheet {part} contains nested sheetView elements"
                    )));
                }
                current_view = Some(parse_worksheet_view(element));
            }
            Event::Empty(ref element) if local_name(element.name().as_ref()) == "sheetView" => {
                select_worksheet_view(&mut selected_view, parse_worksheet_view(element));
            }
            Event::Start(ref element) | Event::Empty(ref element)
                if local_name(element.name().as_ref()) == "pane" && current_view.is_some() =>
            {
                let view = current_view.as_mut().expect("checked current sheet view");
                if view.pane.is_none() {
                    view.pane = parse_worksheet_pane(element);
                }
            }
            Event::Start(ref element) | Event::Empty(ref element)
                if local_name(element.name().as_ref()) == "mergeCell" =>
            {
                if ranges.len() >= MAX_MERGED_RANGES {
                    return Err(HcdError::ResourceLimit(format!(
                        "worksheet {part} exceeds {MAX_MERGED_RANGES} merged ranges"
                    )));
                }
                let reference = attribute(element, "ref").ok_or_else(|| {
                    HcdError::InvalidBundle(format!(
                        "worksheet {part} contains mergeCell without ref"
                    ))
                })?;
                ranges.push(parse_merge_reference(&reference).ok_or_else(|| {
                    HcdError::InvalidBundle(format!(
                        "worksheet {part} has invalid merged range {reference}"
                    ))
                })?);
            }
            Event::End(ref element) if local_name(element.name().as_ref()) == "sheetView" => {
                let view = current_view.take().ok_or_else(|| {
                    HcdError::InvalidBundle(format!(
                        "worksheet {part} closes sheetView without opening it"
                    ))
                })?;
                select_worksheet_view(&mut selected_view, view);
            }
            Event::Eof => {
                budget.finish(part)?;
                if current_view.is_some() {
                    return Err(HcdError::InvalidBundle(format!(
                        "worksheet {part} ends inside sheetView"
                    )));
                }
                break;
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(WorksheetScan {
        merged_ranges: MergeCursor::new(ranges),
        view: selected_view.unwrap_or_default(),
    })
}

fn parse_worksheet_view(element: &BytesStart<'_>) -> WorksheetViewMetadata {
    WorksheetViewMetadata {
        workbook_view_id: attribute(element, "workbookViewId")
            .and_then(|value| value.parse::<u32>().ok()),
        view: attribute(element, "view").and_then(|value| match value.as_str() {
            "normal" => Some("normal"),
            "pageBreakPreview" => Some("page-break-preview"),
            "pageLayout" => Some("page-layout"),
            _ => None,
        }),
        top_left_cell: attribute(element, "topLeftCell")
            .and_then(|value| canonical_cell_reference(&value)),
        right_to_left: boolean_attribute(element, "rightToLeft"),
        show_grid_lines: boolean_attribute(element, "showGridLines"),
        show_row_column_headers: boolean_attribute(element, "showRowColHeaders"),
        show_zeros: boolean_attribute(element, "showZeros"),
        show_formulas: boolean_attribute(element, "showFormulas"),
        zoom_scale: attribute(element, "zoomScale")
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|value| (10..=400).contains(value)),
        pane: None,
    }
}

fn parse_worksheet_pane(element: &BytesStart<'_>) -> Option<WorksheetPaneMetadata> {
    let state = match attribute(element, "state").as_deref() {
        None | Some("split") => "split",
        Some("frozen") => "frozen",
        Some("frozenSplit") => "frozen-split",
        Some(_) => return None,
    };
    Some(WorksheetPaneMetadata {
        state,
        x_split: pane_split(element, "xSplit"),
        y_split: pane_split(element, "ySplit"),
        top_left_cell: attribute(element, "topLeftCell")
            .and_then(|value| canonical_cell_reference(&value)),
        active_pane: attribute(element, "activePane").and_then(|value| match value.as_str() {
            "topLeft" => Some("top-left"),
            "topRight" => Some("top-right"),
            "bottomLeft" => Some("bottom-left"),
            "bottomRight" => Some("bottom-right"),
            _ => None,
        }),
    })
}

fn select_worksheet_view(
    selected: &mut Option<WorksheetViewMetadata>,
    candidate: WorksheetViewMetadata,
) {
    let candidate_is_primary = candidate.workbook_view_id == Some(0);
    let current_is_primary = selected
        .as_ref()
        .is_some_and(|view| view.workbook_view_id == Some(0));
    if selected.is_none() || (candidate_is_primary && !current_is_primary) {
        *selected = Some(candidate);
    }
}

fn boolean_attribute(element: &BytesStart<'_>, name: &str) -> Option<bool> {
    attribute(element, name).and_then(|value| match value.as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    })
}

fn pane_split(element: &BytesStart<'_>, name: &str) -> Option<f64> {
    attribute(element, name)
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && (0.0..=1_000_000_000.0).contains(value))
}

fn frozen_split_count(value: f64, maximum: u32) -> Option<u32> {
    (value.fract() == 0.0 && (0.0..=f64::from(maximum)).contains(&value)).then_some(value as u32)
}

fn canonical_cell_reference(reference: &str) -> Option<String> {
    let (row, column) = cell_coordinates(reference)?;
    Some(format!("{}{row}", column_name(column)))
}

fn parse_merge_reference(reference: &str) -> Option<MergeRange> {
    let (start, end) = reference.split_once(':')?;
    if end.contains(':') {
        return None;
    }
    let (start_row, start_col) = cell_coordinates(start)?;
    let (end_row, end_col) = cell_coordinates(end)?;
    (start_row <= end_row && start_col <= end_col).then_some(MergeRange {
        start_row,
        end_row,
        start_col,
        end_col,
    })
}

fn cell_coordinates(reference: &str) -> Option<(u32, u32)> {
    let mut column = 0u32;
    let mut row = 0u32;
    let mut saw_column = false;
    let mut saw_row = false;
    for character in reference.chars().filter(|character| *character != '$') {
        if character.is_ascii_alphabetic() && !saw_row {
            saw_column = true;
            let digit = character.to_ascii_uppercase() as u32 - 'A' as u32 + 1;
            column = column.checked_mul(26)?.checked_add(digit)?;
        } else if character.is_ascii_digit() && saw_column {
            saw_row = true;
            row = row.checked_mul(10)?.checked_add(character.to_digit(10)?)?;
        } else {
            return None;
        }
    }
    (saw_column && saw_row && (1..=16_384).contains(&column) && (1..=1_048_576).contains(&row))
        .then_some((row, column))
}

fn merge_reference(range: MergeRange) -> String {
    format!(
        "{}{}:{}{}",
        column_name(range.start_col),
        range.start_row,
        column_name(range.end_col),
        range.end_row
    )
}

fn column_name(mut column: u32) -> String {
    let mut output = Vec::new();
    while column > 0 {
        column -= 1;
        output.push((b'A' + (column % 26) as u8) as char);
        column /= 26;
    }
    output.iter().rev().collect()
}

fn begin_worksheet_row(
    element: &BytesStart<'_>,
    sheet: &SheetPart,
    merged_ranges: &mut MergeCursor,
    last_row_number: &mut u32,
) -> Result<RenderedRow, HcdError> {
    let merge_row = match attribute(element, "r") {
        Some(value) => value
            .parse::<u32>()
            .ok()
            .filter(|value| (1..=1_048_576).contains(value))
            .ok_or_else(|| {
                HcdError::InvalidBundle(format!(
                    "worksheet {} has invalid row number {value}",
                    sheet.part
                ))
            })?,
        None => last_row_number.checked_add(1).ok_or_else(|| {
            HcdError::InvalidBundle(format!("worksheet {} row number overflow", sheet.part))
        })?,
    };
    merged_ranges.begin_row(merge_row)?;
    *last_row_number = merge_row;

    let number = u64::from(merge_row);
    let height = attribute(element, "ht")
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| (0.0..=409.0).contains(value));
    let hidden =
        attribute(element, "hidden").is_some_and(|value| matches!(value.as_str(), "1" | "true"));
    let mut row_attributes = String::new();
    if hidden {
        row_attributes.push_str(" data-hcd-hidden=\"true\" style=\"display:none\"");
    } else if let Some(height) = height {
        row_attributes.push_str(&format!(
            " data-hcd-height-points=\"{height:.2}\" style=\"height:{height:.2}pt\""
        ));
    }

    Ok(RenderedRow {
        number,
        html: format!("<tr data-hcd-row=\"{number}\"{row_attributes}>"),
        entries: Vec::new(),
        merge_end_row: None,
        cells: Vec::new(),
        merge_anchors: merged_ranges.current_row_anchors(),
    })
}

fn merge_attributes(range: MergeRange) -> String {
    format!(
        " data-hcd-merge=\"{}\" rowspan=\"{}\" colspan=\"{}\"",
        merge_reference(range),
        range.end_row - range.start_row + 1,
        range.end_col - range.start_col + 1
    )
}

fn finish_worksheet_row(mut row: RenderedRow) -> Result<RenderedRow, HcdError> {
    for range in &row.merge_anchors {
        row.merge_end_row = Some(row.merge_end_row.unwrap_or(0).max(range.end_row));
        if !row
            .cells
            .iter()
            .any(|(column, _)| *column == range.start_col)
        {
            let reference = format!("{}{}", column_name(range.start_col), range.start_row);
            row.cells.push((
                range.start_col,
                format!(
                    "<td class=\"hcd-cell hcd-merge-empty\" data-hcd-cell=\"{reference}\" data-hcd-column=\"{}\" data-hcd-editable=\"false\"{}></td>",
                    range.start_col,
                    merge_attributes(*range)
                ),
            ));
        }
    }
    row.cells.sort_unstable_by_key(|(column, _)| *column);
    for pair in row.cells.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(HcdError::InvalidBundle(format!(
                "worksheet row {} contains duplicate column {}",
                row.number, pair[0].0
            )));
        }
    }
    for (_, cell_html) in row.cells.drain(..) {
        row.html.push_str(&cell_html);
    }
    row.html.push_str("</tr>");
    Ok(row)
}

#[allow(clippy::too_many_arguments)]
fn parse_worksheet<F>(
    source: &mut dyn Read,
    document_id: &str,
    sheet: &SheetPart,
    shared_strings: &mut SharedStringStore,
    merged_ranges: &mut MergeCursor,
    styles: &XlsxStyleCatalog,
    date_1904: bool,
    format_stats: &mut XlsxFormatStats,
    chunks: &mut SheetChunkWriter<'_, F>,
) -> Result<(), HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let mut reader = Reader::from_reader(BufReader::with_capacity(64 * 1024, source));
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::with_capacity(64 * 1024);
    let mut row: Option<RenderedRow> = None;
    let mut cell: Option<CellBuilder> = None;
    let mut cell_ordinal = 0u64;
    let mut last_row_number = 0u32;
    let mut budget = XmlBudget::default();
    loop {
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            HcdError::InvalidBundle(format!("worksheet {} XML: {error}", sheet.part))
        })?;
        budget.observe(&event, &sheet.part)?;
        match event {
            Event::Start(ref start) if local_name(start.name().as_ref()) == "row" => {
                row = Some(begin_worksheet_row(
                    start,
                    sheet,
                    merged_ranges,
                    &mut last_row_number,
                )?);
            }
            Event::Empty(ref start) if local_name(start.name().as_ref()) == "row" => {
                let finished =
                    begin_worksheet_row(start, sheet, merged_ranges, &mut last_row_number)?;
                chunks.push(finish_worksheet_row(finished)?)?;
            }
            Event::Start(ref start) | Event::Empty(ref start)
                if local_name(start.name().as_ref()) == "sheetFormatPr" =>
            {
                chunks.default_column_width = attribute(start, "defaultColWidth")
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|value| (0.0..=255.0).contains(value));
            }
            Event::Start(ref start) | Event::Empty(ref start)
                if local_name(start.name().as_ref()) == "col" =>
            {
                append_column_markup(&mut chunks.column_markup, start);
            }
            Event::Start(ref start) if local_name(start.name().as_ref()) == "c" => {
                cell = Some(CellBuilder {
                    reference: attribute(start, "r").unwrap_or_default(),
                    value_type: attribute(start, "t").unwrap_or_default(),
                    style_index: attribute(start, "s").and_then(|value| value.parse().ok()),
                    ..Default::default()
                });
            }
            Event::Empty(ref start) if local_name(start.name().as_ref()) == "c" => {
                let finished = CellBuilder {
                    reference: attribute(start, "r").unwrap_or_default(),
                    value_type: attribute(start, "t").unwrap_or_default(),
                    style_index: attribute(start, "s").and_then(|value| value.parse().ok()),
                    ..Default::default()
                };
                append_finished_cell_if_visible(
                    &mut row,
                    document_id,
                    sheet,
                    &mut cell_ordinal,
                    finished,
                    shared_strings,
                    merged_ranges,
                    styles,
                    date_1904,
                    format_stats,
                )?;
            }
            Event::Start(ref start)
                if cell.is_some() && local_name(start.name().as_ref()) == "f" =>
            {
                cell.as_mut().expect("checked cell").formula = true;
            }
            Event::Start(ref start)
                if cell.is_some() && local_name(start.name().as_ref()) == "v" =>
            {
                cell.as_mut().expect("checked cell").capture = Capture::Value;
            }
            Event::Start(ref start)
                if cell.is_some() && local_name(start.name().as_ref()) == "t" =>
            {
                cell.as_mut().expect("checked cell").capture = Capture::InlineText;
            }
            Event::Text(text) if cell.is_some() => {
                let decoded = text
                    .unescape()
                    .map_err(|error| HcdError::InvalidBundle(format!("worksheet text: {error}")))?;
                let cell = cell.as_mut().expect("checked cell");
                match cell.capture {
                    Capture::Value => cell.value.push_str(&decoded),
                    Capture::InlineText => cell.inline_text.push_str(&decoded),
                    Capture::None => {}
                }
                if cell.value.len().max(cell.inline_text.len()) > MAX_CHUNK_BYTES {
                    return Err(HcdError::ResourceLimit(format!(
                        "NODE_TOO_LARGE: cell {} exceeds 2 MiB",
                        cell.reference
                    )));
                }
            }
            Event::End(ref end)
                if cell.is_some() && matches!(local_name(end.name().as_ref()), "v" | "t") =>
            {
                cell.as_mut().expect("checked cell").capture = Capture::None;
            }
            Event::End(ref end) if local_name(end.name().as_ref()) == "c" => {
                let finished = cell.take().unwrap_or_default();
                append_finished_cell_if_visible(
                    &mut row,
                    document_id,
                    sheet,
                    &mut cell_ordinal,
                    finished,
                    shared_strings,
                    merged_ranges,
                    styles,
                    date_1904,
                    format_stats,
                )?;
            }
            Event::End(ref end) if local_name(end.name().as_ref()) == "row" => {
                if let Some(finished) = row.take() {
                    chunks.push(finish_worksheet_row(finished)?)?;
                }
            }
            Event::Eof => {
                budget.finish(&sheet.part)?;
                break;
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(())
}

fn append_column_markup(output: &mut String, element: &BytesStart<'_>) {
    let min = attribute(element, "min")
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| (1..=16_384).contains(value))
        .unwrap_or(1);
    let max = attribute(element, "max")
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| (min..=16_384).contains(value))
        .unwrap_or(min);
    let span = max - min + 1;
    let width = attribute(element, "width")
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| (0.0..=255.0).contains(value));
    let hidden =
        attribute(element, "hidden").is_some_and(|value| matches!(value.as_str(), "1" | "true"));
    output.push_str(&format!(
        "<col span=\"{span}\" data-hcd-column-start=\"{min}\" data-hcd-column-end=\"{max}\""
    ));
    if let Some(width) = width {
        output.push_str(&format!(" data-hcd-width=\"{width:.2}\""));
    }
    if hidden {
        output.push_str(" data-hcd-hidden=\"true\" style=\"display:none\"");
    } else if let Some(width) = width {
        // Excel character width is approximated at 7.5 CSS pixels per unit.
        output.push_str(&format!(" style=\"width:{:.2}px\"", width * 7.5));
    }
    output.push_str("/>");
}

#[allow(clippy::too_many_arguments)]
fn append_finished_cell_if_visible(
    row: &mut Option<RenderedRow>,
    document_id: &str,
    sheet: &SheetPart,
    cell_ordinal: &mut u64,
    finished: CellBuilder,
    shared_strings: &mut SharedStringStore,
    merged_ranges: &MergeCursor,
    styles: &XlsxStyleCatalog,
    date_1904: bool,
    format_stats: &mut XlsxFormatStats,
) -> Result<(), HcdError> {
    let Some(row) = row else {
        return Err(HcdError::InvalidBundle(format!(
            "worksheet {} contains a cell outside a row",
            sheet.part
        )));
    };
    let has_content =
        !finished.value.is_empty() || !finished.inline_text.is_empty() || finished.formula;
    let is_merge_anchor = cell_coordinates(&finished.reference)
        .and_then(|(cell_row, cell_col)| merged_ranges.classify(cell_row, cell_col))
        .is_some_and(|position| matches!(position, MergePosition::Anchor(_)));
    if !finished.reference.is_empty() && (has_content || is_merge_anchor) {
        *cell_ordinal += 1;
        append_cell(
            row,
            document_id,
            sheet,
            *cell_ordinal,
            finished,
            shared_strings,
            merged_ranges,
            styles,
            date_1904,
            format_stats,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn append_cell(
    row: &mut RenderedRow,
    document_id: &str,
    sheet: &SheetPart,
    ordinal: u64,
    cell: CellBuilder,
    shared_strings: &mut SharedStringStore,
    merged_ranges: &MergeCursor,
    styles: &XlsxStyleCatalog,
    date_1904: bool,
    format_stats: &mut XlsxFormatStats,
) -> Result<(), HcdError> {
    let is_numeric = matches!(cell.value_type.as_str(), "" | "n");
    let raw_text = match cell.value_type.as_str() {
        "s" => shared_strings.get(cell.value.parse().map_err(|_| {
            HcdError::InvalidBundle(format!("invalid shared string index in {}", cell.reference))
        })?)?,
        "inlineStr" => cell.inline_text,
        "b" => match cell.value.as_str() {
            "1" => "TRUE".to_string(),
            "0" => "FALSE".to_string(),
            _ => cell.value,
        },
        _ => cell.value,
    };
    let formatted = format_xlsx_cell(&raw_text, cell.style_index, is_numeric, styles, date_1904);
    if formatted.text != raw_text {
        format_stats.formatted_cells = format_stats.formatted_cells.saturating_add(1);
    }
    if formatted.approximate {
        format_stats.approximate_cells = format_stats.approximate_cells.saturating_add(1);
    }
    let (cell_row, cell_col) = cell_coordinates(&cell.reference).ok_or_else(|| {
        HcdError::InvalidBundle(format!("invalid XLSX cell reference {}", cell.reference))
    })?;
    if u64::from(cell_row) != row.number {
        return Err(HcdError::InvalidBundle(format!(
            "cell {} is nested in worksheet row {}",
            cell.reference, row.number
        )));
    }
    let merge_attributes = match merged_ranges.classify(cell_row, cell_col) {
        Some(MergePosition::Anchor(range)) => {
            row.merge_end_row = Some(row.merge_end_row.unwrap_or(0).max(range.end_row));
            merge_attributes(range)
        }
        Some(MergePosition::Covered(range)) => {
            return Err(HcdError::InvalidBundle(format!(
                "merged covered cell {} contains a value or formula inside {}",
                cell.reference,
                merge_reference(range)
            )))
        }
        None => String::new(),
    };
    let node_id = stable_node_id(&[document_id, &sheet.part, "cell", &cell.reference]);
    let node_hash = hash_bytes(formatted.text.as_bytes());
    let number_format_attributes = formatted
        .num_fmt_id
        .filter(|id| *id != 0)
        .map(|id| {
            format!(
                " data-hcd-num-fmt-id=\"{id}\" data-hcd-display-kind=\"{}\"",
                formatted.kind
            )
        })
        .unwrap_or_default();
    row.cells.push((
        cell_col,
        format!(
            "<td class=\"hcd-cell{}\" data-hcd-cell=\"{}\" data-hcd-column=\"{}\"{}{}{}{}><span data-hcd-id=\"{}\" data-hcd-node-hash=\"{}\">{}</span></td>",
            cell.style_index
                .map(|index| format!(" hcd-xs-{index}"))
                .unwrap_or_default(),
            escape_attribute(&cell.reference),
            cell_col,
            if cell.formula { " data-hcd-formula=\"true\"" } else { "" },
            cell.style_index
                .map(|index| format!(" data-hcd-style-index=\"{index}\""))
                .unwrap_or_default(),
            number_format_attributes,
            merge_attributes,
            node_id,
            node_hash,
            escape_text(&formatted.text)
        ),
    ));
    row.entries.push(NodeMapEntry {
        node_id,
        node_hash,
        source: SourceAnchor {
            part: sheet.part.clone(),
            text_ordinal: ordinal,
            paragraph_id: Some(cell.reference),
            text_id: None,
            node_kind: "cell".to_string(),
            editable: !cell.formula,
        },
    });
    Ok(())
}

fn workbook_info(archive: &mut StreamingOxmlArchive) -> Result<WorkbookInfo, HcdError> {
    for required in ["xl/workbook.xml", "xl/_rels/workbook.xml.rels"] {
        if !archive.contains(required) {
            return Err(HcdError::InvalidBundle(format!(
                "XLSX is missing {required}"
            )));
        }
    }
    let relationships_xml = archive
        .read_control_part("xl/_rels/workbook.xml.rels", MAX_CONTROL_BYTES)
        .map_err(package_error)?;
    let mut relationships = HashMap::new();
    let mut reader = Reader::from_reader(relationships_xml.as_slice());
    let mut buffer = Vec::new();
    let mut budget = XmlBudget::default();
    loop {
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            HcdError::InvalidBundle(format!("workbook relationships XML: {error}"))
        })?;
        budget.observe(&event, "xl/_rels/workbook.xml.rels")?;
        match event {
            Event::Start(ref start) | Event::Empty(ref start)
                if local_name(start.name().as_ref()) == "Relationship" =>
            {
                if let (Some(id), Some(target)) =
                    (attribute(start, "Id"), attribute(start, "Target"))
                {
                    relationships.insert(id, resolve_part("xl/workbook.xml", &target)?);
                }
            }
            Event::Eof => {
                budget.finish("xl/_rels/workbook.xml.rels")?;
                break;
            }
            _ => {}
        }
        buffer.clear();
    }
    let workbook_xml = archive
        .read_control_part("xl/workbook.xml", MAX_CONTROL_BYTES)
        .map_err(package_error)?;
    let mut reader = Reader::from_reader(workbook_xml.as_slice());
    let mut sheets = Vec::new();
    let mut date_1904 = false;
    let mut budget = XmlBudget::default();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| HcdError::InvalidBundle(format!("workbook XML: {error}")))?;
        budget.observe(&event, "xl/workbook.xml")?;
        match event {
            Event::Start(ref start) | Event::Empty(ref start)
                if local_name(start.name().as_ref()) == "workbookPr" =>
            {
                date_1904 = attribute(start, "date1904")
                    .is_some_and(|value| matches!(value.as_str(), "1" | "true"));
            }
            Event::Start(ref start) | Event::Empty(ref start)
                if local_name(start.name().as_ref()) == "sheet" =>
            {
                let name = attribute(start, "name").unwrap_or_else(|| "Sheet".to_string());
                let relationship_id = attribute(start, "id").ok_or_else(|| {
                    HcdError::InvalidBundle(format!("sheet {name} has no relationship id"))
                })?;
                let part = relationships
                    .get(&relationship_id)
                    .cloned()
                    .ok_or_else(|| {
                        HcdError::InvalidBundle(format!(
                            "sheet {name} relationship {relationship_id} is missing"
                        ))
                    })?;
                sheets.push(SheetPart { name, part });
            }
            Event::Eof => {
                budget.finish("xl/workbook.xml")?;
                break;
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(WorkbookInfo { sheets, date_1904 })
}

pub(crate) fn export_xlsx(
    bundle: &Bundle,
    source: &Path,
    target: &Path,
    options: &ExportOptions,
) -> Result<FidelityReport, HcdError> {
    let (manifest, _, dirty_parts, dirty_node_ids) = checked_export_state(bundle, source, options)?;
    let nodes = collect_dirty_nodes(bundle, &manifest, &dirty_parts, &dirty_node_ids)?;
    let mut replacements: HashMap<String, BTreeMap<String, String>> = HashMap::new();
    for node in nodes {
        if node.source.node_kind != "cell" {
            continue;
        }
        let cell = node.source.paragraph_id.ok_or_else(|| {
            HcdError::InvalidBundle(format!("XLSX node {} has no cell locator", node.node_id))
        })?;
        replacements
            .entry(node.source.part)
            .or_default()
            .insert(cell, node.text);
    }

    let scratch = tempfile::tempdir()?;
    let mut replacement_paths = HashMap::new();
    let mut archive = StreamingOxmlArchive::open(source).map_err(package_error)?;
    for (part, values) in &replacements {
        let path = scratch.path().join(safe_temp_name(part));
        let output = File::create(&path)?;
        archive
            .with_part(part, |input| {
                rewrite_worksheet(input, BufWriter::new(output), values)
                    .map_err(|error| PackageError::ReadPartError(error.to_string()))
            })
            .map_err(package_error)?;
        replacement_paths.insert(part.clone(), path);
    }
    let changed =
        StreamingOxmlRewriter::rewrite(source, target, &replacement_paths, "xl/workbook.xml")
            .map_err(package_error)?;
    let report = FidelityReport {
        schema_version: HCD_SCHEMA_VERSION.to_string(),
        level: if changed.is_empty() {
            FidelityLevel::Exact
        } else {
            FidelityLevel::High
        },
        preserved: vec![
            "unmodified OOXML entries copied as raw compressed payloads".to_string(),
            "cell style index, workbook structure, formulas and drawings".to_string(),
        ],
        flattened: vec![
            "edited cells are serialized as inline strings regardless of their original storage type"
                .to_string(),
        ],
        dropped: vec!["HCD recognition annotations are not exported".to_string()],
        warnings: manifest.warnings,
    };
    write_fidelity_report(options, &report)?;
    Ok(report)
}

fn rewrite_worksheet(
    source: &mut dyn Read,
    output: impl Write,
    replacements: &BTreeMap<String, String>,
) -> Result<(), HcdError> {
    let mut reader = Reader::from_reader(BufReader::with_capacity(64 * 1024, source));
    reader.config_mut().check_end_names = true;
    let mut writer = Writer::new(output);
    let mut buffer = Vec::with_capacity(64 * 1024);
    let mut skip_depth = 0usize;
    let mut seen = BTreeSet::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| HcdError::InvalidBundle(format!("worksheet export XML: {error}")))?;
        if skip_depth > 0 {
            match event {
                Event::Start(_) => skip_depth += 1,
                Event::End(_) => skip_depth -= 1,
                Event::Eof => {
                    return Err(HcdError::InvalidBundle(
                        "target cell ended at worksheet EOF".to_string(),
                    ))
                }
                _ => {}
            }
            buffer.clear();
            continue;
        }
        match event {
            Event::Start(ref start) if local_name(start.name().as_ref()) == "c" => {
                let reference = attribute(start, "r").unwrap_or_default();
                if let Some(text) = replacements.get(&reference) {
                    write_inline_cell(&mut writer, start, text)?;
                    seen.insert(reference);
                    skip_depth = 1;
                } else {
                    writer.write_event(event.into_owned())?;
                }
            }
            Event::Empty(ref empty) if local_name(empty.name().as_ref()) == "c" => {
                let reference = attribute(empty, "r").unwrap_or_default();
                if let Some(text) = replacements.get(&reference) {
                    write_inline_cell(&mut writer, empty, text)?;
                    seen.insert(reference);
                } else {
                    writer.write_event(event.into_owned())?;
                }
            }
            Event::Eof => break,
            _ => writer.write_event(event.into_owned())?,
        }
        buffer.clear();
    }
    if seen.len() != replacements.len() {
        let missing: Vec<_> = replacements
            .keys()
            .filter(|cell| !seen.contains(*cell))
            .cloned()
            .collect();
        return Err(HcdError::InvalidBundle(format!(
            "worksheet is missing mapped cells {missing:?}"
        )));
    }
    Ok(())
}

fn write_inline_cell(
    writer: &mut Writer<impl Write>,
    original: &BytesStart<'_>,
    text: &str,
) -> Result<(), HcdError> {
    let name = String::from_utf8_lossy(original.name().as_ref()).to_string();
    let mut start = BytesStart::new(name);
    let mut attributes = Vec::new();
    for attribute in original.attributes().with_checks(false).flatten() {
        let key = String::from_utf8_lossy(attribute.key.as_ref()).to_string();
        if local_name(attribute.key.as_ref()) != "t" {
            attributes.push((
                key,
                String::from_utf8_lossy(attribute.value.as_ref()).to_string(),
            ));
        }
    }
    for (key, value) in &attributes {
        start.push_attribute((key.as_str(), value.as_str()));
    }
    start.push_attribute(("t", "inlineStr"));
    writer.write_event(Event::Start(start))?;
    writer.write_event(Event::Start(BytesStart::new("is")))?;
    let mut text_start = BytesStart::new("t");
    if text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace) {
        text_start.push_attribute(("xml:space", "preserve"));
    }
    writer.write_event(Event::Start(text_start))?;
    if !text.is_empty() {
        writer.write_event(Event::Text(BytesText::new(text)))?;
    }
    writer.write_event(Event::End(BytesEnd::new("t")))?;
    writer.write_event(Event::End(BytesEnd::new("is")))?;
    writer.write_event(Event::End(BytesEnd::new(String::from_utf8_lossy(
        original.name().as_ref(),
    ))))?;
    Ok(())
}

fn resolve_part(source_part: &str, target: &str) -> Result<String, HcdError> {
    let base = Path::new(source_part)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let combined = if target.starts_with('/') {
        PathBuf::from(target.trim_start_matches('/'))
    } else {
        base.join(target)
    };
    let mut output = PathBuf::new();
    for component in combined.components() {
        match component {
            Component::Normal(value) => output.push(value),
            Component::ParentDir => {
                if !output.pop() {
                    return Err(HcdError::InvalidBundle(format!(
                        "relationship escapes package: {target}"
                    )));
                }
            }
            Component::CurDir => {}
            _ => {
                return Err(HcdError::InvalidBundle(format!(
                    "unsafe relationship target: {target}"
                )))
            }
        }
    }
    Ok(output.to_string_lossy().replace('\\', "/"))
}

fn attribute(element: &BytesStart<'_>, wanted: &str) -> Option<String> {
    element
        .attributes()
        .with_checks(false)
        .flatten()
        .find(|attribute| local_name(attribute.key.as_ref()) == wanted)
        .and_then(|attribute| {
            attribute
                .unescape_value()
                .ok()
                .map(|value| value.into_owned())
        })
}

fn local_name(name: &[u8]) -> &str {
    let local = name
        .iter()
        .rposition(|byte| *byte == b':')
        .map(|index| &name[index + 1..])
        .unwrap_or(name);
    std::str::from_utf8(local).unwrap_or("")
}

fn safe_temp_name(part: &str) -> String {
    part.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn package_error(error: PackageError) -> HcdError {
    match error {
        PackageError::ResourceLimit(message) => HcdError::ResourceLimit(message),
        other => HcdError::InvalidBundle(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hcd_core::{
        extract_text_page, validate_bundle, NodePrecondition, PatchBatch, PatchOperation,
        HCD_PATCH_SCHEMA_VERSION,
    };
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use zip::write::SimpleFileOptions;

    #[test]
    fn shared_string_store_supports_disk_backed_random_reads() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = SharedStringStore::empty(temp.path()).unwrap();
        store.push("first").unwrap();
        store.push("中间 😀").unwrap();
        store.push("last").unwrap();

        assert_eq!(store.get(2).unwrap(), "last");
        assert_eq!(store.get(0).unwrap(), "first");
        assert_eq!(store.get(1).unwrap(), "中间 😀");
    }

    #[test]
    fn formats_common_excel_numbers_and_both_date_systems() {
        let mut catalog = XlsxStyleCatalog {
            cell_formats: vec![
                XlsxCellFormat {
                    num_fmt_id: Some(4),
                    ..Default::default()
                },
                XlsxCellFormat {
                    num_fmt_id: Some(9),
                    ..Default::default()
                },
                XlsxCellFormat {
                    num_fmt_id: Some(178),
                    ..Default::default()
                },
                XlsxCellFormat {
                    num_fmt_id: Some(179),
                    ..Default::default()
                },
                XlsxCellFormat {
                    num_fmt_id: Some(14),
                    ..Default::default()
                },
                XlsxCellFormat {
                    num_fmt_id: Some(22),
                    ..Default::default()
                },
                XlsxCellFormat {
                    num_fmt_id: Some(11),
                    ..Default::default()
                },
                XlsxCellFormat {
                    num_fmt_id: Some(12),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        catalog.number_formats.insert(178, r#"0.0"%""#.to_string());
        catalog
            .number_formats
            .insert(179, "\"$\"#,##0.00".to_string());

        let cases = [
            ("1234.5", 0, false, "1,234.50", "number"),
            ("0.256", 1, false, "26%", "percent"),
            ("92.34", 2, false, "92.3%", "percent"),
            ("1234.5", 3, false, "$1,234.50", "currency"),
            ("1", 4, false, "1/1/00", "date"),
            ("60", 4, false, "2/29/00", "date"),
            ("0", 4, true, "1/1/04", "date"),
            ("1.5", 5, false, "1/1/00 12:00", "datetime"),
            ("1234", 6, false, "1.23E+03", "scientific"),
            ("1.5", 7, false, "1 1/2", "fraction"),
        ];
        for (raw, style, date_1904, expected, kind) in cases {
            let formatted = format_xlsx_cell(raw, Some(style), true, &catalog, date_1904);
            assert_eq!(
                formatted.text, expected,
                "formatting {raw} with style {style}"
            );
            assert_eq!(formatted.kind, kind);
        }
    }

    #[test]
    fn rejects_oversized_custom_number_formats() {
        let format_code = "0".repeat(MAX_FORMAT_CODE_BYTES + 1);
        let styles = format!(
            r#"<styleSheet><numFmts><numFmt numFmtId="178" formatCode="{format_code}"/></numFmts></styleSheet>"#
        );
        let error = match parse_xlsx_styles(styles.as_bytes()) {
            Ok(_) => panic!("oversized format code was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("exceeds 1024 bytes"));
    }

    #[test]
    fn imports_shared_strings_without_a_workbook_wide_string_table() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("shared.xlsx");
        let bundle_path = temp.path().join("bundle");
        let exported = temp.path().join("exported.xlsx");
        create_shared_string_fixture(&source);

        let mut event_names = Vec::new();
        let mut options = ImportOptions::new("shared-string-doc");
        options.chunk_blocks = 1;

        let manifest = import_xlsx(&source, &bundle_path, &options, |event| {
            event_names.push(match event {
                ImportEvent::ImportStarted { .. } => "started",
                ImportEvent::ChunkReady { .. } => "chunk",
                ImportEvent::AssetReady { .. } => "asset",
                ImportEvent::Completed { .. } => "completed",
                ImportEvent::Failed { .. } => "failed",
            });
            Ok(())
        })
        .unwrap();
        assert_eq!(manifest.profile, "grid");
        assert_eq!(manifest.chunk_count, 3);
        assert!(
            event_names.iter().position(|event| *event == "chunk")
                < event_names.iter().position(|event| *event == "asset")
        );
        let bundle = Bundle::open(&bundle_path).unwrap();
        let validation = validate_bundle(&bundle).unwrap();
        assert!(validation.valid, "{:?}", validation.issues);
        let text = extract_text_page(&bundle, None, 10).unwrap();
        assert_eq!(text.entries.len(), 6);
        assert_eq!(text.entries[0].text, "Shared Value 😀");
        assert_eq!(text.entries[0].source.paragraph_id.as_deref(), Some("A1"));
        assert_eq!(text.entries[1].text, "After merge");
        assert_eq!(text.entries[2].text, "1,234.50");
        assert_eq!(text.entries[3].text, "26%");
        assert_eq!(text.entries[4].text, "1/1/00");
        assert_eq!(text.entries[5].text, "92.3%");
        let styles = std::fs::read_to_string(bundle_path.join("styles.css")).unwrap();
        assert!(styles.contains(".hcd-xs-1{"));
        assert!(styles.contains("font-weight:700"));
        assert!(styles.contains("background-color:#ffcc00"));
        assert!(styles.contains(
            ".hcd-sheet[data-hcd-show-grid-lines=\"false\"] .hcd-grid td{border-color:transparent}"
        ));
        let page = bundle.read_index_page(&manifest, 0).unwrap();
        for descriptor in &page.chunks {
            let view_html = bundle.read_chunk(descriptor).unwrap();
            assert!(view_html.contains("data-hcd-sheet-view=\"page-break-preview\""));
            assert!(view_html.contains("data-hcd-view-top-left-cell=\"B2\""));
            assert!(view_html.contains("data-hcd-right-to-left=\"true\""));
            assert!(view_html.contains("data-hcd-show-grid-lines=\"false\""));
            assert!(view_html.contains("data-hcd-show-row-column-headers=\"false\""));
            assert!(view_html.contains("data-hcd-show-zeros=\"false\""));
            assert!(view_html.contains("data-hcd-show-formulas=\"true\""));
            assert!(view_html.contains("data-hcd-zoom-percent=\"125\""));
            assert!(view_html.contains("style=\"direction:rtl\""));
            assert!(view_html.contains("data-hcd-pane-state=\"frozen\""));
            assert!(view_html.contains("data-hcd-frozen-columns=\"2\""));
            assert!(view_html.contains("data-hcd-frozen-rows=\"3\""));
            assert!(view_html.contains("data-hcd-pane-top-left-cell=\"C4\""));
            assert!(view_html.contains("data-hcd-active-pane=\"bottom-right\""));
        }
        let html = bundle.read_chunk(&page.chunks[0]).unwrap();
        assert!(html.contains("class=\"hcd-cell hcd-xs-1\""));
        assert!(html.contains("data-hcd-style-index=\"1\""));
        assert!(html.contains("data-hcd-height-points=\"24.00\""));
        assert!(html.contains("data-hcd-column-start=\"1\""));
        assert!(html.contains("style=\"width:150.00px\""));
        assert!(html.contains("data-hcd-merge=\"A1:B2\""));
        assert!(html.contains("rowspan=\"2\""));
        assert!(html.contains("colspan=\"2\""));
        assert!(html.contains("data-hcd-row=\"2\""));
        assert!(!html.contains("data-hcd-row=\"3\""));
        let next_html = bundle.read_chunk(&page.chunks[1]).unwrap();
        assert!(next_html.contains("data-hcd-row=\"3\""));
        assert!(next_html.contains("data-hcd-cell=\"C3\""));
        assert!(next_html.contains("data-hcd-cell=\"D3\""));
        assert!(next_html.contains("data-hcd-num-fmt-id=\"4\""));
        assert!(next_html.contains("data-hcd-display-kind=\"number\""));
        assert!(next_html.contains(">1,234.50</span>"));
        assert!(next_html.contains("data-hcd-display-kind=\"percent\""));
        assert!(next_html.contains(">26%</span>"));
        assert!(next_html.contains("data-hcd-display-kind=\"date\""));
        assert!(next_html.contains(">1/1/00</span>"));
        assert!(next_html.contains(">92.3%</span>"));
        let empty_merge_html = bundle.read_chunk(&page.chunks[2]).unwrap();
        assert!(empty_merge_html.contains("data-hcd-row=\"4\""));
        assert!(empty_merge_html.contains("class=\"hcd-cell hcd-merge-empty\""));
        assert!(empty_merge_html.contains("data-hcd-editable=\"false\""));
        assert!(empty_merge_html.contains("data-hcd-merge=\"D4:E5\""));

        let first_cell = &text.entries[0];
        let patch = PatchBatch {
            schema_version: HCD_PATCH_SCHEMA_VERSION.to_string(),
            document_id: "shared-string-doc".to_string(),
            patch_id: "mask-merged-anchor".to_string(),
            base_revision: 0,
            actor: BTreeMap::new(),
            operations: vec![PatchOperation::TextSplice {
                node_id: first_cell.node_id.clone(),
                start: 0,
                delete_count: 6,
                insert_text: "Masked".to_string(),
                precondition: NodePrecondition {
                    node_hash: first_cell.node_hash.clone(),
                },
            }],
            metadata: BTreeMap::new(),
        };
        hcd_core::apply_patch(&bundle, &patch, 0).unwrap();
        let report = export_xlsx(&bundle, &source, &exported, &ExportOptions::default()).unwrap();
        assert_eq!(report.level, FidelityLevel::High);
        let worksheet = read_zip_entry(&exported, "xl/worksheets/sheet1.xml");
        assert!(worksheet.contains("Masked Value 😀"));
        assert!(worksheet.contains("mergeCell ref=\"A1:B2\""));
        assert!(worksheet.contains("s=\"1\""));
    }

    #[test]
    fn merged_range_parser_rejects_invalid_and_overlapping_ranges() {
        assert_eq!(
            parse_merge_reference("$A$1:XFD1048576"),
            Some(MergeRange {
                start_row: 1,
                end_row: 1_048_576,
                start_col: 1,
                end_col: 16_384,
            })
        );
        assert!(parse_merge_reference("A0:B2").is_none());
        assert!(parse_merge_reference("A1:XFE2").is_none());
        assert!(parse_merge_reference("B2:A1").is_none());

        let mut cursor = MergeCursor::new(vec![
            parse_merge_reference("A1:B2").unwrap(),
            parse_merge_reference("B2:C3").unwrap(),
        ]);
        cursor.begin_row(1).unwrap();
        let error = cursor.begin_row(2).unwrap_err();
        assert!(error.to_string().contains("overlapping XLSX merged ranges"));
    }

    #[test]
    fn worksheet_view_scan_preserves_split_positions_and_bounds_metadata() {
        let xml = r#"<worksheet><sheetViews><sheetView workbookViewId="0" view="pageLayout" topLeftCell="XFE1" rightToLeft="false" showGridLines="true" zoomScale="401"><pane xSplit="240.5" ySplit="480" topLeftCell="$D$5" activePane="topRight" state="split"/></sheetView></sheetViews><sheetData/></worksheet>"#;
        let mut source = xml.as_bytes();

        let scan = scan_worksheet_metadata(&mut source, "xl/worksheets/sheet.xml").unwrap();
        let attributes = scan.view.html_attributes();

        assert!(attributes.contains("data-hcd-sheet-view=\"page-layout\""));
        assert!(attributes.contains("data-hcd-right-to-left=\"false\""));
        assert!(attributes.contains("data-hcd-show-grid-lines=\"true\""));
        assert!(attributes.contains("data-hcd-pane-state=\"split\""));
        assert!(attributes.contains("data-hcd-split-x-twips=\"240.50\""));
        assert!(attributes.contains("data-hcd-split-y-twips=\"480.00\""));
        assert!(attributes.contains("data-hcd-pane-top-left-cell=\"D5\""));
        assert!(attributes.contains("data-hcd-active-pane=\"top-right\""));
        assert!(!attributes.contains("data-hcd-view-top-left-cell"));
        assert!(!attributes.contains("data-hcd-zoom-percent"));
        assert!(!attributes.contains("data-hcd-frozen-columns"));
        assert_eq!(frozen_split_count(2.0, 16_384), Some(2));
        assert_eq!(frozen_split_count(2.5, 16_384), None);
        assert_eq!(frozen_split_count(16_385.0, 16_384), None);
    }

    #[test]
    fn empty_sheet_chunk_repeats_view_and_default_width_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let bundle_path = temp.path().join("bundle");
        let mut writer = BundleWriter::create(&bundle_path).unwrap();
        let options = ImportOptions::new("empty-sheet-doc");
        let view = WorksheetViewMetadata {
            workbook_view_id: Some(0),
            view: Some("normal"),
            show_grid_lines: Some(false),
            pane: Some(WorksheetPaneMetadata {
                state: "frozen",
                x_split: Some(2.0),
                y_split: Some(1.0),
                top_left_cell: Some("C2".to_string()),
                active_pane: Some("bottom-right"),
            }),
            ..Default::default()
        };
        let mut descriptors = Vec::new();
        let mut emit = |event: &ImportEvent| {
            if let ImportEvent::ChunkReady { descriptor } = event {
                descriptors.push(descriptor.clone());
            }
            Ok(())
        };

        {
            let mut chunks = SheetChunkWriter::new(
                "empty-sheet-doc",
                "Empty",
                "xl/worksheets/sheet1.xml",
                &options,
                &view,
                &mut writer,
                &mut emit,
            );
            chunks.default_column_width = Some(8.43);
            chunks.finish().unwrap();
        }

        assert_eq!(descriptors.len(), 1);
        let html = std::fs::read_to_string(bundle_path.join(&descriptors[0].html_href)).unwrap();
        assert!(html.contains("data-hcd-sheet=\"Empty\""));
        assert!(html.contains("data-hcd-default-column-width=\"8.43\""));
        assert!(html.contains("data-hcd-sheet-view=\"normal\""));
        assert!(html.contains("data-hcd-show-grid-lines=\"false\""));
        assert!(html.contains("data-hcd-pane-state=\"frozen\""));
        assert!(html.contains("data-hcd-frozen-columns=\"2\""));
        assert!(html.contains("data-hcd-frozen-rows=\"1\""));
        assert!(html.contains("data-hcd-pane-top-left-cell=\"C2\""));
        assert!(html.contains("data-hcd-active-pane=\"bottom-right\""));
        assert!(html.contains("<tbody></tbody>"));
    }

    fn create_shared_string_fixture(path: &Path) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let parts = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Shared" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#,
            ),
            (
                "xl/styles.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="178" formatCode="0.0&quot;%&quot;"/></numFmts><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><i/><sz val="14"/><color rgb="FF112233"/><name val="Arial"/></font></fonts><fills count="3"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill><fill><patternFill patternType="solid"><fgColor rgb="FFFFCC00"/></patternFill></fill></fills><borders count="2"><border/><border><left style="thin"><color rgb="FF000000"/></left><bottom style="double"><color rgb="FF112233"/></bottom></border></borders><cellXfs count="6"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/><xf numFmtId="4" fontId="1" fillId="2" borderId="1"><alignment horizontal="center" vertical="center" wrapText="1"/></xf><xf numFmtId="4" fontId="0" fillId="0" borderId="0"/><xf numFmtId="9" fontId="0" fillId="0" borderId="0"/><xf numFmtId="14" fontId="0" fillId="0" borderId="0"/><xf numFmtId="178" fontId="0" fillId="0" borderId="0"/></cellXfs></styleSheet>"#,
            ),
            (
                "xl/sharedStrings.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="1" uniqueCount="1"><si><r><t>Shared </t></r><r><t>Value 😀</t></r></si></sst>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetViews><sheetView workbookViewId="7" zoomScale="75"><pane xSplit="1" ySplit="1" topLeftCell="B2" state="frozen"/></sheetView><sheetView workbookViewId="0" view="pageBreakPreview" topLeftCell="$B$2" rightToLeft="1" showGridLines="0" showRowColHeaders="0" showZeros="0" showFormulas="1" zoomScale="125"><pane xSplit="2" ySplit="3" topLeftCell="$C$4" activePane="bottomRight" state="frozen"/></sheetView></sheetViews><sheetFormatPr defaultColWidth="8.43" defaultRowHeight="15"/><cols><col min="1" max="2" width="20" customWidth="1"/></cols><sheetData><row r="1" ht="24" customHeight="1"><c r="A1" t="s" s="1"><v>0</v></c></row><row r="2"/><row r="3"><c r="C3" t="inlineStr"><is><t>After merge</t></is></c><c r="D3" s="2"><v>1234.5</v></c><c r="E3" s="3"><v>0.256</v></c><c r="F3" s="4"><v>1</v></c><c r="G3" s="5"><v>92.34</v></c></row><row r="4"/><row r="5"/></sheetData><mergeCells count="2"><mergeCell ref="A1:B2"/><mergeCell ref="D4:E5"/></mergeCells></worksheet>"#,
            ),
            ("xl/media/image1.png", "streamed-after-sheet-chunks"),
        ];
        for (name, contents) in parts {
            zip.start_file(name, options).unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    fn read_zip_entry(path: &Path, name: &str) -> String {
        let file = File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut entry = archive.by_name(name).unwrap();
        let mut output = String::new();
        entry.read_to_string(&mut output).unwrap();
        output
    }
}
