use handler_common::HandlerError;
use oxml::OxmlPackage;

/// Render the PowerPoint presentation as SVG for browser preview.
/// Each slide is rendered as a separate SVG group with text, shapes, and basic layout.
pub fn view_as_svg(package: &OxmlPackage) -> Result<String, HandlerError> {
    let presentation = crate::navigation::build_presentation(package)?;

    let mut svg_parts = String::new();

    for (i, slide) in presentation.slides.iter().enumerate() {
        let slide_num = i + 1;
        // Standard slide dimensions: 10" x 7.5" at 96dpi = 960x720
        svg_parts.push_str(&format!(
            "<svg viewBox=\"0 0 960 720\" data-slide=\"{}\" xmlns=\"http://www.w3.org/2000/svg\">\n",
            slide_num
        ));

        // Background rectangle
        svg_parts.push_str("<rect width=\"960\" height=\"720\" fill=\"white\" stroke=\"#ccc\" stroke-width=\"1\"/>\n");

        // Slide number indicator
        svg_parts.push_str(&format!(
            "<text x=\"920\" y=\"710\" font-size=\"12\" fill=\"#888\" text-anchor=\"end\">{}</text>\n",
            slide_num
        ));

        // Render shapes with text
        let mut y_offset = 40;
        for shape in &slide.shapes {
            if !shape.text.is_empty() {
                let escaped = svg_escape(&shape.text);

                // Determine text styling based on placeholder type
                let (font_size, font_weight, fill) = match shape.placeholder_type.as_deref() {
                    Some("title") | Some("ctrTitle") => (32, "bold", "#1a1a1a"),
                    Some("subTitle") => (20, "normal", "#666"),
                    Some("body") => (16, "normal", "#333"),
                    _ => (14, "normal", "#333"),
                };

                let is_header = shape.placeholder_type.as_deref() == Some("title")
                    || shape.placeholder_type.as_deref() == Some("ctrTitle");

                let text_anchor = if is_header { "middle" } else { "start" };
                let x_pos = if is_header { 480 } else { 40 };

                // Split multi-line text
                for (line_idx, line) in escaped.lines().enumerate() {
                    let line_y = y_offset + line_idx * (font_size as usize + 4);
                    svg_parts.push_str(&format!(
                        "<text x=\"{}\" y=\"{}\" font-size=\"{}\" font-weight=\"{}\" fill=\"{}\" text-anchor=\"{}\" font-family=\"Segoe UI, Arial, sans-serif\">{}</text>\n",
                        x_pos, line_y, font_size, font_weight, fill, text_anchor, line
                    ));
                }

                // Calculate next shape offset
                let line_count = escaped.lines().count().max(1);
                y_offset += line_count * (font_size as usize + 4) + 20;
            }
        }

        svg_parts.push_str("</svg>\n");
    }

    // Wrap all slides in a container SVG
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"960\" height=\"720\">\n<desc>PPTX Preview — {} slides</desc>\n{}\n</svg>",
        presentation.slides.len(),
        svg_parts
    ))
}

fn svg_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
