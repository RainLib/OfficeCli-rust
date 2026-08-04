/// Raw XML access operations for xlsx documents.
use handler_common::{HandlerError, RawOptions};
use oxml::xml_util;
use oxml::OxmlPackage;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};
use std::collections::HashMap;

/// Filter worksheet rows and cells for C#-compatible `raw --start/--end/--cols`.
/// The caller is responsible for using this only on an actual worksheet part.
pub fn filter_worksheet_xml(xml: &str, opts: &RawOptions) -> Result<String, HandlerError> {
    if opts.start_row.is_none() && opts.end_row.is_none() && opts.cols.is_none() {
        return Ok(xml.to_string());
    }

    let allowed_columns = opts.cols.as_ref().map(|columns| {
        columns
            .iter()
            .map(|column| column.trim().to_ascii_uppercase())
            .collect::<std::collections::HashSet<_>>()
    });
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());
    let mut in_sheet_data = false;
    let mut skip_depth = 0usize;

    loop {
        let event = reader.read_event().map_err(|e| {
            HandlerError::OperationFailed(format!("worksheet XML parse error: {}", e))
        })?;
        if matches!(event, Event::Eof) {
            break;
        }

        if skip_depth > 0 {
            match event {
                Event::Start(_) => skip_depth += 1,
                Event::End(_) => skip_depth -= 1,
                _ => {}
            }
            continue;
        }

        match &event {
            Event::Start(element) if element.local_name().as_ref() == b"sheetData" => {
                in_sheet_data = true;
            }
            Event::End(element) if element.local_name().as_ref() == b"sheetData" => {
                in_sheet_data = false;
            }
            Event::Start(element) if in_sheet_data && element.local_name().as_ref() == b"row" => {
                if !include_row(element, opts) {
                    skip_depth = 1;
                    continue;
                }
            }
            Event::Empty(element) if in_sheet_data && element.local_name().as_ref() == b"row" => {
                if !include_row(element, opts) {
                    continue;
                }
            }
            Event::Start(element) if in_sheet_data && element.local_name().as_ref() == b"c" => {
                if !include_cell(element, allowed_columns.as_ref()) {
                    skip_depth = 1;
                    continue;
                }
            }
            Event::Empty(element) if in_sheet_data && element.local_name().as_ref() == b"c" => {
                if !include_cell(element, allowed_columns.as_ref()) {
                    continue;
                }
            }
            _ => {}
        }
        writer.write_event(event.into_owned()).map_err(|e| {
            HandlerError::OperationFailed(format!("worksheet XML write error: {}", e))
        })?;
    }

    String::from_utf8(writer.into_inner())
        .map_err(|e| HandlerError::OperationFailed(format!("worksheet XML UTF-8 error: {}", e)))
}

fn include_row(element: &BytesStart<'_>, opts: &RawOptions) -> bool {
    let Some(row) = attribute(element, b"r").and_then(|value| value.parse::<usize>().ok()) else {
        return true;
    };
    opts.start_row.is_none_or(|start| row >= start) && opts.end_row.is_none_or(|end| row <= end)
}

fn include_cell(
    element: &BytesStart<'_>,
    allowed_columns: Option<&std::collections::HashSet<String>>,
) -> bool {
    let Some(allowed_columns) = allowed_columns else {
        return true;
    };
    let Some(cell_ref) = attribute(element, b"r") else {
        return true;
    };
    let column: String = cell_ref
        .chars()
        .take_while(|character| character.is_ascii_alphabetic())
        .collect();
    column.is_empty() || allowed_columns.contains(&column.to_ascii_uppercase())
}

fn attribute(element: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    element
        .attributes()
        .filter_map(|attribute| attribute.ok())
        .find(|attribute| attribute.key.as_ref() == name)
        .map(|attribute| String::from_utf8_lossy(attribute.value.as_ref()).to_string())
}

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

/// Restore one edge of an arbitrary XLSX OOXML relationship graph emitted by
/// `officecli dump`.  It intentionally does not interpret the resource type:
/// chart caches, richData, embedded workbooks, drawings and external links all
/// use the same package-level relationship representation.
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
        .split_once(",")
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
