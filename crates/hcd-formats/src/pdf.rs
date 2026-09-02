use crate::common::{
    base_manifest, checked_export_state, collect_dirty_nodes, emit_failed, emit_started,
    escape_attribute, escape_text, finish_import, source_identity, write_fidelity_report,
    ExportOptions, ImportOptions,
};
use handler_common::DocumentHandler;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{render as render_pdf_page, RenderSettings};
use hcd_core::{
    hash_bytes, stable_node_id, AssetDescriptor, Bundle, BundleWriter, ChunkSourceMap,
    FidelityLevel, FidelityReport, FidelityWarning, HcdError, HcdManifest, ImportEvent,
    NodeMapEntry, SourceAnchor, DEFAULT_CHUNK_BLOCKS, HCD_SCHEMA_VERSION, MAX_CHUNK_BYTES,
};
use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

const MAX_HCD_PDF_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_HCD_PDF_PAGES: usize = 100_000;
const MAX_HCD_PDF_PAGE_CONTENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_HCD_PDF_AUX_STREAM_BYTES: usize = 8 * 1024 * 1024;
const MAX_HCD_PDF_PAGE_IMAGE_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;
const MAX_HCD_PDF_STREAM_DICTIONARY_BYTES: usize = 256 * 1024;
const MAX_HCD_PDF_STRUCTURAL_STREAM_BYTES: usize = 16 * 1024 * 1024;
const MAX_HCD_PDF_STRUCTURAL_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_HCD_PDF_STRUCTURAL_STREAMS: usize = 100_000;
const MAX_HCD_PDF_XREF_ENTRIES: usize = 1_000_000;
const HCD_PDF_RASTER_SCALE: f32 = 96.0 / 72.0;
const MAX_HCD_PDF_RASTER_DIMENSION: u32 = 8_192;
const MAX_HCD_PDF_RASTER_PIXELS: u64 = 32 * 1024 * 1024;
const MAX_HCD_PDF_RASTER_PNG_BYTES: usize = 64 * 1024 * 1024;

fn sanitize_pdf_text(text: String) -> (String, usize) {
    let mut replaced = 0usize;
    let sanitized = text
        .chars()
        .map(|character| {
            if matches!(
                character as u32,
                0x0..=0x8 | 0xB | 0xC | 0xE..=0x1F | 0xFFFE | 0xFFFF
            ) {
                replaced += 1;
                ' '
            } else {
                character
            }
        })
        .collect();
    (sanitized, replaced)
}

struct PdfPageWriter<'a, F>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    document_id: &'a str,
    page: usize,
    writer: &'a mut BundleWriter,
    emit: &'a mut F,
    soft_bytes: usize,
    max_blocks: usize,
    page_width: f32,
    page_height: f32,
    page_llx: f32,
    page_lly: f32,
    ordinal: usize,
    blocks: usize,
    html: String,
    entries: Vec<NodeMapEntry>,
    page_raster: Option<String>,
    source_raster: bool,
}

struct PdfRenderedText {
    index: usize,
    text: String,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    style: String,
}

struct PdfRenderedImage {
    index: usize,
    href: Option<String>,
    hash: Option<String>,
    xobject_name: String,
    xobject_kind: pdf_handler::content_stream::PdfXObjectKind,
    transform: [f32; 6],
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl<'a, F> PdfPageWriter<'a, F>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    fn set_page_raster(&mut self, href: &str, hash: &str) {
        let loading = if self.page == 1 { "eager" } else { "lazy" };
        self.page_raster = Some(format!(
            "<img class=\"hcd-pdf-page-raster\" src=\"asset://sha256/{hash}\" data-hcd-asset-href=\"{}\" loading=\"{loading}\" decoding=\"async\" alt=\"\"/>",
            escape_attribute(href)
        ));
        self.source_raster = true;
    }

    fn push_image(&mut self, image: PdfRenderedImage) -> Result<(), HcdError> {
        let PdfRenderedImage {
            index,
            href,
            hash,
            xobject_name,
            xobject_kind,
            transform,
            x,
            y,
            width,
            height,
        } = image;
        let part = format!("pdf/pages/{}", self.page);
        let source_path = format!("/page[{}]/image[{index}]", self.page);
        let node_id = stable_node_id(&[
            self.document_id,
            &part,
            "xobject-placement",
            &index.to_string(),
        ]);
        let left = x - self.page_llx;
        let top = self.page_height - (y - self.page_lly) - height;
        let kind = xobject_kind.as_str();
        let common = format!(
            "class=\"hcd-pdf-image hcd-pdf-visual-node\" data-hcd-id=\"{node_id}\" data-hcd-node-kind=\"pdf-{kind}\" data-hcd-source-path=\"{source_path}\" data-hcd-source-order=\"{index}\" data-hcd-xobject=\"{}\" data-hcd-mapping=\"source\" data-hcd-geometry=\"xobject-ctm\" data-hcd-bbox=\"{x},{y},{width},{height}\" data-hcd-transform=\"{},{},{},{},{},{}\" style=\"position:absolute;left:{left:.1}pt;top:{top:.1}pt;width:{width:.1}pt;height:{height:.1}pt\"",
            escape_attribute(&xobject_name),
            transform[0],
            transform[1],
            transform[2],
            transform[3],
            transform[4],
            transform[5],
        );
        let block = match (href, hash) {
            (Some(href), Some(hash)) => format!(
                "<img {common} src=\"asset://sha256/{hash}\" data-hcd-asset-href=\"{}\" alt=\"\"/>",
                escape_attribute(&href)
            ),
            _ => format!("<div {common}></div>"),
        };
        if self.blocks > 0
            && (self.html.len() + block.len() > self.soft_bytes || self.blocks >= self.max_blocks)
        {
            self.flush()?;
        }
        self.html.push_str(&block);
        self.blocks += 1;
        Ok(())
    }

