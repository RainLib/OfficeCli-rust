use crate::common::{
    base_manifest, checked_export_state, collect_dirty_nodes, emit_failed, emit_started,
    escape_attribute, escape_text, finish_import, source_identity, write_fidelity_report,
    ExportOptions, ImportOptions, XmlBudget,
};
use hcd_core::{
    hash_bytes, stable_node_id, Bundle, BundleWriter, ChunkSourceMap, FidelityLevel,
    FidelityReport, FidelityWarning, HcdError, HcdManifest, ImportEvent, NodeMapEntry,
    SourceAnchor, DEFAULT_CHUNK_BLOCKS, HCD_SCHEMA_VERSION, MAX_CHUNK_BYTES,
};
use oxml::{PackageError, StreamingOxmlArchive, StreamingOxmlRewriter};
use quick_xml::events::{BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, BufWriter, Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};

const MAX_PPTX_TABLE_COLUMNS: usize = 16_384;
const MAX_PPTX_TABLE_CELLS: usize = 1_000_000;
const PPTX_TABLE_ROWS_PER_FRAGMENT: usize = 128;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssetRecord {
    source_part: String,
    hash: String,
    href: String,
    byte_length: u64,
}

struct SlideChunkWriter<'a, F>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    document_id: &'a str,
    part: &'a str,
    region: &'a str,
    writer: &'a mut BundleWriter,
    emit: &'a mut F,
    soft_bytes: usize,
    max_blocks: usize,
    ordinal: usize,
    blocks: usize,
    html: String,
    entries: Vec<NodeMapEntry>,
    slide_width_emu: u64,
    slide_height_emu: u64,
    background: SlidePaint,
}

#[derive(Default)]
struct RenderedSlideBlock {
    html: String,
    entries: Vec<NodeMapEntry>,
}

#[derive(Default)]
struct ShapeBuilder {
    source_id: Option<String>,
    name: Option<String>,
    ordinal: usize,
    x: Option<i64>,
    y: Option<i64>,
    width: Option<u64>,
    height: Option<u64>,
    rotation: Option<i64>,
    geometry: Option<String>,
    fill: SlidePaint,
    line: SlideLine,
    vertical_anchor: Option<&'static str>,
    margin_left: Option<u64>,
    margin_right: Option<u64>,
    margin_top: Option<u64>,
    margin_bottom: Option<u64>,
    content: String,
    entries: Vec<NodeMapEntry>,
}

#[derive(Default)]
struct SlideLine {
    width: Option<u64>,
    paint: SlidePaint,
}

#[derive(Default)]
struct SlidePaint {
    none: bool,
    color: Option<String>,
    alpha: Option<u32>,
    gradient_stops: Vec<SlideGradientStop>,
    gradient_angle: Option<i64>,
}

#[derive(Default)]
struct SlideGradientStop {
    position: u32,
    color: Option<String>,
    alpha: Option<u32>,
}

#[derive(Clone, Copy)]
enum PaintTarget {
    Background,
    ShapeFill,
    ShapeLine,
}

#[derive(Default)]
struct PictureBuilder {
    source_id: Option<String>,
    name: Option<String>,
    ordinal: usize,
    x: Option<i64>,
    y: Option<i64>,
    width: Option<u64>,
    height: Option<u64>,
    relationship_id: Option<String>,
}

#[derive(Default)]
struct GraphicFrameBuilder {
    source_id: Option<String>,
    name: Option<String>,
    ordinal: usize,
    x: Option<i64>,
    y: Option<i64>,
    width: Option<u64>,
    height: Option<u64>,
    table: Option<SlideTableBuilder>,
    chart_relationship_id: Option<String>,
}

#[derive(Default)]
struct SlideTableBuilder {
    grid_columns: Vec<u64>,
    first_row: bool,
    last_row: bool,
    first_column: bool,
    last_column: bool,
    band_rows: bool,
    band_columns: bool,
    rows_html: String,
    entries: Vec<NodeMapEntry>,
    current_row: Option<SlideTableRowBuilder>,
    current_cell: Option<SlideTableCellBuilder>,
    row_count: usize,
    cell_count: usize,
    max_row_span_end: usize,
    hold_until_row: usize,
    fragment_ordinal: usize,
    fragment_start_row: usize,
    fragment_ready: bool,
}

#[derive(Default)]
struct SlideTableRowBuilder {
    height: Option<u64>,
    html: String,
    entries: Vec<NodeMapEntry>,
    cell_count: usize,
    hold_before_row: usize,
}

#[derive(Default)]
struct SlideTableCellBuilder {
    grid_span: usize,
    row_span: usize,
    horizontal_merge: bool,
    vertical_merge: bool,
    content: String,
    entries: Vec<NodeMapEntry>,
    fill_color: Option<String>,
    anchor: Option<&'static str>,
    text_direction: Option<&'static str>,
    margin_left: Option<u64>,
    margin_right: Option<u64>,
    margin_top: Option<u64>,
    margin_bottom: Option<u64>,
    border_left: SlideTableBorder,
    border_right: SlideTableBorder,
    border_top: SlideTableBorder,
    border_bottom: SlideTableBorder,
}

#[derive(Default)]
struct SlideTableBorder {
    present: bool,
    none: bool,
    width: Option<u64>,
    color: Option<String>,
    dash: Option<&'static str>,
}

#[derive(Default)]
struct SlideParagraphBuilder {
    alignment: Option<String>,
    rtl: bool,
    html: String,
}

#[derive(Default)]
struct SlideRunBuilder {
    format: SlideRunFormat,
    opened: bool,
}

#[derive(Default)]
struct SlideRunFormat {
    size_hundredth_points: Option<u32>,
    spacing_hundredth_points: Option<i32>,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    color: Option<String>,
    font: Option<String>,
    rtl: bool,
    language: Option<String>,
}

impl<'a, F> SlideChunkWriter<'a, F>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    fn push_text(&mut self, text: String, text_ordinal: u64) -> Result<(), HcdError> {
        if text.len() > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: DrawingML text in {} exceeds 2 MiB",
                self.part
            )));
        }
        let node_id = stable_node_id(&[
            self.document_id,
            self.part,
            "text",
            &text_ordinal.to_string(),
        ]);
        let node_hash = hash_bytes(text.as_bytes());
        let html = format!(
            "<p class=\"hcd-slide-text\"><span data-hcd-id=\"{}\" data-hcd-node-hash=\"{}\">{}</span></p>",
            node_id,
            node_hash,
            escape_text(&text)
        );
        self.push_block(RenderedSlideBlock {
            html,
            entries: vec![NodeMapEntry {
                node_id,
                node_hash,
                source: SourceAnchor {
                    part: self.part.to_string(),
                    text_ordinal,
                    paragraph_id: None,
                    text_id: None,
                    node_kind: "slide-text".to_string(),
                    editable: true,
                },
            }],
        })
    }

    fn push_block(&mut self, block: RenderedSlideBlock) -> Result<(), HcdError> {
        if block.html.len() > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: slide block in {} exceeds 2 MiB",
                self.part
            )));
        }
        if self.blocks > 0
            && (self.html.len() + block.html.len() > self.soft_bytes
                || self.blocks >= self.max_blocks)
        {
            self.flush()?;
        }
        self.html.push_str(&block.html);
        self.entries.extend(block.entries);
        self.blocks += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), HcdError> {
        if self.blocks == 0 && self.ordinal > 0 {
            return Ok(());
        }
        let chunk_id = stable_node_id(&[
            self.document_id,
            self.part,
            "slide-chunk",
            &self.ordinal.to_string(),
        ])
        .replacen("n_", "c_", 1);
        let content = if self.blocks == 0 {
            "<div class=\"hcd-empty-slide\"></div>".to_string()
        } else {
            std::mem::take(&mut self.html)
        };
        let width_px = emu_to_px(self.slide_width_emu);
        let height_px = emu_to_px(self.slide_height_emu);
        let background_style = paint_background_css(&self.background)
            .map(|style| format!(";{style}"))
            .unwrap_or_default();
        let html = format!(
            "<section class=\"hcd-slide\" data-hcd-source-part=\"{}\" data-hcd-width-emu=\"{}\" data-hcd-height-emu=\"{}\" style=\"position:relative;width:{width_px:.2}px;height:{height_px:.2}px{background_style}\">{}</section>",
            escape_attribute(self.part),
            self.slide_width_emu,
            self.slide_height_emu,
            content
        );
        let map = ChunkSourceMap {
            schema_version: HCD_SCHEMA_VERSION.to_string(),
            chunk_id: chunk_id.clone(),
            entries: std::mem::take(&mut self.entries),
        };
        let descriptor = self.writer.write_chunk(
            chunk_id,
            self.region.to_string(),
            html,
            map,
            self.blocks.max(1),
            self.ordinal > 0,
        )?;
        (self.emit)(&ImportEvent::ChunkReady { descriptor })?;
        self.ordinal += 1;
        self.blocks = 0;
        Ok(())
    }
}

pub(crate) fn import_pptx<F>(
    source: &Path,
    output: &Path,
    options: &ImportOptions,
    mut emit: F,
) -> Result<HcdManifest, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let (source_hash, source_size) = source_identity(source, "pptx")?;
    emit_started(&mut emit, options, &source_hash)?;
    let result = import_pptx_inner(source, output, options, source_hash, source_size, &mut emit);
    if let Err(error) = &result {
        emit_failed(&mut emit, options, error);
    }
    result
}

