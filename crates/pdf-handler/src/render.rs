use crate::reader::PdfReader;
use handler_common::HandlerError;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{render as render_pdf_page, RenderSettings};
use std::sync::Arc;

const PREVIEW_RASTER_SCALE: f32 = 96.0 / 72.0;
const MAX_RASTER_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_RASTER_DIMENSION: u32 = 8_192;
const MAX_RASTER_PIXELS: u64 = 32 * 1024 * 1024;
const MAX_RASTER_PNG_BYTES: usize = 64 * 1024 * 1024;

/// Reusable in-process PDF rasterizer for high-fidelity browser previews.
/// The source is parsed once, then pages are rendered independently so watch
/// mode can remain page-lazy.
pub(crate) struct PdfRasterizer {
    pdf: Pdf,
}

impl PdfRasterizer {
    pub(crate) fn open(path: &str) -> Result<Self, HandlerError> {
        let metadata = std::fs::metadata(path).map_err(HandlerError::IoError)?;
        if metadata.len() > MAX_RASTER_SOURCE_BYTES {
            return Err(HandlerError::OperationFailed(format!(
                "PDF raster preview source is {} bytes; maximum is {MAX_RASTER_SOURCE_BYTES}",
                metadata.len()
            )));
        }
        let bytes = std::fs::read(path).map_err(HandlerError::IoError)?;
        let pdf = Pdf::new(Arc::new(bytes)).map_err(|error| {
            HandlerError::OperationFailed(format!(
                "pure-Rust PDF raster preview could not open the source: {error:?}"
            ))
        })?;
        Ok(Self { pdf })
    }

    pub(crate) fn render_page(&self, page: usize) -> Result<Vec<u8>, HandlerError> {
        let page = self
            .pdf
            .pages()
            .get(page.checked_sub(1).ok_or_else(|| {
                HandlerError::InvalidArgument("PDF page numbers are 1-based".to_string())
            })?)
            .ok_or_else(|| {
                HandlerError::InvalidArgument(format!("PDF page {page} does not exist"))
            })?;
        let (page_width, page_height) = page.render_dimensions();
        let width = (page_width * PREVIEW_RASTER_SCALE).ceil().max(1.0) as u32;
        let height = (page_height * PREVIEW_RASTER_SCALE).ceil().max(1.0) as u32;
        if width > MAX_RASTER_DIMENSION || height > MAX_RASTER_DIMENSION {
            return Err(HandlerError::OperationFailed(format!(
                "PDF raster preview dimensions {width}x{height} exceed {MAX_RASTER_DIMENSION}px"
            )));
        }
        if u64::from(width).saturating_mul(u64::from(height)) > MAX_RASTER_PIXELS {
            return Err(HandlerError::OperationFailed(format!(
                "PDF raster preview dimensions {width}x{height} exceed {MAX_RASTER_PIXELS} pixels"
            )));
        }
        let settings = RenderSettings {
            x_scale: PREVIEW_RASTER_SCALE,
            y_scale: PREVIEW_RASTER_SCALE,
            width: Some(width as u16),
            height: Some(height as u16),
            bg_color: WHITE,
        };
        let pixmap = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            render_pdf_page(page, &InterpreterSettings::default(), &settings)
        }))
        .map_err(|_| {
            HandlerError::OperationFailed("pure-Rust PDF raster preview panicked".to_string())
        })?;
        let png = pixmap.into_png().map_err(|error| {
            HandlerError::OperationFailed(format!(
                "pure-Rust PDF raster preview PNG encoding failed: {error}"
            ))
        })?;
        if png.len() > MAX_RASTER_PNG_BYTES {
            return Err(HandlerError::OperationFailed(format!(
                "PDF raster preview is {} bytes; maximum is {MAX_RASTER_PNG_BYTES}",
                png.len()
            )));
        }
        Ok(png)
    }
}

/// PDF rendering with an in-process pure-Rust raster path and a semantic SVG
/// fallback/inspection path. No poppler, mutool, browser, or LibreOffice is
/// invoked.
pub struct PdfRenderer;

impl PdfRenderer {
    /// Render a PDF page to PNG bytes.
    pub fn render_page_to_png(path: &str, page: usize) -> Result<Vec<u8>, HandlerError> {
        PdfRasterizer::open(path)?.render_page(page)
    }