    fn push(&mut self, rendered: PdfRenderedText) -> Result<(), HcdError> {
        let PdfRenderedText {
            index,
            text,
            x,
            y,
            width,
            height,
            style,
        } = rendered;
        if text.len() > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: PDF page {} text block {} exceeds 2 MiB",
                self.page, index
            )));
        }
        let part = format!("pdf/pages/{}", self.page);
        let node_id = stable_node_id(&[self.document_id, &part, "text-block", &index.to_string()]);
        let node_hash = hash_bytes(text.as_bytes());
        let source_path = format!("/page[{}]/text[{index}]", self.page);
        let block = format!(
            "<p class=\"hcd-pdf-text\" data-hcd-text-node=\"{node_id}\" data-hcd-source-path=\"{source_path}\" data-hcd-source-order=\"{index}\" data-hcd-mapping=\"source\" data-hcd-bbox=\"{x},{y},{width},{height}\" data-hcd-x=\"{x}\" data-hcd-y=\"{y}\" data-hcd-width=\"{width}\" data-hcd-height=\"{height}\" style=\"{style}\"><span data-hcd-id=\"{node_id}\" data-hcd-node-hash=\"{node_hash}\">{}</span></p>",
            escape_text(&text)
        );
        if self.blocks > 0
            && (self.html.len() + block.len() > self.soft_bytes || self.blocks >= self.max_blocks)
        {
            self.flush()?;
        }
        self.html.push_str(&block);
        self.entries.push(NodeMapEntry {
            node_id,
            node_hash,
            source: SourceAnchor {
                part,
                text_ordinal: index as u64,
                paragraph_id: Some(source_path),
                text_id: None,
                node_kind: "pdf-text".to_string(),
                editable: true,
            },
        });
        self.blocks += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), HcdError> {
        if self.blocks == 0 && self.ordinal > 0 {
            return Ok(());
        }
        let part = format!("pdf/pages/{}", self.page);
        let chunk_id = stable_node_id(&[
            self.document_id,
            &part,
            "page-chunk",
            &self.ordinal.to_string(),
        ])
        .replacen("n_", "c_", 1);
        let mut content = String::new();
        if self.ordinal == 0 {
            if let Some(page_raster) = &self.page_raster {
                content.push_str(page_raster);
            }
        }
        if self.blocks == 0 && content.is_empty() {
            content.push_str("<div class=\"hcd-empty-page\"></div>");
        } else {
            content.push_str(&std::mem::take(&mut self.html));
        }
        let continuation_margin = if self.ordinal > 0 {
            format!(";margin-top:-{:.1}pt", self.page_height + 18.0)
        } else {
            String::new()
        };
        let html = format!(
            "<section class=\"hcd-pdf-page\" data-hcd-page=\"{}\" data-hcd-continuation=\"{}\" data-hcd-source-raster=\"{}\" style=\"position:relative;width:{:.1}pt;height:{:.1}pt;overflow:hidden{continuation_margin}\">{content}</section>",
            self.page,
            self.ordinal > 0,
            self.source_raster,
            self.page_width,
            self.page_height,
        );
        let map = ChunkSourceMap {
            schema_version: HCD_SCHEMA_VERSION.to_string(),
            chunk_id: chunk_id.clone(),
            entries: std::mem::take(&mut self.entries),
        };
        let descriptor = self.writer.write_chunk(
            chunk_id,
            "page".to_string(),
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

fn render_page_raster(
    pdf: &Pdf,
    page_index: usize,
    page_width: f32,
    page_height: f32,
) -> Result<Vec<u8>, String> {
    let width = (page_width * HCD_PDF_RASTER_SCALE).ceil().max(1.0) as u32;
    let height = (page_height * HCD_PDF_RASTER_SCALE).ceil().max(1.0) as u32;
    if width > MAX_HCD_PDF_RASTER_DIMENSION || height > MAX_HCD_PDF_RASTER_DIMENSION {
        return Err(format!(
            "raster dimensions {width}x{height} exceed {MAX_HCD_PDF_RASTER_DIMENSION}px"
        ));
    }
    if u64::from(width).saturating_mul(u64::from(height)) > MAX_HCD_PDF_RASTER_PIXELS {
        return Err(format!(
            "raster dimensions {width}x{height} exceed the {MAX_HCD_PDF_RASTER_PIXELS}-pixel page limit"
        ));
    }
    let page = pdf
        .pages()
        .get(page_index)
        .ok_or_else(|| format!("Hayro page {} does not exist", page_index + 1))?;
    let settings = RenderSettings {
        x_scale: HCD_PDF_RASTER_SCALE,
        y_scale: HCD_PDF_RASTER_SCALE,
        width: Some(width as u16),
        height: Some(height as u16),
        bg_color: WHITE,
    };
    let pixmap = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        render_pdf_page(page, &InterpreterSettings::default(), &settings)
    }))
    .map_err(|_| format!("Hayro panicked while rendering page {}", page_index + 1))?;
    let png = pixmap
        .into_png()
        .map_err(|error| format!("failed to encode page {} as PNG: {error}", page_index + 1))?;
    if png.len() > MAX_HCD_PDF_RASTER_PNG_BYTES {
        return Err(format!(
            "rendered page {} is {} bytes; maximum is {MAX_HCD_PDF_RASTER_PNG_BYTES}",
            page_index + 1,
            png.len()
        ));
    }
    Ok(png)
}