fn import_pptx_inner<F>(
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
    if !archive.contains("ppt/presentation.xml") {
        return Err(HcdError::InvalidBundle(
            "PPTX is missing ppt/presentation.xml".to_string(),
        ));
    }
    let (slide_width_emu, slide_height_emu) = presentation_size(&mut archive)?;
    let theme_colors = presentation_theme_colors(&mut archive)?;
    let mut writer = BundleWriter::create(output)?;
    writer.write_styles(
        ".hcd-slide{display:block;overflow:hidden;background:#fff;font-family:Arial,'Helvetica Neue','PingFang SC','Microsoft YaHei',sans-serif}.hcd-slide-shape{box-sizing:border-box;white-space:pre-wrap}.hcd-slide-picture,.hcd-slide-chart{box-sizing:border-box;overflow:hidden}.hcd-slide-picture img,.hcd-slide-chart img{display:block;width:100%;height:100%;object-fit:contain}.hcd-slide-table-frame{box-sizing:border-box;overflow:hidden}.hcd-ppt-table{border-collapse:collapse}.hcd-ppt-table td{box-sizing:border-box;overflow:hidden;vertical-align:top}.hcd-slide-text{white-space:pre-wrap;margin:0}.hcd-ppt-run{white-space:pre-wrap}.hcd-empty-slide{min-height:10em}body:not([data-hcd-image-hitboxes=\"off\"]) :is(.hcd-slide-picture,.hcd-slide-chart)[data-hcd-id]{cursor:crosshair}body:not([data-hcd-image-hitboxes=\"off\"]) :is(.hcd-slide-picture,.hcd-slide-chart)[data-hcd-id]:hover{outline:2px solid rgba(255,59,48,.95);outline-offset:-1px}body:not([data-hcd-text-hitboxes=\"off\"]) [data-hcd-node-hash]:not([data-hcd-node-kind=\"image\"]):hover{background:rgba(10,132,255,.12);outline:1px solid rgba(10,132,255,.8)}",
    )?;
    let mut assets = import_assets(&mut archive, &writer, emit)?;
    assets.extend(import_chart_assets(&mut archive, &writer, emit)?);
    let asset_records: HashMap<String, AssetRecord> = assets
        .iter()
        .map(|asset| (asset.source_part.clone(), asset.clone()))
        .collect();
    let mut indexed_hashes = HashSet::new();
    let asset_index = assets
        .iter()
        .filter(|asset| indexed_hashes.insert(asset.hash.clone()))
        .collect::<Vec<_>>();
    std::fs::write(
        writer.root().join("assets/index.json"),
        serde_json::to_vec(&asset_index)?,
    )?;

    let mut parts = presentation_slides(&mut archive)?;
    let mut notes: Vec<(String, &'static str)> = archive
        .entries()
        .iter()
        .filter_map(|entry| {
            if entry.name.starts_with("ppt/notesSlides/notesSlide") && entry.name.ends_with(".xml")
            {
                Some((entry.name.clone(), "note"))
            } else {
                None
            }
        })
        .collect();
    notes.sort_by_key(|(part, _)| numeric_suffix(part));
    parts.extend(notes);
    for (part, region) in parts {
        let part_assets = part_asset_relationships(&mut archive, &part, &asset_records)?;
        let part_charts = part_chart_relationships(&mut archive, &part, &asset_records)?;
        let mut chunks = SlideChunkWriter {
            document_id: &options.document_id,
            part: &part,
            region,
            writer: &mut writer,
            emit,
            soft_bytes: options.chunk_soft_bytes.min(MAX_CHUNK_BYTES),
            max_blocks: options.chunk_blocks.clamp(1, DEFAULT_CHUNK_BLOCKS),
            ordinal: 0,
            blocks: 0,
            html: String::new(),
            entries: Vec::new(),
            slide_width_emu,
            slide_height_emu,
            background: SlidePaint::default(),
        };
        archive
            .with_part(&part, |source| {
                parse_text_part(
                    source,
                    &part_assets,
                    &part_charts,
                    &theme_colors,
                    &mut chunks,
                )
                .map_err(|error| PackageError::ReadPartError(error.to_string()))
            })
            .map_err(package_error)?;
        chunks.flush()?;
    }

    let mut manifest = base_manifest(options, "pptx", "slide-canvas", source_hash, source_size);
    manifest.warnings.push(FidelityWarning {
        code: "PPTX_PARTIAL_VISUAL_LAYOUT".to_string(),
        message: "HCD materializes direct slide backgrounds, shape geometry/fills/outlines/rotation, text layout and run formatting, relationship-bound embedded pictures, cached-data chart SVG previews and bounded progressive DrawingML table row-group fragments with repeated geometry/grid, merges and direct cell formatting; inherited masters/layouts and table styles, grouped transforms, picture cropping/effects, native chart styling, SmartArt and animations remain authoritative in the immutable source".to_string(),
        node_id: None,
        source_part: Some("ppt/presentation.xml".to_string()),
    });
    manifest.fidelity = Some(FidelityReport {
        schema_version: HCD_SCHEMA_VERSION.to_string(),
        level: FidelityLevel::Visual,
        preserved: vec![
            "slide order, direct solid/gradient backgrounds, editable DrawingML text and direct text-shape geometry/fills/outlines/rotation".to_string(),
            "direct paragraph alignment, font, size, color and emphasis".to_string(),
            "relationship-bound embedded pictures with direct position and size".to_string(),
            "relationship-bound chart placement with bounded pure-Rust SVG previews generated from cached OOXML series data".to_string(),
            "DrawingML table position, size, row heights, column widths, merged cells, direct cell fill/margins/borders/alignment and editable anchor-cell text in bounded progressive row-group fragments".to_string(),
            "opaque presentation parts and media in the immutable source".to_string(),
        ],
        flattened: vec![
            "master/layout and table-style inheritance, grouped shape transforms, picture cropping/effects, native chart styling/interactivity, SmartArt and animations are not fully rendered in HCD HTML".to_string(),
            "text found in merged table continuation cells is preserved as hidden read-only source text; the merge anchor remains the editable visible cell".to_string(),
            "clients reconstruct a large table canvas from repeated-geometry fragments sharing data-hcd-table-node-id and contiguous row ranges".to_string(),
        ],
        dropped: Vec::new(),
        warnings: manifest.warnings.clone(),
    });
    finish_import(writer, manifest, emit)
}

fn parse_text_part<F>(
    source: &mut dyn Read,
    asset_relationships: &HashMap<String, AssetRecord>,
    chart_relationships: &HashMap<String, AssetRecord>,
    theme_colors: &HashMap<String, String>,
    chunks: &mut SlideChunkWriter<'_, F>,
) -> Result<(), HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let mut reader = Reader::from_reader(BufReader::with_capacity(64 * 1024, source));
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::with_capacity(64 * 1024);
    let mut current: Option<String> = None;
    let mut ordinal = 0u64;
    let mut shape_ordinal = 0usize;
    let mut picture_ordinal = 0usize;
    let mut graphic_frame_ordinal = 0usize;
    let mut shape: Option<ShapeBuilder> = None;
    let mut picture: Option<PictureBuilder> = None;
    let mut graphic_frame: Option<GraphicFrameBuilder> = None;
    let mut paragraph: Option<SlideParagraphBuilder> = None;
    let mut run: Option<SlideRunBuilder> = None;
    let mut xml_depth = 0usize;
    let mut transform_depth = None;
    let mut background_properties_depth = None;
    let mut shape_properties_depth = None;
    let mut shape_line_depth = None;
    let mut paint_state: Option<(usize, PaintTarget)> = None;
    let mut gradient_stop: Option<(usize, usize)> = None;
    let mut run_properties_depth = None;
    let mut table_depth = None;
    let mut table_cell_properties_depth = None;
    let mut table_fill_depth = None;
    let mut table_border: Option<(usize, &'static str)> = None;
    let mut budget = XmlBudget::default();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| HcdError::InvalidBundle(format!("slide XML parse error: {error}")))?;
        budget.observe(&event, chunks.part)?;
        match event {
            Event::Start(ref start) => {
                xml_depth += 1;
                let qualified_name = start.name();
                let name = local_name(qualified_name.as_ref());
                match name {
                    "sp" => {
                        shape_ordinal += 1;
                        shape = Some(ShapeBuilder {
                            ordinal: shape_ordinal,
                            ..Default::default()
                        });
                    }
                    "pic" => {
                        picture_ordinal += 1;
                        picture = Some(PictureBuilder {
                            ordinal: picture_ordinal,
                            ..Default::default()
                        });
                    }
                    "graphicFrame" => {
                        graphic_frame_ordinal += 1;
                        graphic_frame = Some(GraphicFrameBuilder {
                            ordinal: graphic_frame_ordinal,
                            ..Default::default()
                        });
                    }
                    "cNvPr" if shape.is_some() => capture_shape_identity(start, shape.as_mut()),
                    "cNvPr" if picture.is_some() => {
                        capture_picture_identity(start, picture.as_mut())
                    }
                    "cNvPr" if graphic_frame.is_some() => {
                        capture_graphic_frame_identity(start, graphic_frame.as_mut())
                    }
                    "xfrm" if shape.is_some() || picture.is_some() || graphic_frame.is_some() => {
                        transform_depth = Some(xml_depth);
                        if let Some(shape) = shape.as_mut() {
                            shape.rotation =
                                attribute(start, "rot").and_then(|value| value.parse::<i64>().ok());
                        }
                    }
                    "bgPr" if shape.is_none() => background_properties_depth = Some(xml_depth),
                    "spPr" if shape.is_some() => shape_properties_depth = Some(xml_depth),
                    "prstGeom" if shape_properties_depth.is_some() => {
                        if let Some(shape) = shape.as_mut() {
                            shape.geometry = attribute(start, "prst");
                        }
                    }
                    "bodyPr" if shape.is_some() => {
                        capture_shape_body_properties(start, shape.as_mut())
                    }
                    "ln" if shape_properties_depth.is_some() => {
                        shape_line_depth = Some(xml_depth);
                        if let Some(shape) = shape.as_mut() {
                            shape.line.width =
                                attribute(start, "w").and_then(|value| value.parse::<u64>().ok());
                        }
                    }
                    "solidFill" | "gradFill"
                        if background_properties_depth.is_some()
                            || shape_properties_depth.is_some() =>
                    {
                        let target = if background_properties_depth.is_some() && shape.is_none() {
                            PaintTarget::Background
                        } else if shape_line_depth.is_some() {
                            PaintTarget::ShapeLine
                        } else {
                            PaintTarget::ShapeFill
                        };
                        reset_paint_for_start(
                            paint_mut(target, &mut chunks.background, shape.as_mut()),
                            name == "gradFill",
                        );
                        paint_state = Some((xml_depth, target));
                    }
                    "noFill" if shape_line_depth.is_some() => {
                        if let Some(shape) = shape.as_mut() {
                            shape.line.paint.none = true;
                        }
                    }
                    "noFill" if shape_properties_depth.is_some() => {
                        if let Some(shape) = shape.as_mut() {
                            shape.fill.none = true;
                        }
                    }
                    "noFill" if background_properties_depth.is_some() && shape.is_none() => {
                        chunks.background.none = true;
                    }
                    "gs" if paint_state.is_some() => {
                        if let Some((_, target)) = paint_state {
                            let position = attribute(start, "pos")
                                .and_then(|value| value.parse::<u32>().ok())
                                .unwrap_or(0)
                                .min(100_000);
                            let paint = paint_mut(target, &mut chunks.background, shape.as_mut());
                            paint.gradient_stops.push(SlideGradientStop {
                                position,
                                ..Default::default()
                            });
                            gradient_stop = Some((xml_depth, paint.gradient_stops.len() - 1));
                        }
                    }
                    "lin" if paint_state.is_some() => {
                        if let Some((_, target)) = paint_state {
                            paint_mut(target, &mut chunks.background, shape.as_mut())
                                .gradient_angle =
                                attribute(start, "ang").and_then(|value| value.parse::<i64>().ok());
                        }
                    }
                    "alpha" if paint_state.is_some() => capture_paint_alpha(
                        start,
                        paint_state,
                        gradient_stop,
                        &mut chunks.background,
                        shape.as_mut(),
                    ),
                    "off" if transform_depth.is_some() => {
                        capture_shape_offset(start, shape.as_mut());
                        capture_picture_offset(start, picture.as_mut());
                        capture_graphic_frame_offset(start, graphic_frame.as_mut());
                    }
                    "ext" if transform_depth.is_some() => {
                        capture_shape_extent(start, shape.as_mut());
                        capture_picture_extent(start, picture.as_mut());
                        capture_graphic_frame_extent(start, graphic_frame.as_mut());
                    }
                    "blip" if picture.is_some() => {
                        capture_picture_relationship(start, picture.as_mut())
                    }
                    "tbl" if graphic_frame.is_some() => {
                        start_slide_table(graphic_frame.as_mut(), chunks.part)?;
                        table_depth = Some(xml_depth);
                    }
                    "chart" if graphic_frame.is_some() => {
                        if let Some(frame) = graphic_frame.as_mut() {
                            frame.chart_relationship_id = relationship_id_attribute(start);
                        }
                    }
                    "tblPr" if table_depth.is_some() => {
                        capture_slide_table_properties(start, graphic_frame.as_mut());
                    }
                    "gridCol" if table_depth.is_some() => {
                        push_slide_table_grid_column(start, graphic_frame.as_mut(), chunks.part)?;
                    }
                    "tr" if table_depth.is_some() => {
                        start_slide_table_row(start, graphic_frame.as_mut(), chunks)?;
                    }
                    "tc" if table_depth.is_some() => {
                        start_slide_table_cell(start, graphic_frame.as_mut(), chunks.part)?;
                    }
                    "tcPr" if slide_table_cell_mut(graphic_frame.as_mut()).is_some() => {
                        table_cell_properties_depth = Some(xml_depth);
                        capture_slide_table_cell_properties(start, graphic_frame.as_mut());
                    }
                    "lnL" | "lnR" | "lnT" | "lnB" if table_cell_properties_depth.is_some() => {
                        let side = table_border_side(name).expect("matched table border");
                        capture_slide_table_border(start, graphic_frame.as_mut(), side);
                        table_border = Some((xml_depth, side));
                    }
                    "solidFill" if table_cell_properties_depth.is_some() => {
                        table_fill_depth = Some(xml_depth);
                    }
                    "noFill" if table_border.is_some() => {
                        capture_slide_table_border_no_fill(graphic_frame.as_mut(), table_border);
                    }
                    "prstDash" if table_border.is_some() => {
                        capture_slide_table_border_dash(
                            start,
                            graphic_frame.as_mut(),
                            table_border,
                        );
                    }
                    "p" if shape.is_some()
                        || slide_table_cell_mut(graphic_frame.as_mut()).is_some() =>
                    {
                        paragraph = Some(SlideParagraphBuilder::default());
                    }
                    "pPr" if paragraph.is_some() => {
                        capture_slide_paragraph_property(start, paragraph.as_mut())
                    }
                    "r" | "fld" if paragraph.is_some() => {
                        run = Some(SlideRunBuilder::default());
                    }
                    "rPr" if run.is_some() => {
                        run_properties_depth = Some(xml_depth);
                        capture_slide_run_property(start, run.as_mut());
                    }
                    "latin" if run_properties_depth.is_some() => {
                        if let Some(run) = run.as_mut() {
                            run.format.font = attribute(start, "typeface");
                        }
                    }
                    "srgbClr" if run_properties_depth.is_some() => {
                        if let Some(run) = run.as_mut() {
                            run.format.color = attribute(start, "val").and_then(strict_rgb);
                        }
                    }
                    "schemeClr" if run_properties_depth.is_some() => {
                        if let Some(run) = run.as_mut() {
                            run.format.color = attribute(start, "val")
                                .and_then(|value| theme_colors.get(&value).cloned());
                        }
                    }
                    "srgbClr" if table_border.is_some() => {
                        capture_slide_table_border_color(
                            start,
                            graphic_frame.as_mut(),
                            table_border,
                        );
                    }
                    "srgbClr" if table_fill_depth.is_some() => {
                        capture_slide_table_fill_color(start, graphic_frame.as_mut());
                    }
                    "srgbClr" if paint_state.is_some() => capture_paint_color(
                        start,
                        paint_state,
                        gradient_stop,
                        &mut chunks.background,
                        shape.as_mut(),
                    ),
                    "schemeClr" if paint_state.is_some() => capture_paint_scheme_color(
                        start,
                        paint_state,
                        gradient_stop,
                        theme_colors,
                        &mut chunks.background,
                        shape.as_mut(),
                    ),
                    "t" => current = Some(String::new()),
                    _ => {}
                }
            }
            Event::Empty(ref empty) => {
                let qualified_name = empty.name();
                match local_name(qualified_name.as_ref()) {
                    "cNvPr" if shape.is_some() => capture_shape_identity(empty, shape.as_mut()),
                    "cNvPr" if picture.is_some() => {
                        capture_picture_identity(empty, picture.as_mut())
                    }
                    "cNvPr" if graphic_frame.is_some() => {
                        capture_graphic_frame_identity(empty, graphic_frame.as_mut())
                    }
                    "bgPr" if shape.is_none() => {}
                    "spPr" if shape.is_some() => {}
                    "prstGeom" if shape_properties_depth.is_some() => {
                        if let Some(shape) = shape.as_mut() {
                            shape.geometry = attribute(empty, "prst");
                        }
                    }
                    "bodyPr" if shape.is_some() => {
                        capture_shape_body_properties(empty, shape.as_mut())
                    }
                    "off" if transform_depth.is_some() => {
                        capture_shape_offset(empty, shape.as_mut());
                        capture_picture_offset(empty, picture.as_mut());
                        capture_graphic_frame_offset(empty, graphic_frame.as_mut());
                    }
                    "ext" if transform_depth.is_some() => {
                        capture_shape_extent(empty, shape.as_mut());
                        capture_picture_extent(empty, picture.as_mut());
                        capture_graphic_frame_extent(empty, graphic_frame.as_mut());
                    }
                    "blip" if picture.is_some() => {
                        capture_picture_relationship(empty, picture.as_mut())
                    }
                    "tblPr" if table_depth.is_some() => {
                        capture_slide_table_properties(empty, graphic_frame.as_mut());
                    }
                    "chart" if graphic_frame.is_some() => {
                        if let Some(frame) = graphic_frame.as_mut() {
                            frame.chart_relationship_id = relationship_id_attribute(empty);
                        }
                    }
                    "gridCol" if table_depth.is_some() => {
                        push_slide_table_grid_column(empty, graphic_frame.as_mut(), chunks.part)?;
                    }
                    "tcPr" if slide_table_cell_mut(graphic_frame.as_mut()).is_some() => {
                        capture_slide_table_cell_properties(empty, graphic_frame.as_mut());
                    }
                    "lnL" | "lnR" | "lnT" | "lnB" if table_cell_properties_depth.is_some() => {
                        let side = table_border_side(local_name(empty.name().as_ref()))
                            .expect("matched table border");
                        capture_slide_table_border(empty, graphic_frame.as_mut(), side);
                    }
                    "noFill" if table_border.is_some() => {
                        capture_slide_table_border_no_fill(graphic_frame.as_mut(), table_border);
                    }
                    "prstDash" if table_border.is_some() => {
                        capture_slide_table_border_dash(
                            empty,
                            graphic_frame.as_mut(),
                            table_border,
                        );
                    }
                    "pPr" if paragraph.is_some() => {
                        capture_slide_paragraph_property(empty, paragraph.as_mut())
                    }
                    "rPr" if run.is_some() => capture_slide_run_property(empty, run.as_mut()),
                    "latin" if run_properties_depth.is_some() => {
                        if let Some(run) = run.as_mut() {
                            run.format.font = attribute(empty, "typeface");
                        }
                    }
                    "srgbClr" if run_properties_depth.is_some() => {
                        if let Some(run) = run.as_mut() {
                            run.format.color = attribute(empty, "val").and_then(strict_rgb);
                        }
                    }
                    "schemeClr" if run_properties_depth.is_some() => {
                        if let Some(run) = run.as_mut() {
                            run.format.color = attribute(empty, "val")
                                .and_then(|value| theme_colors.get(&value).cloned());
                        }
                    }
                    "srgbClr" if table_border.is_some() => {
                        capture_slide_table_border_color(
                            empty,
                            graphic_frame.as_mut(),
                            table_border,
                        );
                    }
                    "srgbClr" if table_fill_depth.is_some() => {
                        capture_slide_table_fill_color(empty, graphic_frame.as_mut());
                    }
                    "srgbClr" if paint_state.is_some() => capture_paint_color(
                        empty,
                        paint_state,
                        gradient_stop,
                        &mut chunks.background,
                        shape.as_mut(),
                    ),
                    "schemeClr" if paint_state.is_some() => capture_paint_scheme_color(
                        empty,
                        paint_state,
                        gradient_stop,
                        theme_colors,
                        &mut chunks.background,
                        shape.as_mut(),
                    ),
                    "alpha" if paint_state.is_some() => capture_paint_alpha(
                        empty,
                        paint_state,
                        gradient_stop,
                        &mut chunks.background,
                        shape.as_mut(),
                    ),
                    "lin" if paint_state.is_some() => {
                        if let Some((_, target)) = paint_state {
                            paint_mut(target, &mut chunks.background, shape.as_mut())
                                .gradient_angle =
                                attribute(empty, "ang").and_then(|value| value.parse::<i64>().ok());
                        }
                    }
                    "noFill" if shape_line_depth.is_some() => {
                        if let Some(shape) = shape.as_mut() {
                            shape.line.paint.none = true;
                        }
                    }
                    "noFill" if shape_properties_depth.is_some() => {
                        if let Some(shape) = shape.as_mut() {
                            shape.fill.none = true;
                        }
                    }
                    "noFill" if background_properties_depth.is_some() && shape.is_none() => {
                        chunks.background.none = true;
                    }
                    "t" => {
                        ordinal += 1;
                        append_slide_text(
                            String::new(),
                            ordinal,
                            shape.as_mut(),
                            graphic_frame.as_mut(),
                            paragraph.as_mut(),
                            run.as_mut(),
                            chunks,
                        )?;
                    }
                    _ => {}
                }
            }
            Event::Text(text) if current.is_some() => {
                let decoded = text.unescape().map_err(|error| {
                    HcdError::InvalidBundle(format!("slide text decode: {error}"))
                })?;
                let value = current.as_mut().expect("checked text");
                value.push_str(&decoded);
                if value.len() > MAX_CHUNK_BYTES {
                    return Err(HcdError::ResourceLimit(
                        "NODE_TOO_LARGE: slide text exceeds 2 MiB".to_string(),
                    ));
                }
            }
            Event::End(ref end) => {
                let qualified_name = end.name();
                let name = local_name(qualified_name.as_ref());
                match name {
                    "t" => {
                        ordinal += 1;
                        append_slide_text(
                            current.take().unwrap_or_default(),
                            ordinal,
                            shape.as_mut(),
                            graphic_frame.as_mut(),
                            paragraph.as_mut(),
                            run.as_mut(),
                            chunks,
                        )?;
                    }
                    "r" | "fld" => close_slide_run(paragraph.as_mut(), run.take()),
                    "rPr" if run_properties_depth == Some(xml_depth) => run_properties_depth = None,
                    "p" if shape.is_some()
                        || slide_table_cell_mut(graphic_frame.as_mut()).is_some() =>
                    {
                        close_slide_run(paragraph.as_mut(), run.take());
                        if let Some(paragraph) = paragraph.take() {
                            append_finished_slide_paragraph(
                                finish_slide_paragraph(paragraph),
                                shape.as_mut(),
                                graphic_frame.as_mut(),
                                chunks.part,
                            )?;
                        }
                    }
                    "solidFill" if table_fill_depth == Some(xml_depth) => table_fill_depth = None,
                    "gs" if gradient_stop.is_some_and(|(depth, _)| depth == xml_depth) => {
                        gradient_stop = None
                    }
                    "solidFill" | "gradFill"
                        if paint_state.is_some_and(|(depth, _)| depth == xml_depth) =>
                    {
                        paint_state = None;
                        gradient_stop = None;
                    }
                    "ln" if shape_line_depth == Some(xml_depth) => shape_line_depth = None,
                    "spPr" if shape_properties_depth == Some(xml_depth) => {
                        shape_properties_depth = None
                    }
                    "bgPr" if background_properties_depth == Some(xml_depth) => {
                        background_properties_depth = None
                    }
                    "lnL" | "lnR" | "lnT" | "lnB"
                        if table_border.is_some_and(|(depth, _)| depth == xml_depth) =>
                    {
                        table_border = None
                    }
                    "tcPr" if table_cell_properties_depth == Some(xml_depth) => {
                        table_cell_properties_depth = None
                    }
                    "tc" if table_depth.is_some() => {
                        finish_slide_table_cell(graphic_frame.as_mut(), chunks.part)?;
                    }
                    "tr" if table_depth.is_some() => {
                        finish_slide_table_row(graphic_frame.as_mut(), chunks)?;
                    }
                    "tbl" if table_depth == Some(xml_depth) => table_depth = None,
                    "xfrm" if transform_depth == Some(xml_depth) => transform_depth = None,
                    "sp" => {
                        if let Some(shape) = shape.take() {
                            if !shape.content.is_empty() || !shape.entries.is_empty() {
                                chunks.push_block(finish_shape(
                                    chunks.document_id,
                                    chunks.part,
                                    shape,
                                ))?;
                            }
                        }
                    }
                    "pic" => {
                        if let Some(picture) = picture.take() {
                            chunks.push_block(finish_picture(
                                chunks.document_id,
                                chunks.part,
                                picture,
                                asset_relationships,
                            ))?;
                        }
                    }
                    "graphicFrame" => {
                        if let Some(mut graphic_frame) = graphic_frame.take() {
                            if graphic_frame.table.is_some() {
                                finish_slide_table(&mut graphic_frame, chunks)?;
                            } else if graphic_frame.chart_relationship_id.is_some() {
                                chunks.push_block(finish_chart(
                                    chunks.document_id,
                                    chunks.part,
                                    graphic_frame,
                                    chart_relationships,
                                ))?;
                            }
                        }
                    }
                    _ => {}
                }
                xml_depth = xml_depth.checked_sub(1).ok_or_else(|| {
                    HcdError::InvalidBundle(format!("unbalanced slide XML in {}", chunks.part))
                })?;
            }
            Event::Eof => {
                budget.finish(chunks.part)?;
                if table_depth.is_some()
                    || table_cell_properties_depth.is_some()
                    || table_border.is_some()
                    || graphic_frame
                        .as_ref()
                        .and_then(|frame| frame.table.as_ref())
                        .is_some_and(|table| {
                            table.current_row.is_some() || table.current_cell.is_some()
                        })
                {
                    return Err(HcdError::InvalidBundle(format!(
                        "unfinished DrawingML table in {}",
                        chunks.part
                    )));
                }
                break;
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(())
}

fn append_slide_text<F>(
    text: String,
    ordinal: u64,
    mut shape: Option<&mut ShapeBuilder>,
    graphic_frame: Option<&mut GraphicFrameBuilder>,
    paragraph: Option<&mut SlideParagraphBuilder>,
    run: Option<&mut SlideRunBuilder>,
    chunks: &mut SlideChunkWriter<'_, F>,
) -> Result<(), HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let Some(paragraph) = paragraph else {
        return chunks.push_text(text, ordinal);
    };
    ensure_slide_run_open(paragraph, run);
    let node_id = stable_node_id(&[
        chunks.document_id,
        chunks.part,
        "text",
        &ordinal.to_string(),
    ]);
    let node_hash = hash_bytes(text.as_bytes());
    let span = format!(
        "<span data-hcd-id=\"{node_id}\" data-hcd-node-hash=\"{node_hash}\">{}</span>",
        escape_text(&text)
    );
    let buffered_bytes = if let Some(shape) = shape.as_deref() {
        shape.content.len() + paragraph.html.len()
    } else if let Some(table) = graphic_frame
        .as_deref()
        .and_then(|frame| frame.table.as_ref())
    {
        table.current_row.as_ref().map_or(0, |row| row.html.len())
            + table
                .current_cell
                .as_ref()
                .map_or(0, |cell| cell.content.len())
            + paragraph.html.len()
    } else {
        return chunks.push_text(text, ordinal);
    };
    if buffered_bytes.saturating_add(span.len()) > MAX_CHUNK_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "NODE_TOO_LARGE: slide text container in {} exceeds 2 MiB",
            chunks.part
        )));
    }
    paragraph.html.push_str(&span);

    let mut entry = NodeMapEntry {
        node_id,
        node_hash,
        source: SourceAnchor {
            part: chunks.part.to_string(),
            text_ordinal: ordinal,
            paragraph_id: None,
            text_id: None,
            node_kind: "slide-text".to_string(),
            editable: true,
        },
    };
    if let Some(shape) = shape.as_mut() {
        entry.source.paragraph_id = shape.source_id.clone();
        shape.entries.push(entry);
        return Ok(());
    }
    if let Some(frame) = graphic_frame {
        entry.source.paragraph_id = frame.source_id.clone();
        entry.source.node_kind = "table-cell-text".to_string();
        let cell = slide_table_cell_mut(Some(frame)).ok_or_else(|| {
            HcdError::InvalidBundle(format!("table text is outside a cell in {}", chunks.part))
        })?;
        entry.source.editable = !cell.horizontal_merge && !cell.vertical_merge;
        cell.entries.push(entry);
        return Ok(());
    }
    chunks.push_text(text, ordinal)
}

