use crate::content_stream::PdfColor;
use crate::reader::PdfReader;
use handler_common::HandlerError;

/// Map a PDF BaseFont name (like "TimesNewRomanPS-BoldItalicMT" or "Helvetica-Bold")
/// to standard web CSS font family, weight, and style properties.
fn map_pdf_font_to_css(base_font_name: &str) -> (String, String, String) {
    // Strip subset prefix (e.g. "AAAAAA+Arial" -> "Arial")
    let clean_name = if let Some(pos) = base_font_name.find('+') {
        &base_font_name[pos + 1..]
    } else {
        base_font_name
    };

    let name_lower = clean_name.to_lowercase();

    // Determine Font Weight
    let weight = if name_lower.contains("bold")
        || name_lower.contains("heavy")
        || name_lower.contains("black")
        || name_lower.contains("bd")
    {
        "bold"
    } else {
        "normal"
    };

    // Determine Font Style
    let style = if name_lower.contains("italic")
        || name_lower.contains("oblique")
        || name_lower.contains("it")
    {
        "italic"
    } else {
        "normal"
    };

    // Determine Font Family
    let family = if name_lower.contains("song") || name_lower.contains("simsun") {
        "SimSun, 'Songti SC', Georgia, 'Times New Roman', Times, serif"
    } else if name_lower.contains("hei")
        || name_lower.contains("simhei")
        || name_lower.contains("gothic")
    {
        "'Microsoft YaHei', SimHei, 'Heiti SC', sans-serif"
    } else if name_lower.contains("kai") || name_lower.contains("simkai") {
        "KaiTi, 'Kaiti SC', Georgia, serif"
    } else if name_lower.contains("fangsong") {
        "FangSong, 'FangSong SC', Georgia, serif"
    } else if name_lower.contains("times")
        || name_lower.contains("roman")
        || name_lower.contains("serif")
        || name_lower.contains("minion")
        || name_lower.contains("georgia")
    {
        "Georgia, 'Times New Roman', Times, serif"
    } else if name_lower.contains("courier")
        || name_lower.contains("mono")
        || name_lower.contains("consolas")
        || name_lower.contains("code")
    {
        "Consolas, Monaco, 'Courier New', Courier, monospace"
    } else if name_lower.contains("arial")
        || name_lower.contains("helvetica")
        || name_lower.contains("sans")
    {
        "Arial, Helvetica, sans-serif"
    } else {
        "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif"
    };

    (family.to_string(), weight.to_string(), style.to_string())
}

