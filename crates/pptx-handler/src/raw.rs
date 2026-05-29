use handler_common::{HandlerError, RawOptions};
use oxml::xml_util;
use oxml::OxmlPackage;
use std::collections::HashMap;

/// Read raw XML from a PPTX part.
pub fn read_raw(
    package: &OxmlPackage,
    part_path: &str,
    opts: RawOptions,
) -> Result<String, HandlerError> {
    let xml = package
        .read_part_xml(part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

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

/// Apply a raw XML modification to a PPTX part.
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

/// Add a new part to the PPTX package.
pub fn add_part(
    package: &mut OxmlPackage,
    parent: &str,
    part_type: &str,
    properties: Option<&HashMap<String, String>>,
) -> Result<(String, String), HandlerError> {
    match part_type {
        "image" => {
            let src_path = properties.and_then(|p| p.get("source")).ok_or_else(|| {
                HandlerError::InvalidArgument(
                    "image requires 'source' property (file path)".to_string(),
                )
            })?;

            // Read the image file
            let image_data = std::fs::read(src_path).map_err(|e| {
                HandlerError::OperationFailed(format!("failed to read image '{}': {}", src_path, e))
            })?;

            // Determine image format from extension
            let ext = src_path.rsplit('.').next().unwrap_or("png");
            let next_idx = package.list_parts().len() + 1;
            let (mime_type, part_path) = match ext {
                "png" => ("image/png", format!("ppt/media/image{}.png", next_idx)),
                "jpg" | "jpeg" => ("image/jpeg", format!("ppt/media/image{}.jpeg", next_idx)),
                "gif" => ("image/gif", format!("ppt/media/image{}.gif", next_idx)),
                "bmp" => ("image/bmp", format!("ppt/media/image{}.bmp", next_idx)),
                "svg" => ("image/svg+xml", format!("ppt/media/image{}.svg", next_idx)),
                other => (
                    "image/png",
                    format!("ppt/media/image{}.{}", next_idx, other),
                ),
            };

            // Add the image part to the package
            package
                .write_part(&part_path, image_data)
                .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

            // Add relationship to parent slide
            let rel_id = format!("rId{}", package.list_parts().len() + 10);
            if parent.starts_with("/slide[") {
                let slide_num = parent[7..]
                    .find(']')
                    .and_then(|pos| parent[7..7 + pos].parse::<usize>().ok())
                    .ok_or_else(|| HandlerError::InvalidPath(parent.to_string()))?;
                let _slide_path = format!("ppt/slides/slide{}.xml", slide_num);
                let rels_path = format!("ppt/slides/_rels/slide{}.xml.rels", slide_num);

                let rel_type =
                    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
                let target = format!(
                    "../media/{}",
                    part_path.split('/').last().unwrap_or("image.png")
                );

                add_relationship(package, &rels_path, &rel_id, rel_type, &target);
            }

            Ok((part_path, mime_type.to_string()))
        }
        other => Err(HandlerError::UnsupportedType(format!(
            "PPTX add_part '{}' not supported",
            other
        ))),
    }
}

/// Add a relationship entry to a .rels file.
fn add_relationship(
    package: &mut OxmlPackage,
    rels_path: &str,
    id: &str,
    type_: &str,
    target: &str,
) {
    if let Ok(rels_xml) = package.read_part_xml(rels_path) {
        let new_rel = format!(
            "<Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"/>",
            id, type_, target
        );

        let modified = if let Some(pos) = rels_xml.find("</Relationships>") {
            let mut result = rels_xml[..pos].to_string();
            result.push_str(&new_rel);
            result.push_str(&rels_xml[pos..]);
            result
        } else {
            rels_xml
        };

        if let Err(e) = package.write_part_xml(rels_path, &modified) {
            eprintln!("Warning: failed to update rels {}: {}", rels_path, e);
        }
    }
}