fn append_finished_slide_paragraph(
    html: String,
    shape: Option<&mut ShapeBuilder>,
    graphic_frame: Option<&mut GraphicFrameBuilder>,
    part: &str,
) -> Result<(), HcdError> {
    if let Some(shape) = shape {
        if shape.content.len().saturating_add(html.len()) > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: slide shape in {part} exceeds 2 MiB"
            )));
        }
        shape.content.push_str(&html);
        return Ok(());
    }
    let cell = slide_table_cell_mut(graphic_frame).ok_or_else(|| {
        HcdError::InvalidBundle(format!("table paragraph is outside a cell in {part}"))
    })?;
    if cell.content.len().saturating_add(html.len()) > MAX_CHUNK_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "NODE_TOO_LARGE: table cell in {part} exceeds 2 MiB"
        )));
    }
    cell.content.push_str(&html);
    Ok(())
}

fn ensure_slide_run_open(paragraph: &mut SlideParagraphBuilder, run: Option<&mut SlideRunBuilder>) {
    let Some(run) = run else {
        return;
    };
    if run.opened {
        return;
    }
    paragraph.html.push_str(&slide_run_start(&run.format));
    run.opened = true;
}

fn close_slide_run(paragraph: Option<&mut SlideParagraphBuilder>, run: Option<SlideRunBuilder>) {
    if run.is_some_and(|run| run.opened) {
        if let Some(paragraph) = paragraph {
            paragraph.html.push_str("</span>");
        }
    }
}

fn slide_run_start(format: &SlideRunFormat) -> String {
    let mut attributes = String::from(" class=\"hcd-ppt-run\"");
    if let Some(language) = format.language.as_deref() {
        attributes.push_str(&format!(" lang=\"{}\"", escape_attribute(language)));
    }
    let mut css = Vec::new();
    if let Some(size) = format
        .size_hundredth_points
        .filter(|size| (100..=40_000).contains(size))
    {
        css.push(format!("font-size:{:.2}pt", size as f64 / 100.0));
    }
    if let Some(spacing) = format.spacing_hundredth_points {
        css.push(format!("letter-spacing:{:.2}pt", spacing as f64 / 100.0));
    }
    if format.bold {
        css.push("font-weight:700".to_string());
    }
    if format.italic {
        css.push("font-style:italic".to_string());
    }
    let mut decorations = Vec::new();
    if format.underline {
        decorations.push("underline");
    }
    if format.strike {
        decorations.push("line-through");
    }
    if !decorations.is_empty() {
        css.push(format!("text-decoration:{}", decorations.join(" ")));
    }
    if let Some(color) = &format.color {
        css.push(format!("color:#{color}"));
    }
    if let Some(font) = format.font.as_deref().and_then(safe_css_font) {
        css.push(format!(
            "font-family:'{}',Arial,'Helvetica Neue',sans-serif",
            font.replace('\'', "")
        ));
    }
    if format.rtl {
        css.push("direction:rtl".to_string());
    }
    if !css.is_empty() {
        attributes.push_str(" style=\"");
        attributes.push_str(&css.join(";"));
        attributes.push('"');
    }
    format!("<span{attributes}>")
}

fn capture_shape_identity(element: &BytesStart<'_>, shape: Option<&mut ShapeBuilder>) {
    let Some(shape) = shape else {
        return;
    };
    if let Some(source_id) = attribute(element, "id") {
        shape.source_id = Some(source_id);
    }
    if let Some(name) = attribute(element, "name") {
        shape.name = Some(name);
    }
}

fn capture_shape_offset(element: &BytesStart<'_>, shape: Option<&mut ShapeBuilder>) {
    let Some(shape) = shape else {
        return;
    };
    shape.x = attribute(element, "x").and_then(|value| value.parse().ok());
    shape.y = attribute(element, "y").and_then(|value| value.parse().ok());
}

fn capture_shape_extent(element: &BytesStart<'_>, shape: Option<&mut ShapeBuilder>) {
    let Some(shape) = shape else {
        return;
    };
    shape.width = attribute(element, "cx").and_then(|value| value.parse().ok());
    shape.height = attribute(element, "cy").and_then(|value| value.parse().ok());
}

fn capture_shape_body_properties(element: &BytesStart<'_>, shape: Option<&mut ShapeBuilder>) {
    let Some(shape) = shape else {
        return;
    };
    shape.vertical_anchor = attribute(element, "anchor").and_then(|value| match value.as_str() {
        "ctr" => Some("center"),
        "b" => Some("flex-end"),
        "t" => Some("flex-start"),
        _ => None,
    });
    shape.margin_left = attribute(element, "lIns").and_then(|value| value.parse().ok());
    shape.margin_right = attribute(element, "rIns").and_then(|value| value.parse().ok());
    shape.margin_top = attribute(element, "tIns").and_then(|value| value.parse().ok());
    shape.margin_bottom = attribute(element, "bIns").and_then(|value| value.parse().ok());
}

fn paint_mut<'a>(
    target: PaintTarget,
    background: &'a mut SlidePaint,
    shape: Option<&'a mut ShapeBuilder>,
) -> &'a mut SlidePaint {
    match target {
        PaintTarget::Background => background,
        PaintTarget::ShapeFill => &mut shape.expect("shape fill target requires a shape").fill,
        PaintTarget::ShapeLine => {
            &mut shape
                .expect("shape line target requires a shape")
                .line
                .paint
        }
    }
}

fn reset_paint_for_start(paint: &mut SlidePaint, gradient: bool) {
    paint.none = false;
    paint.color = None;
    paint.alpha = None;
    paint.gradient_stops.clear();
    paint.gradient_angle = gradient.then_some(0);
}

fn capture_paint_color(
    element: &BytesStart<'_>,
    state: Option<(usize, PaintTarget)>,
    gradient_stop: Option<(usize, usize)>,
    background: &mut SlidePaint,
    shape: Option<&mut ShapeBuilder>,
) {
    let Some((_, target)) = state else {
        return;
    };
    let Some(color) = attribute(element, "val").and_then(strict_rgb) else {
        return;
    };
    let paint = paint_mut(target, background, shape);
    if let Some((_, index)) = gradient_stop {
        if let Some(stop) = paint.gradient_stops.get_mut(index) {
            stop.color = Some(color);
        }
    } else {
        paint.color = Some(color);
    }
}

fn capture_paint_scheme_color(
    element: &BytesStart<'_>,
    state: Option<(usize, PaintTarget)>,
    gradient_stop: Option<(usize, usize)>,
    theme_colors: &HashMap<String, String>,
    background: &mut SlidePaint,
    shape: Option<&mut ShapeBuilder>,
) {
    let Some((_, target)) = state else {
        return;
    };
    let Some(color) = attribute(element, "val").and_then(|value| theme_colors.get(&value).cloned())
    else {
        return;
    };
    let paint = paint_mut(target, background, shape);
    if let Some((_, index)) = gradient_stop {
        if let Some(stop) = paint.gradient_stops.get_mut(index) {
            stop.color = Some(color);
        }
    } else {
        paint.color = Some(color);
    }
}

fn capture_paint_alpha(
    element: &BytesStart<'_>,
    state: Option<(usize, PaintTarget)>,
    gradient_stop: Option<(usize, usize)>,
    background: &mut SlidePaint,
    shape: Option<&mut ShapeBuilder>,
) {
    let Some((_, target)) = state else {
        return;
    };
    let Some(alpha) = attribute(element, "val")
        .and_then(|value| value.parse::<u32>().ok())
        .map(|value| value.min(100_000))
    else {
        return;
    };
    let paint = paint_mut(target, background, shape);
    if let Some((_, index)) = gradient_stop {
        if let Some(stop) = paint.gradient_stops.get_mut(index) {
            stop.alpha = Some(alpha);
        }
    } else {
        paint.alpha = Some(alpha);
    }
}

fn capture_picture_identity(element: &BytesStart<'_>, picture: Option<&mut PictureBuilder>) {
    let Some(picture) = picture else {
        return;
    };
    if let Some(source_id) = attribute(element, "id") {
        picture.source_id = Some(source_id);
    }
    if let Some(name) = attribute(element, "name") {
        picture.name = Some(name);
    }
}

fn capture_picture_offset(element: &BytesStart<'_>, picture: Option<&mut PictureBuilder>) {
    let Some(picture) = picture else {
        return;
    };
    picture.x = attribute(element, "x").and_then(|value| value.parse().ok());
    picture.y = attribute(element, "y").and_then(|value| value.parse().ok());
}

fn capture_picture_extent(element: &BytesStart<'_>, picture: Option<&mut PictureBuilder>) {
    let Some(picture) = picture else {
        return;
    };
    picture.width = attribute(element, "cx").and_then(|value| value.parse().ok());
    picture.height = attribute(element, "cy").and_then(|value| value.parse().ok());
}

fn capture_picture_relationship(element: &BytesStart<'_>, picture: Option<&mut PictureBuilder>) {
    let Some(picture) = picture else {
        return;
    };
    picture.relationship_id = attribute(element, "embed");
}

fn capture_graphic_frame_identity(
    element: &BytesStart<'_>,
    frame: Option<&mut GraphicFrameBuilder>,
) {
    let Some(frame) = frame else {
        return;
    };
    if let Some(source_id) = attribute(element, "id") {
        frame.source_id = Some(source_id);
    }
    if let Some(name) = attribute(element, "name") {
        frame.name = Some(name);
    }
}

fn capture_graphic_frame_offset(element: &BytesStart<'_>, frame: Option<&mut GraphicFrameBuilder>) {
    let Some(frame) = frame else {
        return;
    };
    frame.x = bounded_i64_attribute(element, "x", -100_000_000_000, 100_000_000_000);
    frame.y = bounded_i64_attribute(element, "y", -100_000_000_000, 100_000_000_000);
}

