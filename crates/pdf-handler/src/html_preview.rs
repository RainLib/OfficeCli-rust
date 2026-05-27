use handler_common::HandlerError;
use crate::reader::PdfReader;
use crate::content_stream::PdfColor;

/// Render the PDF document as HTML for browser preview.
/// Each page is rendered as a relative container with a physical size in points,
/// and each text block is absolutely positioned within it using inverted PDF coordinates.
pub fn view_as_html(reader: &PdfReader) -> Result<String, HandlerError> {
    let mut pages_html = String::new();

    for i in 1..=reader.page_count() {
        let mut width = 612.0;  // default Letter width
        let mut height = 792.0; // default Letter height
        let mut llx = 0.0;
        let mut lly = 0.0;

        let pages = reader.document().get_pages();
        if let Some(&page_id) = pages.get(&(i as u32)) {
            if let Ok(page_dict) = reader.document().get_dictionary(page_id) {
                let box_obj = page_dict.get(b"MediaBox").or_else(|_| page_dict.get(b"CropBox"));
                if let Ok(obj) = box_obj {
                    let resolved = reader.document().dereference(obj).map(|(_, o)| o).unwrap_or(obj);
                    if let Ok(arr) = resolved.as_array() {
                        if arr.len() == 4 {
                            let x0 = arr[0].as_float()
                                .or_else(|_| arr[0].as_i64().map(|x| x as f32))
                                .unwrap_or(0.0);
                            let y0 = arr[1].as_float()
                                .or_else(|_| arr[1].as_i64().map(|x| x as f32))
                                .unwrap_or(0.0);
                            let x1 = arr[2].as_float()
                                .or_else(|_| arr[2].as_i64().map(|x| x as f32))
                                .unwrap_or(612.0);
                            let y1 = arr[3].as_float()
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
            "<div class=\"page\" data-path=\"/page[{}]\" style=\"position:relative; width:{:.1}pt; height:{:.1}pt; background:white; box-shadow:0 4px 16px rgba(0,0,0,0.15); margin:20px auto; border-radius:4px; overflow:hidden;\">\n  <div class=\"page-number\">Page {}</div>\n",
            i, width, height, i
        ));

        if let Some(parsed) = reader.parse_page_text_blocks(i) {
            for block in &parsed.text_blocks {
                let escaped = html_escape(&block.text);
                let bbox = &block.bbox;
                let font = block.style.font_name.as_deref().unwrap_or("sans-serif");
                let size = block.style.font_size.unwrap_or(12.0);

                let color_style = block.style.fill_color.as_ref().map(|c| {
                    match c {
                        PdfColor::Gray(g) => format!("rgb({},{},{})", (g*255.0) as u8, (g*255.0) as u8, (g*255.0) as u8),
                        PdfColor::Rgb(r, g, b) => format!("rgb({},{},{})", (r*255.0) as u8, (g*255.0) as u8, (b*255.0) as u8),
                        PdfColor::Cmyk(c, m, y, k) => {
                            let r = ((1.0-c)*(1.0-k)*255.0) as u8;
                            let g = ((1.0-m)*(1.0-k)*255.0) as u8;
                            let b = ((1.0-y)*(1.0-k)*255.0) as u8;
                            format!("rgb({},{},{})", r, g, b)
                        }
                    }
                });

                let color_attr = color_style.as_deref().unwrap_or("black");

                // PDF y coordinate is bottom-up (0 is bottom).
                // HTML y coordinate is top-down (0 is top).
                // Subtract bbox.y - lly from page height to get top offset, and subtract height of block
                let top = height - (bbox.y - lly) - bbox.height;
                let left = bbox.x - llx;

                pages_html.push_str(&format!(
                    "  <span class=\"text-block\" data-path=\"/page[{}]/text[{}]\" data-bbox=\"{:.1},{:.1},{:.1},{:.1}\" style=\"position:absolute; left:{:.1}pt; top:{:.1}pt; width:{:.1}pt; height:{:.1}pt; font-family:'{}', sans-serif; font-size:{:.1}pt; color:{}; white-space:nowrap;\">{}</span>\n",
                    i, block.index, bbox.x, bbox.y, bbox.width, bbox.height, left, top, bbox.width, bbox.height, font, size, color_attr, escaped
                ));
            }
            if parsed.text_blocks.is_empty() {
                pages_html.push_str("  <div class=\"no-text\" style=\"position:absolute; inset:0; display:flex; align-items:center; justify-content:center; color:#999; font-style:italic;\">(no extractable text)</div>\n");
            }
        } else {
            pages_html.push_str("  <div class=\"no-text\" style=\"position:absolute; inset:0; display:flex; align-items:center; justify-content:center; color:#999; font-style:italic;\">(no extractable text)</div>\n");
        }

        pages_html.push_str("</div>\n");
    }

    Ok(format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>PDF Preview</title>
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
    transition: transform 0.2s, box-shadow 0.2s;
}}
.page:hover {{
    transform: translateY(-2px);
    box-shadow: 0 8px 24px rgba(0,0,0,0.2) !important;
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
.page-number {{
    position: absolute;
    bottom: 10px;
    right: 15px;
    color: #bbb;
    font-size: 10px;
    z-index: 50;
    pointer-events: none;
    user-select: none;
}}
</style>
</head>
<body>
<h1>PDF Document Preview</h1>
<div class="page-container">
{}
</div>
</body>
</html>"#, pages_html))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}