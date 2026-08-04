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

/// Restore a type-agnostic OOXML relationship edge from a replayable dump.
/// This covers media, charts, SmartArt, OLE, model3d and extension sidecars
/// without requiring a separate L2 constructor for each resource family.
pub fn embed_relationship_part(
    package: &mut OxmlPackage,
    source_part: &str,
    relationship_id: &str,
    payload: &str,
) -> Result<(), HandlerError> {
    let value: serde_json::Value = serde_json::from_str(payload).map_err(|error| {
        HandlerError::InvalidArgument(format!("invalid embed-part JSON: {error}"))
    })?;
    let relationship_type = required_payload_string(&value, "type")?;
    let target = required_payload_string(&value, "target")?;
    let target_mode = value
        .get("targetMode")
        .and_then(|value| value.as_str())
        .unwrap_or("Internal");
    if !target_mode.eq_ignore_ascii_case("external") {
        let part_path = required_payload_string(&value, "partPath")?;
        let content_type = required_payload_string(&value, "contentType")?;
        let data = required_payload_string(&value, "data")?;
        package
            .write_part(part_path, parse_data_uri(data)?)
            .map_err(|error| HandlerError::SaveError(error.to_string()))?;
        ensure_content_type_override(package, part_path, content_type)?;
    }
    let rels_path = relationship_part_path(source_part);
    let target_mode_attr = if target_mode.eq_ignore_ascii_case("external") {
        " TargetMode=\"External\""
    } else {
        ""
    };
    upsert_relationship(
        package,
        &rels_path,
        relationship_id,
        &format!(
            "<Relationship Id=\"{}\" Type=\"{}\" Target=\"{}\"{target_mode_attr}/>",
            escape_xml_attr(relationship_id),
            escape_xml_attr(relationship_type),
            escape_xml_attr(target),
        ),
    )
}

fn required_payload_string<'a>(
    value: &'a serde_json::Value,
    field: &str,
) -> Result<&'a str, HandlerError> {
    value
        .get(field)
        .and_then(|value| value.as_str())
        .ok_or_else(|| HandlerError::InvalidArgument(format!("embed-part requires '{field}'")))
}

fn parse_data_uri(value: &str) -> Result<Vec<u8>, HandlerError> {
    let (_, encoded) = value
        .split_once(',')
        .filter(|(head, _)| head.starts_with("data:") && head.ends_with(";base64"))
        .ok_or_else(|| {
            HandlerError::InvalidArgument(
                "embed-part data must be a data:<content-type>;base64 payload".to_string(),
            )
        })?;
    base64_decode(encoded).map_err(|_| {
        HandlerError::InvalidArgument("embed-part data contains invalid base64".to_string())
    })
}

fn relationship_part_path(source_part: &str) -> String {
    if let Some((directory, name)) = source_part.rsplit_once('/') {
        format!("{directory}/_rels/{name}.rels")
    } else {
        format!("_rels/{source_part}.rels")
    }
}

fn upsert_relationship(
    package: &mut OxmlPackage,
    rels_path: &str,
    relationship_id: &str,
    relationship: &str,
) -> Result<(), HandlerError> {
    let xml = package.read_part_xml(rels_path).unwrap_or_else(|_| {
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"/>".to_string()
    });
    let id_attribute = format!("Id=\"{}\"", relationship_id);
    let existing = xml.match_indices("<Relationship").find_map(|(start, _)| {
        let remainder = &xml[start..];
        let end = remainder.find('>')?;
        (!remainder.starts_with("<Relationships") && remainder[..=end].contains(&id_attribute))
            .then_some((start, start + end + 1))
    });
    let updated = if let Some((start, end)) = existing {
        format!("{}{}{}", &xml[..start], relationship, &xml[end..])
    } else {
        insert_relationship(&xml, relationship)?
    };
    package
        .write_part_xml(rels_path, &updated)
        .map_err(|error| HandlerError::SaveError(error.to_string()))
}

fn insert_relationship(xml: &str, relationship: &str) -> Result<String, HandlerError> {
    if xml.contains("</Relationships>") {
        return Ok(xml.replace(
            "</Relationships>",
            &format!("{relationship}</Relationships>"),
        ));
    }
    let root = xml.find("<Relationships").ok_or_else(|| {
        HandlerError::OperationFailed("relationships part has no <Relationships> root".to_string())
    })?;
    let close = xml[root..]
        .find("/>")
        .map(|offset| root + offset)
        .ok_or_else(|| {
            HandlerError::OperationFailed("relationships root is not closed".to_string())
        })?;
    Ok(format!(
        "{}>{relationship}</Relationships>{}",
        &xml[..close],
        &xml[close + 2..]
    ))
}

fn ensure_content_type_override(
    package: &mut OxmlPackage,
    part_path: &str,
    content_type: &str,
) -> Result<(), HandlerError> {
    let xml = package
        .read_part_xml("[Content_Types].xml")
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let part_name = format!("/{}", part_path.trim_start_matches('/'));
    if xml.contains(&format!("PartName=\"{part_name}\"")) {
        return Ok(());
    }
    let override_xml = format!(
        "<Override PartName=\"{part_name}\" ContentType=\"{}\"/>",
        escape_xml_attr(content_type)
    );
    let updated = xml.replace("</Types>", &format!("{override_xml}</Types>"));
    package
        .write_part_xml("[Content_Types].xml", &updated)
        .map_err(|error| HandlerError::SaveError(error.to_string()))
}

fn escape_xml_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn base64_decode(value: &str) -> Result<Vec<u8>, ()> {
    let mut bits = 0u32;
    let mut nbits = 0u32;
    let mut output = Vec::with_capacity(value.len() * 3 / 4);
    for character in value.chars().filter(|character| !character.is_whitespace()) {
        let value = match character {
            'A'..='Z' => character as u32 - 'A' as u32,
            'a'..='z' => character as u32 - 'a' as u32 + 26,
            '0'..='9' => character as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '=' => break,
            _ => return Err(()),
        };
        bits = (bits << 6) | value;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            output.push((bits >> nbits) as u8);
        }
    }
    Ok(output)
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
            if let Some(stripped) = parent.strip_prefix("/slide[") {
                let slide_num = stripped
                    .find(']')
                    .and_then(|pos| stripped[..pos].parse::<usize>().ok())
                    .ok_or_else(|| HandlerError::InvalidPath(parent.to_string()))?;
                let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;
                let rels_path = crate::navigation::relationships_part_path(&slide_path);

                let rel_type =
                    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
                let target = format!(
                    "../media/{}",
                    part_path.split('/').next_back().unwrap_or("image.png")
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