fn capture_graphic_frame_extent(element: &BytesStart<'_>, frame: Option<&mut GraphicFrameBuilder>) {
    let Some(frame) = frame else {
        return;
    };
    frame.width = bounded_u64_attribute(element, "cx", 100_000_000_000);
    frame.height = bounded_u64_attribute(element, "cy", 100_000_000_000);
}

fn slide_table_mut(frame: Option<&mut GraphicFrameBuilder>) -> Option<&mut SlideTableBuilder> {
    frame?.table.as_mut()
}

fn slide_table_cell_mut(
    frame: Option<&mut GraphicFrameBuilder>,
) -> Option<&mut SlideTableCellBuilder> {
    slide_table_mut(frame)?.current_cell.as_mut()
}

fn start_slide_table(frame: Option<&mut GraphicFrameBuilder>, part: &str) -> Result<(), HcdError> {
    let frame = frame.ok_or_else(|| {
        HcdError::InvalidBundle(format!(
            "DrawingML table is outside a graphic frame in {part}"
        ))
    })?;
    if frame.table.is_some() {
        return Err(HcdError::InvalidBundle(format!(
            "nested DrawingML table in {part}"
        )));
    }
    frame.table = Some(SlideTableBuilder {
        fragment_start_row: 1,
        ..Default::default()
    });
    Ok(())
}

fn capture_slide_table_properties(
    element: &BytesStart<'_>,
    frame: Option<&mut GraphicFrameBuilder>,
) {
    let Some(table) = slide_table_mut(frame) else {
        return;
    };
    table.first_row = boolean_attribute(element, "firstRow").unwrap_or(false);
    table.last_row = boolean_attribute(element, "lastRow").unwrap_or(false);
    table.first_column = boolean_attribute(element, "firstCol").unwrap_or(false);
    table.last_column = boolean_attribute(element, "lastCol").unwrap_or(false);
    table.band_rows = boolean_attribute(element, "bandRow").unwrap_or(false);
    table.band_columns = boolean_attribute(element, "bandCol").unwrap_or(false);
}

fn push_slide_table_grid_column(
    element: &BytesStart<'_>,
    frame: Option<&mut GraphicFrameBuilder>,
    part: &str,
) -> Result<(), HcdError> {
    let table = slide_table_mut(frame).ok_or_else(|| {
        HcdError::InvalidBundle(format!("table grid column is outside a table in {part}"))
    })?;
    if table.grid_columns.len() >= MAX_PPTX_TABLE_COLUMNS {
        return Err(HcdError::ResourceLimit(format!(
            "DrawingML table in {part} exceeds {MAX_PPTX_TABLE_COLUMNS} columns"
        )));
    }
    table
        .grid_columns
        .push(bounded_u64_attribute(element, "w", 100_000_000_000).unwrap_or(0));
    Ok(())
}

fn start_slide_table_row<F>(
    element: &BytesStart<'_>,
    frame: Option<&mut GraphicFrameBuilder>,
    chunks: &mut SlideChunkWriter<'_, F>,
) -> Result<(), HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let frame = frame.ok_or_else(|| {
        HcdError::InvalidBundle(format!("table row is outside a table in {}", chunks.part))
    })?;
    if frame
        .table
        .as_ref()
        .is_some_and(|table| table.fragment_ready)
    {
        flush_slide_table_fragment(frame, chunks, false)?;
    }
    let table = frame.table.as_mut().ok_or_else(|| {
        HcdError::InvalidBundle(format!("table row is outside a table in {}", chunks.part))
    })?;
    if table.current_row.is_some() || table.current_cell.is_some() {
        return Err(HcdError::InvalidBundle(format!(
            "nested DrawingML table row in {}",
            chunks.part
        )));
    }
    if table.row_count >= MAX_PPTX_TABLE_CELLS {
        return Err(HcdError::ResourceLimit(format!(
            "DrawingML table in {} exceeds {MAX_PPTX_TABLE_CELLS} rows",
            chunks.part
        )));
    }
    table.current_row = Some(SlideTableRowBuilder {
        height: bounded_u64_attribute(element, "h", 100_000_000_000),
        hold_before_row: table.hold_until_row,
        ..Default::default()
    });
    Ok(())
}

fn start_slide_table_cell(
    element: &BytesStart<'_>,
    frame: Option<&mut GraphicFrameBuilder>,
    part: &str,
) -> Result<(), HcdError> {
    let table = slide_table_mut(frame).ok_or_else(|| {
        HcdError::InvalidBundle(format!("table cell is outside a table in {part}"))
    })?;
    if table.current_row.is_none() {
        return Err(HcdError::InvalidBundle(format!(
            "table cell is outside a row in {part}"
        )));
    }
    if table.current_cell.is_some() {
        return Err(HcdError::InvalidBundle(format!(
            "nested DrawingML table cell in {part}"
        )));
    }
    if table.cell_count >= MAX_PPTX_TABLE_CELLS {
        return Err(HcdError::ResourceLimit(format!(
            "DrawingML table in {part} exceeds {MAX_PPTX_TABLE_CELLS} cells"
        )));
    }
    table.cell_count += 1;
    table.current_cell = Some(SlideTableCellBuilder {
        grid_span: positive_span_attribute(element, "gridSpan", MAX_PPTX_TABLE_COLUMNS, part)?,
        row_span: positive_span_attribute(element, "rowSpan", MAX_PPTX_TABLE_CELLS, part)?,
        horizontal_merge: boolean_attribute(element, "hMerge").unwrap_or(false),
        vertical_merge: boolean_attribute(element, "vMerge").unwrap_or(false),
        ..Default::default()
    });
    Ok(())
}

fn capture_slide_table_cell_properties(
    element: &BytesStart<'_>,
    frame: Option<&mut GraphicFrameBuilder>,
) {
    let Some(cell) = slide_table_cell_mut(frame) else {
        return;
    };
    cell.anchor = attribute(element, "anchor").and_then(|value| match value.as_str() {
        "t" => Some("top"),
        "ctr" => Some("middle"),
        "b" => Some("bottom"),
        _ => None,
    });
    cell.text_direction = attribute(element, "vert").and_then(|value| match value.as_str() {
        "horz" => Some("horizontal"),
        "vert" | "wordArtVert" => Some("vertical"),
        "vert270" => Some("vertical-270"),
        "eaVert" => Some("east-asian-vertical"),
        _ => None,
    });
    cell.margin_left = bounded_u64_attribute(element, "marL", 100_000_000_000);
    cell.margin_right = bounded_u64_attribute(element, "marR", 100_000_000_000);
    cell.margin_top = bounded_u64_attribute(element, "marT", 100_000_000_000);
    cell.margin_bottom = bounded_u64_attribute(element, "marB", 100_000_000_000);
}

fn table_border_side(name: &str) -> Option<&'static str> {
    match name {
        "lnL" => Some("left"),
        "lnR" => Some("right"),
        "lnT" => Some("top"),
        "lnB" => Some("bottom"),
        _ => None,
    }
}

fn slide_table_border_mut<'a>(
    frame: Option<&'a mut GraphicFrameBuilder>,
    side: &str,
) -> Option<&'a mut SlideTableBorder> {
    let cell = slide_table_cell_mut(frame)?;
    match side {
        "left" => Some(&mut cell.border_left),
        "right" => Some(&mut cell.border_right),
        "top" => Some(&mut cell.border_top),
        "bottom" => Some(&mut cell.border_bottom),
        _ => None,
    }
}

fn capture_slide_table_border(
    element: &BytesStart<'_>,
    frame: Option<&mut GraphicFrameBuilder>,
    side: &'static str,
) {
    let Some(border) = slide_table_border_mut(frame, side) else {
        return;
    };
    border.present = true;
    border.width = bounded_u64_attribute(element, "w", 100_000_000_000);
}

