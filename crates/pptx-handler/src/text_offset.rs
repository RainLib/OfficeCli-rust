use crate::navigation::build_presentation;
use handler_common::{HandlerError, TextOffsetMap};

/// Extract text with offset mappings for the entire presentation.
/// This enables AI agents to locate text by character offset.
pub fn extract_text_with_offsets(
    package: &oxml::OxmlPackage,
) -> Result<TextOffsetMap, HandlerError> {
    let pres = build_presentation(package)?;
    let mut map = TextOffsetMap::empty("pptx");

    for slide in &pres.slides {
        // Slide separator: "--- Slide N ---"
        let slide_header = format!("--- Slide {} ---\n", slide.index);
        map.push_span(
            &slide_header,
            &format!("/slide[{}]", slide.index),
            "slide-header",
        );

        for (si, shape) in slide.shapes.iter().enumerate() {
            if shape.text.is_empty() {
                continue;
            }

            let shape_path = format!("/slide[{}]/shape[{}]", slide.index, si + 1);

            // Push shape text as a single span
            map.push_span(&shape.text, &shape_path, "shape");

            // Also push individual paragraph spans for finer granularity
            for (pi, para) in shape.paragraphs.iter().enumerate() {
                if para.text.is_empty() {
                    continue;
                }
                let para_path = format!(
                    "/slide[{}]/shape[{}]/paragraph[{}]",
                    slide.index,
                    si + 1,
                    pi + 1
                );
                map.push_span(&para.text, &para_path, "paragraph");
            }
        }

        // Newline between slides
        map.push_span("\n", &format!("/slide[{}]", slide.index), "paragraph-break");
    }

    Ok(map)
}