pub(crate) fn import_pdf<F>(
    source: &Path,
    output: &Path,
    options: &ImportOptions,
    mut emit: F,
) -> Result<HcdManifest, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let (source_hash, source_size) = source_identity(source, "pdf")?;
    if source_size > MAX_HCD_PDF_SOURCE_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "PDF source is {source_size} bytes; bounded HCD PDF import supports at most {MAX_HCD_PDF_SOURCE_BYTES} bytes"
        )));
    }
    emit_started(&mut emit, options, &source_hash)?;
    let result = import_pdf_inner(source, output, options, source_hash, source_size, &mut emit);
    if let Err(error) = &result {
        emit_failed(&mut emit, options, error);
    }
    result
}

fn import_pdf_inner<F>(
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
    pdf_handler::reader::preflight_structural_streams(
        source,
        pdf_handler::reader::PdfStructuralLimits {
            maximum_source_bytes: MAX_HCD_PDF_SOURCE_BYTES as usize,
            maximum_dictionary_bytes: MAX_HCD_PDF_STREAM_DICTIONARY_BYTES,
            maximum_encoded_stream_bytes: MAX_HCD_PDF_STRUCTURAL_STREAM_BYTES,
            maximum_decoded_stream_bytes: MAX_HCD_PDF_STRUCTURAL_STREAM_BYTES,
            maximum_total_decoded_bytes: MAX_HCD_PDF_STRUCTURAL_TOTAL_BYTES,
            maximum_structural_streams: MAX_HCD_PDF_STRUCTURAL_STREAMS,
            maximum_xref_entries: MAX_HCD_PDF_XREF_ENTRIES,
        },
    )
    .map_err(pdf_parse_error)?;
    let source_path = source
        .to_str()
        .ok_or_else(|| HcdError::InvalidBundle("PDF path is not UTF-8".to_string()))?;
    let reader = pdf_handler::reader::PdfReader::open(source_path)
        .map_err(|error| HcdError::InvalidBundle(error.to_string()))?;
    if reader.page_count() > MAX_HCD_PDF_PAGES {
        return Err(HcdError::ResourceLimit(format!(
            "PDF contains {} pages; maximum is {MAX_HCD_PDF_PAGES}",
            reader.page_count()
        )));
    }
    let mut writer = BundleWriter::create(output)?;
    writer.write_styles(
        ".hcd-pdf-page{display:block;padding:0;margin:0 auto 18pt;background:#fff;overflow:hidden;box-shadow:0 2px 12px #0003}.hcd-pdf-page[data-hcd-continuation=\"true\"]{background:transparent;box-shadow:none}.hcd-pdf-page-raster{position:absolute;inset:0;width:100%;height:100%;display:block;z-index:0;pointer-events:none}.hcd-pdf-image{position:absolute;display:block;z-index:1}.hcd-pdf-visual-node{pointer-events:none;background:transparent}.hcd-pdf-text{position:absolute;min-width:max-content;white-space:nowrap;margin:0;line-height:1;z-index:2;cursor:text}.hcd-pdf-page[data-hcd-source-raster=\"true\"] .hcd-pdf-text{color:transparent!important;text-shadow:none!important}body:not([data-hcd-image-hitboxes=\"off\"]) .hcd-pdf-visual-node{pointer-events:auto;cursor:crosshair}body:not([data-hcd-image-hitboxes=\"off\"]) .hcd-pdf-visual-node:hover{background:rgba(255,59,48,.10);outline:2px solid rgba(255,59,48,.95);outline-offset:-1px}body:not([data-hcd-text-hitboxes=\"off\"]) .hcd-pdf-text:hover{background:rgba(10,132,255,.12);outline:1px solid rgba(10,132,255,.8);outline-offset:0}.hcd-empty-page{position:absolute;inset:0}",
    )?;
    let raster_source = std::fs::read(source)?;
    let (raster_pdf, raster_initialization_error) = match Pdf::new(Arc::new(raster_source)) {
        Ok(pdf) => (Some(pdf), None),
        Err(error) => (
            None,
            Some(format!("Hayro could not open the PDF: {error:?}")),
        ),
    };
    let mut replaced_control_characters = 0usize;
    let mut rasterized_pages = 0usize;
    let mut raster_fallbacks = Vec::new();
    let mut assets = BTreeMap::<String, AssetDescriptor>::new();
    for page in 1..=reader.page_count() {
        let (page_width, page_height, llx, lly) =
            pdf_handler::html_preview::page_dimensions(&reader, page);
        let mut parsed = reader
            .parse_page_text_blocks_bounded(
                page,
                MAX_HCD_PDF_PAGE_CONTENT_BYTES,
                MAX_HCD_PDF_AUX_STREAM_BYTES,
            )
            .map_err(pdf_parse_error)?;
        let page_raster = raster_pdf.as_ref().and_then(|pdf| {
            match render_page_raster(pdf, page - 1, page_width, page_height) {
                Ok(png) => Some(png),
                Err(error) => {
                    raster_fallbacks.push(format!("page {page}: {error}"));
                    None
                }
            }
        });
        if page_raster.is_none() {
            if let Some(error) = &raster_initialization_error {
                if raster_fallbacks.is_empty() {
                    raster_fallbacks.push(error.clone());
                }
            }
            parsed = reader
                .parse_page_text_blocks_with_images_bounded(
                    page,
                    MAX_HCD_PDF_PAGE_CONTENT_BYTES,
                    MAX_HCD_PDF_AUX_STREAM_BYTES,
                    MAX_HCD_PDF_PAGE_IMAGE_PAYLOAD_BYTES,
                )
                .map_err(pdf_parse_error)?;
        }
        let mut rendered_images = Vec::with_capacity(parsed.image_blocks.len());
        for image in &parsed.image_blocks {
            let mut href = None;
            let mut hash = None;
            if page_raster.is_none() {
                if let Some(data_uri) = parsed.image_map.get(&image.xobject_name) {
                    let (extension, bytes) = decode_image_data_uri(data_uri)?;
                    let (asset_href, asset_hash, byte_length) =
                        writer.write_asset_from_reader(extension, &mut Cursor::new(bytes))?;
                    if !assets.contains_key(&asset_hash) {
                        emit(&ImportEvent::AssetReady {
                            hash: asset_hash.clone(),
                            href: asset_href.clone(),
                            byte_length,
                        })?;
                        assets.insert(
                            asset_hash.clone(),
                            AssetDescriptor {
                                source_part: format!(
                                    "pdf/pages/{page}/xobjects/{}",
                                    image.xobject_name
                                ),
                                hash: asset_hash.clone(),
                                href: asset_href.clone(),
                                byte_length,
                            },
                        );
                    }
                    href = Some(asset_href);
                    hash = Some(asset_hash);
                }
            }
            rendered_images.push(PdfRenderedImage {
                index: image.index,
                href,
                hash,
                xobject_name: image.xobject_name.clone(),
                xobject_kind: image.xobject_kind,
                transform: image.transform,
                x: image.bbox.x,
                y: image.bbox.y,
                width: image.bbox.width,
                height: image.bbox.height,
            });
        }
        let mut chunks = PdfPageWriter {
            document_id: &options.document_id,
            page,
            writer: &mut writer,
            emit,
            soft_bytes: options.chunk_soft_bytes.min(MAX_CHUNK_BYTES),
            max_blocks: options.chunk_blocks.clamp(1, DEFAULT_CHUNK_BLOCKS),
            page_width,
            page_height,
            page_llx: llx,
            page_lly: lly,
            ordinal: 0,
            blocks: 0,
            html: String::new(),
            entries: Vec::new(),
            page_raster: None,
            source_raster: false,
        };
        if let Some(png) = page_raster {
            let (href, hash, byte_length) = chunks
                .writer
                .write_asset_from_reader("png", &mut Cursor::new(png))?;
            if !assets.contains_key(&hash) {
                (chunks.emit)(&ImportEvent::AssetReady {
                    hash: hash.clone(),
                    href: href.clone(),
                    byte_length,
                })?;
                assets.insert(
                    hash.clone(),
                    AssetDescriptor {
                        source_part: format!("pdf/pages/{page}/composited-raster"),
                        hash: hash.clone(),
                        href: href.clone(),
                        byte_length,
                    },
                );
            }
            chunks.set_page_raster(&href, &hash);
            rasterized_pages += 1;
        }
        for image in rendered_images {
            chunks.push_image(image)?;
        }
        for block in &parsed.text_blocks {
            let style = pdf_handler::html_preview::text_block_inline_style(
                &parsed,
                block,
                page_height,
                llx,
                lly,
            );
            let (text, replaced) = sanitize_pdf_text(block.text.clone());
            replaced_control_characters = replaced_control_characters.saturating_add(replaced);
            chunks.push(PdfRenderedText {
                index: block.index,
                text,
                x: block.bbox.x,
                y: block.bbox.y,
                width: block.bbox.width,
                height: block.bbox.height,
                style,
            })?;
        }
        chunks.flush()?;
    }
    std::fs::write(
        writer.root().join("assets/index.json"),
        serde_json::to_vec(&assets.into_values().collect::<Vec<_>>())?,
    )?;
    let mut manifest = base_manifest(options, "pdf", "fixed-layout", source_hash, source_size);
    manifest.warnings.push(FidelityWarning {
        code: "PDF_OBJECT_GRAPH_IN_MEMORY".to_string(),
        message: "PDF import retains lopdf's compressed object graph in memory. Before lopdf is called, /ObjStm and /XRef streams receive bounded structural preflight; page content and auxiliary font streams are then decoded with independent limits".to_string(),
        node_id: None,
        source_part: None,
    });
    if replaced_control_characters > 0 {
        manifest.warnings.push(FidelityWarning {
            code: "PDF_FORBIDDEN_CONTROL_REPLACED".to_string(),
            message: format!(
                "Replaced {replaced_control_characters} PDF text-layer control characters that XML document formats cannot represent with spaces"
            ),
            node_id: None,
            source_part: None,
        });
    }
    manifest.warnings.push(FidelityWarning {
        code: "PDF_SOURCE_VISUAL_LAYER_READ_ONLY".to_string(),
        message: format!("{rasterized_pages} of {} PDF pages were composited by the pure-Rust renderer as a read-only 96-DPI visual authority layer; extractable nodeId text remains selectable/editable but is transparent in source-view mode, and scanned text requires OCR before text patching", reader.page_count()),
        node_id: None,
        source_part: None,
    });
    if !raster_fallbacks.is_empty() {
        manifest.warnings.push(FidelityWarning {
            code: "PDF_PAGE_RASTER_FALLBACK".to_string(),
            message: format!(
                "{} page-rendering fallback(s) used the older XObject/text composition path: {}",
                raster_fallbacks.len(),
                raster_fallbacks.join("; ")
            ),
            node_id: None,
            source_part: None,
        });
    }
    manifest.fidelity = Some(FidelityReport {
        schema_version: HCD_SCHEMA_VERSION.to_string(),
        level: FidelityLevel::Visual,
        preserved: vec![
            format!("page order and {rasterized_pages} fully composited source-page presentation raster(s) at CSS 96-DPI scale"),
            "extractable nodeId text and text block coordinates in a separate interaction layer"
                .to_string(),
            "the immutable source PDF as the export boundary".to_string(),
        ],
        flattened: vec![
            "PDF drawing operations, fonts, vector graphics, masks and shaping are flattened into read-only page PNGs; the pure-Rust renderer remains best-effort for unsupported PDF features".to_string(),
        ],
        dropped: vec!["scanned text without an OCR layer".to_string()],
        warnings: manifest.warnings.clone(),
    });
    finish_import(writer, manifest, emit)
}

