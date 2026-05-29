use crate::navigation::build_presentation;
use handler_common::{DocumentNode, HandlerError, Selector};

/// Query PPTX elements using a selector.
pub fn query_elements(
    package: &oxml::OxmlPackage,
    selector_str: &str,
) -> Result<Vec<DocumentNode>, HandlerError> {
    let selector =
        Selector::parse(selector_str).map_err(|e| HandlerError::InvalidArgument(e.to_string()))?;
    let pres = build_presentation(package)?;

    let mut results = Vec::new();
    let element_type = selector.element_type.as_deref().unwrap_or("*");

    match element_type {
        "slide" | "*" => {
            for slide in &pres.slides {
                let path = format!("/slide[{}]", slide.index);
                let text: Vec<String> = slide
                    .shapes
                    .iter()
                    .filter(|s| !s.text.is_empty())
                    .map(|s| s.text.clone())
                    .collect();
                results.push(DocumentNode::new(&path, "slide").with_text(text.join("\n")));
            }
        }
        "shape" => {
            for slide in &pres.slides {
                for (j, shape) in slide.shapes.iter().enumerate() {
                    let path = format!("/slide[{}]/shape[{}]", slide.index, j + 1);
                    results.push(
                        DocumentNode::new(&path, "shape")
                            .with_text(&shape.text)
                            .with_preview(shape.name.clone()),
                    );
                }
            }
        }
        "text" => {
            for slide in &pres.slides {
                for (j, shape) in slide.shapes.iter().enumerate() {
                    if !shape.text.is_empty() {
                        let path = format!("/slide[{}]/shape[{}]", slide.index, j + 1);
                        results.push(DocumentNode::new(&path, "text-block").with_text(&shape.text));
                    }
                }
            }
        }
        other => {
            return Err(HandlerError::InvalidArgument(format!(
                "unsupported selector type: {}",
                other
            )))
        }
    }

    Ok(results)
}