fn capture_slide_table_border_no_fill(
    frame: Option<&mut GraphicFrameBuilder>,
    border_state: Option<(usize, &'static str)>,
) {
    if let Some(border) = border_state.and_then(|(_, side)| slide_table_border_mut(frame, side)) {
        border.none = true;
    }
}

fn capture_slide_table_border_dash(
    element: &BytesStart<'_>,
    frame: Option<&mut GraphicFrameBuilder>,
    border_state: Option<(usize, &'static str)>,
) {
    let Some(border) = border_state.and_then(|(_, side)| slide_table_border_mut(frame, side))
    else {
        return;
    };
    border.dash = attribute(element, "val").and_then(|value| match value.as_str() {
        "solid" => Some("solid"),
        "dot" | "sysDot" => Some("dotted"),
        "dash" | "lgDash" | "sysDash" | "sysDashDot" | "lgDashDot" | "lgDashDotDot" => {
            Some("dashed")
        }
        _ => None,
    });
}

fn capture_slide_table_border_color(
    element: &BytesStart<'_>,
    frame: Option<&mut GraphicFrameBuilder>,
    border_state: Option<(usize, &'static str)>,
) {
    let Some(border) = border_state.and_then(|(_, side)| slide_table_border_mut(frame, side))
    else {
        return;
    };
    border.color = attribute(element, "val").and_then(strict_rgb);
}

fn capture_slide_table_fill_color(
    element: &BytesStart<'_>,
    frame: Option<&mut GraphicFrameBuilder>,
) {
    if let Some(cell) = slide_table_cell_mut(frame) {
        cell.fill_color = attribute(element, "val").and_then(strict_rgb);
    }
}

fn capture_slide_paragraph_property(
    element: &BytesStart<'_>,
    paragraph: Option<&mut SlideParagraphBuilder>,
) {
    let Some(paragraph) = paragraph else {
        return;
    };
    paragraph.alignment = attribute(element, "algn");
    paragraph.rtl =
        attribute(element, "rtl").is_some_and(|value| matches!(value.as_str(), "1" | "true"));
}

fn capture_slide_run_property(element: &BytesStart<'_>, run: Option<&mut SlideRunBuilder>) {
    let Some(run) = run else {
        return;
    };
    run.format.size_hundredth_points =
        attribute(element, "sz").and_then(|value| value.parse().ok());
    run.format.spacing_hundredth_points =
        attribute(element, "spc").and_then(|value| value.parse().ok());
    run.format.bold =
        attribute(element, "b").is_some_and(|value| matches!(value.as_str(), "1" | "true"));
    run.format.italic =
        attribute(element, "i").is_some_and(|value| matches!(value.as_str(), "1" | "true"));
    run.format.underline = attribute(element, "u")
        .is_some_and(|value| !matches!(value.as_str(), "none" | "0" | "false"));
    run.format.strike = attribute(element, "strike")
        .is_some_and(|value| !matches!(value.as_str(), "noStrike" | "0" | "false"));
    run.format.rtl =
        attribute(element, "rtl").is_some_and(|value| matches!(value.as_str(), "1" | "true"));
    run.format.language = attribute(element, "lang");
}

fn finish_slide_paragraph(paragraph: SlideParagraphBuilder) -> String {
    let mut css = Vec::new();
    if let Some(alignment) = paragraph.alignment.as_deref().and_then(pptx_alignment) {
        css.push(format!("text-align:{alignment}"));
    }
    if paragraph.rtl {
        css.push("direction:rtl".to_string());
    }
    let style = if css.is_empty() {
        String::new()
    } else {
        format!(" style=\"{}\"", css.join(";"))
    };
    format!("<p class=\"hcd-slide-text\"{style}>{}</p>", paragraph.html)
}

fn finish_shape(document_id: &str, part: &str, shape: ShapeBuilder) -> RenderedSlideBlock {
    let identity = shape
        .source_id
        .clone()
        .unwrap_or_else(|| shape.ordinal.to_string());
    let node_id = stable_node_id(&[document_id, part, "shape", &identity]);
    let mut attributes = format!(" data-hcd-id=\"{node_id}\"");
    if let Some(source_id) = &shape.source_id {
        attributes.push_str(&format!(
            " data-hcd-shape-id=\"{}\"",
            escape_attribute(source_id)
        ));
    }
    if let Some(name) = &shape.name {
        attributes.push_str(&format!(
            " data-hcd-shape-name=\"{}\"",
            escape_attribute(name)
        ));
    }
    for (name, value) in [
        ("data-hcd-x-emu", shape.x.map(|value| value.to_string())),
        ("data-hcd-y-emu", shape.y.map(|value| value.to_string())),
        (
            "data-hcd-width-emu",
            shape.width.map(|value| value.to_string()),
        ),
        (
            "data-hcd-height-emu",
            shape.height.map(|value| value.to_string()),
        ),
        (
            "data-hcd-rotation",
            shape.rotation.map(|value| value.to_string()),
        ),
    ] {
        if let Some(value) = value {
            attributes.push_str(&format!(" {name}=\"{value}\""));
        }
    }
    let mut css = Vec::new();
    if let (Some(x), Some(y), Some(width), Some(height)) =
        (shape.x, shape.y, shape.width, shape.height)
    {
        css.extend([
            "position:absolute".to_string(),
            format!("left:{:.2}px", emu_to_px_signed(x)),
            format!("top:{:.2}px", emu_to_px_signed(y)),
            format!("width:{:.2}px", emu_to_px(width)),
            format!("height:{:.2}px", emu_to_px(height)),
            "overflow:hidden".to_string(),
        ]);
    }
    if let Some(background) = paint_background_css(&shape.fill) {
        css.push(background);
    }
    if !shape.line.paint.none {
        let width_pt = shape
            .line
            .width
            .map_or(0.75, emu_to_points)
            .clamp(0.1, 100.0);
        if let Some(color) = paint_representative_color(&shape.line.paint) {
            for side in ["top", "right", "bottom", "left"] {
                css.push(format!("border-{side}:{width_pt:.2}pt solid {color}"));
            }
        }
    }
    if let Some(geometry) = shape.geometry.as_deref() {
        if geometry == "ellipse" {
            css.push("border-radius:50%".to_string());
        } else if matches!(
            geometry,
            "roundRect" | "round1Rect" | "round2SameRect" | "round2DiagRect"
        ) {
            css.push("border-radius:12px".to_string());
        }
    }
    if let Some(rotation) = shape.rotation {
        css.push(format!(
            "transform:rotate({:.4}deg)",
            rotation as f64 / 60_000.0
        ));
        css.push("transform-origin:center".to_string());
    }
    if let Some(anchor) = shape.vertical_anchor {
        css.push("display:flex".to_string());
        css.push("flex-direction:column".to_string());
        css.push(format!("justify-content:{anchor}"));
    }
    for (property, value) in [
        ("padding-left", shape.margin_left),
        ("padding-right", shape.margin_right),
        ("padding-top", shape.margin_top),
        ("padding-bottom", shape.margin_bottom),
    ] {
        if let Some(value) = value {
            css.push(format!("{property}:{:.2}pt", emu_to_points(value)));
        }
    }
    let style = if css.is_empty() {
        String::new()
    } else {
        format!(" style=\"{}\"", css.join(";"))
    };
    RenderedSlideBlock {
        html: format!(
            "<div class=\"hcd-slide-shape\"{attributes}{style}>{}</div>",
            shape.content
        ),
        entries: shape.entries,
    }
}

fn paint_background_css(paint: &SlidePaint) -> Option<String> {
    if paint.none {
        return None;
    }
    let stops = paint
        .gradient_stops
        .iter()
        .filter_map(|stop| {
            stop.color.as_deref().map(|color| {
                format!(
                    "{} {:.2}%",
                    css_hex_alpha(color, stop.alpha),
                    stop.position as f64 / 1000.0
                )
            })
        })
        .collect::<Vec<_>>();
    if stops.len() >= 2 {
        let angle = paint.gradient_angle.unwrap_or(0) as f64 / 60_000.0 + 90.0;
        return Some(format!(
            "background-image:linear-gradient({angle:.2}deg,{})",
            stops.join(",")
        ));
    }
    paint
        .color
        .as_deref()
        .map(|color| format!("background-color:{}", css_hex_alpha(color, paint.alpha)))
}

fn paint_representative_color(paint: &SlidePaint) -> Option<String> {
    paint
        .color
        .as_deref()
        .map(|color| css_hex_alpha(color, paint.alpha))
        .or_else(|| {
            paint.gradient_stops.iter().find_map(|stop| {
                stop.color
                    .as_deref()
                    .map(|color| css_hex_alpha(color, stop.alpha))
            })
        })
}

fn css_hex_alpha(color: &str, alpha: Option<u32>) -> String {
    let mut output = format!("#{color}");
    if let Some(alpha) = alpha.filter(|alpha| *alpha < 100_000) {
        let byte = ((alpha as u64 * 255 + 50_000) / 100_000) as u8;
        output.push_str(&format!("{byte:02X}"));
    }
    output
}

fn finish_picture(
    document_id: &str,
    part: &str,
    picture: PictureBuilder,
    asset_relationships: &HashMap<String, AssetRecord>,
) -> RenderedSlideBlock {
    let identity = picture
        .source_id
        .clone()
        .unwrap_or_else(|| picture.ordinal.to_string());
    let node_id = stable_node_id(&[document_id, part, "picture", &identity]);
    let asset = picture
        .relationship_id
        .as_ref()
        .and_then(|id| asset_relationships.get(id));
    let geometry = match (picture.x, picture.y, picture.width, picture.height) {
        (Some(x), Some(y), Some(width), Some(height)) => Some(hcd_core::ImageGeometry {
            x: x as f64,
            y: y as f64,
            width: width as f64,
            height: height as f64,
            unit: hcd_core::ImageGeometryUnit::Emu,
        }),
        _ => None,
    };
    let node_hash = hash_bytes(b"");
    let visual_hash =
        hcd_core::image_visual_hash(asset.map(|asset| asset.hash.as_str()), geometry.as_ref());
    let source_path = picture
        .source_id
        .as_deref()
        .map(|source_id| format!("/picture[@id={}]", escape_attribute(source_id)))
        .unwrap_or_else(|| format!("/picture[{}]", picture.ordinal));
    let mut attributes = format!(
        " data-hcd-id=\"{node_id}\" data-hcd-node-hash=\"{node_hash}\" data-hcd-visual-hash=\"{visual_hash}\" data-hcd-node-kind=\"image\" data-hcd-editable=\"true\" data-hcd-source-part=\"{}\" data-hcd-source-path=\"{source_path}\"",
        escape_attribute(part)
    );
    if let Some(asset) = asset {
        attributes.push_str(&format!(" data-hcd-asset-hash=\"{}\"", asset.hash));
    }
    if let Some(geometry) = &geometry {
        attributes.push_str(&format!(
            " data-hcd-x=\"{}\" data-hcd-y=\"{}\" data-hcd-width=\"{}\" data-hcd-height=\"{}\" data-hcd-geometry-unit=\"emu\"",
            geometry.x, geometry.y, geometry.width, geometry.height
        ));
    }
    if let Some(source_id) = &picture.source_id {
        attributes.push_str(&format!(
            " data-hcd-picture-id=\"{}\"",
            escape_attribute(source_id)
        ));
    }
    if let Some(name) = &picture.name {
        attributes.push_str(&format!(
            " data-hcd-picture-name=\"{}\"",
            escape_attribute(name)
        ));
    }
    if let Some(relationship_id) = &picture.relationship_id {
        attributes.push_str(&format!(
            " data-hcd-image-relationship=\"{}\"",
            escape_attribute(relationship_id)
        ));
    }
    for (name, value) in [
        ("data-hcd-x-emu", picture.x.map(|value| value.to_string())),
        ("data-hcd-y-emu", picture.y.map(|value| value.to_string())),
        (
            "data-hcd-width-emu",
            picture.width.map(|value| value.to_string()),
        ),
        (
            "data-hcd-height-emu",
            picture.height.map(|value| value.to_string()),
        ),
    ] {
        if let Some(value) = value {
            attributes.push_str(&format!(" {name}=\"{value}\""));
        }
    }
    let style = match (picture.x, picture.y, picture.width, picture.height) {
        (Some(x), Some(y), Some(width), Some(height)) => format!(
            " style=\"position:absolute;left:{:.2}px;top:{:.2}px;width:{:.2}px;height:{:.2}px;overflow:hidden\"",
            emu_to_px_signed(x),
            emu_to_px_signed(y),
            emu_to_px(width),
            emu_to_px(height)
        ),
        _ => String::new(),
    };
    let image = asset
        .map(|asset| {
            format!(
                "<img src=\"asset://sha256/{}\" data-hcd-asset-href=\"{}\" alt=\"{}\"/>",
                asset.hash,
                escape_attribute(&asset.href),
                escape_attribute(picture.name.as_deref().unwrap_or(""))
            )
        })
        .unwrap_or_default();
    RenderedSlideBlock {
        html: format!("<div class=\"hcd-slide-picture\"{attributes}{style}>{image}</div>"),
        entries: vec![NodeMapEntry {
            node_id,
            node_hash,
            source: SourceAnchor {
                part: part.to_string(),
                text_ordinal: picture.ordinal as u64,
                paragraph_id: Some(source_path),
                text_id: asset.map(|asset| asset.source_part.clone()),
                node_kind: "image".to_string(),
                editable: true,
            },
        }],
    }
}

fn finish_chart(
    document_id: &str,
    part: &str,
    frame: GraphicFrameBuilder,
    chart_relationships: &HashMap<String, AssetRecord>,
) -> RenderedSlideBlock {
    let identity = frame
        .source_id
        .clone()
        .unwrap_or_else(|| frame.ordinal.to_string());
    let node_id = stable_node_id(&[document_id, part, "chart", &identity]);
    let source_path = frame
        .source_id
        .as_deref()
        .map(|source_id| format!("/chart[@id={}]", escape_attribute(source_id)))
        .unwrap_or_else(|| format!("/chart[{}]", frame.ordinal));
    let mut attributes = format!(
        " data-hcd-id=\"{node_id}\" data-hcd-node-kind=\"chart\" data-hcd-editable=\"false\" data-hcd-source-part=\"{}\" data-hcd-source-path=\"{source_path}\"",
        escape_attribute(part)
    );
    if let Some(source_id) = &frame.source_id {
        attributes.push_str(&format!(
            " data-hcd-chart-id=\"{}\"",
            escape_attribute(source_id)
        ));
    }
    if let Some(name) = &frame.name {
        attributes.push_str(&format!(
            " data-hcd-chart-name=\"{}\"",
            escape_attribute(name)
        ));
    }
    if let Some(relationship_id) = &frame.chart_relationship_id {
        attributes.push_str(&format!(
            " data-hcd-chart-relationship=\"{}\"",
            escape_attribute(relationship_id)
        ));
    }
    for (name, value) in [
        ("data-hcd-x-emu", frame.x.map(|value| value.to_string())),
        ("data-hcd-y-emu", frame.y.map(|value| value.to_string())),
        (
            "data-hcd-width-emu",
            frame.width.map(|value| value.to_string()),
        ),
        (
            "data-hcd-height-emu",
            frame.height.map(|value| value.to_string()),
        ),
    ] {
        if let Some(value) = value {
            attributes.push_str(&format!(" {name}=\"{value}\""));
        }
    }
    let style = match (frame.x, frame.y, frame.width, frame.height) {
        (Some(x), Some(y), Some(width), Some(height)) => format!(
            " style=\"position:absolute;left:{:.2}px;top:{:.2}px;width:{:.2}px;height:{:.2}px;overflow:hidden\"",
            emu_to_px_signed(x),
            emu_to_px_signed(y),
            emu_to_px(width),
            emu_to_px(height)
        ),
        _ => String::new(),
    };
    let image = frame
        .chart_relationship_id
        .as_ref()
        .and_then(|id| chart_relationships.get(id))
        .map(|asset| {
            format!(
                "<img src=\"asset://sha256/{}\" data-hcd-asset-href=\"{}\" alt=\"{}\"/>",
                asset.hash,
                escape_attribute(&asset.href),
                escape_attribute(frame.name.as_deref().unwrap_or("Chart"))
            )
        })
        .unwrap_or_default();
    RenderedSlideBlock {
        html: format!("<div class=\"hcd-slide-chart\"{attributes}{style}>{image}</div>"),
        entries: Vec::new(),
    }
}

fn finish_slide_table_cell(
    frame: Option<&mut GraphicFrameBuilder>,
    part: &str,
) -> Result<(), HcdError> {
    let table = slide_table_mut(frame).ok_or_else(|| {
        HcdError::InvalidBundle(format!("table cell ended outside a table in {part}"))
    })?;
    let cell = table.current_cell.take().ok_or_else(|| {
        HcdError::InvalidBundle(format!("table cell ended without opening in {part}"))
    })?;
    let row = table.current_row.as_mut().ok_or_else(|| {
        HcdError::InvalidBundle(format!("table cell ended outside a row in {part}"))
    })?;
    let grid_span = cell.grid_span.max(1);
    let row_span = cell.row_span.max(1);
    let continuation = cell.horizontal_merge || cell.vertical_merge;
    let column = row.cell_count.saturating_add(1);
    if !continuation
        && !table.grid_columns.is_empty()
        && column.saturating_add(grid_span).saturating_sub(1) > table.grid_columns.len()
    {
        return Err(HcdError::InvalidBundle(format!(
            "DrawingML table cell at row {}, column {column} in {part} spans beyond its {}-column grid",
            table.row_count + 1,
            table.grid_columns.len()
        )));
    }
    if !continuation {
        let span_end = table.row_count.saturating_add(row_span);
        table.max_row_span_end = table.max_row_span_end.max(span_end);
        table.hold_until_row = table.hold_until_row.max(span_end);
    }
    let mut attributes = format!(
        " data-hcd-row=\"{}\" data-hcd-column=\"{column}\"",
        table.row_count + 1
    );
    if continuation {
        attributes.push_str(" data-hcd-merge-continuation=\"true\" data-hcd-editable=\"false\"");
    } else {
        if grid_span > 1 {
            attributes.push_str(&format!(
                " colspan=\"{grid_span}\" data-hcd-grid-span=\"{grid_span}\""
            ));
        }
        if row_span > 1 {
            attributes.push_str(&format!(
                " rowspan=\"{row_span}\" data-hcd-row-span=\"{row_span}\""
            ));
        }
    }
    if let Some(direction) = cell.text_direction {
        attributes.push_str(&format!(" data-hcd-text-direction=\"{direction}\""));
    }

    let mut css = Vec::new();
    if continuation {
        css.push("display:none".to_string());
    }
    if let Some(color) = &cell.fill_color {
        css.push(format!("background-color:#{color}"));
    }
    if let Some(anchor) = cell.anchor {
        css.push(format!("vertical-align:{anchor}"));
    }
    for (property, value) in [
        ("padding-left", cell.margin_left),
        ("padding-right", cell.margin_right),
        ("padding-top", cell.margin_top),
        ("padding-bottom", cell.margin_bottom),
    ] {
        if let Some(value) = value {
            css.push(format!("{property}:{:.2}pt", emu_to_points(value)));
        }
    }
    for (property, border) in [
        ("border-left", &cell.border_left),
        ("border-right", &cell.border_right),
        ("border-top", &cell.border_top),
        ("border-bottom", &cell.border_bottom),
    ] {
        if let Some(value) = slide_table_border_css(border) {
            css.push(format!("{property}:{value}"));
        }
    }
    if !css.is_empty() {
        attributes.push_str(" style=\"");
        attributes.push_str(&css.join(";"));
        attributes.push('"');
    }
    let html = format!("<td{attributes}>{}</td>", cell.content);
    let buffered = row.html.len().saturating_add(html.len());
    if buffered > MAX_CHUNK_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "NODE_TOO_LARGE: DrawingML table in {part} exceeds 2 MiB"
        )));
    }
    row.html.push_str(&html);
    row.entries.extend(cell.entries);
    row.cell_count += 1;
    Ok(())
}

fn finish_slide_table_row<F>(
    frame: Option<&mut GraphicFrameBuilder>,
    chunks: &mut SlideChunkWriter<'_, F>,
) -> Result<(), HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let frame = frame.ok_or_else(|| {
        HcdError::InvalidBundle(format!(
            "table row ended outside a table in {}",
            chunks.part
        ))
    })?;
    let (row, row_number) = {
        let table = frame.table.as_mut().ok_or_else(|| {
            HcdError::InvalidBundle(format!(
                "table row ended outside a table in {}",
                chunks.part
            ))
        })?;
        if table.current_cell.is_some() {
            return Err(HcdError::InvalidBundle(format!(
                "table row ended inside a cell in {}",
                chunks.part
            )));
        }
        let row = table.current_row.take().ok_or_else(|| {
            HcdError::InvalidBundle(format!(
                "table row ended without opening in {}",
                chunks.part
            ))
        })?;
        (row, table.row_count + 1)
    };
    let height = row
        .height
        .map(|height| {
            format!(
                " data-hcd-height-emu=\"{height}\" style=\"height:{:.2}pt\"",
                emu_to_points(height)
            )
        })
        .unwrap_or_default();
    let html = format!(
        "<tr data-hcd-row=\"{row_number}\" data-hcd-cell-count=\"{}\"{height}>{}</tr>",
        row.cell_count, row.html
    );
    let needs_preflush = frame.table.as_ref().is_some_and(|table| {
        !table.rows_html.is_empty()
            && table.rows_html.len().saturating_add(html.len()) > MAX_CHUNK_BYTES
    });
    if needs_preflush {
        let can_split = frame
            .table
            .as_ref()
            .is_some_and(|table| table.row_count >= row.hold_before_row);
        if !can_split {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: DrawingML merged row group in {} exceeds 2 MiB",
                chunks.part
            )));
        }
        flush_slide_table_fragment(frame, chunks, false)?;
    }
    let table = frame.table.as_mut().ok_or_else(|| {
        HcdError::InvalidBundle(format!(
            "table row ended outside a table in {}",
            chunks.part
        ))
    })?;
    if table.rows_html.len().saturating_add(html.len()) > MAX_CHUNK_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "NODE_TOO_LARGE: DrawingML table row in {} exceeds 2 MiB",
            chunks.part
        )));
    }
    table.rows_html.push_str(&html);
    table.entries.extend(row.entries);
    table.row_count += 1;
    let fragment_rows = table
        .row_count
        .saturating_sub(table.fragment_start_row.max(1))
        .saturating_add(1);
    table.fragment_ready = table.row_count >= table.hold_until_row
        && (table.rows_html.len() >= chunks.soft_bytes
            || fragment_rows >= PPTX_TABLE_ROWS_PER_FRAGMENT);
    Ok(())
}

fn finish_slide_table<F>(
    frame: &mut GraphicFrameBuilder,
    chunks: &mut SlideChunkWriter<'_, F>,
) -> Result<(), HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let table = frame.table.as_ref().ok_or_else(|| {
        HcdError::InvalidBundle(format!("graphic frame has no table in {}", chunks.part))
    })?;
    if table.current_cell.is_some() || table.current_row.is_some() {
        return Err(HcdError::InvalidBundle(format!(
            "DrawingML table in {} ended with an unfinished row or cell",
            chunks.part
        )));
    }
    if table.max_row_span_end > table.row_count {
        return Err(HcdError::InvalidBundle(format!(
            "DrawingML table in {} has a rowSpan ending at row {}, beyond its {} rows",
            chunks.part, table.max_row_span_end, table.row_count
        )));
    }
    flush_slide_table_fragment(frame, chunks, true)
}