    /// Render a PDF page to a basic SVG preview using extracted text.
    /// Uses real bbox coordinates from text blocks for positioning.
    pub fn render_page_to_svg(path: &str, page: usize) -> Result<String, HandlerError> {
        let reader = PdfReader::open(path)?;
        let page_height = 792.0; // Default US Letter height in PDF points

        // Try to get actual page height from MediaBox
        let actual_height = get_page_height(&reader, page).unwrap_or(page_height);

        let mut svg = String::new();
        svg.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        svg.push_str(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 612 {:.0}\" width=\"612\" height=\"{:.0}\">\n",
            actual_height, actual_height
        ));

        // Background
        svg.push_str(&format!(
            "  <rect width=\"612\" height=\"{:.0}\" fill=\"white\"/>\n",
            actual_height
        ));

        // Render text blocks at their real bbox coordinates (PDF y is bottom-up, SVG y is top-down)
        if let Some(parsed) = reader.parse_page_text_blocks(page) {
            for block in &parsed.text_blocks {
                let bbox = &block.bbox;
                // Convert PDF coordinates to SVG: svg_y = page_height - pdf_y
                let svg_x = bbox.x;
                let svg_y = actual_height - bbox.y;

                let escaped = block
                    .text
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
                    .replace('"', "&quot;");

                let font_family = block.style.font_name.as_deref().unwrap_or("Helvetica");
                let font_size = block.style.font_size.unwrap_or(12.0);

                // Build fill color from style
                let fill_color = block
                    .style
                    .fill_color
                    .as_ref()
                    .map(|c| match c {
                        crate::content_stream::PdfColor::Gray(g) => {
                            let v = (g * 255.0) as u8;
                            format!("rgb({},{},{})", v, v, v)
                        }
                        crate::content_stream::PdfColor::Rgb(r, g, b) => {
                            format!(
                                "rgb({},{},{})",
                                (r * 255.0) as u8,
                                (g * 255.0) as u8,
                                (b * 255.0) as u8
                            )
                        }
                        crate::content_stream::PdfColor::Cmyk(c, m, y, k) => {
                            let r = ((1.0 - c) * (1.0 - k) * 255.0) as u8;
                            let g = ((1.0 - m) * (1.0 - k) * 255.0) as u8;
                            let b = ((1.0 - y) * (1.0 - k) * 255.0) as u8;
                            format!("rgb({},{},{})", r, g, b)
                        }
                    })
                    .unwrap_or("black".to_string());

                svg.push_str(&format!(
                    "  <text x=\"{:.1}\" y=\"{:.1}\" font-family=\"{}\" font-size=\"{:.0}\" fill=\"{}\" data-path=\"/page[{}]/text[{}]\">{}</text>\n",
                    svg_x, svg_y, font_family, font_size, fill_color, page, block.index, escaped
                ));
            }

            if parsed.text_blocks.is_empty() {
                svg.push_str(&format!(
                    "  <text x=\"306\" y=\"{:.0}\" font-family=\"Helvetica\" font-size=\"14\" fill=\"#999\" text-anchor=\"middle\">(No extractable text)</text>\n",
                    actual_height / 2.0
                ));
            }
        } else {
            svg.push_str(&format!(
                "  <text x=\"306\" y=\"{:.0}\" font-family=\"Helvetica\" font-size=\"14\" fill=\"#999\" text-anchor=\"middle\">(No extractable text)</text>\n",
                actual_height / 2.0
            ));
        }

        // Page number footer
        svg.push_str(&format!(
            "  <text x=\"306\" y=\"{:.0}\" font-family=\"Helvetica\" font-size=\"10\" fill=\"#999\" text-anchor=\"middle\">Page {}</text>\n",
            actual_height - 22.0, page
        ));

        svg.push_str("</svg>");
        Ok(svg)
    }
}

/// Extract page height from the /MediaBox entry.
fn get_page_height(reader: &PdfReader, page_num: usize) -> Option<f32> {
    let pages = reader.document().get_pages();
    let page_id = pages.get(&(page_num as u32))?;
    let page_obj = reader.document().get_object(*page_id).ok()?;
    let dict = page_obj.as_dict().ok()?;
    let media_box = dict.get(b"MediaBox").ok()?;
    // MediaBox is [0 0 width height] or similar
    if let lopdf::Object::Array(arr) = media_box {
        if arr.len() >= 4 {
            // height is the 4th element (index 3)
            arr.get(3).and_then(|h| h.as_float().ok())
        } else {
            None
        }
    } else {
        None
    }
}