fn decode_image_data_uri(value: &str) -> Result<(&'static str, Vec<u8>), HcdError> {
    let (metadata, payload) = value.split_once(',').ok_or_else(|| {
        HcdError::InvalidBundle("PDF image data URI is missing a payload separator".to_string())
    })?;
    if !metadata.ends_with(";base64") {
        return Err(HcdError::Unsupported(
            "PDF image data URI is not Base64 encoded".to_string(),
        ));
    }
    let extension = match metadata
        .trim_start_matches("data:")
        .trim_end_matches(";base64")
    {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        other => {
            return Err(HcdError::Unsupported(format!(
                "PDF image MIME type {other} is not supported in HCD presentation"
            )))
        }
    };
    let bytes = decode_base64(payload).map_err(|()| {
        HcdError::InvalidBundle("PDF image contains invalid Base64 data".to_string())
    })?;
    if bytes.len() > MAX_HCD_PDF_PAGE_IMAGE_PAYLOAD_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "PDF image is {} bytes; per-page presentation asset limit is {}",
            bytes.len(),
            MAX_HCD_PDF_PAGE_IMAGE_PAYLOAD_BYTES
        )));
    }
    Ok((extension, bytes))
}

fn decode_base64(value: &str) -> Result<Vec<u8>, ()> {
    let mut bits = 0u32;
    let mut bit_count = 0u32;
    let mut output = Vec::with_capacity(value.len().saturating_mul(3) / 4);
    for character in value.chars().filter(|character| !character.is_whitespace()) {
        let decoded = match character {
            'A'..='Z' => character as u32 - 'A' as u32,
            'a'..='z' => character as u32 - 'a' as u32 + 26,
            '0'..='9' => character as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '=' => break,
            _ => return Err(()),
        };
        bits = (bits << 6) | decoded;
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            output.push((bits >> bit_count) as u8);
        }
    }
    Ok(output)
}