fn flush_slide_table_fragment<F>(
    frame: &mut GraphicFrameBuilder,
    chunks: &mut SlideChunkWriter<'_, F>,
    final_fragment: bool,
) -> Result<(), HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let document_id = chunks.document_id;
    let part = chunks.part;
    let identity = frame
        .source_id
        .clone()
        .unwrap_or_else(|| frame.ordinal.to_string());
    let table_node_id = stable_node_id(&[document_id, part, "table", &identity]);
    let (
        fragment_ordinal,
        row_start,
        row_end,
        fragment_row_count,
        total_row_count,
        column_count,
        first_row,
        last_row,
        first_column,
        last_column,
        band_rows,
        band_columns,
        columns,
        rows_html,
        entries,
    ) = {
        let table = frame.table.as_mut().ok_or_else(|| {
            HcdError::InvalidBundle(format!("graphic frame has no table in {part}"))
        })?;
        if table.rows_html.is_empty() && !(final_fragment && table.fragment_ordinal == 0) {
            table.fragment_ready = false;
            return Ok(());
        }
        let row_end = table.row_count;
        let row_start = if row_end == 0 {
            0
        } else {
            table.fragment_start_row.max(1)
        };
        let fragment_row_count = if row_end < row_start {
            0
        } else {
            row_end - row_start + 1
        };
        let mut columns = String::new();
        for (index, width) in table.grid_columns.iter().enumerate() {
            if *width > 0 {
                columns.push_str(&format!(
                    "<col data-hcd-column=\"{}\" data-hcd-width-emu=\"{width}\" style=\"width:{:.2}pt\"/>",
                    index + 1,
                    emu_to_points(*width)
                ));
            } else {
                columns.push_str(&format!("<col data-hcd-column=\"{}\"/>", index + 1));
            }
        }
        let result = (
            table.fragment_ordinal,
            row_start,
            row_end,
            fragment_row_count,
            table.row_count,
            table.grid_columns.len(),
            table.first_row,
            table.last_row,
            table.first_column,
            table.last_column,
            table.band_rows,
            table.band_columns,
            columns,
            std::mem::take(&mut table.rows_html),
            std::mem::take(&mut table.entries),
        );
        table.fragment_ordinal += 1;
        table.fragment_start_row = table.row_count.saturating_add(1);
        table.fragment_ready = false;
        result
    };
    let fragment_node_id = stable_node_id(&[
        document_id,
        part,
        "table-fragment",
        &identity,
        &fragment_ordinal.to_string(),
    ]);
    let mut frame_attributes = format!(
        " data-hcd-id=\"{fragment_node_id}\" data-hcd-table-node-id=\"{table_node_id}\" data-hcd-table-fragment=\"{fragment_ordinal}\""
    );
    if let Some(source_id) = &frame.source_id {
        frame_attributes.push_str(&format!(
            " data-hcd-table-id=\"{}\"",
            escape_attribute(source_id)
        ));
    }
    if let Some(name) = &frame.name {
        frame_attributes.push_str(&format!(
            " data-hcd-table-name=\"{}\"",
            escape_attribute(name)
        ));
    }
    for (name, value) in [
        ("data-hcd-x-emu", frame.x.map(|value| value.to_string())),
        ("data-hcd-y-emu", frame.y.map(|value| value.to_string())),
        (
            "data-hcd-width-emu",
            frame.width.map(|value| value.to_string()),
        ),
        (
            "data-hcd-height-emu",
            frame.height.map(|value| value.to_string()),
        ),
    ] {
        if let Some(value) = value {
            frame_attributes.push_str(&format!(" {name}=\"{value}\""));
        }
    }
    let frame_style = match (frame.x, frame.y, frame.width, frame.height) {
        (Some(x), Some(y), Some(width), Some(height)) => format!(
            " style=\"position:absolute;left:{:.2}px;top:{:.2}px;width:{:.2}px;height:{:.2}px;overflow:hidden\"",
            emu_to_px_signed(x),
            emu_to_px_signed(y),
            emu_to_px(width),
            emu_to_px(height)
        ),
        _ => String::new(),
    };
    let mut table_attributes = format!(
        " data-hcd-table-node-id=\"{table_node_id}\" data-hcd-table-fragment=\"{fragment_ordinal}\" data-hcd-row-start=\"{row_start}\" data-hcd-row-end=\"{row_end}\" data-hcd-fragment-row-count=\"{fragment_row_count}\" data-hcd-column-count=\"{column_count}\""
    );
    if fragment_ordinal > 0 {
        table_attributes.push_str(" data-hcd-table-continuation=\"true\"");
    }
    if final_fragment {
        table_attributes.push_str(&format!(
            " data-hcd-table-final=\"true\" data-hcd-row-count=\"{total_row_count}\""
        ));
    }
    for (name, enabled) in [
        ("data-hcd-first-row", first_row),
        ("data-hcd-last-row", last_row),
        ("data-hcd-first-column", first_column),
        ("data-hcd-last-column", last_column),
        ("data-hcd-band-rows", band_rows),
        ("data-hcd-band-columns", band_columns),
    ] {
        if enabled {
            table_attributes.push_str(&format!(" {name}=\"true\""));
        }
    }
    let html = format!(
        "<div class=\"hcd-slide-table-frame\"{frame_attributes}{frame_style}><table class=\"hcd-ppt-table\"{table_attributes} style=\"width:100%;height:100%;table-layout:fixed\"><colgroup>{columns}</colgroup><tbody>{}</tbody></table></div>",
        rows_html
    );
    if html.len() > MAX_CHUNK_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "NODE_TOO_LARGE: DrawingML table in {part} exceeds 2 MiB"
        )));
    }
    if chunks.blocks > 0 {
        chunks.flush()?;
    }
    chunks.push_block(RenderedSlideBlock { html, entries })?;
    chunks.flush()
}

fn slide_table_border_css(border: &SlideTableBorder) -> Option<String> {
    if !border.present {
        return None;
    }
    if border.none {
        return Some("none".to_string());
    }
    let width = emu_to_points(border.width.unwrap_or(12_700)).max(0.25);
    let style = border.dash.unwrap_or("solid");
    let color = border.color.as_deref().unwrap_or("000000");
    Some(format!("{width:.2}pt {style} #{color}"))
}

fn pptx_alignment(value: &str) -> Option<&'static str> {
    match value {
        "l" => Some("left"),
        "ctr" => Some("center"),
        "r" => Some("right"),
        "just" | "dist" | "thaiDist" => Some("justify"),
        _ => None,
    }
}

fn strict_rgb(value: String) -> Option<String> {
    (value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

fn boolean_attribute(element: &BytesStart<'_>, name: &str) -> Option<bool> {
    attribute(element, name).and_then(|value| match value.as_str() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    })
}

fn bounded_u64_attribute(element: &BytesStart<'_>, name: &str, max: u64) -> Option<u64> {
    attribute(element, name)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value <= max)
}

fn bounded_i64_attribute(element: &BytesStart<'_>, name: &str, min: i64, max: i64) -> Option<i64> {
    attribute(element, name)
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (min..=max).contains(value))
}

fn positive_span_attribute(
    element: &BytesStart<'_>,
    name: &str,
    max: usize,
    part: &str,
) -> Result<usize, HcdError> {
    let Some(raw) = attribute(element, name) else {
        return Ok(1);
    };
    let value = raw.parse::<usize>().map_err(|_| {
        HcdError::InvalidBundle(format!("invalid DrawingML {name} value {raw} in {part}"))
    })?;
    if value == 0 || value > max {
        return Err(HcdError::ResourceLimit(format!(
            "DrawingML {name} value {value} in {part} exceeds 1..={max}"
        )));
    }
    Ok(value)
}

fn safe_css_font(value: &str) -> Option<&str> {
    (!value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|character| character.is_alphanumeric() || " -_,.'".contains(character)))
    .then_some(value)
}

fn emu_to_px(value: u64) -> f64 {
    value.min(100_000_000_000) as f64 * 96.0 / 914_400.0
}

fn emu_to_px_signed(value: i64) -> f64 {
    value.clamp(-100_000_000_000, 100_000_000_000) as f64 * 96.0 / 914_400.0
}

fn emu_to_points(value: u64) -> f64 {
    value.min(100_000_000_000) as f64 / 12_700.0
}

fn presentation_size(archive: &mut StreamingOxmlArchive) -> Result<(u64, u64), HcdError> {
    const DEFAULT_WIDTH: u64 = 12_192_000;
    const DEFAULT_HEIGHT: u64 = 6_858_000;
    let xml = archive
        .read_control_part("ppt/presentation.xml", 16 * 1024 * 1024)
        .map_err(package_error)?;
    let mut reader = Reader::from_reader(xml.as_slice());
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(ref element)) | Ok(Event::Empty(ref element))
                if local_name(element.name().as_ref()) == "sldSz" =>
            {
                let width = attribute(element, "cx")
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|value| (1..=100_000_000_000).contains(value))
                    .unwrap_or(DEFAULT_WIDTH);
                let height = attribute(element, "cy")
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|value| (1..=100_000_000_000).contains(value))
                    .unwrap_or(DEFAULT_HEIGHT);
                return Ok((width, height));
            }
            Ok(Event::Eof) => return Ok((DEFAULT_WIDTH, DEFAULT_HEIGHT)),
            Ok(_) => {}
            Err(error) => {
                return Err(HcdError::InvalidBundle(format!(
                    "presentation size XML: {error}"
                )))
            }
        }
        buffer.clear();
    }
}

fn presentation_theme_colors(
    archive: &mut StreamingOxmlArchive,
) -> Result<HashMap<String, String>, HcdError> {
    let mut colors = HashMap::from([
        ("dk1".to_string(), "000000".to_string()),
        ("lt1".to_string(), "ffffff".to_string()),
        ("dk2".to_string(), "1f497d".to_string()),
        ("lt2".to_string(), "e5e0ec".to_string()),
        ("accent1".to_string(), "4f81bd".to_string()),
        ("accent2".to_string(), "c0504d".to_string()),
        ("accent3".to_string(), "9bbb59".to_string()),
        ("accent4".to_string(), "8064a2".to_string()),
        ("accent5".to_string(), "4bacc6".to_string()),
        ("accent6".to_string(), "f79646".to_string()),
        ("hlink".to_string(), "0000ff".to_string()),
        ("folHlink".to_string(), "800080".to_string()),
    ]);
    let mut themes = archive
        .entries()
        .iter()
        .filter(|entry| entry.name.starts_with("ppt/theme/theme") && entry.name.ends_with(".xml"))
        .map(|entry| entry.name.clone())
        .collect::<Vec<_>>();
    themes.sort_by_key(|part| numeric_suffix(part));
    let Some(theme) = themes.first() else {
        return Ok(colors);
    };
    let xml = archive
        .read_control_part(theme, 16 * 1024 * 1024)
        .map_err(package_error)?;
    let mut reader = Reader::from_reader(xml.as_slice());
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut current: Option<(usize, String)> = None;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(ref element)) => {
                depth += 1;
                let qualified_name = element.name();
                let name = local_name(qualified_name.as_ref());
                if matches!(
                    name,
                    "dk1"
                        | "lt1"
                        | "dk2"
                        | "lt2"
                        | "accent1"
                        | "accent2"
                        | "accent3"
                        | "accent4"
                        | "accent5"
                        | "accent6"
                        | "hlink"
                        | "folHlink"
                ) {
                    current = Some((depth, name.to_string()));
                } else if let Some((_, key)) = current.as_ref() {
                    let value = match name {
                        "srgbClr" => attribute(element, "val").and_then(strict_rgb),
                        "sysClr" => attribute(element, "lastClr").and_then(strict_rgb),
                        _ => None,
                    };
                    if let Some(value) = value {
                        colors.insert(key.clone(), value);
                    }
                }
            }
            Ok(Event::Empty(ref element)) => {
                if let Some((_, key)) = current.as_ref() {
                    let value = match local_name(element.name().as_ref()) {
                        "srgbClr" => attribute(element, "val").and_then(strict_rgb),
                        "sysClr" => attribute(element, "lastClr").and_then(strict_rgb),
                        _ => None,
                    };
                    if let Some(value) = value {
                        colors.insert(key.clone(), value);
                    }
                }
            }
            Ok(Event::End(_)) => {
                if current.as_ref().is_some_and(|(start, _)| *start == depth) {
                    current = None;
                }
                depth = depth.checked_sub(1).ok_or_else(|| {
                    HcdError::InvalidBundle(format!("unbalanced theme XML in {theme}"))
                })?;
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => {
                return Err(HcdError::InvalidBundle(format!(
                    "presentation theme XML: {error}"
                )))
            }
        }
        buffer.clear();
    }
    Ok(colors)
}

fn presentation_slides(
    archive: &mut StreamingOxmlArchive,
) -> Result<Vec<(String, &'static str)>, HcdError> {
    const PRESENTATION: &str = "ppt/presentation.xml";
    const RELATIONSHIPS: &str = "ppt/_rels/presentation.xml.rels";
    if !archive.contains(RELATIONSHIPS) {
        return Err(HcdError::InvalidBundle(format!(
            "PPTX is missing {RELATIONSHIPS}"
        )));
    }

    let relationships_xml = archive
        .read_control_part(RELATIONSHIPS, 16 * 1024 * 1024)
        .map_err(package_error)?;
    let mut relationships = HashMap::new();
    let mut reader = Reader::from_reader(relationships_xml.as_slice());
    let mut buffer = Vec::new();
    let mut budget = XmlBudget::default();
    loop {
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            HcdError::InvalidBundle(format!("presentation relationships XML: {error}"))
        })?;
        budget.observe(&event, RELATIONSHIPS)?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element)
                if local_name(element.name().as_ref()) == "Relationship" =>
            {
                let is_slide =
                    attribute(element, "Type").is_some_and(|kind| kind.ends_with("/slide"));
                let is_external = attribute(element, "TargetMode")
                    .is_some_and(|mode| mode.eq_ignore_ascii_case("external"));
                if is_slide && !is_external {
                    if let (Some(id), Some(target)) =
                        (attribute(element, "Id"), attribute(element, "Target"))
                    {
                        relationships.insert(id, resolve_part(PRESENTATION, &target)?);
                    }
                }
            }
            Event::Eof => {
                budget.finish(RELATIONSHIPS)?;
                break;
            }
            _ => {}
        }
        buffer.clear();
    }

    let presentation_xml = archive
        .read_control_part(PRESENTATION, 16 * 1024 * 1024)
        .map_err(package_error)?;
    let mut reader = Reader::from_reader(presentation_xml.as_slice());
    let mut slides = Vec::new();
    let mut budget = XmlBudget::default();
    loop {
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            HcdError::InvalidBundle(format!("presentation slide order XML: {error}"))
        })?;
        budget.observe(&event, PRESENTATION)?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element)
                if local_name(element.name().as_ref()) == "sldId" =>
            {
                let relationship_id = relationship_id_attribute(element).ok_or_else(|| {
                    HcdError::InvalidBundle(
                        "presentation slide entry has no relationship id".to_string(),
                    )
                })?;
                let part = relationships
                    .get(&relationship_id)
                    .cloned()
                    .ok_or_else(|| {
                        HcdError::InvalidBundle(format!(
                            "presentation slide relationship {relationship_id} is missing"
                        ))
                    })?;
                if !archive.contains(&part) {
                    return Err(HcdError::InvalidBundle(format!(
                        "presentation slide part {part} is missing"
                    )));
                }
                slides.push((part, "slide"));
            }
            Event::Eof => {
                budget.finish(PRESENTATION)?;
                break;
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(slides)
}

fn part_asset_relationships(
    archive: &mut StreamingOxmlArchive,
    source_part: &str,
    asset_records: &HashMap<String, AssetRecord>,
) -> Result<HashMap<String, AssetRecord>, HcdError> {
    let source_path = Path::new(source_part);
    let file_name = source_path.file_name().ok_or_else(|| {
        HcdError::InvalidBundle(format!("invalid presentation part path {source_part}"))
    })?;
    let parent = source_path.parent().unwrap_or_else(|| Path::new(""));
    let relationships_part = parent
        .join("_rels")
        .join(format!("{}.rels", file_name.to_string_lossy()))
        .to_string_lossy()
        .replace('\\', "/");
    if !archive.contains(&relationships_part) {
        return Ok(HashMap::new());
    }

    let xml = archive
        .read_control_part(&relationships_part, 16 * 1024 * 1024)
        .map_err(package_error)?;
    let mut reader = Reader::from_reader(xml.as_slice());
    let mut buffer = Vec::new();
    let mut relationships = HashMap::new();
    let mut budget = XmlBudget::default();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| HcdError::InvalidBundle(format!("part relationships XML: {error}")))?;
        budget.observe(&event, &relationships_part)?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element)
                if local_name(element.name().as_ref()) == "Relationship" =>
            {
                let is_image =
                    attribute(element, "Type").is_some_and(|kind| kind.ends_with("/image"));
                let is_external = attribute(element, "TargetMode")
                    .is_some_and(|mode| mode.eq_ignore_ascii_case("external"));
                if is_image && !is_external {
                    if let (Some(id), Some(target)) =
                        (attribute(element, "Id"), attribute(element, "Target"))
                    {
                        let target_part = resolve_part(source_part, &target)?;
                        if let Some(asset) = asset_records.get(&target_part) {
                            relationships.insert(id, asset.clone());
                        }
                    }
                }
            }
            Event::Eof => {
                budget.finish(&relationships_part)?;
                break;
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(relationships)
}

