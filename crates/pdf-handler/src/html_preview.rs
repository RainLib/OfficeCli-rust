use handler_common::HandlerError;
use crate::reader::PdfReader;
use crate::content_stream::PdfColor;

/// Render the PDF document as HTML for browser preview.
/// Each text block is a <span> with data-path and data-bbox attributes.
pub fn view_as_html(reader: &PdfReader) -> Result<String, HandlerError> {
    let mut pages_html = String::new();

    for i in 1..=reader.page_count() {
        pages_html.push_str(&format!(
            "<div class=\"page\" data-path=\"/page[{}]\">\n<div class=\"page-number\">Page {}</div>\n",
            i, i
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

                pages_html.push_str(&format!(
                    "<span class=\"text-block\" data-path=\"/page[{}]/text[{}]\" data-bbox=\"{:.0},{:.0},{:.1},{:.0}\" style=\"font-family:{};font-size:{:.0}px;color:{}\">{}</span>\n",
                    i, block.index, bbox.x, bbox.y, bbox.width, bbox.height, font, size, color_attr, escaped
                ));
            }
            if parsed.text_blocks.is_empty() {
                pages_html.push_str("<pre>(no extractable text)</pre>\n");
            }
        } else {
            pages_html.push_str("<pre>(no extractable text)</pre>\n");
        }

        pages_html.push_str("</div>\n");
    }

    Ok(format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<style>
body {{ font-family: "Segoe UI", Arial, sans-serif; margin: 20px; background: #f5f5f5; }}
.page {{ background: white; border: 1px solid #ddd; margin: 20px auto; max-width: 800px; padding: 40px; }}
.page-number {{ color: #888; font-size: 0.8em; margin-bottom: 10px; }}
.text-block {{ display: inline; margin-right: 4px; }}
.text-block:hover {{ outline: 1px dashed #4CAF50; cursor: pointer; }}
h1 {{ text-align: center; }}
</style>
</head>
<body>
<h1>PDF Preview</h1>
{}
</body>
</html>"#, pages_html))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}