/// Render the PDF document as HTML for browser preview.
/// Each page is rendered as a relative container with a physical size in points,
/// and each text block is absolutely positioned within it using inverted PDF coordinates.
pub fn view_as_html(reader: &PdfReader) -> Result<String, HandlerError> {
    let mut pages_html = String::new();

    let file_name = std::path::Path::new(reader.file_path())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Document.pdf");
    let total_pages = reader.page_count();

    for i in 1..=total_pages {
        let mut width = 612.0; // default Letter width
        let mut height = 792.0; // default Letter height
        let mut llx = 0.0;
        let mut lly = 0.0;

        let pages = reader.document().get_pages();
        if let Some(&page_id) = pages.get(&(i as u32)) {
            if let Ok(page_dict) = reader.document().get_dictionary(page_id) {
                let box_obj = page_dict
                    .get(b"MediaBox")
                    .or_else(|_| page_dict.get(b"CropBox"));
                if let Ok(obj) = box_obj {
                    let resolved = reader
                        .document()
                        .dereference(obj)
                        .map(|(_, o)| o)
                        .unwrap_or(obj);
                    if let Ok(arr) = resolved.as_array() {
                        if arr.len() == 4 {
                            let x0 = arr[0]
                                .as_float()
                                .or_else(|_| arr[0].as_i64().map(|x| x as f32))
                                .unwrap_or(0.0);
                            let y0 = arr[1]
                                .as_float()
                                .or_else(|_| arr[1].as_i64().map(|x| x as f32))
                                .unwrap_or(0.0);
                            let x1 = arr[2]
                                .as_float()
                                .or_else(|_| arr[2].as_i64().map(|x| x as f32))
                                .unwrap_or(612.0);
                            let y1 = arr[3]
                                .as_float()
                                .or_else(|_| arr[3].as_i64().map(|x| x as f32))
                                .unwrap_or(792.0);
                            llx = x0;
                            lly = y0;
                            width = x1 - x0;
                            height = y1 - y0;
                        }
                    }
                }
            }
        }

        pages_html.push_str(&format!(
            "<div class=\"page\" data-path=\"/page[{}]\" style=\"position:relative; width:{:.1}pt; height:{:.1}pt; background:white; border-radius:4px; overflow:hidden;\">\n  <div class=\"page-number-label\">Page {}</div>\n",
            i, width, height, i
        ));

        if let Some(parsed) = reader.parse_page_text_blocks(i) {
            // 1. Render XObject Images (so text renders on top of them)
            for img in &parsed.image_blocks {
                if let Some(data_uri) = parsed.image_map.get(&img.xobject_name) {
                    let bbox = &img.bbox;
                    let top = height - (bbox.y - lly) - bbox.height;
                    let left = bbox.x - llx;
                    pages_html.push_str(&format!(
                        "  <img class=\"page-image\" data-path=\"/page[{}]/image[{}]\" data-bbox=\"{:.1},{:.1},{:.1},{:.1}\" src=\"{}\" style=\"position:absolute; left:{:.1}pt; top:{:.1}pt; width:{:.1}pt; height:{:.1}pt; object-fit:fill; pointer-events:none;\" />\n",
                        i, img.index, bbox.x, bbox.y, bbox.width, bbox.height, data_uri, left, top, bbox.width, bbox.height
                    ));
                }
            }

            // 2. Render Text Blocks
            for block in &parsed.text_blocks {
                let escaped = html_escape(&block.text);
                let bbox = &block.bbox;
                let size = block.style.font_size.unwrap_or(12.0);

                let color_style = block.style.fill_color.as_ref().map(|c| match c {
                    PdfColor::Gray(g) => format!(
                        "rgb({},{},{})",
                        (g * 255.0) as u8,
                        (g * 255.0) as u8,
                        (g * 255.0) as u8
                    ),
                    PdfColor::Rgb(r, g, b) => format!(
                        "rgb({},{},{})",
                        (r * 255.0) as u8,
                        (g * 255.0) as u8,
                        (b * 255.0) as u8
                    ),
                    PdfColor::Cmyk(c, m, y, k) => {
                        let r = ((1.0 - c) * (1.0 - k) * 255.0) as u8;
                        let g = ((1.0 - m) * (1.0 - k) * 255.0) as u8;
                        let b = ((1.0 - y) * (1.0 - k) * 255.0) as u8;
                        format!("rgb({},{},{})", r, g, b)
                    }
                });

                let color_attr = color_style.as_deref().unwrap_or("black");

                let bg_color_style = block.style.bg_color.as_ref().map(|c| match c {
                    PdfColor::Gray(g) => format!(
                        "rgb({},{},{})",
                        (g * 255.0) as u8,
                        (g * 255.0) as u8,
                        (g * 255.0) as u8
                    ),
                    PdfColor::Rgb(r, g, b) => format!(
                        "rgb({},{},{})",
                        (r * 255.0) as u8,
                        (g * 255.0) as u8,
                        (b * 255.0) as u8
                    ),
                    PdfColor::Cmyk(c, m, y, k) => {
                        let r = ((1.0 - c) * (1.0 - k) * 255.0) as u8;
                        let g = ((1.0 - m) * (1.0 - k) * 255.0) as u8;
                        let b = ((1.0 - y) * (1.0 - k) * 255.0) as u8;
                        format!("rgb({},{},{})", r, g, b)
                    }
                });

                let bg_style_str = bg_color_style.as_ref().map(|bg| format!("background-color:{};", bg)).unwrap_or_default();

                // Map font resources to standard styles
                let mut font_family = "sans-serif".to_string();
                let mut font_weight = "normal".to_string();
                let mut font_style = "normal".to_string();

                if let Some(ref font_id) = block.style.font_name {
                    if let Some(font_info) = parsed.font_map.get(font_id) {
                        if let Some(ref base_font) = font_info.base_font {
                            let (fam, w, s) = map_pdf_font_to_css(base_font);
                            font_family = fam;
                            font_weight = w;
                            font_style = s;
                        }
                    }
                }

                // PDF y coordinate is bottom-up (0 is bottom).
                // HTML y coordinate is top-down (0 is top).
                let top = height - (bbox.y - lly) - bbox.height;
                let left = bbox.x - llx;

                pages_html.push_str(&format!(
                    "  <span class=\"text-block\" data-path=\"/page[{}]/text[{}]\" data-bbox=\"{:.1},{:.1},{:.1},{:.1}\" style=\"position:absolute; left:{:.1}pt; top:{:.1}pt; width:{:.1}pt; height:{:.1}pt; font-family:{}; font-size:{:.1}pt; font-weight:{}; font-style:{}; color:{}; {}white-space:nowrap;\">{}</span>\n",
                    i, block.index, bbox.x, bbox.y, bbox.width, bbox.height, left, top, bbox.width, bbox.height, font_family, size, font_weight, font_style, color_attr, bg_style_str, escaped
                ));
            }
            if parsed.text_blocks.is_empty() && parsed.image_blocks.is_empty() {
                pages_html.push_str("  <div class=\"no-text\" style=\"position:absolute; inset:0; display:flex; align-items:center; justify-content:center; color:#999; font-style:italic;\">(no extractable text)</div>\n");
            }
        } else {
            pages_html.push_str("  <div class=\"no-text\" style=\"position:absolute; inset:0; display:flex; align-items:center; justify-content:center; color:#999; font-style:italic;\">(no extractable text)</div>\n");
        }

        // 3. Render Native PDF Highlight Annotations
        if let Some(&page_id) = pages.get(&(i as u32)) {
            if let Ok(page_dict) = reader.document().get_dictionary(page_id) {
                if let Ok(annots_obj) = page_dict.get(b"Annots") {
                    if let Ok(lopdf::Object::Array(annots_arr)) = reader.document().dereference(annots_obj).map(|(_, o)| o) {
                        for annot_ref in annots_arr {
                            if let Ok((_, lopdf::Object::Dictionary(annot_dict))) = reader.document().dereference(annot_ref) {
                                if let Ok(subtype) = annot_dict.get(b"Subtype").and_then(|v| v.as_name_str()) {
                                    if subtype == "Highlight" {
                                        // Extract color /C
                                        let mut r = 255;
                                        let mut g = 255;
                                        let mut b = 0;
                                        if let Ok(lopdf::Object::Array(c_arr)) = annot_dict.get(b"C").and_then(|o| reader.document().dereference(o).map(|(_, val)| val)) {
                                            if c_arr.len() >= 3 {
                                                let c_r = c_arr[0].as_float().or_else(|_| c_arr[0].as_i64().map(|x| x as f32)).unwrap_or(1.0);
                                                let c_g = c_arr[1].as_float().or_else(|_| c_arr[1].as_i64().map(|x| x as f32)).unwrap_or(1.0);
                                                let c_b = c_arr[2].as_float().or_else(|_| c_arr[2].as_i64().map(|x| x as f32)).unwrap_or(0.0);
                                                r = (c_r * 255.0).clamp(0.0, 255.0) as u8;
                                                g = (c_g * 255.0).clamp(0.0, 255.0) as u8;
                                                b = (c_b * 255.0).clamp(0.0, 255.0) as u8;
                                            }
                                        }

                                        let mut highlight_rects = Vec::new();
                                        if let Ok(lopdf::Object::Array(quads)) = annot_dict.get(b"QuadPoints").and_then(|o| reader.document().dereference(o).map(|(_, val)| val)) {
                                            let mut idx = 0;
                                            while idx + 7 < quads.len() {
                                                let x1 = quads[idx].as_float().or_else(|_| quads[idx].as_i64().map(|x| x as f32)).unwrap_or(0.0);
                                                let y1 = quads[idx+1].as_float().or_else(|_| quads[idx+1].as_i64().map(|x| x as f32)).unwrap_or(0.0);
                                                let x2 = quads[idx+2].as_float().or_else(|_| quads[idx+2].as_i64().map(|x| x as f32)).unwrap_or(0.0);
                                                let y2 = quads[idx+3].as_float().or_else(|_| quads[idx+3].as_i64().map(|x| x as f32)).unwrap_or(0.0);
                                                let x3 = quads[idx+4].as_float().or_else(|_| quads[idx+4].as_i64().map(|x| x as f32)).unwrap_or(0.0);
                                                let y3 = quads[idx+5].as_float().or_else(|_| quads[idx+5].as_i64().map(|x| x as f32)).unwrap_or(0.0);
                                                let x4 = quads[idx+6].as_float().or_else(|_| quads[idx+6].as_i64().map(|x| x as f32)).unwrap_or(0.0);
                                                let y4 = quads[idx+7].as_float().or_else(|_| quads[idx+7].as_i64().map(|x| x as f32)).unwrap_or(0.0);

                                                let x = x1.min(x2).min(x3).min(x4);
                                                let y = y1.min(y2).min(y3).min(y4);
                                                let w = (x1.max(x2).max(x3).max(x4) - x).max(1.0);
                                                let h = (y1.max(y2).max(y3).max(y4) - y).max(1.0);

                                                highlight_rects.push((x, y, w, h));
                                                idx += 8;
                                            }
                                        }

                                        if highlight_rects.is_empty() {
                                            if let Ok(lopdf::Object::Array(rect_arr)) = annot_dict.get(b"Rect").and_then(|o| reader.document().dereference(o).map(|(_, val)| val)) {
                                                if rect_arr.len() == 4 {
                                                    let x0 = rect_arr[0].as_float().or_else(|_| rect_arr[0].as_i64().map(|x| x as f32)).unwrap_or(0.0);
                                                    let y0 = rect_arr[1].as_float().or_else(|_| rect_arr[1].as_i64().map(|x| x as f32)).unwrap_or(0.0);
                                                    let x1 = rect_arr[2].as_float().or_else(|_| rect_arr[2].as_i64().map(|x| x as f32)).unwrap_or(0.0);
                                                    let y1 = rect_arr[3].as_float().or_else(|_| rect_arr[3].as_i64().map(|x| x as f32)).unwrap_or(0.0);
                                                    highlight_rects.push((x0, y0, x1 - x0, y1 - y0));
                                                }
                                            }
                                        }

                                        for &(rx, ry, rw, rh) in &highlight_rects {
                                            let top = height - (ry - lly) - rh;
                                            let left = rx - llx;
                                            pages_html.push_str(&format!(
                                                "  <div class=\"highlight-annot\" style=\"position:absolute; left:{:.1}pt; top:{:.1}pt; width:{:.1}pt; height:{:.1}pt; background-color:rgba({},{},{},0.35); mix-blend-mode:multiply; pointer-events:none;\"></div>\n",
                                                left, top, rw, rh, r, g, b
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        pages_html.push_str("</div>\n");
    }

    Ok(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{} - PDF Preview</title>
<style>
body {{
    font-family: "Segoe UI", -apple-system, BlinkMacSystemFont, Roboto, Arial, sans-serif;
    margin: 0;
    background: #eef2f5;
    padding: 30px 20px;
    display: flex;
    flex-direction: column;
    align-items: center;
}}
h1 {{
    color: #2c3e50;
    margin-top: 0;
    margin-bottom: 24px;
    font-weight: 600;
    text-shadow: 0 1px 2px rgba(0,0,0,0.05);
}}
.page-container {{
    display: flex;
    flex-direction: column;
    gap: 20px;
    width: 100%;
    align-items: center;
}}
.page {{
    box-shadow: 0 4px 12px rgba(0,0,0,0.15);
    border-radius: 4px;
    transition: transform 0.2s, box-shadow 0.2s;
    isolation: isolate;
}}
.page:hover {{
    transform: translateY(-2px);
    box-shadow: 0 8px 24px rgba(0,0,0,0.2) !important;
}}
.page-number-label {{
    position: absolute;
    bottom: 10px;
    right: 15px;
    color: #bbb;
    font-size: 10px;
    z-index: 50;
    pointer-events: none;
    user-select: none;
    background: rgba(248, 250, 252, 0.85);
    backdrop-filter: blur(4px);
    padding: 2px 8px;
    border-radius: 4px;
    border: 1px solid rgba(226, 232, 240, 0.8);
}}
.text-block {{
    display: inline-block;
    cursor: pointer;
    line-height: 1;
    transform-origin: left top;
    transition: background-color 0.1s, outline 0.1s;
}}
.text-block:hover {{
    background-color: rgba(76, 175, 80, 0.1);
    outline: 1px dashed #4CAF50;
    z-index: 100;
}}
</style>
</head>
<body>
<div class="page-container">
{}
</div>
<script>
function adjustTextScaling() {{
    const blocks = document.querySelectorAll(".text-block");
    blocks.forEach(block => {{
        block.style.transform = "";
        const expectedWidth = block.getBoundingClientRect().width;
        const actualWidth = block.scrollWidth;
        if (actualWidth > expectedWidth && expectedWidth > 0) {{
            const scale = expectedWidth / actualWidth;
            block.style.transform = "scaleX(" + scale + ")";
        }}
    }});
}}
window.addEventListener("load", adjustTextScaling);
if (document.fonts && document.fonts.ready) {{
    document.fonts.ready.then(adjustTextScaling);
}}
</script>
</body>
</html>"#,
        file_name, pages_html
    ))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