fn part_chart_relationships(
    archive: &mut StreamingOxmlArchive,
    source_part: &str,
    asset_records: &HashMap<String, AssetRecord>,
) -> Result<HashMap<String, AssetRecord>, HcdError> {
    let source_path = Path::new(source_part);
    let file_name = source_path.file_name().ok_or_else(|| {
        HcdError::InvalidBundle(format!("invalid presentation part path {source_part}"))
    })?;
    let parent = source_path.parent().unwrap_or_else(|| Path::new(""));
    let relationships_part = parent
        .join("_rels")
        .join(format!("{}.rels", file_name.to_string_lossy()))
        .to_string_lossy()
        .replace('\\', "/");
    if !archive.contains(&relationships_part) {
        return Ok(HashMap::new());
    }
    let xml = archive
        .read_control_part(&relationships_part, 16 * 1024 * 1024)
        .map_err(package_error)?;
    let mut reader = Reader::from_reader(xml.as_slice());
    let mut buffer = Vec::new();
    let mut relationships = HashMap::new();
    let mut budget = XmlBudget::default();
    loop {
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            HcdError::InvalidBundle(format!("part chart relationships XML: {error}"))
        })?;
        budget.observe(&event, &relationships_part)?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element)
                if local_name(element.name().as_ref()) == "Relationship" =>
            {
                let is_chart = attribute(element, "Type")
                    .is_some_and(|kind| kind.ends_with("/chart") || kind.ends_with("/chartEx"));
                let is_external = attribute(element, "TargetMode")
                    .is_some_and(|mode| mode.eq_ignore_ascii_case("external"));
                if is_chart && !is_external {
                    if let (Some(id), Some(target)) =
                        (attribute(element, "Id"), attribute(element, "Target"))
                    {
                        let target_part = resolve_part(source_part, &target)?;
                        if let Some(asset) = asset_records.get(&target_part) {
                            relationships.insert(id, asset.clone());
                        }
                    }
                }
            }
            Event::Eof => {
                budget.finish(&relationships_part)?;
                break;
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(relationships)
}

fn import_chart_assets<F>(
    archive: &mut StreamingOxmlArchive,
    writer: &BundleWriter,
    emit: &mut F,
) -> Result<Vec<AssetRecord>, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let mut parts = archive
        .entries()
        .iter()
        .filter(|entry| !entry.is_dir && is_presentation_chart_part(&entry.name))
        .map(|entry| entry.name.clone())
        .collect::<Vec<_>>();
    parts.sort_by_key(|part| numeric_suffix(part));
    let mut assets = Vec::new();
    for part in parts {
        let xml = archive
            .read_control_part(&part, 16 * 1024 * 1024)
            .map_err(package_error)?;
        let xml = std::str::from_utf8(&xml).map_err(|error| {
            HcdError::InvalidBundle(format!("chart {part} is not UTF-8: {error}"))
        })?;
        let svg = oxml::chart_preview::render_chart_svg(xml).map_err(|error| {
            HcdError::InvalidBundle(format!("cannot render cached chart {part}: {error}"))
        })?;
        let (href, hash, byte_length) =
            writer.write_asset_from_reader("svg", &mut Cursor::new(svg.as_bytes()))?;
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

fn is_presentation_chart_part(part: &str) -> bool {
    let lower = part.to_ascii_lowercase();
    if !lower.starts_with("ppt/") || !lower.contains("/charts/") || !lower.ends_with(".xml") {
        return false;
    }
    let stem = Path::new(&lower)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    stem.strip_prefix("chartex")
        .or_else(|| stem.strip_prefix("chart"))
        .is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
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
        .filter(|entry| !entry.is_dir && entry.name.starts_with("ppt/media/"))
        .map(|entry| entry.name.clone())
        .collect();
    let mut assets = Vec::new();
    let mut published_hashes = HashSet::new();
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
        if published_hashes.insert(hash.clone()) {
            emit(&ImportEvent::AssetReady {
                hash: hash.clone(),
                href: href.clone(),
                byte_length,
            })?;
        }
        assets.push(AssetRecord {
            source_part: part,
            hash,
            href,
            byte_length,
        });
    }
    Ok(assets)
}

pub(crate) fn export_pptx(
    bundle: &Bundle,
    source: &Path,
    target: &Path,
    options: &ExportOptions,
) -> Result<FidelityReport, HcdError> {
    let (manifest, _, dirty_parts, dirty_node_ids) = checked_export_state(bundle, source, options)?;
    let nodes = collect_dirty_nodes(bundle, &manifest, &dirty_parts, &dirty_node_ids)?;
    let mut replacements: HashMap<String, BTreeMap<u64, String>> = HashMap::new();
    for node in nodes {
        replacements
            .entry(node.source.part)
            .or_default()
            .insert(node.source.text_ordinal, node.text);
    }
    let scratch = tempfile::tempdir()?;
    let mut replacement_paths = HashMap::new();
    let mut archive = StreamingOxmlArchive::open(source).map_err(package_error)?;
    for (part, values) in &replacements {
        let path = scratch.path().join(safe_temp_name(part));
        let output = File::create(&path)?;
        archive
            .with_part(part, |input| {
                rewrite_text_part(input, BufWriter::new(output), values)
                    .map_err(|error| PackageError::ReadPartError(error.to_string()))
            })
            .map_err(package_error)?;
        replacement_paths.insert(part.clone(), path);
    }
    let changed =
        StreamingOxmlRewriter::rewrite(source, target, &replacement_paths, "ppt/presentation.xml")
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
            "slide geometry, run formatting, media, charts, notes and animations".to_string(),
        ],
        flattened: Vec::new(),
        dropped: vec!["HCD recognition annotations are not exported".to_string()],
        warnings: manifest.warnings,
    };
    write_fidelity_report(options, &report)?;
    Ok(report)
}

fn rewrite_text_part(
    source: &mut dyn Read,
    output: impl Write,
    replacements: &BTreeMap<u64, String>,
) -> Result<(), HcdError> {
    let mut reader = Reader::from_reader(BufReader::with_capacity(64 * 1024, source));
    reader.config_mut().check_end_names = true;
    let mut writer = Writer::new(output);
    let mut buffer = Vec::with_capacity(64 * 1024);
    let mut ordinal = 0u64;
    let mut replacing = false;
    let mut seen = BTreeSet::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| HcdError::InvalidBundle(format!("slide export XML: {error}")))?;
        match event {
            Event::Start(ref start) if local_name(start.name().as_ref()) == "t" => {
                ordinal += 1;
                if let Some(text) = replacements.get(&ordinal) {
                    writer.write_event(event.into_owned())?;
                    if !text.is_empty() {
                        writer.write_event(Event::Text(BytesText::new(text)))?;
                    }
                    replacing = true;
                    seen.insert(ordinal);
                } else {
                    writer.write_event(event.into_owned())?;
                }
            }
            Event::Empty(ref empty) if local_name(empty.name().as_ref()) == "t" => {
                ordinal += 1;
                if let Some(text) = replacements.get(&ordinal) {
                    writer.write_event(Event::Start(empty.to_owned()))?;
                    if !text.is_empty() {
                        writer.write_event(Event::Text(BytesText::new(text)))?;
                    }
                    writer.write_event(Event::End(quick_xml::events::BytesEnd::new(
                        String::from_utf8_lossy(empty.name().as_ref()),
                    )))?;
                    seen.insert(ordinal);
                } else {
                    writer.write_event(event.into_owned())?;
                }
            }
            Event::Text(_) | Event::CData(_) if replacing => {}
            Event::End(ref end) if local_name(end.name().as_ref()) == "t" && replacing => {
                writer.write_event(event.into_owned())?;
                replacing = false;
            }
            Event::Eof => break,
            _ => writer.write_event(event.into_owned())?,
        }
        buffer.clear();
    }
    if seen.len() != replacements.len() {
        return Err(HcdError::InvalidBundle(
            "slide source map contains missing text ordinals".to_string(),
        ));
    }
    Ok(())
}

fn numeric_suffix(path: &str) -> u64 {
    path.chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(u64::MAX)
}

