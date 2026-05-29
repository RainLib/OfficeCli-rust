use handler_common::{HandlerError, RawOptions};
use oxml::xml_util;
use oxml::OxmlPackage;
use std::collections::HashMap;

/// Read raw XML from a part in the package.
pub fn read_raw(
    package: &OxmlPackage,
    part_path: &str,
    opts: RawOptions,
) -> Result<String, HandlerError> {
    let xml = package
        .read_part_xml(part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Apply line range if specified
    if opts.start_row.is_some() || opts.end_row.is_some() {
        let lines: Vec<&str> = xml.lines().collect();
        let start = opts.start_row.unwrap_or(0);
        let end = opts.end_row.unwrap_or(lines.len());
        let start = if start > 0 { start - 1 } else { 0 };
        let end = end.min(lines.len());
        if start < end {
            Ok(lines[start..end].join("\n"))
        } else {
            Ok(String::new())
        }
    } else {
        Ok(xml)
    }
}

/// Apply a raw XML modification action to a part.
/// Supported actions: setattr, remove
pub fn apply_raw_set(
    package: &mut OxmlPackage,
    part_path: &str,
    xpath: &str,
    action: &str,
    xml: Option<&str>,
) -> Result<(), HandlerError> {
    let original = package
        .read_part_xml(part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let modified = xml_util::apply_xpath_action(&original, xpath, action, xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    package
        .write_part_xml(part_path, &modified)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    Ok(())
}

/// Add a new part (relationship + content) to the package.
pub fn add_part(
    package: &mut OxmlPackage,
    parent: &str,
    part_type: &str,
    properties: Option<&HashMap<String, String>>,
) -> Result<(String, String), HandlerError> {
    // Determine part path based on type and parent
    let part_path = resolve_new_part_path(package, parent, part_type)?;

    // Create minimal content based on part type
    let content = create_part_content(part_type, properties);

    package
        .write_part_xml(&part_path, &content)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    Ok((part_path.clone(), part_path))
}

/// Resolve a new part path based on the type and parent relationship.
fn resolve_new_part_path(
    package: &OxmlPackage,
    _parent: &str,
    part_type: &str,
) -> Result<String, HandlerError> {
    match part_type {
        "styles" => Ok("word/styles.xml".to_string()),
        "numbering" => Ok("word/numbering.xml".to_string()),
        "footnotes" => Ok("word/footnotes.xml".to_string()),
        "endnotes" => Ok("word/endnotes.xml".to_string()),
        "comments" => Ok("word/comments.xml".to_string()),
        "header" | "footer" => {
            // Generate a unique header/footer part
            let existing = package.list_parts();
            let mut idx = 1;
            for _ in existing {
                idx += 1;
            }
            if part_type == "header" {
                Ok(format!("word/header{}.xml", idx))
            } else {
                Ok(format!("word/footer{}.xml", idx))
            }
        }
        other => Err(HandlerError::UnsupportedType(format!(
            "unsupported part type: {}",
            other
        ))),
    }
}

/// Create minimal XML content for a new part.
fn create_part_content(part_type: &str, _properties: Option<&HashMap<String, String>>) -> String {
    let w_ns = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";
    let r_ns = "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";

    match part_type {
        "styles" => {
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles {} {}></w:styles>", w_ns, r_ns)
        }
        "numbering" => {
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:numbering {} {}></w:numbering>", w_ns, r_ns)
        }
        "footnotes" => {
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:footnotes {} {}></w:footnotes>", w_ns, r_ns)
        }
        "endnotes" => {
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:endnotes {} {}></w:endnotes>", w_ns, r_ns)
        }
        "comments" => {
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:comments {} {}></w:comments>", w_ns, r_ns)
        }
        "header" | "footer" => {
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:{} {} {}><w:p></w:p></w:{}>", part_type, w_ns, r_ns, part_type)
        }
        _ => "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>".to_string(),
    }
}
