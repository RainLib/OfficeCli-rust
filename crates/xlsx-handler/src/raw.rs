/// Raw XML access operations for xlsx documents.
use handler_common::HandlerError;
use oxml::xml_util;
use oxml::OxmlPackage;
use std::collections::HashMap;

/// Apply a raw XPath action to a part XML.
pub fn raw_set(
    package: &mut OxmlPackage,
    part_path: &str,
    xpath: &str,
    action: &str,
    xml: Option<&str>,
) -> Result<(), HandlerError> {
    // Read the current part XML
    let current_xml = package
        .read_part_xml(part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Apply the action using xml_util
    let modified_xml = xml_util::apply_xpath_action(&current_xml, xpath, action, xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Write back
    package
        .write_part_xml(part_path, &modified_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    Ok(())
}

/// Add a new part to the xlsx package.
pub fn add_part(
    package: &mut OxmlPackage,
    _parent: &str,
    part_type: &str,
    properties: Option<&HashMap<String, String>>,
) -> Result<(String, String), HandlerError> {
    match part_type {
        "shared-strings" => {
            // Ensure xl/sharedStrings.xml exists (even if empty)
            let ss_path = "xl/sharedStrings.xml";
            if !package.has_part(ss_path) {
                let empty_ss = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
                    <sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" count=\"0\" uniqueCount=\"0\"/>";
                package
                    .write_part_xml(ss_path, empty_ss)
                    .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
            }
            Ok((
                ss_path.to_string(),
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"
                    .to_string(),
            ))
        }
        "style" => {
            // Ensure xl/styles.xml exists
            let styles_path = "xl/styles.xml";
            if !package.has_part(styles_path) {
                let empty_styles = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
                    <styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"/>";
                package
                    .write_part_xml(styles_path, empty_styles)
                    .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
            }
            Ok((
                styles_path.to_string(),
                "application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"
                    .to_string(),
            ))
        }
        "image" => {
            let src_path = properties.and_then(|p| p.get("source")).ok_or_else(|| {
                HandlerError::InvalidArgument("image requires 'source' property".to_string())
            })?;

            let image_data = std::fs::read(src_path).map_err(|e| {
                HandlerError::OperationFailed(format!("failed to read image '{}': {}", src_path, e))
            })?;

            let ext = src_path.rsplit('.').next().unwrap_or("png");
            let next_idx = package.list_parts().len() + 1;
            let (mime_type, part_path) = match ext {
                "png" => ("image/png", format!("xl/media/image{}.png", next_idx)),
                "jpg" | "jpeg" => ("image/jpeg", format!("xl/media/image{}.jpeg", next_idx)),
                "gif" => ("image/gif", format!("xl/media/image{}.gif", next_idx)),
                other => ("image/png", format!("xl/media/image{}.{}", next_idx, other)),
            };

            package
                .write_part(&part_path, image_data)
                .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

            Ok((part_path, mime_type.to_string()))
        }
        other => Err(HandlerError::UnsupportedType(format!(
            "xlsx add_part '{}' not supported",
            other
        ))),
    }
}