fn pdf_parse_error(error: handler_common::HandlerError) -> HcdError {
    let message = error.to_string();
    if message.contains("resource limit exceeded") {
        HcdError::ResourceLimit(message)
    } else if message.contains("not supported by the bounded HCD PDF decoder")
        || message.contains("unsupported bounded-decoder filter")
    {
        HcdError::Unsupported(message)
    } else {
        HcdError::InvalidBundle(message)
    }
}

pub(crate) fn export_pdf(
    bundle: &Bundle,
    source: &Path,
    target: &Path,
    options: &ExportOptions,
) -> Result<FidelityReport, HcdError> {
    if target.exists() {
        return Err(HcdError::InvalidBundle(format!(
            "target already exists: {}",
            target.display()
        )));
    }
    let (manifest, _, dirty_parts, dirty_node_ids) = checked_export_state(bundle, source, options)?;
    let mut nodes = collect_dirty_nodes(bundle, &manifest, &dirty_parts, &dirty_node_ids)?;
    nodes.sort_by(|left, right| {
        let left_page = page_number(&left.source.part);
        let right_page = page_number(&right.source.part);
        (right_page, right.source.text_ordinal).cmp(&(left_page, left.source.text_ordinal))
    });

    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temp = tempfile::Builder::new()
        .prefix(".officecli-hcd-pdf-")
        .suffix(".pdf")
        .tempfile_in(parent)?;
    std::fs::copy(source, temp.path())?;
    if !nodes.is_empty() {
        let temp_path = temp.path().to_str().ok_or_else(|| {
            HcdError::InvalidBundle("temporary PDF path is not UTF-8".to_string())
        })?;
        let handler = pdf_handler::PdfHandler::open(temp_path, true)
            .map_err(|error| HcdError::InvalidBundle(error.to_string()))?;
        for node in nodes {
            let path = node.source.paragraph_id.ok_or_else(|| {
                HcdError::InvalidBundle(format!("PDF node {} has no path", node.node_id))
            })?;
            let properties = HashMap::from([("text".to_string(), node.text)]);
            let unsupported = handler
                .set(&path, &properties)
                .map_err(|error| HcdError::Unsupported(error.to_string()))?;
            if !unsupported.is_empty() {
                return Err(HcdError::Unsupported(format!(
                    "PDF text replacement returned unsupported properties {unsupported:?}"
                )));
            }
        }
        handler
            .save()
            .map_err(|error| HcdError::InvalidBundle(error.to_string()))?;
    }
    pdf_handler::reader::PdfReader::open(&temp.path().to_string_lossy())
        .map_err(|error| HcdError::InvalidBundle(format!("exported PDF is invalid: {error}")))?;
    temp.persist(target)
        .map_err(|error| HcdError::Io(error.error))?;

    let report = FidelityReport {
        schema_version: HCD_SCHEMA_VERSION.to_string(),
        level: if dirty_parts.is_empty() {
            FidelityLevel::Exact
        } else {
            FidelityLevel::Semantic
        },
        preserved: vec![
            "page tree, graphics and unedited PDF objects".to_string(),
            "text block position where supported by the source font/content stream".to_string(),
        ],
        flattened: vec![
            "edited PDF text may use a fallback embedded font when the original subset lacks glyphs"
                .to_string(),
        ],
        dropped: vec!["HCD recognition annotations are not exported".to_string()],
        warnings: manifest.warnings,
    };
    write_fidelity_report(options, &report)?;
    Ok(report)
}

