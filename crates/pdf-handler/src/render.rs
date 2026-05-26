use handler_common::HandlerError;
use crate::reader::PdfReader;

/// PDF rendering — converts page text content to SVG for basic preview.
/// Full rasterization (PNG) requires external tools like poppler/mutool.
pub struct PdfRenderer;

impl PdfRenderer {
    /// Render a PDF page to PNG bytes.
    /// This requires an external tool (poppler/mutool) — returns an error if not available.
    pub fn render_page_to_png(path: &str, page: usize) -> Result<Vec<u8>, HandlerError> {
        // Try using mutool (muPDF command-line tool) if available
        let output = std::process::Command::new("mutool")
            .args(["draw", "-F", "png", "-o", "-", "-r", "150", path, &page.to_string()])
            .output();

        match output {
            Ok(result) if result.status.success() => Ok(result.stdout),
            Ok(result) => Err(HandlerError::OperationFailed(
                format!("mutool failed: {}", String::from_utf8_lossy(&result.stderr))
            )),
            Err(_) => Err(HandlerError::UnsupportedMode(
                "PNG rendering requires 'mutool' (muPDF tools) — install with: brew install mupdf-tools".to_string()
            )),
        }
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
        svg.push_str(&format!("  <rect width=\"612\" height=\"{:.0}\" fill=\"white\"/>\n", actual_height));

        // Render text blocks at their real bbox coordinates (PDF y is bottom-up, SVG y is top-down)
        if let Some(parsed) = reader.parse_page_text_blocks(page) {
            for block in &parsed.text_blocks {
                let bbox = &block.bbox;
                // Convert PDF coordinates to SVG: svg_y = page_height - pdf_y
                let svg_x = bbox.x;
                let svg_y = actual_height - bbox.y;

                let escaped = block.text.replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
                    .replace('"', "&quot;");

                let font_family = block.style.font_name.as_deref().unwrap_or("Helvetica");
                let font_size = block.style.font_size.unwrap_or(12.0);

                // Build fill color from style
                let fill_color = block.style.fill_color.as_ref().map(|c| {
                    match c {
                        crate::content_stream::PdfColor::Gray(g) => {
                            let v = (g * 255.0) as u8;
                            format!("rgb({},{},{})", v, v, v)
                        }
                        crate::content_stream::PdfColor::Rgb(r, g, b) => {
                            format!("rgb({},{},{})", (r*255.0) as u8, (g*255.0) as u8, (b*255.0) as u8)
                        }
                        crate::content_stream::PdfColor::Cmyk(c, m, y, k) => {
                            let r = ((1.0-c)*(1.0-k)*255.0) as u8;
                            let g = ((1.0-m)*(1.0-k)*255.0) as u8;
                            let b = ((1.0-y)*(1.0-k)*255.0) as u8;
                            format!("rgb({},{},{})", r, g, b)
                        }
                    }
                }).unwrap_or("black".to_string());

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