fn relationship_id_attribute(element: &BytesStart<'_>) -> Option<String> {
    element
        .attributes()
        .with_checks(false)
        .flatten()
        .find(|attribute| {
            let name = attribute.key.as_ref();
            name.contains(&b':') && local_name(name) == "id"
        })
        .and_then(|attribute| {
            attribute
                .unescape_value()
                .ok()
                .map(|value| value.into_owned())
        })
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
        apply_patch, extract_text_page, validate_bundle, NodePrecondition, PatchBatch,
        PatchOperation, HCD_PATCH_SCHEMA_VERSION,
    };
    use std::cell::Cell;
    use std::rc::Rc;
    use zip::write::SimpleFileOptions;

    struct EofTrackingReader {
        source: std::io::Cursor<Vec<u8>>,
        eof_seen: Rc<Cell<bool>>,
        max_read: usize,
    }

    impl Read for EofTrackingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let wanted = buffer.len().min(self.max_read);
            let count = self.source.read(&mut buffer[..wanted])?;
            if count == 0 {
                self.eof_seen.set(true);
            }
            Ok(count)
        }
    }

    #[test]
    fn imports_text_shape_geometry_and_direct_run_formatting() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("styled.pptx");
        let bundle_path = temp.path().join("bundle");
        create_styled_fixture(&source);
        let mut asset_events = 0usize;

        let manifest = import_pptx(
            &source,
            &bundle_path,
            &ImportOptions::new("styled-presentation"),
            |event| {
                if matches!(event, ImportEvent::AssetReady { .. }) {
                    asset_events += 1;
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(asset_events, 1);
        assert_eq!(
            manifest.fidelity.as_ref().map(|report| &report.level),
            Some(&FidelityLevel::Visual)
        );
        assert!(manifest
            .warnings
            .iter()
            .any(|warning| warning.code == "PPTX_PARTIAL_VISUAL_LAYOUT"));

        let bundle = Bundle::open(&bundle_path).unwrap();
        let validation = validate_bundle(&bundle).unwrap();
        assert!(validation.valid, "{:?}", validation.issues);
        let text = extract_text_page(&bundle, None, 10).unwrap();
        assert_eq!(text.entries.len(), 2);
        assert_eq!(text.entries[0].text, "样式 😀");
        assert_eq!(text.entries[1].text, "第二页");
        assert_eq!(manifest.chunk_count, 2);

        let page = bundle.read_index_page(&manifest, 0).unwrap();
        let html = bundle.read_chunk(&page.chunks[0]).unwrap();
        assert!(html.contains("data-hcd-width-emu=\"12192000\""));
        assert!(html.contains("data-hcd-shape-id=\"2\""));
        assert!(html.contains("data-hcd-shape-name=\"Title &amp; More\""));
        assert!(html.contains("data-hcd-x-emu=\"914400\""));
        assert!(html.contains("left:96.00px"));
        assert!(html.contains("width:192.00px"));
        assert!(html.contains("text-align:center"));
        assert!(html.contains("font-size:24.00pt"));
        assert!(html.contains("letter-spacing:1.50pt"));
        assert!(html.contains("font-weight:700"));
        assert!(html.contains("font-style:italic"));
        assert!(html.contains("text-decoration:underline"));
        assert!(html.contains("color:#112233"));
        assert!(html.contains("font-family:'Arial'"));
        assert!(html.contains("data-hcd-picture-id=\"4\""));
        assert!(html.contains("data-hcd-picture-name=\"Picture &amp; One\""));
        assert!(html.contains("data-hcd-node-kind=\"image\""));
        assert!(html.contains("data-hcd-editable=\"true\""));
        assert!(html.contains("data-hcd-source-part=\"ppt/slides/slide2.xml\""));
        assert!(html.contains("data-hcd-source-path=\"/picture[@id=4]\""));
        assert!(html.contains("<img src=\"asset://sha256/"));
        assert!(html.contains("alt=\"Picture &amp; One\""));
        let styles = std::fs::read_to_string(bundle_path.join("styles.css")).unwrap();
        assert!(styles.contains("data-hcd-image-hitboxes"));
        assert!(styles.contains("data-hcd-text-hitboxes"));
        let assets = bundle.read_asset_index().unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].source_part, "ppt/media/image1.png");
    }

    #[test]
    fn renders_direct_slide_background_and_shape_paint_geometry() {
        let temp = tempfile::tempdir().unwrap();
        let bundle_path = temp.path().join("bundle");
        let mut writer = BundleWriter::create(&bundle_path).unwrap();
        let mut descriptors = Vec::new();
        let mut emit = |event: &ImportEvent| {
            if let ImportEvent::ChunkReady { descriptor } = event {
                descriptors.push(descriptor.clone());
            }
            Ok(())
        };
        let mut chunks = SlideChunkWriter {
            document_id: "painted-pptx",
            part: "ppt/slides/slide1.xml",
            region: "slide",
            writer: &mut writer,
            emit: &mut emit,
            soft_bytes: MAX_CHUNK_BYTES,
            max_blocks: DEFAULT_CHUNK_BLOCKS,
            ordinal: 0,
            blocks: 0,
            html: String::new(),
            entries: Vec::new(),
            slide_width_emu: 12_192_000,
            slide_height_emu: 6_858_000,
            background: SlidePaint::default(),
        };
        let xml = r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:bg><p:bgPr><a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="0D1B2A"/></a:gs><a:gs pos="100000"><a:srgbClr val="0A1628"/></a:gs></a:gsLst><a:lin ang="5400000"/></a:gradFill></p:bgPr></p:bg><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="7" name="Painted ellipse"/></p:nvSpPr><p:spPr><a:xfrm rot="2700000"><a:off x="914400" y="457200"/><a:ext cx="1828800" cy="914400"/></a:xfrm><a:prstGeom prst="ellipse"/><a:solidFill><a:schemeClr val="accent1"><a:alpha val="8000"/></a:schemeClr></a:solidFill><a:ln w="25400"><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill></a:ln></p:spPr><p:txBody><a:bodyPr anchor="b" lIns="12700"/><a:p><a:r><a:t>styled</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;
        let mut source = xml.as_bytes();
        let theme = HashMap::from([("accent1".to_string(), "00b4d8".to_string())]);
        parse_text_part(
            &mut source,
            &HashMap::new(),
            &HashMap::new(),
            &theme,
            &mut chunks,
        )
        .unwrap();
        chunks.flush().unwrap();
        drop(chunks);

        assert_eq!(descriptors.len(), 1);
        let html = std::fs::read_to_string(bundle_path.join(&descriptors[0].html_href)).unwrap();
        assert!(html
            .contains("background-image:linear-gradient(180.00deg,#0d1b2a 0.00%,#0a1628 100.00%)"));
        assert!(html.contains("background-color:#00b4d814"));
        assert!(html.contains("border-top:2.00pt solid #ffffff"));
        assert!(html.contains("border-radius:50%"));
        assert!(html.contains("transform:rotate(45.0000deg)"));
        assert!(html.contains("justify-content:flex-end"));
        assert!(html.contains("padding-left:1.00pt"));
    }

    #[test]
    fn imports_drawingml_table_geometry_merges_styles_and_roundtrips_text() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("table.pptx");
        let bundle_path = temp.path().join("bundle");
        let exported = temp.path().join("exported.pptx");
        create_table_fixture(&source);

        let mut options = ImportOptions::new("table-presentation");
        options.chunk_soft_bytes = 1;
        let manifest = import_pptx(&source, &bundle_path, &options, |_| Ok(())).unwrap();
        assert_eq!(manifest.chunk_count, 2);
        let bundle = Bundle::open(&bundle_path).unwrap();
        let validation = validate_bundle(&bundle).unwrap();
        assert!(validation.valid, "{:?}", validation.issues);
        let text = extract_text_page(&bundle, None, 10).unwrap();
        assert_eq!(
            text.entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            vec!["Merged 😀", "Continuation", "左", "右"]
        );
        assert!(text
            .entries
            .iter()
            .all(|entry| entry.source.node_kind == "table-cell-text"));
        assert!(text.entries[0].source.editable);
        assert!(!text.entries[1].source.editable);

        let page = bundle.read_index_page(&manifest, 0).unwrap();
        assert_eq!(page.chunks.len(), 2);
        let first_html = bundle.read_chunk(&page.chunks[0]).unwrap();
        let second_html = bundle.read_chunk(&page.chunks[1]).unwrap();
        assert!(first_html.contains("data-hcd-table-fragment=\"0\""));
        assert!(first_html.contains("data-hcd-row-start=\"1\""));
        assert!(first_html.contains("data-hcd-row-end=\"2\""));
        assert!(first_html.contains("data-hcd-fragment-row-count=\"2\""));
        assert!(!first_html.contains("data-hcd-table-continuation"));
        assert!(!first_html.contains("data-hcd-table-final"));
        assert!(second_html.contains("data-hcd-table-fragment=\"1\""));
        assert!(second_html.contains("data-hcd-row-start=\"3\""));
        assert!(second_html.contains("data-hcd-row-end=\"3\""));
        assert!(second_html.contains("data-hcd-table-continuation=\"true\""));
        assert!(second_html.contains("data-hcd-table-final=\"true\""));
        assert!(second_html.contains("data-hcd-row-count=\"3\""));
        let html = format!("{first_html}{second_html}");
        assert!(html.contains("class=\"hcd-slide-table-frame\""));
        assert!(html.contains("data-hcd-table-id=\"5\""));
        assert!(html.contains("data-hcd-table-name=\"Table &amp; One\""));
        assert!(html.contains("left:96.00px"));
        assert!(html.contains("top:48.00px"));
        assert!(html.contains("width:384.00px"));
        assert!(html.contains("height:192.00px"));
        assert!(html.contains("data-hcd-column-count=\"2\""));
        assert!(html.contains("data-hcd-first-row=\"true\""));
        assert!(html.contains("data-hcd-band-rows=\"true\""));
        assert!(html.contains("data-hcd-width-emu=\"1828800\" style=\"width:144.00pt\""));
        assert!(html.contains("data-hcd-height-emu=\"609600\" style=\"height:48.00pt\""));
        assert!(html.contains("colspan=\"2\" data-hcd-grid-span=\"2\""));
        assert!(html.contains("rowspan=\"2\" data-hcd-row-span=\"2\""));
        assert_eq!(
            html.matches("data-hcd-merge-continuation=\"true\"").count(),
            3
        );
        assert!(html.contains("background-color:#ffeedd"));
        assert!(html.contains("vertical-align:middle"));
        assert!(html.contains("padding-left:1.00pt"));
        assert!(html.contains("padding-right:2.00pt"));
        assert!(html.contains("padding-top:3.00pt"));
        assert!(html.contains("padding-bottom:4.00pt"));
        assert!(html.contains("border-left:2.00pt dashed #112233"));
        assert!(html.contains("data-hcd-text-direction=\"vertical-270\""));

        let continuation = &text.entries[1];
        let read_only_patch = PatchBatch {
            schema_version: HCD_PATCH_SCHEMA_VERSION.to_string(),
            document_id: manifest.document_id.clone(),
            patch_id: "reject-pptx-merge-continuation".to_string(),
            base_revision: 0,
            actor: BTreeMap::new(),
            operations: vec![PatchOperation::TextSplice {
                node_id: continuation.node_id.clone(),
                start: 0,
                delete_count: 1,
                insert_text: "X".to_string(),
                precondition: NodePrecondition {
                    node_hash: continuation.node_hash.clone(),
                },
            }],
            metadata: BTreeMap::new(),
        };
        let error = apply_patch(&bundle, &read_only_patch, 0).unwrap_err();
        assert!(error.to_string().contains("read-only"));
        assert_eq!(bundle.manifest().unwrap().revision, 0);

        let target = &text.entries[0];
        let patch = PatchBatch {
            schema_version: HCD_PATCH_SCHEMA_VERSION.to_string(),
            document_id: manifest.document_id.clone(),
            patch_id: "mask-pptx-table-cell".to_string(),
            base_revision: 0,
            actor: BTreeMap::new(),
            operations: vec![PatchOperation::TextSplice {
                node_id: target.node_id.clone(),
                start: 0,
                delete_count: 6,
                insert_text: "Masked".to_string(),
                precondition: NodePrecondition {
                    node_hash: target.node_hash.clone(),
                },
            }],
            metadata: BTreeMap::new(),
        };
        apply_patch(&bundle, &patch, 0).unwrap();
        let report = export_pptx(&bundle, &source, &exported, &ExportOptions::default()).unwrap();
        assert_eq!(report.level, FidelityLevel::High);
        let slide = read_zip_entry(&exported, "ppt/slides/slide1.xml");
        assert!(slide.contains("Masked 😀"));
        assert!(slide.contains("gridSpan=\"2\""));
        assert!(slide.contains("rowSpan=\"2\""));
        assert!(slide.contains("val=\"FFEEDD\""));
    }

    #[test]
    fn rejects_oversized_drawingml_table_span_before_chunk_publication() {
        let temp = tempfile::tempdir().unwrap();
        let bundle_path = temp.path().join("bundle");
        let mut writer = BundleWriter::create(&bundle_path).unwrap();
        let mut published_chunks = 0usize;
        let mut emit = |event: &ImportEvent| {
            if matches!(event, ImportEvent::ChunkReady { .. }) {
                published_chunks += 1;
            }
            Ok(())
        };
        let mut chunks = SlideChunkWriter {
            document_id: "oversized-pptx-table",
            part: "ppt/slides/slide1.xml",
            region: "slide",
            writer: &mut writer,
            emit: &mut emit,
            soft_bytes: MAX_CHUNK_BYTES,
            max_blocks: DEFAULT_CHUNK_BLOCKS,
            ordinal: 0,
            blocks: 0,
            html: String::new(),
            entries: Vec::new(),
            slide_width_emu: 12_192_000,
            slide_height_emu: 6_858_000,
            background: SlidePaint::default(),
        };
        let xml = format!(
            r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree><p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="9" name="Oversized"/></p:nvGraphicFramePr><a:graphic><a:graphicData><a:tbl><a:tblGrid><a:gridCol w="1"/></a:tblGrid><a:tr h="1"><a:tc gridSpan="{}"><a:txBody><a:p><a:r><a:t>x</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame></p:spTree></p:cSld></p:sld>"#,
            MAX_PPTX_TABLE_COLUMNS + 1
        );
        let mut source = xml.as_bytes();
        let error = parse_text_part(
            &mut source,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &mut chunks,
        )
        .unwrap_err();
        assert!(error.to_string().contains("gridSpan"));
        assert!(error
            .to_string()
            .contains(&MAX_PPTX_TABLE_COLUMNS.to_string()));
        drop(chunks);
        assert_eq!(published_chunks, 0);
        assert!(!bundle_path.join("manifest.json").exists());
    }

    #[test]
    fn large_drawingml_table_streams_fragments_before_part_eof() {
        let temp = tempfile::tempdir().unwrap();
        let bundle_path = temp.path().join("bundle");
        let mut writer = BundleWriter::create(&bundle_path).unwrap();
        let eof_seen = Rc::new(Cell::new(false));
        let event_eof = Rc::clone(&eof_seen);
        let mut descriptors = Vec::new();
        let mut first_chunk_before_eof = false;
        let mut emit = |event: &ImportEvent| {
            if let ImportEvent::ChunkReady { descriptor } = event {
                if descriptors.is_empty() {
                    first_chunk_before_eof = !event_eof.get();
                }
                descriptors.push(descriptor.clone());
            }
            Ok(())
        };
        let mut chunks = SlideChunkWriter {
            document_id: "progressive-pptx-table",
            part: "ppt/slides/slide1.xml",
            region: "slide",
            writer: &mut writer,
            emit: &mut emit,
            soft_bytes: 32 * 1024,
            max_blocks: DEFAULT_CHUNK_BLOCKS,
            ordinal: 0,
            blocks: 0,
            html: String::new(),
            entries: Vec::new(),
            slide_width_emu: 12_192_000,
            slide_height_emu: 6_858_000,
            background: SlidePaint::default(),
        };
        let payload = "x".repeat(2_048);
        let mut xml = String::from(
            r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree><p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="10" name="Progressive"/></p:nvGraphicFramePr><a:graphic><a:graphicData><a:tbl><a:tblGrid><a:gridCol w="914400"/></a:tblGrid>"#,
        );
        for row in 1..=300 {
            xml.push_str(&format!(
                "<a:tr h=\"12700\"><a:tc><a:txBody><a:p><a:r><a:t>row-{row:03}-{payload}</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc></a:tr>"
            ));
        }
        xml.push_str(
            "</a:tbl></a:graphicData></a:graphic></p:graphicFrame></p:spTree></p:cSld></p:sld>",
        );
        let mut source = EofTrackingReader {
            source: std::io::Cursor::new(xml.into_bytes()),
            eof_seen: Rc::clone(&eof_seen),
            max_read: 4 * 1024,
        };

        parse_text_part(
            &mut source,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &mut chunks,
        )
        .unwrap();
        drop(chunks);

        assert!(first_chunk_before_eof);
        assert!(eof_seen.get());
        assert!(descriptors.len() > 2);
        assert!(descriptors
            .iter()
            .all(|descriptor| descriptor.byte_length as usize <= MAX_CHUNK_BYTES));
        let mut expected_start = 1usize;
        for (index, descriptor) in descriptors.iter().enumerate() {
            let html = std::fs::read_to_string(bundle_path.join(&descriptor.html_href)).unwrap();
            assert!(html.contains("data-hcd-table-id=\"10\""));
            assert!(html.contains("<colgroup><col data-hcd-column=\"1\""));
            assert!(html.contains(&format!("data-hcd-row-start=\"{expected_start}\"")));
            let end_marker = "data-hcd-row-end=\"";
            let end_start = html.find(end_marker).unwrap() + end_marker.len();
            let end_offset = html[end_start..].find('"').unwrap();
            let row_end = html[end_start..end_start + end_offset]
                .parse::<usize>()
                .unwrap();
            assert!(row_end >= expected_start);
            expected_start = row_end + 1;
            if index + 1 == descriptors.len() {
                assert!(html.contains("data-hcd-table-final=\"true\""));
                assert!(html.contains("data-hcd-row-count=\"300\""));
            } else {
                assert!(!html.contains("data-hcd-table-final"));
            }
        }
        assert_eq!(expected_start, 301);
    }

    #[test]
    fn oversized_pptx_merged_row_group_fails_before_fragment_publication() {
        let temp = tempfile::tempdir().unwrap();
        let bundle_path = temp.path().join("bundle");
        let mut writer = BundleWriter::create(&bundle_path).unwrap();
        let mut published_chunks = 0usize;
        let mut emit = |event: &ImportEvent| {
            if matches!(event, ImportEvent::ChunkReady { .. }) {
                published_chunks += 1;
            }
            Ok(())
        };
        let mut chunks = SlideChunkWriter {
            document_id: "oversized-pptx-merge",
            part: "ppt/slides/slide1.xml",
            region: "slide",
            writer: &mut writer,
            emit: &mut emit,
            soft_bytes: 1,
            max_blocks: DEFAULT_CHUNK_BLOCKS,
            ordinal: 0,
            blocks: 0,
            html: String::new(),
            entries: Vec::new(),
            slide_width_emu: 12_192_000,
            slide_height_emu: 6_858_000,
            background: SlidePaint::default(),
        };
        // Keep every XML text node comfortably below the node limit while making the
        // indivisible three-row merge group larger than a single HCD chunk.
        let payload = "x".repeat(710_000);
        let xml = format!(
            r#"<p:sld xmlns:p="p" xmlns:a="a"><p:cSld><p:spTree><p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="11" name="Oversized merge"/></p:nvGraphicFramePr><a:graphic><a:graphicData><a:tbl><a:tblGrid><a:gridCol w="914400"/></a:tblGrid><a:tr h="12700"><a:tc rowSpan="3"><a:txBody><a:p><a:r><a:t>{payload}</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc></a:tr><a:tr h="12700"><a:tc vMerge="1"><a:txBody><a:p><a:r><a:t>{payload}</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc></a:tr><a:tr h="12700"><a:tc vMerge="1"><a:txBody><a:p><a:r><a:t>{payload}</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame></p:spTree></p:cSld></p:sld>"#
        );
        let mut source = xml.as_bytes();

        let error = parse_text_part(
            &mut source,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &mut chunks,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("merged row group"),
            "unexpected error: {error}"
        );
        drop(chunks);
        assert_eq!(published_chunks, 0);
        assert!(!bundle_path.join("manifest.json").exists());
    }

    fn create_table_fixture(path: &Path) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let parts = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#,
            ),
            (
                "ppt/presentation.xml",
                r#"<?xml version="1.0"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldSz cx="12192000" cy="6858000"/><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#,
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#,
            ),
            (
                "ppt/slides/slide1.xml",
                r#"<?xml version="1.0"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="5" name="Table &amp; One"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="914400" y="457200"/><a:ext cx="3657600" cy="1828800"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr firstRow="1" bandRow="1"/><a:tblGrid><a:gridCol w="1828800"/><a:gridCol w="1828800"/></a:tblGrid><a:tr h="609600"><a:tc gridSpan="2" rowSpan="2"><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="1800" b="1"/><a:t>Merged 😀</a:t></a:r></a:p></a:txBody><a:tcPr marL="12700" marR="25400" marT="38100" marB="50800" anchor="ctr"><a:lnL w="25400"><a:solidFill><a:srgbClr val="112233"/></a:solidFill><a:prstDash val="dash"/></a:lnL><a:solidFill><a:srgbClr val="FFEEDD"/></a:solidFill></a:tcPr></a:tc><a:tc hMerge="1"><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Continuation</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc></a:tr><a:tr h="609600"><a:tc vMerge="1"><a:txBody><a:bodyPr/><a:lstStyle/><a:p/></a:txBody><a:tcPr/></a:tc><a:tc hMerge="1" vMerge="1"><a:txBody><a:bodyPr/><a:lstStyle/><a:p/></a:txBody><a:tcPr/></a:tc></a:tr><a:tr h="609600"><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>左</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc><a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>右</a:t></a:r></a:p></a:txBody><a:tcPr vert="vert270"/></a:tc></a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame></p:spTree></p:cSld></p:sld>"#,
            ),
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
        let mut contents = String::new();
        entry.read_to_string(&mut contents).unwrap();
        contents
    }

    fn create_styled_fixture(path: &Path) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let parts = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/><Override PartName="/ppt/slides/slide2.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#,
            ),
            (
                "ppt/presentation.xml",
                r#"<?xml version="1.0"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldSz cx="12192000" cy="6858000"/><p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="257" r:id="rId1"/></p:sldIdLst></p:presentation>"#,
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide2.xml"/></Relationships>"#,
            ),
            (
                "ppt/slides/slide2.xml",
                r#"<?xml version="1.0"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title &amp; More"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm rot="5400000"><a:off x="914400" y="457200"/><a:ext cx="1828800" cy="914400"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:pPr algn="ctr"/><a:r><a:rPr lang="zh-CN" sz="2400" spc="150" b="1" i="1" u="sng"><a:solidFill><a:srgbClr val="112233"/></a:solidFill><a:latin typeface="Arial"/></a:rPr><a:t>样式 😀</a:t></a:r></a:p></p:txBody></p:sp><p:pic><p:nvPicPr><p:cNvPr id="4" name="Picture &amp; One"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdImage1"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="3657600" y="914400"/><a:ext cx="1828800" cy="1371600"/></a:xfrm></p:spPr></p:pic></p:spTree></p:cSld></p:sld>"#,
            ),
            (
                "ppt/slides/_rels/slide2.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdImage1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/></Relationships>"#,
            ),
            (
                "ppt/slides/slide1.xml",
                r#"<?xml version="1.0"?><p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:cNvPr id="3" name="Second"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>第二页</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#,
            ),
            (
                "ppt/media/image1.png",
                "not-a-real-png-but-streamed-as-an-asset",
            ),
            (
                "ppt/media/image2.png",
                "not-a-real-png-but-streamed-as-an-asset",
            ),
        ];
        for (name, contents) in parts {
            zip.start_file(name, options).unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
}