fn page_number(part: &str) -> u64 {
    part.rsplit('/')
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use lopdf::{dictionary, Document, Object, Stream};
    use std::io::Write as IoWrite;

    #[test]
    fn pdf_text_controls_are_sanitized_before_hcd_publication() {
        let (text, replaced) = sanitize_pdf_text("a\0b\u{b}c\td".to_string());
        assert_eq!(text, "a b c\td");
        assert_eq!(replaced, 2);
    }

    fn write_page_stream_bomb(path: &Path) {
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let mut content = Stream::new(
            dictionary! {},
            vec![b'x'; MAX_HCD_PDF_PAGE_CONTENT_BYTES + 1],
        );
        content.compress().unwrap();
        let content_id = document.add_object(content);
        let resources_id = document.add_object(dictionary! {});
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);
        document.save(path).unwrap();
    }

    fn write_simple_visual_pdf(path: &Path) {
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let font_id = document.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let image_id = document.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 1,
                "Height" => 1,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
            },
            vec![128],
        ));
        let content_id = document.add_object(Stream::new(
            dictionary! {},
            b"0.1 0.6 0.9 rg 0 0 200 300 re f q 40 0 0 30 10 20 cm /Im1 Do Q BT /F1 12 Tf 20 250 Td (Hello HCD) Tj ET".to_vec(),
        ));
        let resources_id = document.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
            "XObject" => dictionary! { "Im1" => image_id },
        });
        let page_id = document.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 300.into()],
            "CropBox" => vec![0.into(), 0.into(), 200.into(), 300.into()],
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);
        document.save(path).unwrap();
    }

    #[test]
    fn hcd_pdf_import_publishes_a_valid_lazy_page_raster() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("visual.pdf");
        let bundle_path = temp.path().join("bundle");
        write_simple_visual_pdf(&source);
        let manifest = import_pdf(
            &source,
            &bundle_path,
            &ImportOptions::new("pdf-page-raster"),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(manifest.chunk_count, 1);
        let bundle = Bundle::open(&bundle_path).unwrap();
        let validation = hcd_core::validate_bundle(&bundle).unwrap();
        assert!(validation.valid, "{:?}", validation.issues);
        let styles = std::fs::read_to_string(bundle_path.join("styles.css")).unwrap();
        assert!(styles.contains("body:not([data-hcd-text-hitboxes=\"off\"]) .hcd-pdf-text:hover"));
        assert!(styles
            .contains("body:not([data-hcd-image-hitboxes=\"off\"]) .hcd-pdf-visual-node:hover"));
        assert!(styles.contains(".hcd-pdf-visual-node{pointer-events:none"));
        assert!(styles.contains(".hcd-pdf-visual-node{pointer-events:auto"));
        assert!(styles.contains(".hcd-pdf-text{position:absolute;min-width:max-content"));
        let assets: Vec<AssetDescriptor> =
            serde_json::from_slice(&std::fs::read(bundle_path.join("assets/index.json")).unwrap())
                .unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].source_part, "pdf/pages/1/composited-raster");
        let index = bundle.read_index_page(&manifest, 0).unwrap();
        let html = bundle.read_chunk(&index.chunks[0]).unwrap();
        assert!(html.contains("class=\"hcd-pdf-page-raster\""));
        assert!(html.contains("loading=\"eager\""));
        assert!(html.contains("data-hcd-source-raster=\"true\""));
        assert!(html.contains("data-hcd-text-node=\"n_"));
        assert!(html.contains("data-hcd-source-path=\"/page[1]/text[1]\""));
        assert!(html.contains("data-hcd-source-order=\"1\""));
        assert!(html.contains("data-hcd-mapping=\"source\""));
        assert!(html.contains("data-hcd-bbox=\""));
        assert!(html.contains("data-hcd-node-kind=\"pdf-image\""));
        assert!(html.contains("data-hcd-source-path=\"/page[1]/image[1]\""));
        assert!(html.contains("data-hcd-xobject=\"Im1\""));
        assert!(html.contains("data-hcd-transform=\"40,0,0,30,10,20\""));
        assert!(html.contains("data-hcd-geometry=\"xobject-ctm\""));

        let source_map = bundle.read_map(&index.chunks[0]).unwrap();
        assert_eq!(source_map.entries.len(), 1);
        assert_eq!(
            source_map.entries[0].source.paragraph_id.as_deref(),
            Some("/page[1]/text[1]")
        );
        assert!(source_map.entries[0].source.editable);
    }

    fn compressed_repeated(byte: u8, decoded_size: usize) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        let block = vec![byte; 64 * 1024];
        let mut remaining = decoded_size;
        while remaining > 0 {
            let amount = remaining.min(block.len());
            encoder.write_all(&block[..amount]).unwrap();
            remaining -= amount;
        }
        encoder.finish().unwrap()
    }

    fn push_pdf_object(pdf: &mut Vec<u8>, offsets: &mut Vec<usize>, id: usize, body: &[u8]) {
        offsets.push(pdf.len());
        writeln!(pdf, "{id} 0 obj").unwrap();
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }

    fn write_object_stream_bomb(path: &Path, decoded_size: usize) {
        let mut pdf = b"%PDF-1.7\n%\xFF\xFF\xFF\xFF\n".to_vec();
        let mut offsets = Vec::new();
        push_pdf_object(
            &mut pdf,
            &mut offsets,
            1,
            b"<< /Type /Catalog /Pages 2 0 R >>",
        );
        push_pdf_object(
            &mut pdf,
            &mut offsets,
            2,
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        );
        push_pdf_object(
            &mut pdf,
            &mut offsets,
            3,
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources <<>> /MediaBox [0 0 612 792] >>",
        );
        push_pdf_object(
            &mut pdf,
            &mut offsets,
            4,
            b"<< /Length 0 >>\nstream\n\nendstream",
        );
        let compressed = compressed_repeated(b'x', decoded_size);
        let mut stream = format!(
            "<< /Type /ObjStm /N 0 /First 0 /Filter /FlateDecode /Length {} >>\nstream\n",
            compressed.len()
        )
        .into_bytes();
        stream.extend_from_slice(&compressed);
        stream.extend_from_slice(b"\nendstream");
        push_pdf_object(&mut pdf, &mut offsets, 5, &stream);

        let xref = pdf.len();
        pdf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            writeln!(pdf, "{offset:010} 00000 n ").unwrap();
        }
        write!(
            pdf,
            "trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF"
        )
        .unwrap();
        std::fs::write(path, pdf).unwrap();
    }

    fn write_xref_stream_bomb(path: &Path, decoded_size: usize) {
        let mut pdf = b"%PDF-1.7\n%\xFF\xFF\xFF\xFF\n".to_vec();
        let mut offsets = Vec::new();
        push_pdf_object(
            &mut pdf,
            &mut offsets,
            1,
            b"<< /Type /Catalog /Pages 2 0 R >>",
        );
        push_pdf_object(
            &mut pdf,
            &mut offsets,
            2,
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        );
        push_pdf_object(
            &mut pdf,
            &mut offsets,
            3,
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources <<>> /MediaBox [0 0 612 792] >>",
        );
        push_pdf_object(
            &mut pdf,
            &mut offsets,
            4,
            b"<< /Length 0 >>\nstream\n\nendstream",
        );
        let xref_offset = pdf.len();
        let mut decoded = Vec::with_capacity(decoded_size);
        decoded.extend_from_slice(&[0, 0, 0, 0, 0, 0xff, 0xff]);
        for offset in offsets.iter().copied().chain(std::iter::once(xref_offset)) {
            decoded.push(1);
            decoded.extend_from_slice(&(offset as u32).to_be_bytes());
            decoded.extend_from_slice(&0u16.to_be_bytes());
        }
        decoded.resize(decoded_size, 0);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&decoded).unwrap();
        let compressed = encoder.finish().unwrap();
        write!(pdf, "5 0 obj\n<< /Type /XRef /Size 6 /Root 1 0 R /W [1 4 2] /Index [0 6] /Filter /FlateDecode /Length {} >>\nstream\n", compressed.len()).unwrap();
        pdf.extend_from_slice(&compressed);
        write!(pdf, "\nendstream\nendobj\nstartxref\n{xref_offset}\n%%EOF").unwrap();
        std::fs::write(path, pdf).unwrap();
    }

    fn assert_structural_bomb_is_rejected(source: &Path, bundle: &Path, document_id: &str) {
        let mut events = Vec::new();
        let error = import_pdf(source, bundle, &ImportOptions::new(document_id), |event| {
            events.push(event.clone());
            Ok(())
        })
        .unwrap_err();
        assert!(matches!(error, HcdError::ResourceLimit(_)), "{error}");
        assert!(events
            .iter()
            .any(|event| matches!(event, ImportEvent::ImportStarted { .. })));
        assert!(events
            .iter()
            .any(|event| matches!(event, ImportEvent::Failed { .. })));
        assert!(!bundle.exists());
    }

    #[test]
    fn hcd_import_rejects_a_compressed_page_stream_bomb_without_publishing() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("bomb.pdf");
        let bundle = temp.path().join("bundle");
        write_page_stream_bomb(&source);
        let mut events = Vec::new();
        let error = import_pdf(&source, &bundle, &ImportOptions::new("pdf-bomb"), |event| {
            events.push(event.clone());
            Ok(())
        })
        .unwrap_err();
        assert!(matches!(error, HcdError::ResourceLimit(_)));
        assert!(events
            .iter()
            .any(|event| matches!(event, ImportEvent::ImportStarted { .. })));
        assert!(events
            .iter()
            .any(|event| matches!(event, ImportEvent::Failed { .. })));
        assert!(!bundle.exists());
    }

    #[test]
    fn hcd_import_rejects_an_object_stream_bomb_before_lopdf_without_publishing() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("object-stream-bomb.pdf");
        let bundle = temp.path().join("bundle");
        write_object_stream_bomb(&source, MAX_HCD_PDF_STRUCTURAL_STREAM_BYTES + 1);
        assert_structural_bomb_is_rejected(&source, &bundle, "pdf-object-stream-bomb");
    }

    #[test]
    fn hcd_import_rejects_an_xref_stream_bomb_before_lopdf_without_publishing() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("xref-stream-bomb.pdf");
        let bundle = temp.path().join("bundle");
        write_xref_stream_bomb(&source, MAX_HCD_PDF_STRUCTURAL_STREAM_BYTES + 1);
        assert_structural_bomb_is_rejected(&source, &bundle, "pdf-xref-stream-bomb");
    }
}
