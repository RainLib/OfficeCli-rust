use crate::dom_types::{WordDom, WordElementType, WordNode};
use crate::helpers::{build_paragraph_properties, build_run_properties};
use crate::navigation::{navigate_to_element, navigate_to_element_mut, parse_path};
use handler_common::{
    self, extract_find_replace_props, find_all_replacements, find_all_spans,
    find_replace_property_keys, replace_in_string, DocumentNode, FindReplaceOptions, HandlerError,
    InsertPosition,
};
use oxml::OxmlPackage;
use std::collections::HashMap;

const DOCX_COMMENTS_PART: &str = "word/comments.xml";
const DOCX_DOCUMENT_PART: &str = "word/document.xml";
const DOCX_DOCUMENT_RELS_PART: &str = "word/_rels/document.xml.rels";
const DOCX_COMMENTS_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";
const DOCX_COMMENTS_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml";
const DOCX_COMMENTS_EXT_PART: &str = "word/commentsExtended.xml";
const DOCX_COMMENTS_EXT_REL_TYPE: &str =
    "http://schemas.microsoft.com/office/2011/relationships/commentsExtended";
const DOCX_COMMENTS_EXT_CONTENT_TYPE: &str = "application/vnd.ms-word.commentsExt+xml";
const W14_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
const W15_NS: &str = "http://schemas.microsoft.com/office/word/2012/wordml";
const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const WP_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const WPG_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup";
const WPS_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DrawingShapeKind {
    Shape,
    Textbox,
}

const DOCX_NUMBERING_PART: &str = "word/numbering.xml";
const DOCX_NUMBERING_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering";
const DOCX_NUMBERING_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml";

/// Create a numbering template or instance while maintaining the package parts
/// Word requires.  The template emits all nine OOXML levels so it remains a
/// valid reusable abstractNum even when callers initially use only level zero.
pub fn add_numbering_definition(
    package: &mut OxmlPackage,
    parent: &str,
    element_type: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    if parent != "/numbering" {
        return Err(HandlerError::InvalidPath(format!(
            "{} must be added under /numbering",
            element_type
        )));
    }
    let xml = package.read_part_xml(DOCX_NUMBERING_PART).unwrap_or_else(|_| {
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:numbering xmlns:w=\"{}\"></w:numbering>", W_NS)
    });
    let is_abstract = element_type.eq_ignore_ascii_case("abstractNum");
    let (fragment, path) = if is_abstract {
        let id = properties
            .get("id")
            .map(String::as_str)
            .map(parse_numbering_id)
            .transpose()?
            .unwrap_or_else(|| next_numbering_id(&xml, "abstractNumId"));
        if xml.contains(&format!("w:abstractNumId=\"{}\"", id)) {
            return Err(HandlerError::InvalidArgument(format!(
                "abstractNumId {} already exists",
                id
            )));
        }
        (
            build_abstract_num(id, properties)?,
            format!("/numbering/abstractNum[@id={}]", id),
        )
    } else {
        let abs_id = properties
            .get("abstractNumId")
            .ok_or_else(|| HandlerError::InvalidArgument("num requires abstractNumId".to_string()))
            .and_then(|v| parse_numbering_id(v))?;
        if !xml.contains(&format!("w:abstractNumId=\"{}\"", abs_id)) {
            return Err(HandlerError::InvalidArgument(format!(
                "abstractNumId={} not found",
                abs_id
            )));
        }
        let id = properties
            .get("id")
            .map(String::as_str)
            .map(parse_numbering_id)
            .transpose()?
            .unwrap_or_else(|| next_numbering_id(&xml, "numId").max(1));
        if xml.contains(&format!("w:numId=\"{}\"", id)) {
            return Err(HandlerError::InvalidArgument(format!(
                "numId {} already exists",
                id
            )));
        }
        (
            build_num(id, abs_id, &xml, properties)?,
            format!("/numbering/num[@id={}]", id),
        )
    };
    let close = "</w:numbering>";
    let pos = xml
        .find(close)
        .ok_or_else(|| HandlerError::OperationFailed("invalid numbering.xml".to_string()))?;
    let mut updated = xml;
    updated.insert_str(pos, &fragment);
    package
        .write_part_xml(DOCX_NUMBERING_PART, &updated)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    ensure_numbering_package_wiring(package)?;
    Ok(path)
}

/// Add or replace one `w:lvl` beneath an existing numbering template.  Levels
/// are keyed by their OOXML `ilvl`, rather than their physical child position:
/// this mirrors Word's semantics and avoids creating duplicate counters when a
/// template was initially seeded with all nine default levels.
pub fn add_numbering_level(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let abstract_num_id = parent
        .strip_prefix("/numbering/abstractNum[@id=")
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(|| {
            HandlerError::InvalidPath(
                "level must be added under /numbering/abstractNum[@id=N]".to_string(),
            )
        })?;
    parse_numbering_id(abstract_num_id)?;
    let level = properties
        .get("ilvl")
        .ok_or_else(|| HandlerError::InvalidArgument("level requires ilvl=0..8".to_string()))
        .and_then(|value| parse_level_index(value, "ilvl"))?;
    let xml = package
        .read_part_xml(DOCX_NUMBERING_PART)
        .map_err(|_| HandlerError::PathNotFound("numbering definition not found".to_string()))?;
    let (abstract_start, abstract_end) = numbering_abstract_bounds(&xml, abstract_num_id)?;
    let level_xml = build_numbering_level(level, properties)?;
    let mut updated = xml;
    if let Some((start, end)) =
        numbering_level_bounds(&updated, abstract_start, abstract_end, level)?
    {
        updated.replace_range(start..end, &level_xml);
    } else {
        let insert_at = abstract_end - "</w:abstractNum>".len();
        updated.insert_str(insert_at, &level_xml);
    }
    package
        .write_part_xml(DOCX_NUMBERING_PART, &updated)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(format!(
        "/numbering/abstractNum[@id={}]/level[{}]",
        abstract_num_id, level
    ))
}

/// Remove one level definition without touching its containing template or
/// numbering instances.  A `w:num` may legitimately keep pointing at the
/// template; Word then applies its normal fallback for that missing level.
pub fn remove_numbering_level(package: &mut OxmlPackage, path: &str) -> Result<(), HandlerError> {
    let (abstract_num_id, level) = parse_numbering_level_path(path)?;
    let xml = package
        .read_part_xml(DOCX_NUMBERING_PART)
        .map_err(|_| HandlerError::PathNotFound("numbering definition not found".to_string()))?;
    let (abstract_start, abstract_end) = numbering_abstract_bounds(&xml, abstract_num_id)?;
    let (start, end) = numbering_level_bounds(&xml, abstract_start, abstract_end, level)?
        .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?;
    let mut updated = xml;
    updated.replace_range(start..end, "");
    package
        .write_part_xml(DOCX_NUMBERING_PART, &updated)
        .map_err(|error| HandlerError::SaveError(error.to_string()))
}

/// Update a numbering instance's template reference without permitting a
/// dangling `w:abstractNumId` pointer.
pub fn set_numbering_definition(
    package: &mut OxmlPackage,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    if path.starts_with("/numbering/abstractNum[@id=") {
        return set_abstract_numbering_definition(package, path, properties);
    }
    let id = path
        .strip_prefix("/numbering/num[@id=")
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(|| {
            HandlerError::UnsupportedProperty(
                "only /numbering/num[@id=N] is currently settable".to_string(),
            )
        })?;
    parse_numbering_id(id)?;
    let xml = package
        .read_part_xml(DOCX_NUMBERING_PART)
        .map_err(|_| HandlerError::PathNotFound("numbering definition not found".to_string()))?;
    let start_marker = format!("<w:num w:numId=\"{}\"", id);
    let start = xml
        .find(&start_marker)
        .ok_or_else(|| HandlerError::PathNotFound(format!("numId {} not found", id)))?;
    let end = xml[start..]
        .find("</w:num>")
        .map(|offset| start + offset + "</w:num>".len())
        .ok_or_else(|| {
            HandlerError::OperationFailed("invalid numbering num element".to_string())
        })?;
    let mut updated_block = xml[start..end].to_string();
    if let Some(target) = properties.get("abstractNumId") {
        parse_numbering_id(target)?;
        if !xml.contains(&format!("w:abstractNumId=\"{}\"", target)) {
            return Err(HandlerError::InvalidArgument(format!(
                "abstractNumId={} not found",
                target
            )));
        }
        let old = updated_block
            .find("<w:abstractNumId w:val=\"")
            .ok_or_else(|| HandlerError::OperationFailed("num has no abstractNumId".to_string()))?;
        let value_start = old + "<w:abstractNumId w:val=\"".len();
        let value_end = updated_block[value_start..]
            .find('"')
            .map(|offset| value_start + offset)
            .ok_or_else(|| HandlerError::OperationFailed("invalid abstractNumId".to_string()))?;
        updated_block.replace_range(value_start..value_end, target);
    }
    for (level, value) in start_overrides(properties)? {
        set_start_override(&mut updated_block, level, value)?;
    }
    let mut updated = xml;
    updated.replace_range(start..end, &updated_block);
    package
        .write_part_xml(DOCX_NUMBERING_PART, &updated)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(properties
        .keys()
        .filter(|key| !is_num_property(key))
        .cloned()
        .collect())
}

pub fn set_numbering_level(
    package: &mut OxmlPackage,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let target = path
        .strip_prefix("/numbering/abstractNum[@id=")
        .and_then(|value| value.split_once("]/"))
        .and_then(|(id, rest)| {
            rest.strip_prefix("level[")
                .and_then(|level| level.strip_suffix(']'))
                .map(|level| (id, level))
        })
        .ok_or_else(|| HandlerError::InvalidPath(path.to_string()))?;
    let (abstract_num_id, level) = target;
    parse_numbering_id(abstract_num_id)?;
    parse_numbering_id(level)?;
    let xml = package
        .read_part_xml(DOCX_NUMBERING_PART)
        .map_err(|_| HandlerError::PathNotFound("numbering definition not found".to_string()))?;
    let abstract_marker = format!("<w:abstractNum w:abstractNumId=\"{}\"", abstract_num_id);
    let abstract_start = xml
        .find(&abstract_marker)
        .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?;
    let abstract_end = xml[abstract_start..]
        .find("</w:abstractNum>")
        .map(|n| abstract_start + n + "</w:abstractNum>".len())
        .ok_or_else(|| HandlerError::OperationFailed("invalid abstractNum".to_string()))?;
    let marker = format!("<w:lvl w:ilvl=\"{}\"", level);
    let start = xml[abstract_start..abstract_end]
        .find(&marker)
        .map(|n| abstract_start + n)
        .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?;
    let end = xml[start..abstract_end]
        .find("</w:lvl>")
        .map(|n| start + n + 8)
        .ok_or_else(|| HandlerError::OperationFailed("invalid level".to_string()))?;
    let mut block = xml[start..end].to_string();
    for (key, tag) in [("format", "numFmt"), ("text", "lvlText")] {
        if let Some(value) = properties.get(key) {
            let token = format!("<w:{} w:val=\"", tag);
            let pos = block
                .find(&token)
                .ok_or_else(|| HandlerError::OperationFailed(format!("level missing {}", tag)))?
                + token.len();
            let close = block[pos..].find('"').map(|n| pos + n).unwrap();
            block.replace_range(pos..close, &escape_attr(value));
        }
    }
    let mut updated = xml;
    updated.replace_range(start..end, &block);
    package
        .write_part_xml(DOCX_NUMBERING_PART, &updated)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(properties
        .keys()
        .filter(|k| k.as_str() != "format" && k.as_str() != "text")
        .cloned()
        .collect())
}

fn parse_numbering_id(value: &str) -> Result<i32, HandlerError> {
    value
        .parse::<i32>()
        .map_err(|_| HandlerError::InvalidArgument(format!("invalid numbering id '{}'", value)))
}

fn parse_level_index(value: &str, property: &str) -> Result<u8, HandlerError> {
    let level = value.parse::<u8>().map_err(|_| {
        HandlerError::InvalidArgument(format!("{} must be an integer 0..8", property))
    })?;
    if level > 8 {
        return Err(HandlerError::InvalidArgument(format!(
            "{} must be 0..8 (got {})",
            property, level
        )));
    }
    Ok(level)
}

fn parse_numbering_level_path(path: &str) -> Result<(&str, u8), HandlerError> {
    let (abstract_num_id, level) = path
        .strip_prefix("/numbering/abstractNum[@id=")
        .and_then(|value| value.split_once("]/level["))
        .and_then(|(id, level)| level.strip_suffix(']').map(|level| (id, level)))
        .ok_or_else(|| HandlerError::InvalidPath(path.to_string()))?;
    parse_numbering_id(abstract_num_id)?;
    Ok((abstract_num_id, parse_level_index(level, "level")?))
}

fn numbering_abstract_bounds(xml: &str, id: &str) -> Result<(usize, usize), HandlerError> {
    let marker = format!("<w:abstractNum w:abstractNumId=\"{}\"", id);
    let start = xml
        .find(&marker)
        .ok_or_else(|| HandlerError::PathNotFound(format!("abstractNum {} not found", id)))?;
    let end = xml[start..]
        .find("</w:abstractNum>")
        .map(|offset| start + offset + "</w:abstractNum>".len())
        .ok_or_else(|| HandlerError::OperationFailed("invalid abstractNum".to_string()))?;
    Ok((start, end))
}

fn numbering_level_bounds(
    xml: &str,
    abstract_start: usize,
    abstract_end: usize,
    level: u8,
) -> Result<Option<(usize, usize)>, HandlerError> {
    let marker = format!("<w:lvl w:ilvl=\"{}\"", level);
    let Some(offset) = xml[abstract_start..abstract_end].find(&marker) else {
        return Ok(None);
    };
    let start = abstract_start + offset;
    let end = xml[start..abstract_end]
        .find("</w:lvl>")
        .map(|offset| start + offset + "</w:lvl>".len())
        .ok_or_else(|| HandlerError::OperationFailed("invalid level".to_string()))?;
    Ok(Some((start, end)))
}

fn build_numbering_level(
    level: u8,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let start = properties
        .get("start")
        .map(|value| parse_numbering_id(value))
        .transpose()?
        .unwrap_or(1);
    let format = properties
        .get("format")
        .or_else(|| properties.get("numFmt"))
        .map(String::as_str)
        .unwrap_or("decimal");
    let text = properties
        .get("lvlText")
        .or_else(|| properties.get("text"))
        .cloned()
        .unwrap_or_else(|| {
            if format.eq_ignore_ascii_case("bullet") {
                "•".to_string()
            } else {
                format!("%{}.", level + 1)
            }
        });
    let mut content = format!(
        "<w:start w:val=\"{}\"/><w:numFmt w:val=\"{}\"/>",
        start,
        escape_attr(format)
    );
    if let Some(value) = properties.get("lvlRestart") {
        content.push_str(&format!(
            "<w:lvlRestart w:val=\"{}\"/>",
            parse_numbering_id(value)?
        ));
    }
    if properties
        .get("isLgl")
        .is_some_and(|value| is_truthy_string(value))
    {
        content.push_str("<w:isLgl/>");
    }
    if let Some(value) = properties.get("suff") {
        let suffix = match value.to_ascii_lowercase().as_str() {
            "tab" | "space" | "nothing" | "none" => value,
            _ => {
                return Err(HandlerError::InvalidArgument(format!(
                    "invalid suff '{}': tab, space, or nothing expected",
                    value
                )))
            }
        };
        content.push_str(&format!("<w:suff w:val=\"{}\"/>", escape_attr(suffix)));
    }
    content.push_str(&format!("<w:lvlText w:val=\"{}\"/>", escape_attr(&text)));
    if let Some(value) = properties
        .get("justification")
        .or_else(|| properties.get("jc"))
    {
        let value = match value.to_ascii_lowercase().as_str() {
            "left" | "start" => "left",
            "center" => "center",
            "right" | "end" => "right",
            _ => {
                return Err(HandlerError::InvalidArgument(format!(
                    "invalid justification '{}'",
                    value
                )))
            }
        };
        content.push_str(&format!("<w:lvlJc w:val=\"{}\"/>", value));
    }
    let indent = properties
        .get("indent")
        .map(|value| parse_numbering_id(value))
        .transpose()?;
    let hanging = properties
        .get("hanging")
        .map(|value| parse_numbering_id(value))
        .transpose()?;
    let bidi = properties
        .get("direction")
        .or_else(|| properties.get("dir"))
        .or_else(|| properties.get("bidi"))
        .map(|value| match value.to_ascii_lowercase().as_str() {
            "rtl" | "righttoleft" | "right-to-left" | "true" | "1" => Ok(true),
            "ltr" | "lefttoright" | "left-to-right" | "false" | "0" | "" => Ok(false),
            _ => Err(HandlerError::InvalidArgument(format!(
                "invalid direction '{}'",
                value
            ))),
        })
        .transpose()?;
    if indent.is_some() || hanging.is_some() || bidi == Some(true) {
        content.push_str("<w:pPr>");
        if bidi == Some(true) {
            content.push_str("<w:bidi/>");
        }
        if indent.is_some() || hanging.is_some() {
            content.push_str("<w:ind");
            if let Some(value) = indent {
                content.push_str(&format!(" w:left=\"{}\"", value));
            }
            if let Some(value) = hanging {
                content.push_str(&format!(" w:hanging=\"{}\"", value));
            }
            content.push_str("/>");
        }
        content.push_str("</w:pPr>");
    }
    let mut run_properties = String::new();
    if let Some(value) = properties.get("font").filter(|value| !value.is_empty()) {
        let value = escape_attr(value);
        run_properties.push_str(&format!(
            "<w:rFonts w:ascii=\"{0}\" w:hAnsi=\"{0}\" w:eastAsia=\"{0}\"/>",
            value
        ));
    }
    if properties
        .get("bold")
        .is_some_and(|value| is_truthy_string(value))
    {
        run_properties.push_str("<w:b/>");
    }
    if properties
        .get("italic")
        .is_some_and(|value| is_truthy_string(value))
    {
        run_properties.push_str("<w:i/>");
    }
    if let Some(value) = properties.get("color").filter(|value| !value.is_empty()) {
        run_properties.push_str(&format!(
            "<w:color w:val=\"{}\"/>",
            escape_attr(value.trim_start_matches('#'))
        ));
    }
    if let Some(value) = properties.get("size").filter(|value| !value.is_empty()) {
        let points = value.parse::<f64>().map_err(|_| {
            HandlerError::InvalidArgument(format!("size must be a point value (got '{}')", value))
        })?;
        if !points.is_finite() {
            return Err(HandlerError::InvalidArgument(
                "size must be finite".to_string(),
            ));
        }
        run_properties.push_str(&format!("<w:sz w:val=\"{}\"/>", (points * 2.0).round()));
    }
    if !run_properties.is_empty() {
        content.push_str(&format!("<w:rPr>{}</w:rPr>", run_properties));
    }
    Ok(format!("<w:lvl w:ilvl=\"{}\">{}</w:lvl>", level, content))
}

fn is_truthy_string(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}
fn next_numbering_id(xml: &str, attr: &str) -> i32 {
    xml.split(&format!("w:{}=\"", attr))
        .skip(1)
        .filter_map(|tail| tail.split('"').next()?.parse().ok())
        .max()
        .unwrap_or(-1)
        + 1
}
fn build_abstract_num(id: i32, props: &HashMap<String, String>) -> Result<String, HandlerError> {
    let format = props.get("format").map(String::as_str).unwrap_or("decimal");
    let text = props.get("text").cloned().unwrap_or_else(|| {
        if format == "bullet" {
            "•".into()
        } else {
            "%1.".into()
        }
    });
    let mut levels = String::new();
    for level in 0..9 {
        let f = props
            .get(&format!("level{}.format", level))
            .map(String::as_str)
            .unwrap_or(format);
        let t = props
            .get(&format!("level{}.text", level))
            .cloned()
            .unwrap_or_else(|| {
                if level == 0 {
                    text.clone()
                } else {
                    format!("%{}. ", level + 1).trim().to_string()
                }
            });
        levels.push_str(&format!("<w:lvl w:ilvl=\"{}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{}\"/><w:lvlText w:val=\"{}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{}\" w:hanging=\"360\"/></w:pPr></w:lvl>",level,escape_attr(f),escape_attr(&t),(level+1)*720));
    }
    let multi_level_type = normalize_multi_level_type(
        props
            .get("type")
            .or_else(|| props.get("multiLevelType"))
            .map(String::as_str)
            .unwrap_or("hybridMultilevel"),
    )?;
    let mut header = format!("<w:multiLevelType w:val=\"{}\"/>", multi_level_type);
    for (property, tag) in [
        ("name", "name"),
        ("styleLink", "styleLink"),
        ("numStyleLink", "numStyleLink"),
    ] {
        if let Some(value) = props.get(property).filter(|value| !value.is_empty()) {
            header.push_str(&format!("<w:{} w:val=\"{}\"/>", tag, escape_attr(value)));
        }
    }
    Ok(format!(
        "<w:abstractNum w:abstractNumId=\"{}\">{}{}</w:abstractNum>",
        id, header, levels
    ))
}

/// Update template-scoped properties while retaining every level child in its
/// schema-prescribed position.  IDs are deliberately immutable because
/// renaming them would silently orphan every `w:num` reference.
fn set_abstract_numbering_definition(
    package: &mut OxmlPackage,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let id = path
        .strip_prefix("/numbering/abstractNum[@id=")
        .and_then(|value| value.strip_suffix(']'))
        .ok_or_else(|| HandlerError::InvalidPath(path.to_string()))?;
    parse_numbering_id(id)?;
    let xml = package
        .read_part_xml(DOCX_NUMBERING_PART)
        .map_err(|_| HandlerError::PathNotFound("numbering definition not found".to_string()))?;
    let (start, end) = numbering_abstract_bounds(&xml, id)?;
    let mut block = xml[start..end].to_string();
    let mut unsupported = Vec::new();
    if let Some(value) = properties
        .get("type")
        .or_else(|| properties.get("multiLevelType"))
    {
        upsert_abstract_header_value(
            &mut block,
            "multiLevelType",
            normalize_multi_level_type(value)?,
        )?;
    }
    for (property, tag) in [
        ("name", "name"),
        ("styleLink", "styleLink"),
        ("numStyleLink", "numStyleLink"),
    ] {
        if let Some(value) = properties.get(property) {
            upsert_abstract_header_value(&mut block, tag, escape_attr(value))?;
        }
    }
    for key in properties.keys() {
        if !matches!(
            key.as_str(),
            "type" | "multiLevelType" | "name" | "styleLink" | "numStyleLink"
        ) {
            unsupported.push(key.clone());
        }
    }
    let mut updated = xml;
    updated.replace_range(start..end, &block);
    package
        .write_part_xml(DOCX_NUMBERING_PART, &updated)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    Ok(unsupported)
}

fn normalize_multi_level_type(value: &str) -> Result<&'static str, HandlerError> {
    match value.to_ascii_lowercase().as_str() {
        "hybridmultilevel" | "hybrid" => Ok("hybridMultilevel"),
        "multilevel" | "multi" => Ok("multilevel"),
        "singlelevel" | "single" => Ok("singleLevel"),
        _ => Err(HandlerError::InvalidArgument(format!(
            "invalid multiLevelType '{}'",
            value
        ))),
    }
}

fn upsert_abstract_header_value(
    block: &mut String,
    tag: &str,
    value: impl AsRef<str>,
) -> Result<(), HandlerError> {
    let marker = format!("<w:{} ", tag);
    if let Some(start) = block.find(&marker) {
        let value_marker = "w:val=\"";
        let value_start = block[start..]
            .find(value_marker)
            .map(|offset| start + offset + value_marker.len())
            .ok_or_else(|| HandlerError::OperationFailed(format!("invalid {} element", tag)))?;
        let value_end = block[value_start..]
            .find('"')
            .map(|offset| value_start + offset)
            .ok_or_else(|| HandlerError::OperationFailed(format!("invalid {} value", tag)))?;
        block.replace_range(value_start..value_end, value.as_ref());
        return Ok(());
    }
    let order = ["multiLevelType", "name", "styleLink", "numStyleLink"];
    let current = order
        .iter()
        .position(|item| *item == tag)
        .ok_or_else(|| HandlerError::OperationFailed(format!("unknown abstractNum tag {}", tag)))?;
    let mut insert_at = block.find('>').ok_or_else(|| {
        HandlerError::OperationFailed("invalid abstractNum opening tag".to_string())
    })? + 1;
    for previous in &order[..current] {
        let previous_marker = format!("<w:{} ", previous);
        if let Some(start) = block.find(&previous_marker) {
            let end = block[start..]
                .find("/>")
                .map(|offset| start + offset + 2)
                .ok_or_else(|| {
                    HandlerError::OperationFailed("invalid abstractNum header".to_string())
                })?;
            insert_at = end;
        }
    }
    block.insert_str(
        insert_at,
        &format!("<w:{} w:val=\"{}\"/>", tag, value.as_ref()),
    );
    Ok(())
}

fn build_num(
    id: i32,
    abstract_num_id: i32,
    numbering_xml: &str,
    props: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let mut overrides = start_overrides(props)?;
    let continues = props
        .get("continue")
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
    if !continues && !overrides.iter().any(|(level, _)| *level == 0) {
        overrides.push((
            0,
            abstract_num_level_start(numbering_xml, abstract_num_id).unwrap_or(1),
        ));
    }
    overrides.sort_by_key(|(level, _)| *level);
    let override_xml: String = overrides
        .into_iter()
        .map(|(level, value)| {
            format!(
                "<w:lvlOverride w:ilvl=\"{}\"><w:startOverride w:val=\"{}\"/></w:lvlOverride>",
                level, value
            )
        })
        .collect();
    Ok(format!(
        "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"{}\"/>{}</w:num>",
        id, abstract_num_id, override_xml
    ))
}

fn start_overrides(props: &HashMap<String, String>) -> Result<Vec<(u8, i32)>, HandlerError> {
    let mut result = Vec::new();
    if let Some(value) = props.get("start") {
        result.push((0, parse_numbering_id(value)?));
    }
    for (key, value) in props {
        if let Some(level) = key.strip_prefix("startOverride.") {
            let level = level.parse::<u8>().map_err(|_| {
                HandlerError::InvalidArgument(format!("invalid startOverride level '{}'", level))
            })?;
            if level > 8 {
                return Err(HandlerError::InvalidArgument(format!(
                    "startOverride level must be 0..8 (got {})",
                    level
                )));
            }
            let value = parse_numbering_id(value)?;
            if let Some(existing) = result
                .iter_mut()
                .find(|(item_level, _)| *item_level == level)
            {
                existing.1 = value;
            } else {
                result.push((level, value));
            }
        }
    }
    Ok(result)
}

fn set_start_override(block: &mut String, level: u8, value: i32) -> Result<(), HandlerError> {
    let marker = format!("<w:lvlOverride w:ilvl=\"{}\"", level);
    if let Some(start) = block.find(&marker) {
        let end = block[start..]
            .find("</w:lvlOverride>")
            .map(|offset| start + offset + "</w:lvlOverride>".len())
            .ok_or_else(|| HandlerError::OperationFailed("invalid lvlOverride".to_string()))?;
        let override_block = &block[start..end];
        if let Some(value_start) = override_block.find("<w:startOverride w:val=\"") {
            let value_start = start + value_start + "<w:startOverride w:val=\"".len();
            let value_end = block[value_start..]
                .find('"')
                .map(|offset| value_start + offset)
                .ok_or_else(|| {
                    HandlerError::OperationFailed("invalid startOverride".to_string())
                })?;
            block.replace_range(value_start..value_end, &value.to_string());
        } else {
            let insert_at = end - "</w:lvlOverride>".len();
            block.insert_str(
                insert_at,
                &format!("<w:startOverride w:val=\"{}\"/>", value),
            );
        }
    } else {
        let insert_at = block.rfind("</w:num>").ok_or_else(|| {
            HandlerError::OperationFailed("invalid numbering num element".to_string())
        })?;
        block.insert_str(
            insert_at,
            &format!(
                "<w:lvlOverride w:ilvl=\"{}\"><w:startOverride w:val=\"{}\"/></w:lvlOverride>",
                level, value
            ),
        );
    }
    Ok(())
}

fn abstract_num_level_start(numbering_xml: &str, abstract_num_id: i32) -> Option<i32> {
    let marker = format!("<w:abstractNum w:abstractNumId=\"{}\"", abstract_num_id);
    let start = numbering_xml.find(&marker)?;
    let end = numbering_xml[start..].find("</w:abstractNum>")? + start;
    let abstract_num = &numbering_xml[start..end];
    let start = abstract_num.find("<w:lvl w:ilvl=\"0\"")?;
    let end = abstract_num[start..].find("</w:lvl>")? + start;
    let level_zero = &abstract_num[start..end];
    let value_start = level_zero.find("<w:start w:val=\"")? + "<w:start w:val=\"".len();
    let value_end = level_zero[value_start..].find('"')? + value_start;
    level_zero[value_start..value_end].parse().ok()
}

fn is_num_property(key: &str) -> bool {
    key == "abstractNumId"
        || key == "start"
        || key == "continue"
        || key
            .strip_prefix("startOverride.")
            .is_some_and(|level| level.parse::<u8>().is_ok_and(|level| level <= 8))
}

fn ensure_numbering_package_wiring(package: &mut OxmlPackage) -> Result<(), HandlerError> {
    let rels = package
        .read_part_xml(DOCX_DOCUMENT_RELS_PART)
        .unwrap_or_default();
    if !rels.contains(DOCX_NUMBERING_REL_TYPE) {
        let relation_id = next_docx_rel_id(package, DOCX_DOCUMENT_RELS_PART);
        inject_docx_relationship(
            package,
            DOCX_DOCUMENT_RELS_PART,
            &format!(
                "<Relationship Id=\"{}\" Type=\"{}\" Target=\"numbering.xml\"/>",
                relation_id, DOCX_NUMBERING_REL_TYPE
            ),
        )?;
    }
    let ct = package
        .read_part_xml("[Content_Types].xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    if !ct.contains("/word/numbering.xml") {
        let pos = ct.find("</Types>").ok_or_else(|| {
            HandlerError::OperationFailed("invalid [Content_Types].xml".to_string())
        })?;
        let mut updated = ct;
        updated.insert_str(
            pos,
            &format!(
                "<Override PartName=\"/word/numbering.xml\" ContentType=\"{}\"/>",
                DOCX_NUMBERING_CONTENT_TYPE
            ),
        );
        package
            .write_part_xml("[Content_Types].xml", &updated)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    }
    Ok(())
}

/// Set properties on an element at a given path.
/// Dispatches to element-type-specific handlers matching the C# WordHandler.Set.Dispatch
/// routing. Supported targets and their property vocabularies:
///
/// **Paragraph (p)**: text, style/pStyle, alignment/jc, indent, spacing, lineSpacing,
///   keepLines, keepNext, outlineLevel, numId, numLevel, border, shading, pageBreakBefore
///
/// **Run (r)**: text, bold/b, italic/i, underline/u, strike, font/fontFamily, size/fontSize,
///   color/fontColor, bgColor/highlight, shading/shd, caps, smallCaps, vanish/hidden,
///   kern, spacing, characterSpacing, border, emphasisMark, lang, rightToLeft, font.font*
///
/// **Text (t)**: text
///
/// **Table (tbl)**: style/tblStyle, width, border, shading, alignment, indent,
///   firstRow, lastRow, firstCol, lastCol, rowBandSize, colBandSize, layout
///
/// **Row (tr)**: height, cantSplit, tableHeader, hidden
///
/// **Cell (tc)**: text, width, shading, border, vAlign, vMerge, gridSpan, noWrap, textDirection
///
/// **Bookmark**: name, text, id
///
/// **SDT**: alias/name, tag, lock, text
///
/// **Section (sectPr)**: pageWidth, pageHeight, orientation, marginLeft/Right/Top/Bottom,
///   columns, headerDistance, footerDistance, gutter
///
/// **Body/document-level** (path "/"): protection, protectionEnforced, docDefaults, defaultTabStop
///
/// **Styles** (/styles/*): basedOn, next, name, qFormat, uiPriority, hidden,
///   pPr/rPr properties (routed through style element helpers)
///
/// **Comments**: text, author, initials, date
///
/// **Footnote/Endnote**: text
///
/// **Hyperlink**: url/target, tooltip
///
/// Returns list of unrecognized property keys (empty = all applied).
pub fn set_properties(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    // Find/replace short-circuit: when the property map carries `find` (+ optional
    // `replace`, `caseSensitive`, `wholeWord`, `regex`), operate on text content
    // instead of formatting props. Mirrors C# FindHelpers integration at Set.find=...
    if properties.contains_key("find") {
        return apply_find_replace(dom, path, properties);
    }

    let segments = parse_path(path)?;
    if segments.is_empty() {
        return Err(HandlerError::InvalidPath("empty path".to_string()));
    }

    // Path-based routing: some paths target special elements before
    // the last segment type is considered
    let path_str = path.to_lowercase();

    // Document-level properties (path "/" or "/body")
    if path_str == "/" || path_str == "/body" {
        return set_document_properties(dom, path, properties);
    }

    // Section properties (/sectPr or body/sectPr)
    if path_str.contains("sectpr") {
        return set_section_properties(dom, path, properties);
    }

    // Styles routing
    // Styles/comments/footnotes/endnotes are routed to part-aware setters
    // in handler.rs before parse_dom() is called, so they never reach here.

    // Hyperlink routing
    if path_str.contains("hyperlink") {
        return set_hyperlink_properties(dom, path, properties);
    }

    // SDT routing
    if path_str.contains("sdt") {
        return set_sdt_properties(dom, path, properties);
    }

    // Determine what type of element we're modifying
    let last_seg = &segments[segments.len() - 1];
    let target_type = last_seg.name.as_str();

    match target_type {
        "p" => set_paragraph_properties(dom, path, properties),
        "r" => set_run_properties(dom, path, properties),
        "t" => set_text_content(dom, path, properties),
        "tbl" => set_table_properties(dom, path, properties),
        "tr" => set_row_properties(dom, path, properties),
        "tc" => set_cell_properties(dom, path, properties),
        "bookmarkStart" => set_bookmark_properties(dom, path, properties),
        "bookmarkEnd" => set_bookmark_end_properties(dom, path, properties),
        "sdt" => set_sdt_properties(dom, path, properties),
        "sectPr" => set_section_properties(dom, path, properties),
        other => Err(HandlerError::UnsupportedProperty(format!(
            "cannot set properties on element type: {}",
            other
        ))),
    }
}

/// Set paragraph properties. Full vocabulary matching C# WordHandler.Set.Element:
/// text, style/pStyle, alignment/jc, indent (indentLeft, indentRight, firstLine, hanging),
/// spacing (spacingBefore, spacingAfter, lineSpacing), keepLines, keepNext, outlineLevel,
/// numId, numLevel, border, shading/shd, pageBreakBefore, widowControl
fn set_paragraph_properties(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    // Property keys that are paragraph-level (w:pPr) — everything else is
    // treated as run-level and applied to the paragraph's runs.
    const PARA_LEVEL_KEYS: &[&str] = &[
        "style",
        "pStyle",
        "alignment",
        "jc",
        "indentLeft",
        "indentRight",
        "indent",
        "firstLine",
        "hanging",
        "spacingBefore",
        "spacingAfter",
        "lineSpacing",
        "spacing",
        "keepLines",
        "keepNext",
        "outlineLevel",
        "numId",
        "numLevel",
        "listStyle",
        "border",
        "shading",
        "shd",
        "pageBreakBefore",
        "widowControl",
    ];

    // Run-level keys (forwarded to runs)
    const RUN_LEVEL_KEYS: &[&str] = &[
        "bold",
        "b",
        "italic",
        "i",
        "underline",
        "u",
        "strike",
        "strikeout",
        "font",
        "fontFamily",
        "size",
        "fontSize",
        "color",
        "fontColor",
        "bgColor",
        "highlight",
        "bg",
        "shading",
        "shd",
        "caps",
        "smallCaps",
        "vanish",
        "hidden",
        "kern",
        "characterSpacing",
        "emphasisMark",
        "lang",
        "rightToLeft",
    ];

    let para = navigate_to_element_mut(dom, path)?;

    // Check if text property is set — this changes paragraph text
    if let Some(new_text) = properties.get("text") {
        set_paragraph_text(para, new_text);
    }

    // Apply run-level properties to all runs in this paragraph
    let run_props: HashMap<String, String> = properties
        .iter()
        .filter(|(k, _)| RUN_LEVEL_KEYS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    if !run_props.is_empty() {
        for child in &mut para.children {
            if child.element_type == WordElementType::Run {
                apply_run_props_to_run(child, &run_props);
            } else if child.element_type == WordElementType::Hyperlink {
                // Apply to runs inside hyperlinks too
                for link_child in &mut child.children {
                    if link_child.element_type == WordElementType::Run {
                        apply_run_props_to_run(link_child, &run_props);
                    }
                }
            }
        }
    }

    // Apply paragraph-level properties to pPr
    let ppr_props: HashMap<String, String> = properties
        .iter()
        .filter(|(k, _)| PARA_LEVEL_KEYS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    if !ppr_props.is_empty() {
        // Merge with existing pPr instead of replacing entirely
        // Remove existing pPr if present (we'll rebuild it with merged props)
        let existing_ppr = para
            .children
            .iter()
            .find(|c| c.element_type == WordElementType::ParagraphProperties);
        let merged_props = if let Some(ppr_node) = existing_ppr {
            // Start with existing pPr children converted to props
            merge_ppr_into_props(ppr_node, &ppr_props)
        } else {
            ppr_props.clone()
        };

        para.children
            .retain(|c| c.element_type != WordElementType::ParagraphProperties);

        if let Some(new_ppr) = build_paragraph_properties(&merged_props) {
            para.children.insert(0, new_ppr);
        }
    }

    // Recognized = text + all PARA_LEVEL_KEYS + all RUN_LEVEL_KEYS
    let recognized: Vec<&str> = {
        let mut v = vec!["text"];
        v.extend_from_slice(PARA_LEVEL_KEYS);
        v.extend_from_slice(RUN_LEVEL_KEYS);
        v
    };
    let unsupported: Vec<String> = properties
        .keys()
        .filter(|k| !recognized.contains(&k.as_str()))
        .cloned()
        .collect();

    Ok(unsupported)
}

/// Apply run-level properties to a single w:r element.
/// Merges with existing rPr if present, otherwise creates a new rPr.
fn apply_run_props_to_run(run: &mut WordNode, props: &HashMap<String, String>) {
    // Find existing rPr
    let existing_rpr = run
        .children
        .iter()
        .find(|c| c.element_type == WordElementType::RunProperties);

    let merged_props = if let Some(rpr_node) = existing_rpr {
        merge_rpr_into_props(rpr_node, props)
    } else {
        props.clone()
    };

    run.children
        .retain(|c| c.element_type != WordElementType::RunProperties);

    if let Some(new_rpr) = build_run_properties(&merged_props) {
        run.children.insert(0, new_rpr);
    }
}

/// Merge existing pPr node children into the new properties map, preserving
/// properties that aren't being overwritten.
fn merge_ppr_into_props(
    ppr_node: &WordNode,
    new_props: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut merged = HashMap::new();

    // Extract existing pPr properties from child elements
    for child in &ppr_node.children {
        let name = match &child.element_type {
            WordElementType::Unknown(n) => n.as_str(),
            _ => continue,
        };
        match name {
            "pStyle" => {
                if let Some(val) = child.attributes.get("val") {
                    if !new_props.contains_key("style") && !new_props.contains_key("pStyle") {
                        merged.insert("pStyle".to_string(), val.clone());
                    }
                }
            }
            "jc" => {
                if let Some(val) = child.attributes.get("val") {
                    if !new_props.contains_key("alignment") && !new_props.contains_key("jc") {
                        merged.insert("jc".to_string(), val.clone());
                    }
                }
            }
            "ind" => {
                for (attr, key) in [
                    ("left", "indentLeft"),
                    ("right", "indentRight"),
                    ("firstLine", "firstLine"),
                    ("hanging", "hanging"),
                ] {
                    if let Some(val) = child.attributes.get(attr) {
                        if !new_props.contains_key(key) {
                            merged.insert(key.to_string(), val.clone());
                        }
                    }
                }
            }
            "spacing" => {
                for (attr, key) in [
                    ("before", "spacingBefore"),
                    ("after", "spacingAfter"),
                    ("line", "lineSpacing"),
                ] {
                    if let Some(val) = child.attributes.get(attr) {
                        if !new_props.contains_key(key) {
                            merged.insert(key.to_string(), val.clone());
                        }
                    }
                }
            }
            "keepLines" => {
                if !new_props.contains_key("keepLines") {
                    merged.insert("keepLines".to_string(), "true".to_string());
                }
            }
            "keepNext" => {
                if !new_props.contains_key("keepNext") {
                    merged.insert("keepNext".to_string(), "true".to_string());
                }
            }
            "pageBreakBefore" => {
                if !new_props.contains_key("pageBreakBefore") {
                    merged.insert("pageBreakBefore".to_string(), "true".to_string());
                }
            }
            "widowControl" => {
                if let Some(val) = child.attributes.get("val") {
                    if !new_props.contains_key("widowControl") {
                        merged.insert("widowControl".to_string(), val.clone());
                    }
                }
            }
            "outlineLvl" => {
                if let Some(val) = child.attributes.get("val") {
                    if !new_props.contains_key("outlineLevel") {
                        merged.insert("outlineLevel".to_string(), val.clone());
                    }
                }
            }
            "numPr" => {
                for nc in &child.children {
                    let nc_name = match &nc.element_type {
                        WordElementType::Unknown(n) => n.as_str(),
                        _ => continue,
                    };
                    if nc_name == "numId" {
                        if let Some(val) = nc.attributes.get("val") {
                            if !new_props.contains_key("numId") {
                                merged.insert("numId".to_string(), val.clone());
                            }
                        }
                    }
                    if nc_name == "ilvl" {
                        if let Some(val) = nc.attributes.get("val") {
                            if !new_props.contains_key("numLevel") {
                                merged.insert("numLevel".to_string(), val.clone());
                            }
                        }
                    }
                }
            }
            "pBdr" => {
                // Preserve existing border if not overwritten
                if !new_props.contains_key("border") {
                    merged.insert("border".to_string(), "preserve".to_string());
                }
            }
            "shd" => {
                if !new_props.contains_key("shading") && !new_props.contains_key("shd") {
                    let fill = child.attributes.get("fill").cloned().unwrap_or_default();
                    let pat = child
                        .attributes
                        .get("val")
                        .cloned()
                        .unwrap_or("clear".to_string());
                    let clr = child
                        .attributes
                        .get("color")
                        .cloned()
                        .unwrap_or("auto".to_string());
                    merged.insert("shd".to_string(), format!("{};{};{}", pat, fill, clr));
                }
            }
            _ => {}
        }
    }

    // Add all new properties (these override existing ones)
    for (k, v) in new_props {
        merged.insert(k.clone(), v.clone());
    }

    merged
}

/// Set paragraph text by replacing all runs with a single run containing the new text.
fn set_paragraph_text(para: &mut WordNode, new_text: &str) {
    // Remove all existing runs (and hyperlinks that contain runs)
    para.children.retain(|c| {
        c.element_type != WordElementType::Run
            && c.element_type != WordElementType::Hyperlink
            && c.element_type != WordElementType::BookmarkStart
            && c.element_type != WordElementType::BookmarkEnd
    });

    // Add a new run with the text
    let text_node = if new_text.starts_with(' ') || new_text.ends_with(' ') {
        let mut tn = WordNode::new(WordElementType::Text).with_text(new_text);
        tn.attributes
            .insert("xml:space".to_string(), "preserve".to_string());
        tn.preserve_space = true;
        tn
    } else {
        WordNode::new(WordElementType::Text).with_text(new_text)
    };

    let run = WordNode::new(WordElementType::Run).with_children(vec![text_node]);

    para.children.push(run);
}

/// Set run properties. Full vocabulary matching C# WordHandler.Set.Element:
/// text, bold/b, italic/i, underline/u, strike/strikeout, font/fontFamily, size/fontSize,
/// color/fontColor, bgColor/highlight/bg, shading/shd, caps, smallCaps, vanish/hidden,
/// kern, spacing, characterSpacing, border, emphasisMark, lang, rightToLeft,
/// font.fontName, font.bold, font.italic, font.size, font.color, font.underline, font.strike
fn set_run_properties(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let run = navigate_to_element_mut(dom, path)?;

    // Check if text property is set
    if let Some(new_text) = properties.get("text") {
        let text_children: Vec<usize> = run
            .children
            .iter()
            .enumerate()
            .filter(|(_, c)| c.element_type == WordElementType::Text)
            .map(|(i, _)| i)
            .collect();

        if text_children.is_empty() {
            let text_node = if new_text.starts_with(' ') || new_text.ends_with(' ') {
                let mut tn = WordNode::new(WordElementType::Text).with_text(new_text);
                tn.attributes
                    .insert("xml:space".to_string(), "preserve".to_string());
                tn.preserve_space = true;
                tn
            } else {
                WordNode::new(WordElementType::Text).with_text(new_text)
            };
            run.children.push(text_node);
        } else {
            for idx in text_children {
                run.children[idx].text_content = Some(new_text.to_string());
                if new_text.starts_with(' ') || new_text.ends_with(' ') {
                    run.children[idx]
                        .attributes
                        .insert("xml:space".to_string(), "preserve".to_string());
                    run.children[idx].preserve_space = true;
                }
            }
        }
    }

    // Build or replace run properties
    let run_props: HashMap<String, String> = properties
        .iter()
        .filter(|(k, _)| k.as_str() != "text")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    if !run_props.is_empty() {
        // Merge with existing rPr instead of replacing entirely
        let existing_rpr = run
            .children
            .iter()
            .find(|c| c.element_type == WordElementType::RunProperties);
        let merged_props = if let Some(rpr_node) = existing_rpr {
            merge_rpr_into_props(rpr_node, &run_props)
        } else {
            run_props.clone()
        };

        run.children
            .retain(|c| c.element_type != WordElementType::RunProperties);

        if let Some(new_rpr) = build_run_properties(&merged_props) {
            run.children.insert(0, new_rpr);
        }
    }

    let recognized = [
        "text",
        "bold",
        "b",
        "italic",
        "i",
        "underline",
        "u",
        "strike",
        "strikeout",
        "font",
        "fontFamily",
        "size",
        "fontSize",
        "color",
        "fontColor",
        "bgColor",
        "highlight",
        "bg",
        "shading",
        "shd",
        "caps",
        "smallCaps",
        "vanish",
        "hidden",
        "kern",
        "spacing",
        "characterSpacing",
        "border",
        "emphasisMark",
        "lang",
        "rightToLeft",
        "font.bold",
        "font.italic",
        "font.size",
        "font.color",
        "font.underline",
        "font.strike",
        "font.name",
    ];
    let unsupported: Vec<String> = properties
        .keys()
        .filter(|k| !recognized.contains(&k.as_str()))
        .cloned()
        .collect();

    Ok(unsupported)
}

/// Merge existing rPr node children into the new properties map.
fn merge_rpr_into_props(
    rpr_node: &WordNode,
    new_props: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut merged = HashMap::new();

    for child in &rpr_node.children {
        let name = match &child.element_type {
            WordElementType::Unknown(n) => n.as_str(),
            _ => continue,
        };
        match name {
            "rFonts" => {
                if let Some(val) = child.attributes.get("ascii") {
                    if !new_props.contains_key("font") && !new_props.contains_key("fontFamily") {
                        merged.insert("font".to_string(), val.clone());
                    }
                }
            }
            "sz" => {
                if let Some(val) = child.attributes.get("val") {
                    if !new_props.contains_key("size") && !new_props.contains_key("fontSize") {
                        merged.insert("fontSize".to_string(), val.clone());
                    }
                }
            }
            "color" => {
                if let Some(val) = child.attributes.get("val") {
                    if !new_props.contains_key("color") && !new_props.contains_key("fontColor") {
                        merged.insert("color".to_string(), val.clone());
                    }
                }
            }
            "b" => {
                if !new_props.contains_key("bold") && !new_props.contains_key("b") {
                    merged.insert("bold".to_string(), "true".to_string());
                }
            }
            "i" => {
                if !new_props.contains_key("italic") && !new_props.contains_key("i") {
                    merged.insert("italic".to_string(), "true".to_string());
                }
            }
            "u" => {
                if let Some(val) = child.attributes.get("val") {
                    if !new_props.contains_key("underline") && !new_props.contains_key("u") {
                        merged.insert("underline".to_string(), val.clone());
                    }
                }
            }
            "strike" => {
                if !new_props.contains_key("strike") && !new_props.contains_key("strikeout") {
                    merged.insert("strike".to_string(), "true".to_string());
                }
            }
            "highlight" => {
                if let Some(val) = child.attributes.get("val") {
                    if !new_props.contains_key("highlight") && !new_props.contains_key("bgColor") {
                        merged.insert("highlight".to_string(), val.clone());
                    }
                }
            }
            "shd" => {
                if let Some(fill) = child.attributes.get("fill") {
                    if !new_props.contains_key("shading") && !new_props.contains_key("shd") {
                        let pat = child
                            .attributes
                            .get("val")
                            .cloned()
                            .unwrap_or("clear".to_string());
                        let clr = child
                            .attributes
                            .get("color")
                            .cloned()
                            .unwrap_or("auto".to_string());
                        merged.insert("shd".to_string(), format!("{};{};{}", pat, fill, clr));
                    }
                }
            }
            "caps" => {
                if !new_props.contains_key("caps") {
                    merged.insert("caps".to_string(), "true".to_string());
                }
            }
            "smallCaps" => {
                if !new_props.contains_key("smallCaps") {
                    merged.insert("smallCaps".to_string(), "true".to_string());
                }
            }
            "vanish" => {
                if !new_props.contains_key("vanish") && !new_props.contains_key("hidden") {
                    merged.insert("hidden".to_string(), "true".to_string());
                }
            }
            "kern" => {
                if let Some(val) = child.attributes.get("val") {
                    if !new_props.contains_key("kern") {
                        merged.insert("kern".to_string(), val.clone());
                    }
                }
            }
            "spacing" => {
                if let Some(val) = child.attributes.get("val") {
                    if !new_props.contains_key("characterSpacing") {
                        merged.insert("characterSpacing".to_string(), val.clone());
                    }
                }
            }
            "lang" => {
                if let Some(val) = child.attributes.get("val") {
                    if !new_props.contains_key("lang") {
                        merged.insert("lang".to_string(), val.clone());
                    }
                }
            }
            "rtl" => {
                if !new_props.contains_key("rightToLeft") {
                    merged.insert("rightToLeft".to_string(), "true".to_string());
                }
            }
            "em" => {
                if let Some(val) = child.attributes.get("val") {
                    if !new_props.contains_key("emphasisMark") {
                        merged.insert("emphasisMark".to_string(), val.clone());
                    }
                }
            }
            _ => {}
        }
    }

    for (k, v) in new_props {
        merged.insert(k.clone(), v.clone());
    }

    merged
}

/// Set text content directly on a w:t element.
fn set_text_content(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    if let Some(new_text) = properties.get("text") {
        let text_node = navigate_to_element_mut(dom, path)?;
        text_node.text_content = Some(new_text.to_string());
        if new_text.starts_with(' ') || new_text.ends_with(' ') {
            text_node
                .attributes
                .insert("xml:space".to_string(), "preserve".to_string());
            text_node.preserve_space = true;
        }
        let unsupported: Vec<String> = properties
            .keys()
            .filter(|k| k.as_str() != "text")
            .cloned()
            .collect();
        Ok(unsupported)
    } else {
        Err(HandlerError::UnsupportedProperty(
            "text node only supports 'text' property".to_string(),
        ))
    }
}

/// Set table properties. Expanded vocabulary:
/// style/tblStyle, width, alignment/jc, indent, border, shading/shd,
/// firstRow, lastRow, firstCol, lastCol, rowBandSize, colBandSize, layout
fn set_table_properties(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let table = navigate_to_element_mut(dom, path)?;

    // Remove existing tblPr
    table
        .children
        .retain(|c| c.element_type != WordElementType::TableProperties);

    let mut tbl_pr = WordNode::new(WordElementType::TableProperties);
    let mut children = Vec::new();

    for (key, value) in properties {
        match key.as_str() {
            "style" | "tblStyle" => {
                let tbl_style = WordNode::new(WordElementType::Unknown("tblStyle".to_string()))
                    .with_attribute("val", value.as_str());
                children.push(tbl_style);
            }
            "width" => {
                let tbl_w = WordNode::new(WordElementType::Unknown("tblW".to_string()))
                    .with_attribute("w", value.as_str())
                    .with_attribute("type", "dxa");
                children.push(tbl_w);
            }
            "alignment" | "jc" => {
                let jc = WordNode::new(WordElementType::Unknown("jc".to_string()))
                    .with_attribute("val", value.as_str());
                children.push(jc);
            }
            "indent" | "tblInd" => {
                let tbl_ind = WordNode::new(WordElementType::Unknown("tblInd".to_string()))
                    .with_attribute("w", value.as_str())
                    .with_attribute("type", "dxa");
                children.push(tbl_ind);
            }
            "layout" | "tblLayout" => {
                let layout = WordNode::new(WordElementType::Unknown("tblLayout".to_string()))
                    .with_attribute("type", value.as_str());
                children.push(layout);
            }
            "firstRow" => {
                let look = build_tbl_look_entry("firstRow", value);
                children.push(look);
            }
            "lastRow" => {
                let look = build_tbl_look_entry("lastRow", value);
                children.push(look);
            }
            "firstCol" => {
                let look = build_tbl_look_entry("firstCol", value);
                children.push(look);
            }
            "lastCol" => {
                let look = build_tbl_look_entry("lastCol", value);
                children.push(look);
            }
            "rowBandSize" | "bandSize" => {
                let band =
                    WordNode::new(WordElementType::Unknown("tblStyleRowBandSize".to_string()))
                        .with_attribute("val", value.as_str());
                children.push(band);
            }
            "colBandSize" => {
                let band =
                    WordNode::new(WordElementType::Unknown("tblStyleColBandSize".to_string()))
                        .with_attribute("val", value.as_str());
                children.push(band);
            }
            "shading" | "shd" => {
                let shd = build_shd_node(value);
                children.push(shd);
            }
            "border" | "borders" | "tblBorders" => {
                let borders = build_table_borders(value);
                children.push(borders);
            }
            _ => {}
        }
    }

    if !children.is_empty() {
        tbl_pr.children = children;
        table.children.insert(0, tbl_pr);
    }

    let recognized = [
        "style",
        "tblStyle",
        "width",
        "alignment",
        "jc",
        "indent",
        "tblInd",
        "layout",
        "tblLayout",
        "firstRow",
        "lastRow",
        "firstCol",
        "lastCol",
        "rowBandSize",
        "colBandSize",
        "bandSize",
        "shading",
        "shd",
        "border",
        "borders",
        "tblBorders",
    ];
    let unsupported: Vec<String> = properties
        .keys()
        .filter(|k| !recognized.contains(&k.as_str()))
        .cloned()
        .collect();

    Ok(unsupported)
}

fn build_tbl_look_entry(attr: &str, value: &str) -> WordNode {
    let val = if value == "true" || value == "1" {
        "1"
    } else {
        "0"
    };
    WordNode::new(WordElementType::Unknown("tblLook".to_string())).with_attribute(attr, val)
}

pub fn build_shd_node(value: &str) -> WordNode {
    let value = value.strip_prefix('#').unwrap_or(value);
    let parts: Vec<&str> = value.split(';').collect();
    let (pat, fill, clr) = match parts.len() {
        3 => (parts[0], parts[1], parts[2]),
        2 => ("clear", parts[0], parts[1]),
        _ => ("clear", value, "auto"),
    };
    WordNode::new(WordElementType::Unknown("shd".to_string()))
        .with_attribute("val", pat)
        .with_attribute("color", clr)
        .with_attribute("fill", fill)
}

pub fn build_table_borders(value: &str) -> WordNode {
    let mut tbl_bdr = WordNode::new(WordElementType::Unknown("tblBorders".to_string()));
    let mut children = Vec::new();
    // Format: "top=single;bottom=single;left=none;right=none;insideH=single;insideV=single"
    // Or shorthand: "all=single" or "none"
    if value == "none" || value == "0" {
        for border_name in ["top", "bottom", "left", "right", "insideH", "insideV"] {
            children.push(
                WordNode::new(WordElementType::Unknown(border_name.to_string()))
                    .with_attribute("val", "none")
                    .with_attribute("sz", "0")
                    .with_attribute("space", "0")
                    .with_attribute("color", "auto"),
            );
        }
    } else if value.starts_with("all=") || value == "single" || value == "thin" {
        let style = value.strip_prefix("all=").unwrap_or("single");
        for border_name in ["top", "bottom", "left", "right", "insideH", "insideV"] {
            children.push(
                WordNode::new(WordElementType::Unknown(border_name.to_string()))
                    .with_attribute("val", style)
                    .with_attribute("sz", "4")
                    .with_attribute("space", "0")
                    .with_attribute("color", "auto"),
            );
        }
    } else {
        // Parse per-border format
        for pair in value.split(';') {
            if let Some(eq) = pair.find('=') {
                let name = &pair[..eq];
                let style = &pair[eq + 1..];
                let sz = match style {
                    "double" => "4",
                    "thick" => "12",
                    "dashed" => "4",
                    _ => "4",
                };
                children.push(
                    WordNode::new(WordElementType::Unknown(name.to_string()))
                        .with_attribute("val", style)
                        .with_attribute("sz", sz)
                        .with_attribute("space", "0")
                        .with_attribute("color", "auto"),
                );
            }
        }
    }
    tbl_bdr.children = children;
    tbl_bdr
}

/// Set row properties. Expanded: height, cantSplit, tableHeader, hidden
fn set_row_properties(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let row = navigate_to_element_mut(dom, path)?;
    row.children
        .retain(|c| c.element_type != WordElementType::TableRowProperties);

    let mut tr_pr = WordNode::new(WordElementType::TableRowProperties);
    let mut children = Vec::new();

    if let Some(height) = properties.get("height") {
        let h_rule = properties
            .get("hRule")
            .cloned()
            .unwrap_or_else(|| "atLeast".to_string());
        let tr_height = WordNode::new(WordElementType::Unknown("trHeight".to_string()))
            .with_attribute("val", height.as_str())
            .with_attribute("hRule", h_rule.as_str());
        children.push(tr_height);
    }

    if let Some(val) = properties.get("cantSplit") {
        if val == "true" || val == "1" {
            children.push(WordNode::new(WordElementType::Unknown(
                "cantSplit".to_string(),
            )));
        }
    }

    if let Some(val) = properties.get("tableHeader") {
        let tf_val = if val == "true" || val == "1" {
            "true"
        } else {
            "false"
        };
        children.push(
            WordNode::new(WordElementType::Unknown("tblHeader".to_string()))
                .with_attribute("val", tf_val),
        );
    }

    if let Some(val) = properties.get("hidden") {
        let h_val = if val == "true" || val == "1" {
            "true"
        } else {
            "false"
        };
        children.push(
            WordNode::new(WordElementType::Unknown("hidden".to_string()))
                .with_attribute("val", h_val),
        );
    }

    if !children.is_empty() {
        tr_pr.children = children;
        row.children.insert(0, tr_pr);
    }

    let recognized = ["height", "hRule", "cantSplit", "tableHeader", "hidden"];
    let unsupported: Vec<String> = properties
        .keys()
        .filter(|k| !recognized.contains(&k.as_str()))
        .cloned()
        .collect();

    Ok(unsupported)
}

/// Set cell properties. Expanded vocabulary:
/// text, width, shading/shd, border, vAlign, vMerge, gridSpan, noWrap, textDirection
fn set_cell_properties(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let cell = navigate_to_element_mut(dom, path)?;

    // If "text" property is set, replace paragraph text in the cell
    if let Some(new_text) = properties.get("text") {
        for child in &mut cell.children {
            if child.element_type == WordElementType::Paragraph {
                set_paragraph_text(child, new_text);
                break;
            }
        }
    }

    let cell_props: HashMap<String, String> = properties
        .iter()
        .filter(|(k, _)| k.as_str() != "text")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    if !cell_props.is_empty() {
        cell.children
            .retain(|c| c.element_type != WordElementType::TableCellProperties);

        let mut tc_pr = WordNode::new(WordElementType::TableCellProperties);
        let mut children = Vec::new();

        if let Some(width) = cell_props.get("width") {
            let tc_w = WordNode::new(WordElementType::Unknown("tcW".to_string()))
                .with_attribute("w", width.as_str())
                .with_attribute("type", "dxa");
            children.push(tc_w);
        }

        if let Some(val) = cell_props.get("shading") {
            let shd = build_shd_node(val);
            children.push(shd);
        }

        if let Some(val) = cell_props.get("shd") {
            let shd = build_shd_node(val);
            children.push(shd);
        }

        if let Some(val) = cell_props.get("border") {
            let borders = build_cell_borders(val);
            children.push(borders);
        }

        if let Some(val) = cell_props.get("vAlign") {
            let v_align = WordNode::new(WordElementType::Unknown("vAlign".to_string()))
                .with_attribute("val", val.as_str());
            children.push(v_align);
        }

        if let Some(val) = cell_props.get("vMerge") {
            let merge_val = if val == "true" || val == "1" || val == "continue" {
                "1"
            } else {
                "0"
            };
            let v_merge = WordNode::new(WordElementType::Unknown("vMerge".to_string()));
            if merge_val == "0" {
                // Restart vertical merge
                children.push(v_merge.with_attribute("val", "restart"));
            } else {
                children.push(v_merge);
            }
        }

        if let Some(val) = cell_props.get("gridSpan") {
            let gs = WordNode::new(WordElementType::Unknown("gridSpan".to_string()))
                .with_attribute("val", val.as_str());
            children.push(gs);
        }

        if let Some(val) = cell_props.get("noWrap") {
            if val == "true" || val == "1" {
                children.push(WordNode::new(WordElementType::Unknown(
                    "noWrap".to_string(),
                )));
            }
        }

        if let Some(val) = cell_props.get("textDirection") {
            let td = WordNode::new(WordElementType::Unknown("textDirection".to_string()))
                .with_attribute("val", val.as_str());
            children.push(td);
        }

        if !children.is_empty() {
            tc_pr.children = children;
            cell.children.insert(0, tc_pr);
        }
    }

    let recognized = [
        "text",
        "width",
        "shading",
        "shd",
        "border",
        "vAlign",
        "vMerge",
        "gridSpan",
        "noWrap",
        "textDirection",
    ];
    let unsupported: Vec<String> = properties
        .keys()
        .filter(|k| !recognized.contains(&k.as_str()))
        .cloned()
        .collect();

    Ok(unsupported)
}

fn build_cell_borders(value: &str) -> WordNode {
    let mut tc_bdr = WordNode::new(WordElementType::Unknown("tcBorders".to_string()));
    let mut children = Vec::new();
    if value == "none" || value == "0" {
        for border_name in ["top", "bottom", "left", "right"] {
            children.push(
                WordNode::new(WordElementType::Unknown(border_name.to_string()))
                    .with_attribute("val", "none")
                    .with_attribute("sz", "0")
                    .with_attribute("space", "0")
                    .with_attribute("color", "auto"),
            );
        }
    } else if value.starts_with("all=") || value == "single" {
        let style = value.strip_prefix("all=").unwrap_or("single");
        for border_name in ["top", "bottom", "left", "right"] {
            children.push(
                WordNode::new(WordElementType::Unknown(border_name.to_string()))
                    .with_attribute("val", style)
                    .with_attribute("sz", "4")
                    .with_attribute("space", "0")
                    .with_attribute("color", "auto"),
            );
        }
    } else {
        for pair in value.split(';') {
            if let Some(eq) = pair.find('=') {
                let name = &pair[..eq];
                let style = &pair[eq + 1..];
                children.push(
                    WordNode::new(WordElementType::Unknown(name.to_string()))
                        .with_attribute("val", style)
                        .with_attribute("sz", "4")
                        .with_attribute("space", "0")
                        .with_attribute("color", "auto"),
                );
            }
        }
    }
    tc_bdr.children = children;
    tc_bdr
}

/// Remove an element at the given path.
/// Returns the path of the removed element.
pub fn remove_element(dom: &mut WordDom, path: &str) -> Result<Option<String>, HandlerError> {
    let segments = parse_path(path)?;
    if segments.len() < 2 {
        return Err(HandlerError::InvalidPath(format!(
            "cannot remove root element: {}",
            path
        )));
    }

    // Navigate to parent
    let parent_segments = &segments[..segments.len() - 1];
    let parent_path_str = format_path_segments(parent_segments);

    let parent = navigate_to_element_mut(dom, &parent_path_str)?;

    let last_seg = &segments[segments.len() - 1];
    let target_type = resolve_element_type_from_name(&last_seg.name);

    // Find matching children and their indices
    let matching_indices: Vec<usize> = parent
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            c.element_type == target_type
                || matches!(&c.element_type, WordElementType::Unknown(ref n) if n == &last_seg.name)
        })
        .map(|(i, _)| i)
        .collect();

    if matching_indices.is_empty() {
        return Err(HandlerError::PathNotFound(format!(
            "no {} children at {}",
            last_seg.name, parent_path_str
        )));
    }

    let idx = last_seg.index.unwrap_or(1);
    if idx == 0 || idx > matching_indices.len() {
        return Err(HandlerError::PathNotFound(format!(
            "index {} out of range at {}",
            idx, path
        )));
    }

    let child_idx = matching_indices[idx - 1];
    parent.children.remove(child_idx);

    Ok(Some(path.to_string()))
}

fn resolve_element_type_from_name(name: &str) -> WordElementType {
    crate::navigation::resolve_element_type_from_name(name)
}

fn format_path_segments(segments: &[handler_common::PathSegment]) -> String {
    let mut result = String::new();
    for seg in segments {
        result.push('/');
        result.push_str(&seg.to_path_fragment());
    }
    result
}

/// Move an element from source to target parent.
pub fn move_element(
    dom: &mut WordDom,
    source: &str,
    target_parent: Option<&str>,
    position: InsertPosition,
) -> Result<String, HandlerError> {
    // Clone the source element first
    let source_node = navigate_to_element(dom, source)?.clone();

    // Remove from source
    remove_element(dom, source)?;

    // Add to target
    let target = target_parent.unwrap_or("/body");
    let elem_type = source_node.element_type.to_path_name();

    let new_path = crate::add::add_element(
        dom,
        target,
        elem_type,
        position,
        &std::collections::HashMap::new(),
        None,
    )?;

    // Now replace the added empty element with the cloned source content
    let target_node = navigate_to_element_mut(dom, &new_path)?;
    *target_node = source_node;

    Ok(new_path)
}

/// Swap two sibling elements in the Word DOM.
/// Both paths must share the same parent element.
pub fn swap_elements(
    dom: &mut WordDom,
    path1: &str,
    path2: &str,
) -> Result<(String, String), HandlerError> {
    let segs1 = parse_path(path1)?;
    let segs2 = parse_path(path2)?;
    if segs1.is_empty() || segs2.is_empty() {
        return Err(HandlerError::InvalidPath("empty path".to_string()));
    }

    // Both paths must share the same parent (all segments except the last)
    if segs1.len() != segs2.len() {
        return Err(HandlerError::InvalidArgument(
            "swap requires both elements at the same nesting depth".to_string(),
        ));
    }
    let parent_segs1 = &segs1[..segs1.len() - 1];
    let parent_segs2 = &segs2[..segs2.len() - 1];
    if !segments_eq(parent_segs1, parent_segs2) {
        return Err(HandlerError::InvalidArgument(
            "swap requires both elements to share the same parent".to_string(),
        ));
    }

    // Extract the indices of the two elements within their parent
    let idx1 = segs1
        .last()
        .and_then(|s| s.index)
        .ok_or_else(|| HandlerError::InvalidPath(format!("path has no index: {}", path1)))?;
    let idx2 = segs2
        .last()
        .and_then(|s| s.index)
        .ok_or_else(|| HandlerError::InvalidPath(format!("path has no index: {}", path2)))?;

    if idx1 == idx2 {
        return Err(HandlerError::InvalidArgument(format!(
            "swap requires two different elements, both were at index {}",
            idx1
        )));
    }

    // Navigate to the parent node
    let parent_path = if parent_segs1.is_empty() {
        "/body".to_string()
    } else {
        let mut p = String::new();
        for seg in parent_segs1 {
            p.push('/');
            p.push_str(&seg.name);
            if let Some(i) = seg.index {
                p.push_str(&format!("[{}]", i));
            }
        }
        p
    };

    let parent = navigate_to_element_mut(dom, &parent_path)?;

    // Convert 1-based to 0-based
    let i1 = idx1 - 1;
    let i2 = idx2 - 1;
    if i1 >= parent.children.len() || i2 >= parent.children.len() {
        return Err(HandlerError::PathNotFound(
            "swap index out of bounds".to_string(),
        ));
    }

    parent.children.swap(i1, i2);

    Ok((path1.to_string(), path2.to_string()))
}

/// Compare two PathSegment slices by name and index (since PathSegment doesn't derive PartialEq).
fn segments_eq(a: &[handler_common::PathSegment], b: &[handler_common::PathSegment]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for (sa, sb) in a.iter().zip(b.iter()) {
        if sa.name != sb.name || sa.index != sb.index {
            return false;
        }
    }
    true
}

// ─── Bookmark Set Properties ──────────────────────────────────

/// Set properties on a BookmarkStart element.
/// Supported properties:
/// - name: rename the bookmark (rejects duplicates)
/// - text: replace content between BookmarkStart and BookmarkEnd
/// - id: update the bookmark ID (updates both start and paired end)
fn set_bookmark_properties(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    // First pass: read-only to gather info
    let node = navigate_to_element(dom, path)?;
    if node.element_type != WordElementType::BookmarkStart {
        return Err(HandlerError::InvalidArgument(format!(
            "path does not point to a bookmarkStart: {:?}",
            node.element_type
        )));
    }
    let current_id = node.attributes.get("id").cloned().unwrap_or_default();

    // Validate name if present
    if let Some(new_name) = properties.get("name") {
        crate::helpers::validate_bookmark_name(new_name)?;
        let body = dom
            .root
            .children
            .iter()
            .find(|c| c.element_type == WordElementType::Body);
        if let Some(body) = body {
            if find_other_bookmark_by_name(body, new_name) {
                return Err(HandlerError::InvalidArgument(format!(
                    "bookmark name '{}' already exists; pick a unique name.",
                    new_name
                )));
            }
        }
    }

    // Validate id if present
    if let Some(new_id) = properties.get("id") {
        let id_val: i32 = new_id.parse().map_err(|_| {
            HandlerError::InvalidArgument(format!(
                "bookmark id must be a non-negative integer, got: {}",
                new_id
            ))
        })?;
        if id_val < 0 {
            return Err(HandlerError::InvalidArgument(
                "bookmark id must be non-negative".to_string(),
            ));
        }
    }

    // Second pass: mutations
    // Handle 'name' property
    if let Some(new_name) = properties.get("name") {
        let node = navigate_to_element_mut(dom, path)?;
        node.attributes.insert("name".to_string(), new_name.clone());
    }

    // Handle 'text' property: replace content between BookmarkStart and BookmarkEnd
    if let Some(new_text) = properties.get("text") {
        let parent_path = crate::navigation::parent_path(path)
            .ok_or_else(|| HandlerError::InvalidPath("bookmark has no parent".to_string()))?;
        let parent = navigate_to_element_mut(dom, &parent_path)?;

        let start_idx = parent
            .children
            .iter()
            .position(|c| {
                c.element_type == WordElementType::BookmarkStart
                    && c.attributes.get("id").map(|s| s.as_str()) == Some(&current_id)
            })
            .ok_or_else(|| {
                HandlerError::PathNotFound("bookmarkStart not found in parent".to_string())
            })?;

        let end_idx = parent
            .children
            .iter()
            .position(|c| {
                c.element_type == WordElementType::BookmarkEnd
                    && c.attributes.get("id").map(|s| s.as_str()) == Some(&current_id)
            })
            .ok_or_else(|| {
                HandlerError::PathNotFound("bookmarkEnd not found in parent".to_string())
            })?;

        // Collect indices of content to remove (between start and end)
        let remove_indices: Vec<usize> = (start_idx + 1..end_idx)
            .filter(|i| {
                let child = &parent.children[*i];
                matches!(
                    child.element_type,
                    WordElementType::Run | WordElementType::Text | WordElementType::Hyperlink
                )
            })
            .collect();

        // Remove in reverse to keep indices stable
        for idx in remove_indices.iter().rev() {
            parent.children.remove(*idx);
        }

        // Insert new run after BookmarkStart
        let run = crate::add::make_run_with_text(new_text, &HashMap::new());
        let new_start_idx = parent
            .children
            .iter()
            .position(|c| {
                c.element_type == WordElementType::BookmarkStart
                    && c.attributes.get("id").map(|s| s.as_str()) == Some(&current_id)
            })
            .unwrap_or(start_idx);
        parent.children.insert(new_start_idx + 1, run);
    }

    // Handle 'id' property: update both BookmarkStart and paired BookmarkEnd
    if let Some(new_id) = properties.get("id") {
        let node = navigate_to_element_mut(dom, path)?;
        node.attributes.insert("id".to_string(), new_id.clone());
        update_paired_bookmark_end(dom, &current_id, new_id)?;
    }

    let recognized = ["name", "text", "id"];
    let unsupported: Vec<String> = properties
        .keys()
        .filter(|k| !recognized.contains(&k.as_str()))
        .cloned()
        .collect();

    Ok(unsupported)
}

/// Set properties on a BookmarkEnd element (minimal: only id update).
fn set_bookmark_end_properties(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let node = navigate_to_element_mut(dom, path)?;

    if node.element_type != WordElementType::BookmarkEnd {
        return Err(HandlerError::InvalidArgument(format!(
            "path does not point to a bookmarkEnd: {:?}",
            node.element_type
        )));
    }

    // Handle 'id' property
    if let Some(new_id) = properties.get("id") {
        node.attributes.insert("id".to_string(), new_id.clone());
    }

    let recognized = ["id"];
    let unsupported: Vec<String> = properties
        .keys()
        .filter(|k| !recognized.contains(&k.as_str()))
        .cloned()
        .collect();

    Ok(unsupported)
}

/// Check if any BookmarkStart in the document has the given name.
fn find_other_bookmark_by_name(node: &WordNode, name: &str) -> bool {
    if node.element_type == WordElementType::BookmarkStart
        && node.attributes.get("name").map(|s| s.as_str()) == Some(name)
    {
        return true;
    }
    node.children
        .iter()
        .any(|c| find_other_bookmark_by_name(c, name))
}

/// Update all BookmarkEnd nodes matching the old ID to the new ID.
fn update_paired_bookmark_end(
    dom: &mut WordDom,
    old_id: &str,
    new_id: &str,
) -> Result<(), HandlerError> {
    let body_idx = dom
        .root
        .children
        .iter()
        .position(|c| c.element_type == WordElementType::Body)
        .ok_or_else(|| HandlerError::OperationFailed("body element not found".to_string()))?;

    update_bookmark_end_in_node(&mut dom.root.children[body_idx], old_id, new_id);
    Ok(())
}

fn update_bookmark_end_in_node(node: &mut WordNode, old_id: &str, new_id: &str) {
    if node.element_type == WordElementType::BookmarkEnd
        && node.attributes.get("id").map(|s| s.as_str()) == Some(old_id)
    {
        node.attributes.insert("id".to_string(), new_id.to_string());
    }
    for child in &mut node.children {
        update_bookmark_end_in_node(child, old_id, new_id);
    }
}

// ─── Document-level Set Properties ──────────────────────────────────

/// Set document-level properties (path "/" or "/body").
/// Vocabulary: protection, protectionEnforced, defaultTabStop
fn set_document_properties(
    dom: &mut WordDom,
    _path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    // For document-level, the properties apply to the root/body element
    // We handle protection by modifying the sectPr (last section properties)
    // and defaultTabStop on the document settings

    for (key, value) in properties {
        match key.as_str() {
            "protection" | "protectionMode" => {
                // Document protection is stored in w:documentProtection inside sectPr
                let body = dom.body_mut();
                if let Some(body) = body {
                    // Find or create sectPr
                    let sect_pr_idx = body
                        .children
                        .iter()
                        .rposition(|c| c.element_type == WordElementType::SectionProperties);
                    if let Some(idx) = sect_pr_idx {
                        let sect_pr = &mut body.children[idx];
                        // Remove existing documentProtection
                        sect_pr.children.retain(|c| {
                            let name = match &c.element_type {
                                WordElementType::Unknown(n) => n.as_str(),
                                _ => "",
                            };
                            name != "documentProtection"
                        });
                        let prot = WordNode::new(WordElementType::Unknown(
                            "documentProtection".to_string(),
                        ))
                        .with_attribute("edit", value.as_str())
                        .with_attribute("enforcement", "1");
                        sect_pr.children.push(prot);
                    }
                }
            }
            "protectionEnforced" => {
                let body = dom.body_mut();
                if let Some(body) = body {
                    let sect_pr_idx = body
                        .children
                        .iter()
                        .rposition(|c| c.element_type == WordElementType::SectionProperties);
                    if let Some(idx) = sect_pr_idx {
                        let sect_pr = &mut body.children[idx];
                        // Find existing documentProtection and update enforcement
                        for child in &mut sect_pr.children {
                            if let WordElementType::Unknown(name) = &child.element_type {
                                if name == "documentProtection" {
                                    let enforcement_val = if value == "true" || value == "1" {
                                        "1"
                                    } else {
                                        "0"
                                    };
                                    child.attributes.insert(
                                        "enforcement".to_string(),
                                        enforcement_val.to_string(),
                                    );
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let recognized = ["protection", "protectionMode", "protectionEnforced"];
    let unsupported: Vec<String> = properties
        .keys()
        .filter(|k| !recognized.contains(&k.as_str()))
        .cloned()
        .collect();

    Ok(unsupported)
}

// ─── Section Properties Set ──────────────────────────────────

/// Set section properties. Vocabulary:
/// pageWidth, pageHeight, orientation, marginLeft, marginRight, marginTop, marginBottom,
/// columns, headerDistance, footerDistance, gutter
fn set_section_properties(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let sect_pr = navigate_to_element_mut(dom, path)?;

    // Handle orientation → page size changes
    if let Some(orient) = properties.get("orientation") {
        // Update pgSz with the correct dimensions for the orientation
        let existing_sz = sect_pr.children.iter_mut().find(|c| {
            let name = match &c.element_type {
                WordElementType::Unknown(n) => n.as_str(),
                _ => "",
            };
            name == "pgSz"
        });

        if let Some(sz) = existing_sz {
            sz.attributes
                .insert("orient".to_string(), orient.to_string());
        } else {
            let pg_sz = WordNode::new(WordElementType::Unknown("pgSz".to_string()))
                .with_attribute("orient", orient.as_str());
            sect_pr.children.insert(0, pg_sz);
        }
    }

    // Handle page dimensions
    for (key, value) in properties {
        match key.as_str() {
            "pageWidth" => {
                let pg_sz = find_or_create_child(sect_pr, "pgSz");
                pg_sz.attributes.insert("w".to_string(), value.clone());
            }
            "pageHeight" => {
                let pg_sz = find_or_create_child(sect_pr, "pgSz");
                pg_sz.attributes.insert("h".to_string(), value.clone());
            }
            "marginLeft" => {
                let pg_mar = find_or_create_child(sect_pr, "pgMar");
                pg_mar.attributes.insert("left".to_string(), value.clone());
            }
            "marginRight" => {
                let pg_mar = find_or_create_child(sect_pr, "pgMar");
                pg_mar.attributes.insert("right".to_string(), value.clone());
            }
            "marginTop" => {
                let pg_mar = find_or_create_child(sect_pr, "pgMar");
                pg_mar.attributes.insert("top".to_string(), value.clone());
            }
            "marginBottom" => {
                let pg_mar = find_or_create_child(sect_pr, "pgMar");
                pg_mar
                    .attributes
                    .insert("bottom".to_string(), value.clone());
            }
            "headerDistance" => {
                let pg_mar = find_or_create_child(sect_pr, "pgMar");
                pg_mar
                    .attributes
                    .insert("header".to_string(), value.clone());
            }
            "footerDistance" => {
                let pg_mar = find_or_create_child(sect_pr, "pgMar");
                pg_mar
                    .attributes
                    .insert("footer".to_string(), value.clone());
            }
            "gutter" => {
                let pg_mar = find_or_create_child(sect_pr, "pgMar");
                pg_mar
                    .attributes
                    .insert("gutter".to_string(), value.clone());
            }
            "columns" => {
                // Number of columns (integer)
                let cols = find_or_create_child(sect_pr, "cols");
                cols.attributes.insert("num".to_string(), value.clone());
                cols.attributes
                    .insert("space".to_string(), "720".to_string()); // Default column spacing
            }
            _ => {}
        }
    }

    let recognized = [
        "orientation",
        "pageWidth",
        "pageHeight",
        "marginLeft",
        "marginRight",
        "marginTop",
        "marginBottom",
        "headerDistance",
        "footerDistance",
        "gutter",
        "columns",
    ];
    let unsupported: Vec<String> = properties
        .keys()
        .filter(|k| !recognized.contains(&k.as_str()))
        .cloned()
        .collect();

    Ok(unsupported)
}

/// Find a child element by name, or create it if it doesn't exist.
fn find_or_create_child<'a>(parent: &'a mut WordNode, name: &str) -> &'a mut WordNode {
    let idx = parent.children.iter().position(|c| {
        let c_name = match &c.element_type {
            WordElementType::Unknown(n) => n.as_str(),
            _ => "",
        };
        c_name == name
    });

    if let Some(idx) = idx {
        &mut parent.children[idx]
    } else {
        let node = WordNode::new(WordElementType::Unknown(name.to_string()));
        parent.children.push(node);
        let last = parent.children.len() - 1;
        &mut parent.children[last]
    }
}

// ─── SDT Set Properties ──────────────────────────────────

/// Set SDT (structured document tag / content control) properties.
/// Vocabulary: alias/name, tag, lock, text
fn set_sdt_properties(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let sdt = navigate_to_element_mut(dom, path)?;

    // Handle SDT properties (sdtPr child)
    let sdt_pr_idx = sdt
        .children
        .iter()
        .position(|c| c.element_type == WordElementType::SdtPr);
    if let Some(idx) = sdt_pr_idx {
        let sdt_pr = &mut sdt.children[idx];

        for (key, value) in properties {
            match key.as_str() {
                "alias" | "name" => {
                    // Update or add w:alias in sdtPr
                    let alias_idx = sdt_pr.children.iter().position(|c| {
                        let name = match &c.element_type {
                            WordElementType::Unknown(n) => n.as_str(),
                            _ => "",
                        };
                        name == "alias"
                    });
                    if let Some(alias_idx) = alias_idx {
                        sdt_pr.children[alias_idx]
                            .attributes
                            .insert("val".to_string(), value.clone());
                    } else {
                        let alias = WordNode::new(WordElementType::Unknown("alias".to_string()))
                            .with_attribute("val", value.as_str());
                        sdt_pr.children.push(alias);
                    }
                }
                "tag" => {
                    let tag_idx = sdt_pr.children.iter().position(|c| {
                        let name = match &c.element_type {
                            WordElementType::Unknown(n) => n.as_str(),
                            _ => "",
                        };
                        name == "tag"
                    });
                    if let Some(tag_idx) = tag_idx {
                        sdt_pr.children[tag_idx]
                            .attributes
                            .insert("val".to_string(), value.clone());
                    } else {
                        let tag = WordNode::new(WordElementType::Unknown("tag".to_string()))
                            .with_attribute("val", value.as_str());
                        sdt_pr.children.push(tag);
                    }
                }
                "lock" => {
                    let lock_val = match value.as_str() {
                        "content" | "contentLocked" => "contentLocked",
                        "sdt" | "sdtLocked" => "sdtLocked",
                        "both" | "sdtContentLocked" => "sdtContentLocked",
                        "unlocked" | "none" => "unlocked",
                        other => other,
                    };
                    let lock_idx = sdt_pr.children.iter().position(|c| {
                        let name = match &c.element_type {
                            WordElementType::Unknown(n) => n.as_str(),
                            _ => "",
                        };
                        name == "lock"
                    });
                    if let Some(lock_idx) = lock_idx {
                        sdt_pr.children[lock_idx]
                            .attributes
                            .insert("val".to_string(), lock_val.to_string());
                    } else {
                        let lock = WordNode::new(WordElementType::Unknown("lock".to_string()))
                            .with_attribute("val", lock_val);
                        sdt_pr.children.push(lock);
                    }
                }
                _ => {}
            }
        }
    }

    // Handle text property: replace SDT content text
    if let Some(new_text) = properties.get("text") {
        let content_idx = sdt
            .children
            .iter()
            .position(|c| c.element_type == WordElementType::SdtContent);
        if let Some(idx) = content_idx {
            let content = &mut sdt.children[idx];
            // Find first paragraph and replace text
            for child in &mut content.children {
                if child.element_type == WordElementType::Paragraph {
                    set_paragraph_text(child, new_text);
                    break;
                }
            }
        }
    }

    let recognized = ["alias", "name", "tag", "lock", "text"];
    let unsupported: Vec<String> = properties
        .keys()
        .filter(|k| !recognized.contains(&k.as_str()))
        .cloned()
        .collect();

    Ok(unsupported)
}

// ─── Hyperlink Set Properties ──────────────────────────────────

/// Set hyperlink properties. Vocabulary: url/target, tooltip
fn set_hyperlink_properties(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let link = navigate_to_element_mut(dom, path)?;

    for (key, value) in properties {
        match key.as_str() {
            "url" | "target" | "link" => {
                // Reject URI schemes that would survive OOXML round-trip but
                // execute script or exfiltrate data on click in the host
                // product (javascript:, data:, vbscript:). See
                // handler_common::hyperlink_validator for the allowlist.
                if let Err(msg) =
                    handler_common::hyperlink_validator::require_safe_scheme(value, "hyperlink")
                {
                    return Err(HandlerError::InvalidArgument(msg));
                }
                // Hyperlink target is stored as r:id attribute pointing to a relationship
                // For direct URLs, we can only update the attribute
                link.attributes.insert("r:id".to_string(), value.clone());
            }
            "tooltip" => {
                // Tooltips use w:tooltip attribute
                link.attributes.insert("tooltip".to_string(), value.clone());
            }
            "text" => {
                // Replace hyperlink text (runs inside the hyperlink)
                let runs: Vec<usize> = link
                    .children
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.element_type == WordElementType::Run)
                    .map(|(i, _)| i)
                    .collect();
                // Remove existing runs
                for idx in runs.iter().rev() {
                    link.children.remove(*idx);
                }
                // Add new run with text
                let run = crate::add::make_run_with_text(value, &HashMap::new());
                link.children.push(run);
            }
            _ => {}
        }
    }

    let recognized = ["url", "target", "link", "tooltip", "text"];
    let unsupported: Vec<String> = properties
        .keys()
        .filter(|k| !recognized.contains(&k.as_str()))
        .cloned()
        .collect();

    Ok(unsupported)
}

// ─── Find & Replace ──────────────────────────────────────────────────

/// Apply find/replace to a single path or whole document body.
///
/// If `path` points to a Paragraph, only that paragraph's text is scanned.
/// If `path` is "/" or "/body", every paragraph in the body is scanned.
/// Returns a Vec containing either zero entries (silent success) or a single
/// summary entry "replaced=<n>" so callers can surface counts in view output.
fn apply_find_replace(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    // Coerce HashMap<String, String> to the shape extract_find_replace_props expects.
    let mut prop_map: HashMap<String, String> = HashMap::new();
    for (k, v) in properties {
        prop_map.insert(k.clone(), v.clone());
    }
    let (find, replace, opts) = extract_find_replace_props(&prop_map).ok_or_else(|| {
        HandlerError::InvalidArgument(
            "find/replace requires at least a 'find=<text>' property".to_string(),
        )
    })?;

    if properties.keys().any(|key| key.starts_with("revision.")) {
        if properties
            .get("revision.type")
            .is_some_and(|kind| kind.eq_ignore_ascii_case("format"))
        {
            return apply_tracked_find_format(dom, path, properties, &find, &opts);
        }
        return apply_tracked_find_replace(dom, path, properties, &find, &replace, &opts);
    }

    let path_lc = path.trim().to_lowercase();
    let total = if path_lc == "/" || path_lc == "/body" || path_lc.is_empty() {
        find_replace_in_body(dom, &find, &replace, &opts)?
    } else {
        // Resolve path → paragraph(s). Try navigation first to validate the path.
        let _segments = parse_path(path)?;
        let body = dom
            .body_mut()
            .ok_or_else(|| HandlerError::InvalidPath("document has no body".to_string()))?;
        // We accept paths of the form /body/p[i] — pull out the index from the raw path.
        find_replace_in_paragraph_path(body, path, &find, &replace, &opts)?
    };

    Ok(vec![format!("replaced={}", total)])
}

fn apply_tracked_find_format(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
    find: &str,
    opts: &FindReplaceOptions,
) -> Result<Vec<String>, HandlerError> {
    if properties.contains_key("replace") || properties.contains_key("revision.id") {
        return Err(HandlerError::UnsupportedProperty(
            "tracked find format does not accept replace= or revision.id".to_string(),
        ));
    }
    let find_keys = find_replace_property_keys();
    let format: HashMap<_, _> = properties
        .iter()
        .filter(|(key, _)| !find_keys.contains(&key.as_str()) && !key.starts_with("revision."))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if format.is_empty() {
        return Err(HandlerError::InvalidArgument(
            "tracked find format requires a run-format property".to_string(),
        ));
    }
    let author = properties
        .get("revision.author")
        .map(String::as_str)
        .unwrap_or("OfficeCLI");
    let date = properties.get("revision.date").map(String::as_str);
    let mut id = crate::add::next_revision_id(&dom.root)
        .parse::<u32>()
        .unwrap_or(1);
    let body = dom
        .body_mut()
        .ok_or_else(|| HandlerError::InvalidPath("document has no body".to_string()))?;
    let mut total = 0;
    let mut context = TrackedFindFormatContext {
        format: &format,
        author,
        date,
        next_id: &mut id,
    };
    for index in tracked_find_paragraph_indices(body, path)? {
        ensure_tracked_find_safe_boundaries(&body.children[index], find, opts)?;
        for group in collect_plain_run_span_groups(&body.children[index])
            .into_iter()
            .rev()
        {
            let text: String = group.iter().map(|span| span.text.as_str()).collect();
            for (start, end) in find_all_spans(&text, find, opts)
                .into_iter()
                .filter(|(start, end)| start < end)
                .rev()
            {
                tracked_find_format_group_match(
                    &mut body.children[index],
                    &group,
                    start,
                    end,
                    &mut context,
                )?;
                total += 1;
            }
        }
    }
    Ok(vec![format!("replaced={}", total)])
}

/// Track a literal find/replace at the actual text-fragment boundary.  Each
/// occurrence keeps its unmatched prefix/suffix in ordinary runs, records the
/// old fragment in `w:del/w:delText`, and records a non-empty replacement in
/// `w:ins`.  This is deliberately separate from the legacy in-place string
/// replacement above: wrapping a whole run would incorrectly mark unmatched
/// text as changed.
fn apply_tracked_find_replace(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
    find: &str,
    replace: &str,
    opts: &FindReplaceOptions,
) -> Result<Vec<String>, HandlerError> {
    if properties.contains_key("revision.action") || properties.contains_key("revision.id") {
        return Err(HandlerError::InvalidArgument(
            "find revisions do not accept revision.action or revision.id; ids are allocated per fragment"
                .to_string(),
        ));
    }
    if let Some(kind) = properties.get("revision.type") {
        if !kind.eq_ignore_ascii_case("format") {
            return Err(HandlerError::InvalidArgument(
                "find infers ins/del from replace; revision.type is only accepted as format for a future format-only path"
                    .to_string(),
            ));
        }
        return Err(HandlerError::UnsupportedProperty(
            "tracked find format-only changes are not implemented yet; provide replace= to create del/ins revisions"
                .to_string(),
        ));
    }
    if !properties.contains_key("replace") {
        return Err(HandlerError::InvalidArgument(
            "tracked find requires replace=NEW (or replace= for a deletion)".to_string(),
        ));
    }
    let author = properties
        .get("revision.author")
        .cloned()
        .unwrap_or_else(|| "OfficeCLI".to_string());
    let date = properties.get("revision.date").cloned();
    let next_id = crate::add::next_revision_id(&dom.root)
        .parse::<u32>()
        .unwrap_or(1);
    let mut context = TrackedFindContext {
        find,
        replace,
        opts,
        author: &author,
        date: date.as_deref(),
        next_id,
        remaining: opts.max_replacements.unwrap_or(usize::MAX),
    };
    let body = dom
        .body_mut()
        .ok_or_else(|| HandlerError::InvalidPath("document has no body".to_string()))?;
    let target_indices = tracked_find_paragraph_indices(body, path)?;
    let mut total = 0;
    for index in target_indices {
        if context.remaining == 0 {
            break;
        }
        ensure_tracked_find_safe_boundaries(&body.children[index], context.find, context.opts)?;
        let changed = tracked_find_in_paragraph(&mut body.children[index], &mut context)?;
        total += changed;
    }
    Ok(vec![format!("replaced={}", total)])
}

struct TrackedFindContext<'a> {
    find: &'a str,
    replace: &'a str,
    opts: &'a FindReplaceOptions,
    author: &'a str,
    date: Option<&'a str>,
    next_id: u32,
    remaining: usize,
}

#[derive(Clone)]
struct PlainRunSpan {
    child_index: usize,
    start: usize,
    end: usize,
    text: String,
}

/// Consecutive direct runs eligible for tracked-find splitting. Inline
/// elements such as hyperlinks, fields and drawings terminate a group: a
/// match can never cross one of those boundaries and flatten its OOXML
/// structure.
fn collect_plain_run_span_groups(paragraph: &WordNode) -> Vec<Vec<PlainRunSpan>> {
    let mut offset = 0;
    let mut group = Vec::new();
    let mut groups = Vec::new();
    for (child_index, child) in paragraph.children.iter().enumerate() {
        let is_plain_run = is_plain_direct_run(child);
        if !is_plain_run {
            if !group.is_empty() {
                groups.push(std::mem::take(&mut group));
            }
            offset = 0;
            continue;
        }
        let text = child
            .children
            .iter()
            .find(|node| node.element_type == WordElementType::Text)
            .and_then(|node| node.text_content.clone())
            .unwrap_or_default();
        let end = offset + text.len();
        group.push(PlainRunSpan {
            child_index,
            start: offset,
            end,
            text,
        });
        offset = end;
    }
    if !group.is_empty() {
        groups.push(group);
    }
    groups
}

fn is_plain_direct_run(node: &WordNode) -> bool {
    node.element_type == WordElementType::Run && {
        let text_nodes: Vec<_> = node
            .children
            .iter()
            .filter(|child| child.element_type == WordElementType::Text)
            .collect();
        text_nodes.len() == 1
            && node.children.iter().all(|child| {
                child.element_type == WordElementType::Text
                    || child.element_type == WordElementType::RunProperties
            })
    }
}

/// Track changes only manipulates ordinary direct runs. Detect a match that
/// enters or crosses an inline structure before mutating anything; silently
/// skipping it would make a successful command report the wrong result, while
/// flattening it would lose hyperlink/field/drawing semantics.
fn ensure_tracked_find_safe_boundaries(
    paragraph: &WordNode,
    find: &str,
    opts: &FindReplaceOptions,
) -> Result<(), HandlerError> {
    let mut full_text = String::new();
    let mut safe_ranges = Vec::new();
    let mut group_start = None;
    for child in &paragraph.children {
        let start = full_text.len();
        let child_text = child.paragraph_text();
        if is_plain_direct_run(child) {
            group_start.get_or_insert(start);
        } else if child.element_type == WordElementType::Run || !child_text.is_empty() {
            if let Some(group_start) = group_start.take() {
                safe_ranges.push((group_start, start));
            }
        }
        full_text.push_str(&child_text);
    }
    if let Some(group_start) = group_start {
        safe_ranges.push((group_start, full_text.len()));
    }
    let matches = if opts.use_regex {
        find_all_replacements(&full_text, find, "", opts)
            .map_err(|error| {
                HandlerError::InvalidArgument(format!("invalid find regex: {}", error))
            })?
            .into_iter()
            .map(|(start, end, _)| (start, end))
            .collect()
    } else {
        find_all_spans(&full_text, find, opts)
    };
    if matches.into_iter().any(|(start, end)| {
        start < end
            && !safe_ranges
                .iter()
                .any(|(range_start, range_end)| start >= *range_start && end <= *range_end)
    }) {
        return Err(HandlerError::UnsupportedProperty(
            "tracked find cannot target or cross an inline structure boundary (hyperlink, field, drawing, or non-text run); narrow the match to an ordinary direct run sequence"
                .to_string(),
        ));
    }
    Ok(())
}

fn tracked_find_paragraph_indices(body: &WordNode, path: &str) -> Result<Vec<usize>, HandlerError> {
    let path_lc = path.trim().to_ascii_lowercase();
    if path_lc.is_empty() || matches!(path_lc.as_str(), "/" | "/body") {
        return Ok(body
            .children
            .iter()
            .enumerate()
            .filter_map(|(index, child)| {
                (child.element_type == WordElementType::Paragraph).then_some(index)
            })
            .collect());
    }
    let index = path_lc
        .strip_prefix("/body/p[")
        .and_then(|value| value.strip_suffix(']'))
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|index| *index > 0)
        .ok_or_else(|| HandlerError::InvalidPath(path.to_string()))?;
    let found = body
        .children
        .iter()
        .enumerate()
        .filter(|(_, child)| child.element_type == WordElementType::Paragraph)
        .nth(index - 1)
        .map(|(index, _)| index)
        .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?;
    Ok(vec![found])
}

fn tracked_find_in_paragraph(
    para: &mut WordNode,
    context: &mut TrackedFindContext<'_>,
) -> Result<usize, HandlerError> {
    let mut total = 0;
    for group in collect_plain_run_span_groups(para).into_iter().rev() {
        if context.remaining == 0 {
            break;
        }
        let text: String = group.iter().map(|span| span.text.as_str()).collect();
        let matches = find_all_replacements(&text, context.find, context.replace, context.opts)
            .map_err(|error| {
                HandlerError::InvalidArgument(format!("invalid find regex: {}", error))
            })?;
        let count = matches.len().min(context.remaining);
        for (start, end, replacement) in matches.into_iter().take(count).rev() {
            tracked_find_replace_group_match(para, &group, start, end, &replacement, context)?;
            total += 1;
            context.remaining -= 1;
        }
    }
    Ok(total)
}

/// Replace one match in a continuous plain-run group. The group is processed
/// right-to-left, so replacing a later range does not invalidate the child
/// indices of a preceding range.
fn tracked_find_replace_group_match(
    para: &mut WordNode,
    group: &[PlainRunSpan],
    match_start: usize,
    match_end: usize,
    replacement: &str,
    context: &mut TrackedFindContext<'_>,
) -> Result<(), HandlerError> {
    let affected: Vec<_> = group
        .iter()
        .filter(|span| span.start < match_end && span.end > match_start)
        .collect();
    let first = affected.first().ok_or_else(|| {
        HandlerError::InvalidArgument("tracked find match has no eligible run".to_string())
    })?;
    let last = affected.last().expect("non-empty affected spans");
    let template_rpr = para.children[first.child_index]
        .children
        .iter()
        .find(|child| child.element_type == WordElementType::RunProperties)
        .cloned();
    let mut replacement_nodes = Vec::new();

    let prefix_end = match_start - first.start;
    if prefix_end > 0 {
        replacement_nodes.push(tracked_text_run(
            template_rpr.as_ref(),
            &first.text[..prefix_end],
            false,
        ));
    }
    for span in &affected {
        let start = match_start.saturating_sub(span.start);
        let end = match_end.min(span.end) - span.start;
        if start >= end {
            continue;
        }
        let rpr = para.children[span.child_index]
            .children
            .iter()
            .find(|child| child.element_type == WordElementType::RunProperties)
            .cloned();
        let del_id = allocate_tracked_id(&mut context.next_id);
        replacement_nodes.push(tracked_wrapper(
            "del",
            del_id,
            context.author,
            context.date,
            tracked_text_run(rpr.as_ref(), &span.text[start..end], true),
        ));
    }
    if !replacement.is_empty() {
        let ins_id = allocate_tracked_id(&mut context.next_id);
        replacement_nodes.push(tracked_wrapper(
            "ins",
            ins_id,
            context.author,
            context.date,
            tracked_text_run(template_rpr.as_ref(), replacement, false),
        ));
    }
    let suffix_start = match_end - last.start;
    if suffix_start < last.text.len() {
        let rpr = para.children[last.child_index]
            .children
            .iter()
            .find(|child| child.element_type == WordElementType::RunProperties)
            .cloned();
        replacement_nodes.push(tracked_text_run(
            rpr.as_ref(),
            &last.text[suffix_start..],
            false,
        ));
    }
    para.children
        .splice(first.child_index..=last.child_index, replacement_nodes);
    Ok(())
}

/// Apply a format revision to one match in a continuous plain-run group. Like
/// replacement, this first materializes exact run boundaries, so unmatched
/// prefix/suffix text keeps its original formatting and revision history.
struct TrackedFindFormatContext<'a> {
    format: &'a HashMap<String, String>,
    author: &'a str,
    date: Option<&'a str>,
    next_id: &'a mut u32,
}

fn tracked_find_format_group_match(
    para: &mut WordNode,
    group: &[PlainRunSpan],
    match_start: usize,
    match_end: usize,
    context: &mut TrackedFindFormatContext<'_>,
) -> Result<(), HandlerError> {
    let affected: Vec<_> = group
        .iter()
        .filter(|span| span.start < match_end && span.end > match_start)
        .collect();
    let first = affected.first().ok_or_else(|| {
        HandlerError::InvalidArgument("tracked find match has no eligible run".to_string())
    })?;
    let last = affected.last().expect("non-empty affected spans");
    let first_rpr = para.children[first.child_index]
        .children
        .iter()
        .find(|child| child.element_type == WordElementType::RunProperties)
        .cloned();
    let mut replacement_nodes = Vec::new();
    let prefix_end = match_start - first.start;
    if prefix_end > 0 {
        replacement_nodes.push(tracked_text_run(
            first_rpr.as_ref(),
            &first.text[..prefix_end],
            false,
        ));
    }
    for span in &affected {
        let start = match_start.saturating_sub(span.start);
        let end = match_end.min(span.end) - span.start;
        if start >= end {
            continue;
        }
        let rpr = para.children[span.child_index]
            .children
            .iter()
            .find(|child| child.element_type == WordElementType::RunProperties)
            .cloned();
        replacement_nodes.push(tracked_format_run(
            rpr.as_ref(),
            &span.text[start..end],
            context.format,
            context.author,
            context.date,
            allocate_tracked_id(context.next_id),
        ));
    }
    let suffix_start = match_end - last.start;
    if suffix_start < last.text.len() {
        let rpr = para.children[last.child_index]
            .children
            .iter()
            .find(|child| child.element_type == WordElementType::RunProperties)
            .cloned();
        replacement_nodes.push(tracked_text_run(
            rpr.as_ref(),
            &last.text[suffix_start..],
            false,
        ));
    }
    para.children
        .splice(first.child_index..=last.child_index, replacement_nodes);
    Ok(())
}

fn tracked_format_run(
    prior_rpr: Option<&WordNode>,
    text: &str,
    format: &HashMap<String, String>,
    author: &str,
    date: Option<&str>,
    id: String,
) -> WordNode {
    let snapshot = prior_rpr
        .cloned()
        .unwrap_or_else(|| WordNode::new(WordElementType::RunProperties));
    let mut run = tracked_text_run(prior_rpr, text, false);
    apply_run_props_to_run(&mut run, format);
    let rpr = run
        .children
        .iter_mut()
        .find(|node| node.element_type == WordElementType::RunProperties)
        .expect("format properties create rPr");
    let mut change = WordNode::new(WordElementType::Unknown("rPrChange".to_string()));
    change.attributes.insert("id".to_string(), id);
    change
        .attributes
        .insert("author".to_string(), author.to_string());
    if let Some(date) = date {
        change
            .attributes
            .insert("date".to_string(), date.to_string());
    }
    change.children.push(snapshot);
    rpr.children.push(change);
    run
}

fn allocate_tracked_id(next_id: &mut u32) -> String {
    let id = *next_id;
    *next_id = next_id.saturating_add(1);
    id.to_string()
}

fn tracked_text_run(rpr: Option<&WordNode>, text: &str, deleted: bool) -> WordNode {
    let mut run = WordNode::new(WordElementType::Run);
    if let Some(rpr) = rpr {
        run.children.push(rpr.clone());
    }
    let mut text_node = WordNode::new(if deleted {
        WordElementType::Unknown("delText".to_string())
    } else {
        WordElementType::Text
    })
    .with_text(text);
    if text.starts_with(' ') || text.ends_with(' ') {
        text_node
            .attributes
            .insert("xml:space".to_string(), "preserve".to_string());
        text_node.preserve_space = true;
    }
    run.children.push(text_node);
    run
}

fn tracked_wrapper(
    kind: &str,
    id: String,
    author: &str,
    date: Option<&str>,
    run: WordNode,
) -> WordNode {
    let mut wrapper = WordNode::new(WordElementType::Unknown(kind.to_string()));
    wrapper.attributes.insert("id".to_string(), id);
    wrapper
        .attributes
        .insert("author".to_string(), author.to_string());
    if let Some(date) = date {
        wrapper
            .attributes
            .insert("date".to_string(), date.to_string());
    }
    wrapper.children.push(run);
    wrapper
}

/// Run find/replace across all paragraphs in the body. Returns total replacements.
fn find_replace_in_body(
    dom: &mut WordDom,
    find: &str,
    replace: &str,
    opts: &FindReplaceOptions,
) -> Result<usize, HandlerError> {
    let body = dom
        .body_mut()
        .ok_or_else(|| HandlerError::InvalidPath("document has no body".to_string()))?;
    // Collect paragraph child indices first so we can borrow mutably without aliasing.
    let para_indices: Vec<usize> = body
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| c.element_type == WordElementType::Paragraph)
        .map(|(i, _)| i)
        .collect();

    let mut total = 0usize;
    for idx in para_indices {
        total += find_replace_in_paragraph(&mut body.children[idx], find, replace, opts);
    }
    Ok(total)
}

/// Resolve /body/p[i] style paths to a paragraph and run find/replace on it.
fn find_replace_in_paragraph_path(
    body: &mut WordNode,
    path: &str,
    find: &str,
    replace: &str,
    opts: &FindReplaceOptions,
) -> Result<usize, HandlerError> {
    // Extract trailing [i] index from a path like /body/p[3]
    let lower = path.to_lowercase();
    let idx = match parse_paragraph_index(&lower) {
        Some(i) => i,
        None => {
            return Err(HandlerError::InvalidArgument(format!(
                "find/replace only supports paths of the form '/body/p[i]' or '/'. Got: '{}'",
                path
            )))
        }
    };

    let mut count = 0;
    let mut found_idx = 0;
    for child in &mut body.children {
        if child.element_type == WordElementType::Paragraph {
            found_idx += 1;
            if found_idx == idx {
                count = find_replace_in_paragraph(child, find, replace, opts);
                break;
            }
        }
    }
    if found_idx < idx {
        return Err(HandlerError::PathNotFound(format!(
            "paragraph index {} out of range (found {} paragraphs)",
            idx, found_idx
        )));
    }
    Ok(count)
}

/// Parse the trailing [n] index from a path ending in p[n].
fn parse_paragraph_index(path_lc: &str) -> Option<usize> {
    let open = path_lc.rfind('[')?;
    let close = path_lc.rfind(']')?;
    if close <= open {
        return None;
    }
    let inner = &path_lc[open + 1..close];
    inner.parse::<usize>().ok()
}

/// Apply find/replace to a single paragraph node. Returns count of replacements.
///
/// Walks every run's text content and replaces matches. The find/replace
/// operates per-run (cross-run matches are not spanned) which matches the
/// common case where users search literal strings.
fn find_replace_in_paragraph(
    para: &mut WordNode,
    find: &str,
    replace: &str,
    opts: &FindReplaceOptions,
) -> usize {
    let run_indices: Vec<usize> = para
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| c.element_type == WordElementType::Run)
        .map(|(i, _)| i)
        .collect();

    let mut total = 0usize;
    for idx in run_indices {
        let run = &mut para.children[idx];
        // Walk run children to find Text nodes and replace in place.
        let text_indices: Vec<usize> = run
            .children
            .iter()
            .enumerate()
            .filter(|(_, c)| c.element_type == WordElementType::Text)
            .map(|(i, _)| i)
            .collect();

        for t_idx in text_indices {
            let t_node = &mut run.children[t_idx];
            let cur = t_node.text_content.clone().unwrap_or_default();
            let (new_text, n) = replace_in_string(&cur, find, replace, opts);
            if n > 0 {
                t_node.text_content = Some(new_text);
                total += n;
            }
        }
    }
    total
}

// ─── Part-aware Set: Styles, Comments, Footnotes, Endnotes ────────────

/// Set properties on a Word style. The path is `/styles/<styleId>` (or
/// `/styles/<styleId>/...` for future sub-targeting). Reads `word/styles.xml`,
/// finds the `<w:style w:styleId="...">` block, and modifies properties within it.
///
/// Supported properties:
///   - name            → set <w:name w:val="..."/>
///   - basedOn         → set <w:basedOn w:val="..."/>
///   - next            → set <w:next w:val="..."/>
///   - uiPriority      → set <w:uiPriority w:val="N"/>
///   - hidden          → toggle <w:hidden/>
///   - semiHidden      → toggle <w:semiHidden/>
///   - qFormat          → toggle <w:qFormat/>
///   - Run-level props (font/size/bold/...) → set within <w:rPr>
///   - Para-level props (alignment/spacing/...) → set within <w:pPr>
pub fn set_style_on_part(
    package: &mut OxmlPackage,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    // Extract styleId from path: /styles/Heading1 → "Heading1"
    let style_id = path
        .trim_start_matches('/')
        .strip_prefix("styles/")
        .map(|s| s.trim_end_matches('/').to_string())
        .ok_or_else(|| {
            HandlerError::InvalidArgument(format!(
                "style set expects path '/styles/<styleId>', got '{}'",
                path
            ))
        })?;

    let xml = package
        .read_part_xml("word/styles.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Find the <w:style w:styleId="ID"> ... </w:style> block.
    let needle = format!("w:styleId=\"{}\"", style_id);
    let sid_offset = xml.find(&needle).ok_or_else(|| {
        HandlerError::PathNotFound(format!("style '{}' not found in word/styles.xml", style_id))
    })?;

    // Walk back to the <w:style opening tag.
    let style_open = xml[..sid_offset].rfind("<w:style").ok_or_else(|| {
        HandlerError::OperationFailed(format!(
            "could not locate <w:style> opening tag for '{}'",
            style_id
        ))
    })?;

    // Find matching </w:style> for this opening tag.
    let style_close = find_matching_close(&xml, style_open, "<w:style", "</w:style>")
        .ok_or_else(|| HandlerError::OperationFailed("malformed style block".to_string()))?;

    // Mutate the style block in place.
    let block = &xml[style_open..style_close];
    let mut new_block = block.to_string();
    let mut unsupported = Vec::new();

    // Style metadata vs. visual properties. Visual properties (font, size,
    // bold, color, alignment, spacing, etc.) belong inside <w:pPr> or <w:rPr>
    // children of the style, while metadata (name, basedOn, uiPriority, ...)
    // are direct children. Partition the property map by which family each
    // key belongs to.
    let mut meta_props: HashMap<String, String> = HashMap::new();
    let mut ppr_props: HashMap<String, String> = HashMap::new();
    let mut rpr_props: HashMap<String, String> = HashMap::new();

    for (k, v) in properties.iter() {
        if STYLE_META_KEYS.contains(&k.as_str()) {
            meta_props.insert(k.clone(), v.clone());
        } else if PARAGRAPH_STYLE_KEYS.contains(&k.as_str()) {
            ppr_props.insert(k.clone(), v.clone());
        } else if RUN_STYLE_KEYS.contains(&k.as_str()) {
            rpr_props.insert(k.clone(), v.clone());
        } else {
            unsupported.push(k.clone());
        }
    }

    for (key, value) in &meta_props {
        match key.as_str() {
            "name" => set_or_replace_attr_child(&mut new_block, "w:name", "w:val", value),
            "basedOn" => set_or_replace_attr_child(&mut new_block, "w:basedOn", "w:val", value),
            "next" => set_or_replace_attr_child(&mut new_block, "w:next", "w:val", value),
            "uiPriority" => {
                set_or_replace_attr_child(&mut new_block, "w:uiPriority", "w:val", value)
            }
            "hidden" => toggle_flag_child(&mut new_block, "w:hidden", value),
            "semiHidden" => toggle_flag_child(&mut new_block, "w:semiHidden", value),
            "qFormat" => toggle_flag_child(&mut new_block, "w:qFormat", value),
            "unhideWhenUsed" => toggle_flag_child(&mut new_block, "w:unhideWhenUsed", value),
            "link" => set_or_replace_attr_child(&mut new_block, "w:link", "w:val", value),
            _ => {}
        }
    }

    // Apply paragraph properties: ensure <w:pPr> exists, then apply each prop.
    if !ppr_props.is_empty() {
        ensure_style_child(&mut new_block, "w:pPr");
        let ppr_xml = build_ppr_fragment(&ppr_props);
        merge_into_child(&mut new_block, "w:pPr", &ppr_xml);
    }

    // Apply run properties: same pattern for <w:rPr>.
    if !rpr_props.is_empty() {
        ensure_style_child(&mut new_block, "w:rPr");
        let rpr_xml = build_rpr_fragment(&rpr_props);
        merge_into_child(&mut new_block, "w:rPr", &rpr_xml);
    }

    if new_block != block {
        let mut new_xml = String::with_capacity(xml.len() + new_block.len());
        new_xml.push_str(&xml[..style_open]);
        new_xml.push_str(&new_block);
        new_xml.push_str(&xml[style_close..]);
        package
            .write_part_xml("word/styles.xml", &new_xml)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    }

    Ok(unsupported)
}

/// Style metadata keys — applied as direct children of <w:style>.
const STYLE_META_KEYS: &[&str] = &[
    "name",
    "basedOn",
    "next",
    "uiPriority",
    "hidden",
    "semiHidden",
    "qFormat",
    "unhideWhenUsed",
    "link",
];

/// Paragraph-style keys — applied inside <w:pPr>.
const PARAGRAPH_STYLE_KEYS: &[&str] = &[
    "alignment",
    "jc",
    "indentLeft",
    "indentRight",
    "firstLine",
    "hanging",
    "indent",
    "spacingBefore",
    "spacingAfter",
    "lineSpacing",
    "spacing",
    "keepLines",
    "keepNext",
    "outlineLevel",
    "numId",
    "numLevel",
    "listStyle",
    "pageBreakBefore",
    "widowControl",
];

/// Run-style keys — applied inside <w:rPr>.
const RUN_STYLE_KEYS: &[&str] = &[
    "bold",
    "b",
    "italic",
    "i",
    "underline",
    "u",
    "strike",
    "strikeout",
    "font",
    "fontFamily",
    "size",
    "fontSize",
    "color",
    "fontColor",
    "bgColor",
    "highlight",
    "bg",
    "shading",
    "shd",
    "caps",
    "smallCaps",
    "vanish",
    "kern",
    "spacing",
    "characterSpacing",
    "lang",
];

/// Build the <w:pPr> children XML for the given paragraph properties.
/// Mirrors the helper shape used by the DOM-based paragraph builder so the
/// resulting XML matches what Word expects.
fn build_ppr_fragment(props: &HashMap<String, String>) -> String {
    let mut s = String::new();
    if let Some(v) = props.get("alignment").or_else(|| props.get("jc")) {
        s.push_str(&format!("<w:jc w:val=\"{}\"/>", escape_attr(v)));
    }
    if let Some(v) = props.get("indentLeft").or_else(|| props.get("indent")) {
        s.push_str(&format!("<w:ind w:left=\"{}\"/>", to_twips(v)));
    }
    if let Some(v) = props.get("indentRight") {
        s.push_str(&format!("<w:ind w:right=\"{}\"/>", to_twips(v)));
    }
    if let Some(v) = props.get("firstLine") {
        s.push_str(&format!("<w:ind w:firstLine=\"{}\"/>", to_twips(v)));
    }
    if let Some(v) = props.get("hanging") {
        s.push_str(&format!("<w:ind w:hanging=\"{}\"/>", to_twips(v)));
    }
    if let Some(v) = props.get("spacingBefore") {
        s.push_str(&format!("<w:spacing w:before=\"{}\"/>", to_twips(v)));
    }
    if let Some(v) = props.get("spacingAfter") {
        s.push_str(&format!("<w:spacing w:after=\"{}\"/>", to_twips(v)));
    }
    if let Some(v) = props.get("lineSpacing") {
        s.push_str(&format!(
            "<w:spacing w:line=\"{}\" w:lineRule=\"auto\"/>",
            to_line(v)
        ));
    }
    if let Some(v) = props.get("outlineLevel") {
        s.push_str(&format!("<w:outlineLvl w:val=\"{}\"/>", escape_attr(v)));
    }
    if props.contains_key("keepLines") {
        s.push_str("<w:keepLines/>");
    }
    if props.contains_key("keepNext") {
        s.push_str("<w:keepNext/>");
    }
    if props.contains_key("pageBreakBefore") {
        s.push_str("<w:pageBreakBefore/>");
    }
    if props.contains_key("widowControl") {
        s.push_str("<w:widowControl/>");
    }
    s
}

/// Build the <w:rPr> children XML for the given run properties.
fn build_rpr_fragment(props: &HashMap<String, String>) -> String {
    let mut s = String::new();
    if let Some(v) = props.get("font").or_else(|| props.get("fontFamily")) {
        s.push_str(&format!(
            "<w:rFonts w:ascii=\"{}\" w:hAnsi=\"{}\" w:cs=\"{}\"/>",
            escape_attr(v),
            escape_attr(v),
            escape_attr(v)
        ));
    }
    if let Some(v) = props.get("size").or_else(|| props.get("fontSize")) {
        s.push_str(&format!(
            "<w:sz w:val=\"{}\"/><w:szCs w:val=\"{}\"/>",
            to_half_points(v),
            to_half_points(v)
        ));
    }
    if let Some(v) = props.get("color").or_else(|| props.get("fontColor")) {
        s.push_str(&format!("<w:color w:val=\"{}\"/>", normalize_hex(v)));
    }
    if let Some(v) = props
        .get("highlight")
        .or_else(|| props.get("bgColor"))
        .or_else(|| props.get("bg"))
    {
        s.push_str(&format!("<w:highlight w:val=\"{}\"/>", escape_attr(v)));
    }
    if let Some(v) = props.get("bold").or_else(|| props.get("b")) {
        if is_on(v) {
            s.push_str("<w:b/><w:bCs/>");
        }
    }
    if let Some(v) = props.get("italic").or_else(|| props.get("i")) {
        if is_on(v) {
            s.push_str("<w:i/><w:iCs/>");
        }
    }
    if let Some(v) = props.get("underline").or_else(|| props.get("u")) {
        s.push_str(&format!(
            "<w:u w:val=\"{}\"/>",
            if is_on(v) { "single" } else { "none" }
        ));
    }
    if let Some(v) = props.get("strike").or_else(|| props.get("strikeout")) {
        if is_on(v) {
            s.push_str("<w:strike/>");
        }
    }
    if props.contains_key("caps") {
        s.push_str("<w:caps/>");
    }
    if props.contains_key("smallCaps") {
        s.push_str("<w:smallCaps/>");
    }
    s
}

/// Ensure the style block contains a `<w:TAG>...</w:TAG>` child. Inserts it
/// immediately after the opening `<w:style ...>` tag if missing.
fn ensure_style_child(block: &mut String, tag: &str) {
    let open = format!("<{}", tag);
    if block.contains(&open) {
        return;
    }
    // Insert right after the opening <w:style ...> tag (after the first '>').
    let Some(gt) = block.find('>') else { return };
    let child = format!("<{}></{}>", tag, tag);
    block.insert_str(gt + 1, &child);
}

/// Merge `fragment_xml` (containing child elements) into the named child block.
/// Removes existing matching leaf children first to avoid duplicates, then
/// appends the new fragment before the closing tag of the child.
fn merge_into_child(block: &mut String, child_tag: &str, fragment_xml: &str) {
    let open = format!("<{}", child_tag);
    let close = format!("</{}>", child_tag);
    let Some(open_idx) = block.find(&open) else {
        return;
    };
    let Some(close_idx) = block.find(&close) else {
        return;
    };

    // Splice out the existing child block content.
    let inner_start = block[open_idx..]
        .find('>')
        .map(|i| open_idx + i + 1)
        .unwrap_or(open_idx + open.len());
    let inner_end = close_idx;
    let inner = &block[inner_start..inner_end];

    // For each leaf element the fragment will introduce (e.g. "<w:b/>" or
    // "<w:color w:val=\"...\"/>"), remove any existing sibling with the same
    // tag from the inner content. This keeps the property update idempotent.
    let mut new_inner = inner.to_string();
    // Extract top-level child tags from the fragment.
    for frag_tag in extract_top_level_tags(fragment_xml) {
        let tag_open = format!("<{}", frag_tag);
        let mut cursor = 0;
        while let Some(p) = new_inner[cursor..].find(&tag_open) {
            let abs = cursor + p;
            // Find end of this element (either '/>' or the matching close).
            let after = &new_inner[abs..];
            let end = if let Some(sc) = after.find("/>") {
                abs + sc + 2
            } else if let Some(oc) = after.find('>') {
                // open-close form: find matching close
                let close_tag = format!("</{}>", frag_tag);
                if let Some(ct) = new_inner[abs + oc..].find(&close_tag) {
                    abs + oc + ct + close_tag.len()
                } else {
                    abs + oc + 1
                }
            } else {
                break;
            };
            new_inner.replace_range(abs..end, "");
            cursor = abs;
        }
    }

    new_inner.push_str(fragment_xml);

    block.replace_range(inner_start..inner_end, &new_inner);
}

/// Extract the unique top-level tag names from an XML fragment.
fn extract_top_level_tags(fragment: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let bytes = fragment.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_alphabetic() {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b':') {
                j += 1;
            }
            let name = fragment[start..j].to_string();
            if !tags.contains(&name) {
                tags.push(name);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    tags
}

fn is_on(v: &str) -> bool {
    matches!(v.to_lowercase().as_str(), "true" | "1" | "on" | "yes" | "")
}

fn to_twips(v: &str) -> String {
    // Accept already-twips numbers or unit suffixed values.
    if let Ok(n) = v.parse::<i64>() {
        return n.to_string();
    }
    if let Some(rest) = v.strip_suffix("in") {
        if let Ok(n) = rest.parse::<f64>() {
            return ((n * 1440.0) as i64).to_string();
        }
    }
    if let Some(rest) = v.strip_suffix("cm") {
        if let Ok(n) = rest.parse::<f64>() {
            return ((n * 567.0) as i64).to_string();
        }
    }
    if let Some(rest) = v.strip_suffix("pt") {
        if let Ok(n) = rest.parse::<f64>() {
            return ((n * 20.0) as i64).to_string();
        }
    }
    "0".to_string()
}

fn to_half_points(v: &str) -> String {
    if let Ok(n) = v.parse::<f64>() {
        return (n * 2.0).round().to_string();
    }
    "24".to_string()
}

fn to_line(v: &str) -> String {
    // lineSpacing in lines (e.g. "1.5") → 240×N
    if let Ok(n) = v.parse::<f64>() {
        return ((n * 240.0).round() as i64).to_string();
    }
    "240".to_string()
}

fn normalize_hex(v: &str) -> String {
    let trimmed = v.trim().trim_start_matches('#');
    trimmed.to_string()
}

/// Set properties on a comment. Path is `/comments/<id>` or `/comments/comment[N]`.
/// Reads the given part (word/comments.xml) and modifies the targeted
/// `<w:comment w:id="N">` block.
///
/// Supported properties: text, author, initials, date
pub fn set_comment_on_part(
    package: &mut OxmlPackage,
    part: &str,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    if comment_body_relative_path(path).is_some() {
        return set_comment_body_element(package, part, path, properties);
    }
    let comment_id = get_comment(package, path)?.id;
    let xml = package
        .read_part_xml(part)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let needle = format!("w:id=\"{}\"", comment_id);
    let id_offset = xml.find(&needle).ok_or_else(|| {
        HandlerError::PathNotFound(format!("comment id '{}' not found in {}", comment_id, part))
    })?;

    let open = xml[..id_offset]
        .rfind("<w:comment")
        .ok_or_else(|| HandlerError::OperationFailed("malformed comment element".to_string()))?;
    let close = find_matching_close(&xml, open, "<w:comment", "</w:comment>")
        .ok_or_else(|| HandlerError::OperationFailed("unterminated comment".to_string()))?;

    let block = &xml[open..close];
    let mut new_block = block.to_string();
    let mut unsupported = Vec::new();
    let mut done_update = None;

    for (key, value) in properties {
        match key.as_str() {
            "author" => set_attr_on_open_tag(&mut new_block, "w:author", value),
            "initials" => set_attr_on_open_tag(&mut new_block, "w:initials", value),
            "date" => set_attr_on_open_tag(&mut new_block, "w:date", value),
            "text" => {
                new_block = replace_comment_body(&new_block, value)?;
            }
            "done" | "resolved" => done_update = Some(is_truthy(Some(value))),
            _ if is_comment_format_key(key) => {}
            _ => unsupported.push(key.clone()),
        }
    }

    let mut new_xml = if new_block != block {
        let mut updated = String::with_capacity(xml.len() + new_block.len());
        updated.push_str(&xml[..open]);
        updated.push_str(&new_block);
        updated.push_str(&xml[close..]);
        updated
    } else {
        xml.clone()
    };
    let format_props: HashMap<_, _> = properties
        .iter()
        .filter(|(key, _)| is_comment_format_key(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if !format_props.is_empty() {
        new_xml = apply_comment_body_format(&new_xml, &comment_id, &format_props)?;
    }
    if new_xml != xml {
        package
            .write_part_xml(part, &new_xml)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    }

    if let Some(done) = done_update {
        let current = package
            .read_part_xml(part)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        let (with_para_id, para_id) = assign_comment_para_id(&current, &comment_id)?;
        if with_para_id != current {
            package
                .write_part_xml(part, &with_para_id)
                .map_err(|e| HandlerError::SaveError(e.to_string()))?;
        }
        upsert_comment_extension(package, &para_id, None, Some(done))?;
    }

    Ok(unsupported)
}

/// Read a nested comments.xml element such as
/// `/comments/comment[@commentId=7]/p[1]/r[2]` without pretending it belongs
/// to document.xml. This gives comment bodies the same inspectable structure
/// as normal Word paragraphs and runs.
pub fn get_comment_body_element(
    package: &OxmlPackage,
    path: &str,
    depth: usize,
) -> Result<DocumentNode, HandlerError> {
    let (comment_path, relative) = split_comment_body_path(path)?;
    let comment_id = get_comment(package, comment_path)?.id;
    let xml = package
        .read_part_xml(DOCX_COMMENTS_PART)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let dom = crate::handler::parse_document_xml(&xml)?;
    let comment = find_comment_node(&dom.root, &comment_id)
        .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?;
    let node = navigate_comment_body_node(comment, relative, path)?;
    Ok(comment_body_element_to_document_node(node, path, depth))
}

/// Query paragraph/run selectors in comments.xml as well as document.xml.
/// Paths remain rooted at the stable logical comment id so returned nodes can
/// be used directly by comment-body get/set/add/remove commands.
pub fn query_comment_body_elements(
    package: &OxmlPackage,
    selector: &str,
) -> Result<Vec<DocumentNode>, HandlerError> {
    let Ok(xml) = package.read_part_xml(DOCX_COMMENTS_PART) else {
        return Ok(Vec::new());
    };
    let dom = crate::handler::parse_document_xml(&xml)?;
    let mut comments = Vec::new();
    collect_comment_nodes(&dom.root, &mut comments);
    let mut results = Vec::new();
    for (ordinal, comment) in comments.into_iter().enumerate() {
        let path = comment
            .attributes
            .get("id")
            .map(|id| format!("/comments/comment[@commentId={}]", id))
            .unwrap_or_else(|| format!("/comments/comment[{}]", ordinal + 1));
        results.extend(crate::query::query_subtree(comment, selector, &path)?);
    }
    Ok(results)
}

fn collect_comment_nodes<'a>(node: &'a WordNode, output: &mut Vec<&'a WordNode>) {
    if matches!(&node.element_type, WordElementType::Unknown(name) if name == "comment") {
        output.push(node);
        return;
    }
    for child in &node.children {
        collect_comment_nodes(child, output);
    }
}

/// Mutate a nested comment body paragraph/run. The path is intentionally
/// separate from `set_comment_on_part`'s metadata/body operation: applying a
/// run path to the whole comment would silently flatten unrelated runs.
fn set_comment_body_element(
    package: &mut OxmlPackage,
    part: &str,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let (comment_path, relative) = split_comment_body_path(path)?;
    let comment_id = get_comment(package, comment_path)?.id;
    let xml = package
        .read_part_xml(part)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let mut dom = crate::handler::parse_document_xml(&xml)?;
    let comment = find_comment_node_mut(&mut dom.root, &comment_id)
        .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?;
    let node = navigate_comment_body_node_mut(comment, relative, path)?;
    let unsupported = match node.element_type {
        WordElementType::Paragraph => set_comment_body_paragraph(node, properties),
        WordElementType::Run => set_comment_body_run(node, properties),
        _ => {
            return Err(HandlerError::UnsupportedProperty(
                "comment body set supports only paragraph or run paths".to_string(),
            ))
        }
    };
    package
        .write_part_xml(part, &crate::handler::serialize_dom(&dom))
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(unsupported)
}

fn split_comment_body_path(path: &str) -> Result<(&str, &str), HandlerError> {
    let Some(close) = path.find("]/") else {
        return Err(HandlerError::InvalidPath(path.to_string()));
    };
    let comment_path = &path[..=close];
    if !comment_path.starts_with("/comments/comment[") {
        return Err(HandlerError::InvalidPath(path.to_string()));
    }
    Ok((comment_path, &path[close + 1..]))
}

fn comment_body_relative_path(path: &str) -> Option<&str> {
    split_comment_body_path(path)
        .ok()
        .map(|(_, relative)| relative)
}

fn find_comment_node<'a>(node: &'a WordNode, id: &str) -> Option<&'a WordNode> {
    if matches!(&node.element_type, WordElementType::Unknown(name) if name == "comment")
        && node.attributes.get("id").map(String::as_str) == Some(id)
    {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_comment_node(child, id))
}

fn navigate_comment_body_node<'a>(
    node: &'a WordNode,
    relative: &str,
    full_path: &str,
) -> Result<&'a WordNode, HandlerError> {
    let segments = parse_path(relative)?;
    let mut current = node;
    for segment in segments {
        let matches: Vec<_> = current
            .children
            .iter()
            .filter(|child| child.element_type.to_path_name() == segment.name)
            .collect();
        let index = segment.index.unwrap_or(1);
        current = matches
            .get(index.saturating_sub(1))
            .copied()
            .ok_or_else(|| HandlerError::PathNotFound(full_path.to_string()))?;
    }
    Ok(current)
}

fn navigate_comment_body_node_mut<'a>(
    node: &'a mut WordNode,
    relative: &str,
    full_path: &str,
) -> Result<&'a mut WordNode, HandlerError> {
    let segments = parse_path(relative)?;
    let mut current = node;
    for segment in segments {
        let index = segment.index.unwrap_or(1);
        let child_index = current
            .children
            .iter()
            .enumerate()
            .filter(|(_, child)| child.element_type.to_path_name() == segment.name)
            .nth(index.saturating_sub(1))
            .map(|(index, _)| index)
            .ok_or_else(|| HandlerError::PathNotFound(full_path.to_string()))?;
        current = &mut current.children[child_index];
    }
    Ok(current)
}

fn comment_body_element_to_document_node(
    node: &WordNode,
    path: &str,
    depth: usize,
) -> DocumentNode {
    let text = node.paragraph_text();
    let mut result = DocumentNode::new(path, node.element_type.to_path_name());
    if !text.is_empty() {
        result = result.with_text(&text).with_preview(&text);
    }
    result.child_count = node.children.len();
    if depth > 0 {
        result = result.with_children(build_comment_body_children(node, path, depth - 1));
    }
    result
}

fn build_comment_body_children(
    node: &WordNode,
    parent_path: &str,
    depth: usize,
) -> Vec<DocumentNode> {
    let mut counts = HashMap::<String, usize>::new();
    node.children
        .iter()
        .map(|child| {
            let name = child.element_type.to_path_name().to_string();
            let index = counts.entry(name.clone()).or_default();
            *index += 1;
            let path = format!("{}/{}[{}]", parent_path, name, index);
            comment_body_element_to_document_node(child, &path, depth)
        })
        .collect()
}

fn set_comment_body_paragraph(
    node: &mut WordNode,
    properties: &HashMap<String, String>,
) -> Vec<String> {
    if let Some(text) = properties.get("text") {
        set_paragraph_text(node, text);
    }
    let paragraph_props: HashMap<_, _> = properties
        .iter()
        .filter(|(key, _)| COMMENT_PARAGRAPH_FORMAT_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if !paragraph_props.is_empty() {
        apply_comment_paragraph_properties(node, &paragraph_props);
    }
    let run_props: HashMap<_, _> = properties
        .iter()
        .filter(|(key, _)| COMMENT_RUN_FORMAT_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if !run_props.is_empty() {
        apply_comment_run_properties(node, &run_props);
    }
    properties
        .keys()
        .filter(|key| key.as_str() != "text" && !is_comment_format_key(key))
        .cloned()
        .collect()
}

fn set_comment_body_run(node: &mut WordNode, properties: &HashMap<String, String>) -> Vec<String> {
    if let Some(text) = properties.get("text") {
        let text_indices: Vec<_> = node
            .children
            .iter()
            .enumerate()
            .filter_map(|(index, child)| {
                (child.element_type == WordElementType::Text).then_some(index)
            })
            .collect();
        if text_indices.is_empty() {
            node.children
                .push(WordNode::new(WordElementType::Text).with_text(text));
        } else {
            for index in text_indices {
                node.children[index].text_content = Some(text.clone());
            }
        }
    }
    let run_props: HashMap<_, _> = properties
        .iter()
        .filter(|(key, _)| COMMENT_RUN_FORMAT_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if !run_props.is_empty() {
        apply_run_props_to_run(node, &run_props);
    }
    properties
        .keys()
        .filter(|key| key.as_str() != "text" && !COMMENT_RUN_FORMAT_KEYS.contains(&key.as_str()))
        .cloned()
        .collect()
}

/// Add a paragraph or run beneath an existing logical comment. This mirrors
/// normal Word `add` but targets word/comments.xml, where comment bodies are
/// their own WordprocessingML trees.
pub fn add_comment_body_element(
    package: &mut OxmlPackage,
    parent: &str,
    element_type: &str,
    position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let (comment_path, relative) = split_comment_parent_path(parent)?;
    let comment_id = get_comment(package, comment_path)?.id;
    let xml = package
        .read_part_xml(DOCX_COMMENTS_PART)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let mut dom = crate::handler::parse_document_xml(&xml)?;
    let comment = find_comment_node_mut(&mut dom.root, &comment_id)
        .ok_or_else(|| HandlerError::PathNotFound(parent.to_string()))?;
    let target = if relative.is_empty() {
        comment
    } else {
        navigate_comment_body_node_mut(comment, relative, parent)?
    };
    let normalized = element_type.to_ascii_lowercase();
    let result = match normalized.as_str() {
        "paragraph" | "p" => {
            if !relative.is_empty()
                || !matches!(&target.element_type, WordElementType::Unknown(name) if name == "comment")
            {
                return Err(HandlerError::InvalidPath(
                    "comment paragraphs must be added directly under a comment path".to_string(),
                ));
            }
            let mut paragraph = WordNode::new(WordElementType::Paragraph);
            if let Some(text) = properties.get("text") {
                paragraph.children.push(comment_body_text_run(text));
            }
            let paragraph_props: HashMap<_, _> = properties
                .iter()
                .filter(|(key, _)| COMMENT_PARAGRAPH_FORMAT_KEYS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            if !paragraph_props.is_empty() {
                apply_comment_paragraph_properties(&mut paragraph, &paragraph_props);
            }
            let run_props: HashMap<_, _> = properties
                .iter()
                .filter(|(key, _)| COMMENT_RUN_FORMAT_KEYS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            if !run_props.is_empty() {
                apply_comment_run_properties(&mut paragraph, &run_props);
            }
            insert_comment_body_child(target, paragraph, &position);
            let ordinal = target
                .children
                .iter()
                .filter(|child| child.element_type == WordElementType::Paragraph)
                .count();
            format!("{}/p[{}]", parent, ordinal)
        }
        "run" | "r" => {
            if target.element_type != WordElementType::Paragraph {
                return Err(HandlerError::InvalidPath(
                    "comment runs must be added under a comment paragraph path".to_string(),
                ));
            }
            let mut run = properties
                .get("text")
                .map(|text| comment_body_text_run(text))
                .unwrap_or_else(|| WordNode::new(WordElementType::Run));
            let run_props: HashMap<_, _> = properties
                .iter()
                .filter(|(key, _)| COMMENT_RUN_FORMAT_KEYS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            if !run_props.is_empty() {
                apply_run_props_to_run(&mut run, &run_props);
            }
            insert_comment_body_child(target, run, &position);
            let ordinal = target
                .children
                .iter()
                .filter(|child| child.element_type == WordElementType::Run)
                .count();
            format!("{}/r[{}]", parent, ordinal)
        }
        _ => {
            return Err(HandlerError::UnsupportedProperty(
                "comment body add supports only paragraph or run".to_string(),
            ))
        }
    };
    package
        .write_part_xml(DOCX_COMMENTS_PART, &crate::handler::serialize_dom(&dom))
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(result)
}

pub fn remove_comment_body_element(
    package: &mut OxmlPackage,
    path: &str,
) -> Result<(), HandlerError> {
    let (comment_path, relative) = split_comment_body_path(path)?;
    let comment_id = get_comment(package, comment_path)?.id;
    let xml = package
        .read_part_xml(DOCX_COMMENTS_PART)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let mut dom = crate::handler::parse_document_xml(&xml)?;
    let comment = find_comment_node_mut(&mut dom.root, &comment_id)
        .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?;
    let segments = parse_path(relative)?;
    remove_comment_body_node(comment, &segments, path)?;
    package
        .write_part_xml(DOCX_COMMENTS_PART, &crate::handler::serialize_dom(&dom))
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(())
}

fn split_comment_parent_path(parent: &str) -> Result<(&str, &str), HandlerError> {
    let Some(close) = parent.find(']') else {
        return Err(HandlerError::InvalidPath(parent.to_string()));
    };
    let comment_path = &parent[..=close];
    if !comment_path.starts_with("/comments/comment[") {
        return Err(HandlerError::InvalidPath(parent.to_string()));
    }
    Ok((comment_path, &parent[close + 1..]))
}

fn comment_body_text_run(text: &str) -> WordNode {
    let mut text_node = WordNode::new(WordElementType::Text).with_text(text);
    if text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace) {
        text_node
            .attributes
            .insert("xml:space".to_string(), "preserve".to_string());
        text_node.preserve_space = true;
    }
    WordNode::new(WordElementType::Run).with_children(vec![text_node])
}

fn insert_comment_body_child(parent: &mut WordNode, child: WordNode, position: &InsertPosition) {
    match position {
        InsertPosition::AtIndex(index) if *index < parent.children.len() => {
            parent.children.insert(*index, child)
        }
        _ => parent.children.push(child),
    }
}

fn remove_comment_body_node(
    parent: &mut WordNode,
    segments: &[handler_common::PathSegment],
    full_path: &str,
) -> Result<(), HandlerError> {
    let (segment, rest) = segments
        .split_first()
        .ok_or_else(|| HandlerError::InvalidPath(full_path.to_string()))?;
    let index = segment.index.unwrap_or(1);
    let child_index = parent
        .children
        .iter()
        .enumerate()
        .filter(|(_, child)| child.element_type.to_path_name() == segment.name)
        .nth(index.saturating_sub(1))
        .map(|(index, _)| index)
        .ok_or_else(|| HandlerError::PathNotFound(full_path.to_string()))?;
    if rest.is_empty() {
        if !matches!(
            parent.children[child_index].element_type,
            WordElementType::Paragraph | WordElementType::Run
        ) {
            return Err(HandlerError::UnsupportedProperty(
                "comment body remove supports only paragraph or run paths".to_string(),
            ));
        }
        parent.children.remove(child_index);
        return Ok(());
    }
    remove_comment_body_node(&mut parent.children[child_index], rest, full_path)
}

const COMMENT_PARAGRAPH_FORMAT_KEYS: &[&str] = &[
    "style",
    "pStyle",
    "alignment",
    "jc",
    "indentLeft",
    "indentRight",
    "indent",
    "firstLine",
    "hanging",
    "spacingBefore",
    "spacingAfter",
    "lineSpacing",
    "spacing",
    "keepLines",
    "keepNext",
    "outlineLevel",
    "numId",
    "numLevel",
    "listStyle",
    "border",
    "shading",
    "shd",
    "pageBreakBefore",
    "widowControl",
];

const COMMENT_RUN_FORMAT_KEYS: &[&str] = &[
    "bold",
    "b",
    "italic",
    "i",
    "underline",
    "u",
    "strike",
    "strikeout",
    "font",
    "fontFamily",
    "size",
    "fontSize",
    "color",
    "fontColor",
    "bgColor",
    "highlight",
    "bg",
    "caps",
    "smallCaps",
    "vanish",
    "hidden",
    "kern",
    "characterSpacing",
    "emphasisMark",
    "lang",
    "rightToLeft",
];

fn is_comment_format_key(key: &str) -> bool {
    COMMENT_PARAGRAPH_FORMAT_KEYS.contains(&key) || COMMENT_RUN_FORMAT_KEYS.contains(&key)
}

/// Apply C#-compatible direct formatting to a comment body without flattening
/// its paragraphs or runs. comments.xml shares WordprocessingML's node shape,
/// so reuse the normal DOM parser/serializer and run-property merger.
fn apply_comment_body_format(
    xml: &str,
    comment_id: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let mut dom = crate::handler::parse_document_xml(xml)?;
    let comment = find_comment_node_mut(&mut dom.root, comment_id).ok_or_else(|| {
        HandlerError::PathNotFound(format!("comment id '{}' not found", comment_id))
    })?;
    let paragraph_props: HashMap<_, _> = properties
        .iter()
        .filter(|(key, _)| COMMENT_PARAGRAPH_FORMAT_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let run_props: HashMap<_, _> = properties
        .iter()
        .filter(|(key, _)| COMMENT_RUN_FORMAT_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if !paragraph_props.is_empty() {
        if let Some(paragraph) = find_first_paragraph_mut(comment) {
            apply_comment_paragraph_properties(paragraph, &paragraph_props);
        }
    }
    if !run_props.is_empty() {
        apply_comment_run_properties(comment, &run_props);
    }
    Ok(crate::handler::serialize_dom(&dom))
}

fn find_comment_node_mut<'a>(node: &'a mut WordNode, id: &str) -> Option<&'a mut WordNode> {
    if matches!(&node.element_type, WordElementType::Unknown(name) if name == "comment")
        && node.attributes.get("id").map(String::as_str) == Some(id)
    {
        return Some(node);
    }
    for child in &mut node.children {
        if let Some(comment) = find_comment_node_mut(child, id) {
            return Some(comment);
        }
    }
    None
}

fn find_first_paragraph_mut(node: &mut WordNode) -> Option<&mut WordNode> {
    if node.element_type == WordElementType::Paragraph {
        return Some(node);
    }
    for child in &mut node.children {
        if let Some(paragraph) = find_first_paragraph_mut(child) {
            return Some(paragraph);
        }
    }
    None
}

fn apply_comment_paragraph_properties(
    paragraph: &mut WordNode,
    properties: &HashMap<String, String>,
) {
    let existing = paragraph
        .children
        .iter()
        .find(|child| child.element_type == WordElementType::ParagraphProperties);
    let merged = existing
        .map(|node| merge_ppr_into_props(node, properties))
        .unwrap_or_else(|| properties.clone());
    paragraph
        .children
        .retain(|child| child.element_type != WordElementType::ParagraphProperties);
    if let Some(ppr) = build_paragraph_properties(&merged) {
        paragraph.children.insert(0, ppr);
    }
}

fn apply_comment_run_properties(node: &mut WordNode, properties: &HashMap<String, String>) {
    if node.element_type == WordElementType::Run {
        apply_run_props_to_run(node, properties);
        return;
    }
    for child in &mut node.children {
        apply_comment_run_properties(child, properties);
    }
}

fn replace_comment_body(block: &str, text: &str) -> Result<String, HandlerError> {
    let open_end = find_tag_close_after(block, 0)
        .ok_or_else(|| HandlerError::OperationFailed("malformed comment element".to_string()))?;
    let close = block
        .rfind("</w:comment>")
        .ok_or_else(|| HandlerError::OperationFailed("unterminated comment".to_string()))?;
    // commentsExtended.xml identifies a comment by the first comment-body
    // paragraph's w14:paraId. Preserve that id when replacing the body so a
    // text edit cannot silently sever reply threading or resolved state.
    let para_id_attr = first_comment_para_id(block)
        .map(|id| {
            format!(
                " xmlns:w14=\"{}\" w14:paraId=\"{}\"",
                W14_NS,
                escape_attr(&id)
            )
        })
        .unwrap_or_default();
    let preserve = text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace);
    let space_attr = if preserve {
        " xml:space=\"preserve\""
    } else {
        ""
    };
    let body = format!(
        "<w:p{}><w:r><w:t{}>{}</w:t></w:r></w:p>",
        para_id_attr,
        space_attr,
        xml_escape_text(text)
    );
    Ok(format!(
        "{}{}{}",
        &block[..=open_end],
        body,
        &block[close..]
    ))
}

/// Set properties on a footnote or endnote. The same XML shape applies to both.
/// Path is `/footnotes/<id>` or `/endnotes/<id>`.
///
/// Supported properties: text
pub fn set_footnote_endnote_on_part(
    package: &mut OxmlPackage,
    part: &str,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let (kind, prefix) = if part.ends_with("footnotes.xml") {
        ("footnote", "footnotes")
    } else {
        ("endnote", "endnotes")
    };
    let note_id = extract_id_from_path(path, prefix)?;
    let xml = package
        .read_part_xml(part)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let needle = format!("w:id=\"{}\"", note_id);
    let id_offset = xml.find(&needle).ok_or_else(|| {
        HandlerError::PathNotFound(format!("{} id '{}' not found in {}", kind, note_id, part))
    })?;

    let open_tag = format!("<w:{}", kind);
    let close_tag = format!("</w:{}>", kind);
    let open = xml[..id_offset]
        .rfind(&open_tag)
        .ok_or_else(|| HandlerError::OperationFailed(format!("malformed {} element", kind)))?;
    let close = find_matching_close(&xml, open, &open_tag, &close_tag)
        .ok_or_else(|| HandlerError::OperationFailed(format!("unterminated {}", kind)))?;

    let block = &xml[open..close];
    let mut new_block = block.to_string();
    let mut unsupported = Vec::new();

    for (key, value) in properties {
        match key.as_str() {
            "text" => {
                let opts = FindReplaceOptions::default();
                let (replaced, n) = replace_first_text_node(&new_block, value, &opts);
                new_block = replaced;
                if n == 0 {
                    unsupported.push("text".to_string());
                }
            }
            _ => unsupported.push(key.clone()),
        }
    }

    if new_block != block {
        let mut new_xml = String::with_capacity(xml.len() + new_block.len());
        new_xml.push_str(&xml[..open]);
        new_xml.push_str(&new_block);
        new_xml.push_str(&xml[close..]);
        package
            .write_part_xml(part, &new_xml)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    }

    Ok(unsupported)
}

// ─── XML helper utilities ─────────────────────────────────────────────

/// Find the matching close tag for `open_tag` starting at `open_start`.
/// Simple depth-counting: each nested `<w:style ...` increments, each
/// `</w:style>` decrements; the close that brings the count to zero is the match.
fn find_matching_close(
    xml: &str,
    open_start: usize,
    open_prefix: &str,
    close_tag: &str,
) -> Option<usize> {
    let mut depth = 0;
    let mut cursor = open_start;
    while let Some(pos) = xml[cursor..].find(open_prefix) {
        let abs = cursor + pos;
        // Skip self-closing tags `<w:tag .../>`.
        let next_close = xml[abs..].find('>')?;
        let tag_end = abs + next_close;
        let is_self_closing = xml[..tag_end].ends_with('/');
        // Only count as an open if it's not self-closing.
        if !is_self_closing {
            depth += 1;
        }
        cursor = tag_end + 1;
        // From here, search for the next close_tag.
        if let Some(c_rel) = xml[cursor..].find(close_tag) {
            let c_abs = cursor + c_rel;
            depth -= 1;
            if depth == 0 {
                return Some(c_abs + close_tag.len());
            }
            cursor = c_abs + close_tag.len();
        } else {
            break;
        }
        // Continue searching for more opens.
    }
    None
}

/// Set or replace a child element of the form `<w:name w:val="VALUE"/>`.
/// Removes any existing child first, then inserts a new one right after
/// the opening `<w:style ...>` tag.
fn set_or_replace_attr_child(block: &mut String, child_tag: &str, attr: &str, value: &str) {
    let open = format!("<{}", child_tag);
    let open_close = format!("</{}>", child_tag);
    // Remove existing child of the same tag in either self-closing or
    // open/close form.
    if let Some(start) = block.find(&open) {
        // Try self-closing form first.
        if let Some(end_rel) = block[start..].find("/>") {
            let end_abs = start + end_rel + 2;
            block.replace_range(start..end_abs, "");
        } else if let Some(close_rel) = block[start..].find(&open_close) {
            let end_abs = start + close_rel + open_close.len();
            block.replace_range(start..end_abs, "");
        }
    }
    // Insert the new child immediately after the opening <w:style ...> tag.
    if let Some(gt) = block.find('>') {
        let new_child = format!("<{} {}=\"{}\"/>", child_tag, attr, escape_attr(value));
        block.insert_str(gt + 1, &new_child);
    }
    let _ = attr; // currently fixed to w:val
}

/// Toggle a flag-style child element (`<w:hidden/>` present = true).
fn toggle_flag_child(block: &mut String, child_tag: &str, value: &str) {
    let on = value == "true" || value == "1" || value.is_empty();
    let open = format!("<{}", child_tag);
    let exists = block.contains(&open);
    if on && !exists {
        if let Some(gt) = block.find('>') {
            let new_child = format!("<{}/>", child_tag);
            block.insert_str(gt + 1, &new_child);
        }
    } else if !on && exists {
        if let Some(start) = block.find(&open) {
            if let Some(end_rel) = block[start..].find("/>") {
                let end_abs = start + end_rel + 2;
                block.replace_range(start..end_abs, "");
            }
        }
    }
}

/// Set or replace an attribute on the first opening tag in `block`.
fn set_attr_on_open_tag(block: &mut String, attr: &str, value: &str) {
    let open_end = match block.find('>') {
        Some(p) => p,
        None => return,
    };
    let open_tag = &block[..open_end];
    let attr_pattern = format!("{}=\"", attr);
    if let Some(attr_start) = open_tag.find(&attr_pattern) {
        let val_start = attr_start + attr_pattern.len();
        let val_end = block[val_start..]
            .find('"')
            .map(|e| val_start + e)
            .unwrap_or(val_start);
        block.replace_range(val_start..val_end, &escape_attr(value));
    } else {
        // Insert before the closing > of the opening tag.
        let insert_at = if block.as_bytes()[open_end - 1] == b'/' {
            open_end - 1
        } else {
            open_end
        };
        let insertion = format!(" {}=\"{}\"", attr, escape_attr(value));
        block.insert_str(insert_at, &insertion);
    }
}

/// Replace the first `<w:t>...</w:t>` content with `new_text`.
fn replace_first_text_node(
    block: &str,
    new_text: &str,
    opts: &FindReplaceOptions,
) -> (String, usize) {
    let Some(t_start) = block.find("<w:t") else {
        return (block.to_string(), 0);
    };
    let Some(close_after_open) = block[t_start..].find('>') else {
        return (block.to_string(), 0);
    };
    let open_end = t_start + close_after_open + 1;
    let Some(t_close_rel) = block[open_end..].find("</w:t>") else {
        return (block.to_string(), 0);
    };
    let t_close = open_end + t_close_rel;
    let mut out = String::with_capacity(block.len() + new_text.len());
    out.push_str(&block[..open_end]);
    out.push_str(new_text);
    out.push_str(&block[t_close..]);
    // We don't actually do find/replace here — we replace the whole run text.
    // Return count = 1 to signal one substitution made.
    let _ = opts;
    (out, 1)
}

fn extract_id_from_path(path: &str, prefix: &str) -> Result<String, HandlerError> {
    let rest = path
        .trim_start_matches('/')
        .strip_prefix(prefix)
        .ok_or_else(|| {
            HandlerError::InvalidArgument(format!("expected '/{}/<id>', got '{}'", prefix, path))
        })?;
    let rest = rest.trim_start_matches('/');
    // `/comments/comment[@commentId=7]` is the stable public path.  Do not
    // return the literal `@commentId=7`: callers need the OOXML w:id value.
    if let Some(marker) = rest.find("[@commentId=") {
        let value_start = marker + "[@commentId=".len();
        let value_end = rest[value_start..]
            .find(']')
            .map(|n| value_start + n)
            .unwrap_or(rest.len());
        return Ok(rest[value_start..value_end]
            .trim_matches(|c| c == '\'' || c == '"')
            .to_string());
    }
    if let Some(bracket) = rest.find('[') {
        let inner = &rest[bracket + 1..rest.find(']').unwrap_or(rest.len())];
        return Ok(inner.to_string());
    }
    Ok(rest.to_string())
}

/// A semantic view of one entry in word/comments.xml.  Keeping this small
/// structure at the package boundary lets get/query/remove operate on comments
/// without pretending that comments are children of word/document.xml.
#[derive(Debug, Clone)]
pub struct DocxComment {
    pub id: String,
    pub author: Option<String>,
    pub initials: Option<String>,
    pub date: Option<String>,
    pub text: String,
    pub anchor: Option<String>,
    pub parent_id: Option<String>,
    pub done: bool,
    para_id: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum CommentAnchorMode {
    Range,
    Point,
    OpenRange,
}

/// Add a Word comment and its document anchor as one package-level mutation.
/// A comment is only valid when all of comments.xml, its relationship/content
/// type, and the three document markers agree on the same id.
pub fn add_comment_part_aware(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    if is_truthy(
        properties
            .get("rangeEnd")
            .or_else(|| properties.get("rangeend")),
    ) {
        return close_open_comment_range(package, parent, properties);
    }
    let text = properties.get("text").ok_or_else(|| {
        HandlerError::InvalidArgument("'text' property is required for comment type".to_string())
    })?;
    let mut comments_xml = ensure_comments_part(package)?;
    let existing = parse_comments(&comments_xml)?;
    let requested_id = properties
        .get("commentId")
        .or_else(|| properties.get("commentid"))
        .or_else(|| properties.get("id"));
    let id = if let Some(id) = requested_id {
        if existing.iter().any(|c| c.id == *id) {
            return Err(HandlerError::InvalidArgument(format!(
                "comment id '{}' already exists",
                id
            )));
        }
        id.clone()
    } else {
        existing
            .iter()
            .filter_map(|c| c.id.parse::<u64>().ok())
            .max()
            .map(|n| n + 1)
            .unwrap_or(0)
            .to_string()
    };

    let author = properties
        .get("author")
        .map(String::as_str)
        .unwrap_or("officecli");
    let initials = properties
        .get("initials")
        .cloned()
        .unwrap_or_else(|| author.chars().next().unwrap_or('o').to_string());
    let generated_date = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| HandlerError::OperationFailed(format!("format comment date: {}", e)))?;
    let date = properties
        .get("date")
        .map(String::as_str)
        .unwrap_or(&generated_date);
    let parent_id = properties
        .get("parentId")
        .or_else(|| properties.get("parentid"))
        .cloned();
    let done = properties
        .get("done")
        .or_else(|| properties.get("resolved"))
        .is_some_and(|value| is_truthy(Some(value)));
    let needs_extension = parent_id.is_some()
        || properties.contains_key("done")
        || properties.contains_key("resolved");
    let comment_para_id = properties
        .get("commentParaId")
        .or_else(|| properties.get("commentparaid"))
        .cloned()
        .or_else(|| needs_extension.then(crate::para_id::generate_para_id));
    if let Some(parent_id) = parent_id.as_deref() {
        let parent = existing
            .iter()
            .find(|comment| comment.id == parent_id)
            .ok_or_else(|| {
                HandlerError::InvalidArgument(format!(
                    "parentId={}: no comment with that id",
                    parent_id
                ))
            })?;
        if parent.para_id.is_none() {
            let (updated, _) = assign_comment_para_id(&comments_xml, parent_id)?;
            comments_xml = updated;
        }
    }
    let comment_xml = build_comment_xml(
        &id,
        author,
        &initials,
        Some(date),
        text,
        comment_para_id.as_deref(),
    );

    let definition_only = properties
        .get("range")
        .is_some_and(|range| range.eq_ignore_ascii_case("none"));
    let updated_document = if definition_only {
        None
    } else {
        let document_xml = package
            .read_part_xml(DOCX_DOCUMENT_PART)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        let mut dom = crate::handler::parse_document_xml(&document_xml)?;
        let point_ref = is_truthy(
            properties
                .get("pointRef")
                .or_else(|| properties.get("pointref")),
        ) || properties
            .get("range")
            .is_some_and(|range| is_explicit_false(range));
        let mode = if point_ref {
            CommentAnchorMode::Point
        } else if is_truthy(
            properties
                .get("rangeOpen")
                .or_else(|| properties.get("rangeopen")),
        ) {
            CommentAnchorMode::OpenRange
        } else {
            CommentAnchorMode::Range
        };
        insert_comment_anchor(&mut dom, parent, &id, properties, mode)?;
        Some(crate::handler::serialize_dom(&dom))
    };

    let comments_close = comments_xml.rfind("</w:comments>").ok_or_else(|| {
        HandlerError::OperationFailed("malformed comments.xml: missing </w:comments>".to_string())
    })?;
    let mut updated_comments = String::with_capacity(comments_xml.len() + comment_xml.len());
    updated_comments.push_str(&comments_xml[..comments_close]);
    updated_comments.push_str(&comment_xml);
    updated_comments.push_str(&comments_xml[comments_close..]);

    if let Some(document) = updated_document {
        package
            .write_part_xml(DOCX_DOCUMENT_PART, &document)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    }
    package
        .write_part_xml(DOCX_COMMENTS_PART, &updated_comments)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    if let Some(parent_id) = parent_id.as_deref() {
        let current = parse_comments(&updated_comments)?;
        let parent_para_id = current
            .iter()
            .find(|comment| comment.id == parent_id)
            .and_then(|comment| comment.para_id.clone())
            .ok_or_else(|| {
                HandlerError::OperationFailed("parent comment missing paraId".to_string())
            })?;
        let child_para_id = comment_para_id.clone().ok_or_else(|| {
            HandlerError::OperationFailed("reply comment missing generated paraId".to_string())
        })?;
        upsert_comment_extension(package, &parent_para_id, None, None)?;
        upsert_comment_extension(package, &child_para_id, Some(&parent_para_id), Some(false))?;
    } else if needs_extension {
        let para_id = comment_para_id.clone().ok_or_else(|| {
            HandlerError::OperationFailed("comment metadata missing generated paraId".to_string())
        })?;
        upsert_comment_extension(package, &para_id, None, Some(done))?;
    }

    Ok(format!("/comments/comment[@commentId={}]", id))
}

/// Return comments as semantic records. Missing comments.xml is a valid empty
/// comment collection, matching a freshly-created Word document.
pub fn list_comments(package: &OxmlPackage) -> Result<Vec<DocxComment>, HandlerError> {
    let Ok(xml) = package.read_part_xml(DOCX_COMMENTS_PART) else {
        return Ok(Vec::new());
    };
    let mut comments = parse_comments(&xml)?;
    let extensions = read_comment_extensions(package)?;
    let ids_by_para: HashMap<String, String> = comments
        .iter()
        .filter_map(|comment| {
            comment
                .para_id
                .as_ref()
                .map(|para_id| (para_id.clone(), comment.id.clone()))
        })
        .collect();
    let document_xml = package
        .read_part_xml(DOCX_DOCUMENT_PART)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    for comment in &mut comments {
        comment.anchor = find_comment_anchor(&document_xml, &comment.id);
        if let Some(para_id) = &comment.para_id {
            if let Some(extension) = extensions.get(para_id) {
                comment.done = extension.done;
                comment.parent_id = extension
                    .parent_para_id
                    .as_ref()
                    .and_then(|parent| ids_by_para.get(parent).cloned());
            }
        }
    }
    Ok(comments)
}

pub fn get_comment(package: &OxmlPackage, path: &str) -> Result<DocxComment, HandlerError> {
    let comments = list_comments(package)?;
    let requested = extract_id_from_path(path, "comments")?;
    if path.contains("[@commentId=") {
        comments
            .into_iter()
            .find(|c| c.id == requested)
            .ok_or_else(|| {
                HandlerError::PathNotFound(format!("comment id '{}' not found", requested))
            })
    } else if let Ok(index) = requested.parse::<usize>() {
        if path.contains("comment[") {
            comments
                .into_iter()
                .nth(index.saturating_sub(1))
                .ok_or_else(|| {
                    HandlerError::PathNotFound(format!("comment index '{}' not found", index))
                })
        } else {
            comments
                .into_iter()
                .find(|c| c.id == requested)
                .ok_or_else(|| {
                    HandlerError::PathNotFound(format!("comment id '{}' not found", requested))
                })
        }
    } else {
        Err(HandlerError::InvalidPath(format!(
            "invalid comment path '{}'",
            path
        )))
    }
}

/// Remove a comment definition and every document marker that refers to it.
/// This avoids Word's orphan-comment repair prompt and mirrors the C# handler's
/// anchor cleanup behaviour.
pub fn remove_comment_part_aware(
    package: &mut OxmlPackage,
    path: &str,
) -> Result<(), HandlerError> {
    let target = get_comment(package, path)?;
    let comments_xml = package
        .read_part_xml(DOCX_COMMENTS_PART)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let new_comments = remove_comment_block(&comments_xml, &target.id)?;
    let document_xml = package
        .read_part_xml(DOCX_DOCUMENT_PART)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let new_document = remove_comment_markers(&document_xml, &target.id);

    package
        .write_part_xml(DOCX_COMMENTS_PART, &new_comments)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    package
        .write_part_xml(DOCX_DOCUMENT_PART, &new_document)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(())
}

fn ensure_comments_part(package: &mut OxmlPackage) -> Result<String, HandlerError> {
    let comments_xml = if package.has_part(DOCX_COMMENTS_PART) {
        package
            .read_part_xml(DOCX_COMMENTS_PART)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?
    } else {
        let xml = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>",
            "<w:comments xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"/>",
        )
        .to_string();
        package
            .write_part_xml(DOCX_COMMENTS_PART, &xml)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
        xml
    };
    ensure_docx_comments_relationship(package)?;
    ensure_docx_comments_content_type(package)?;
    Ok(if comments_xml.trim_end().ends_with("/>") {
        comments_xml.replacen("/>", "></w:comments>", 1)
    } else {
        comments_xml
    })
}

fn ensure_docx_comments_relationship(package: &mut OxmlPackage) -> Result<(), HandlerError> {
    let rels = package
        .read_part_xml(DOCX_DOCUMENT_RELS_PART)
        .unwrap_or_default();
    if rels.contains(DOCX_COMMENTS_REL_TYPE) {
        return Ok(());
    }
    let rid = next_docx_rel_id(package, DOCX_DOCUMENT_RELS_PART);
    let relationship = format!(
        "<Relationship Id=\"{}\" Type=\"{}\" Target=\"comments.xml\"/>",
        rid, DOCX_COMMENTS_REL_TYPE
    );
    inject_docx_relationship(package, DOCX_DOCUMENT_RELS_PART, &relationship)
}

fn ensure_docx_comments_content_type(package: &mut OxmlPackage) -> Result<(), HandlerError> {
    let xml = package
        .read_part_xml("[Content_Types].xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    if xml.contains("PartName=\"/word/comments.xml\"") {
        return Ok(());
    }
    let override_xml = format!(
        "<Override PartName=\"/word/comments.xml\" ContentType=\"{}\"/>",
        DOCX_COMMENTS_CONTENT_TYPE
    );
    let types_open = xml.find("<Types").ok_or_else(|| {
        HandlerError::OperationFailed("malformed [Content_Types].xml".to_string())
    })?;
    let insertion = find_tag_close_after(&xml, types_open).ok_or_else(|| {
        HandlerError::OperationFailed("malformed [Content_Types].xml".to_string())
    })? + 1;
    let mut updated = String::with_capacity(xml.len() + override_xml.len());
    updated.push_str(&xml[..insertion]);
    updated.push_str(&override_xml);
    updated.push_str(&xml[insertion..]);
    package
        .write_part_xml("[Content_Types].xml", &updated)
        .map_err(|e| HandlerError::SaveError(e.to_string()))
}

fn build_comment_xml(
    id: &str,
    author: &str,
    initials: &str,
    date: Option<&str>,
    text: &str,
    para_id: Option<&str>,
) -> String {
    let date_attr = date
        .map(|value| format!(" w:date=\"{}\"", escape_attr(value)))
        .unwrap_or_default();
    let preserve = text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace);
    let space_attr = if preserve {
        " xml:space=\"preserve\""
    } else {
        ""
    };
    let para_id_attr = para_id
        .map(|value| {
            format!(
                " xmlns:w14=\"{}\" w14:paraId=\"{}\"",
                W14_NS,
                escape_attr(value)
            )
        })
        .unwrap_or_default();
    format!(
        "<w:comment w:id=\"{}\" w:author=\"{}\" w:initials=\"{}\"{}><w:p{}><w:r><w:t{}>{}</w:t></w:r></w:p></w:comment>",
        escape_attr(id),
        escape_attr(author),
        escape_attr(initials),
        date_attr,
        para_id_attr,
        space_attr,
        xml_escape_text(text)
    )
}

fn assign_comment_para_id(xml: &str, comment_id: &str) -> Result<(String, String), HandlerError> {
    let needle = format!("w:id=\"{}\"", comment_id);
    let id_pos = xml.find(&needle).ok_or_else(|| {
        HandlerError::PathNotFound(format!("comment id '{}' not found", comment_id))
    })?;
    let comment_start = xml[..id_pos]
        .rfind("<w:comment")
        .ok_or_else(|| HandlerError::OperationFailed("malformed comments.xml".to_string()))?;
    let comment_end = find_matching_close(xml, comment_start, "<w:comment", "</w:comment>")
        .ok_or_else(|| HandlerError::OperationFailed("unterminated comment".to_string()))?;
    let paragraph_rel = xml[comment_start..comment_end]
        .find("<w:p")
        .ok_or_else(|| {
            HandlerError::OperationFailed("comment has no paragraph for metadata".to_string())
        })?;
    let paragraph_start = comment_start + paragraph_rel;
    let paragraph_end = find_tag_close_after(xml, paragraph_start)
        .ok_or_else(|| HandlerError::OperationFailed("malformed comment paragraph".to_string()))?;
    if let Some(existing) = xml_attribute(&xml[paragraph_start..=paragraph_end], "w14:paraId") {
        return Ok((xml.to_string(), existing));
    }
    let para_id = crate::para_id::generate_para_id();
    let insertion = format!(" xmlns:w14=\"{}\" w14:paraId=\"{}\"", W14_NS, para_id);
    let mut updated = String::with_capacity(xml.len() + insertion.len());
    updated.push_str(&xml[..paragraph_end]);
    updated.push_str(&insertion);
    updated.push_str(&xml[paragraph_end..]);
    Ok((updated, para_id))
}

fn insert_comment_anchor(
    dom: &mut WordDom,
    parent: &str,
    id: &str,
    properties: &HashMap<String, String>,
    mode: CommentAnchorMode,
) -> Result<(), HandlerError> {
    let segments = parse_path(parent)?;
    let last = segments
        .last()
        .ok_or_else(|| HandlerError::InvalidPath("empty comment parent".to_string()))?;
    let start = WordNode::new(WordElementType::Unknown("commentRangeStart".to_string()))
        .with_attribute("id", id);
    let end = WordNode::new(WordElementType::Unknown("commentRangeEnd".to_string()))
        .with_attribute("id", id);
    let reference = WordNode::new(WordElementType::Run)
        .with_children(vec![
            WordNode::new(WordElementType::CommentReference).with_attribute("id", id)
        ]);

    if last.name == "r" {
        let run_number = last.index.ok_or_else(|| {
            HandlerError::InvalidPath("comment run parent must include a 1-based index".to_string())
        })?;
        let paragraph_path = parent.rsplit_once('/').map(|(p, _)| p).unwrap_or(parent);
        let paragraph = navigate_to_element_mut(dom, paragraph_path)?;
        if paragraph.element_type != WordElementType::Paragraph {
            return Err(HandlerError::InvalidArgument(
                "comments must be added to a paragraph or a direct run inside one".to_string(),
            ));
        }
        let run_index = nth_direct_run_index(paragraph, run_number).ok_or_else(|| {
            HandlerError::PathNotFound(format!(
                "run {} not found in '{}'",
                run_number, paragraph_path
            ))
        })?;
        match mode {
            CommentAnchorMode::Point => paragraph.children.insert(run_index + 1, reference),
            CommentAnchorMode::OpenRange => paragraph.children.insert(run_index, start),
            CommentAnchorMode::Range => {
                paragraph.children.insert(run_index, start);
                paragraph.children.insert(run_index + 2, end);
                paragraph.children.insert(run_index + 3, reference);
            }
        }
        return Ok(());
    }

    if last.name != "p" {
        return Err(HandlerError::InvalidArgument(
            "comments must be added to a paragraph or run: /body/p[N] or /body/p[N]/r[M]"
                .to_string(),
        ));
    }
    let paragraph = navigate_to_element_mut(dom, parent)?;
    let run_start = properties
        .get("runStart")
        .or_else(|| properties.get("runstart"))
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| {
            HandlerError::InvalidArgument("runStart must be a non-negative integer".to_string())
        })?
        .unwrap_or(0);
    let start_index = if run_start == 0 {
        paragraph
            .children
            .iter()
            .position(|child| child.element_type != WordElementType::ParagraphProperties)
            .unwrap_or(paragraph.children.len())
    } else {
        nth_direct_run_index(paragraph, run_start)
            .map(|index| index + 1)
            .ok_or_else(|| {
                HandlerError::PathNotFound(format!("run {} not found in '{}'", run_start, parent))
            })?
    };
    match mode {
        CommentAnchorMode::Point => {
            let point_index = if run_start == 0 {
                paragraph.children.len()
            } else {
                start_index
            };
            paragraph.children.insert(point_index, reference);
        }
        CommentAnchorMode::OpenRange => paragraph.children.insert(start_index, start),
        CommentAnchorMode::Range => {
            paragraph.children.insert(start_index, start);
            paragraph.children.push(end);
            paragraph.children.push(reference);
        }
    }
    Ok(())
}

fn close_open_comment_range(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let document_xml = package
        .read_part_xml(DOCX_DOCUMENT_PART)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let mut dom = crate::handler::parse_document_xml(&document_xml)?;
    let id = last_open_comment_range_id(&dom).ok_or_else(|| {
        HandlerError::InvalidArgument(
            "comment rangeEnd has no matching open comment range (add with rangeOpen=true first)"
                .to_string(),
        )
    })?;
    insert_comment_range_end(&mut dom, parent, &id, properties)?;
    package
        .write_part_xml(DOCX_DOCUMENT_PART, &crate::handler::serialize_dom(&dom))
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(format!("/comments/comment[@commentId={}]/rangeEnd", id))
}

fn last_open_comment_range_id(dom: &WordDom) -> Option<String> {
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    collect_comment_markers(&dom.root, &mut starts, &mut ends);
    starts.into_iter().rev().find(|id| !ends.contains(id))
}

fn collect_comment_markers(node: &WordNode, starts: &mut Vec<String>, ends: &mut Vec<String>) {
    match &node.element_type {
        WordElementType::Unknown(name) if name == "commentRangeStart" => {
            if let Some(id) = node.attributes.get("id") {
                starts.push(id.clone());
            }
        }
        WordElementType::Unknown(name) if name == "commentRangeEnd" => {
            if let Some(id) = node.attributes.get("id") {
                ends.push(id.clone());
            }
        }
        _ => {}
    }
    for child in &node.children {
        collect_comment_markers(child, starts, ends);
    }
}

fn insert_comment_range_end(
    dom: &mut WordDom,
    parent: &str,
    id: &str,
    properties: &HashMap<String, String>,
) -> Result<(), HandlerError> {
    let segments = parse_path(parent)?;
    let last = segments
        .last()
        .ok_or_else(|| HandlerError::InvalidPath("empty comment parent".to_string()))?;
    let end = WordNode::new(WordElementType::Unknown("commentRangeEnd".to_string()))
        .with_attribute("id", id);
    let reference = WordNode::new(WordElementType::Run)
        .with_children(vec![
            WordNode::new(WordElementType::CommentReference).with_attribute("id", id)
        ]);
    if last.name == "r" {
        let run = last.index.ok_or_else(|| {
            HandlerError::InvalidPath("comment run parent must include a 1-based index".to_string())
        })?;
        let paragraph_path = parent.rsplit_once('/').map(|(p, _)| p).unwrap_or(parent);
        let paragraph = navigate_to_element_mut(dom, paragraph_path)?;
        let index = nth_direct_run_index(paragraph, run).ok_or_else(|| {
            HandlerError::PathNotFound(format!("run {} not found in '{}'", run, paragraph_path))
        })? + 1;
        paragraph.children.insert(index, end);
        paragraph.children.insert(index + 1, reference);
        return Ok(());
    }
    if last.name != "p" {
        return Err(HandlerError::InvalidArgument(
            "comments must be added to a paragraph or run: /body/p[N] or /body/p[N]/r[M]"
                .to_string(),
        ));
    }
    let paragraph = navigate_to_element_mut(dom, parent)?;
    let run_end = properties
        .get("runEnd")
        .or_else(|| properties.get("runend"))
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| {
            HandlerError::InvalidArgument("runEnd must be a non-negative integer".to_string())
        })?
        .unwrap_or(0);
    if run_end == 0 {
        paragraph.children.push(end);
        paragraph.children.push(reference);
    } else {
        let index = nth_direct_run_index(paragraph, run_end).ok_or_else(|| {
            HandlerError::PathNotFound(format!("run {} not found in '{}'", run_end, parent))
        })? + 1;
        paragraph.children.insert(index, end);
        paragraph.children.insert(index + 1, reference);
    }
    Ok(())
}

fn is_truthy(value: Option<&String>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()),
        Some(value) if matches!(value.as_str(), "true" | "1" | "yes" | "on")
    )
}

fn is_explicit_false(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "false" | "0" | "no" | "off"
    )
}

fn nth_direct_run_index(paragraph: &WordNode, number: usize) -> Option<usize> {
    paragraph
        .children
        .iter()
        .enumerate()
        .filter(|(_, child)| child.element_type == WordElementType::Run)
        .nth(number.saturating_sub(1))
        .map(|(index, _)| index)
}

fn parse_comments(xml: &str) -> Result<Vec<DocxComment>, HandlerError> {
    let mut comments = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = xml[cursor..].find("<w:comment") {
        let open = cursor + relative;
        let after_name = xml.as_bytes().get(open + "<w:comment".len()).copied();
        if !matches!(
            after_name,
            Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') | Some(b'>')
        ) {
            cursor = open + 1;
            continue;
        }
        let close =
            find_matching_close(xml, open, "<w:comment", "</w:comment>").ok_or_else(|| {
                HandlerError::OperationFailed("unterminated comment in comments.xml".to_string())
            })?;
        let block = &xml[open..close];
        let open_end = find_tag_close_after(block, 0).ok_or_else(|| {
            HandlerError::OperationFailed("malformed comment opening tag".to_string())
        })?;
        let opening = &block[..=open_end];
        let id = xml_attribute(opening, "w:id")
            .ok_or_else(|| HandlerError::OperationFailed("comment without w:id".to_string()))?;
        comments.push(DocxComment {
            id,
            author: xml_attribute(opening, "w:author"),
            initials: xml_attribute(opening, "w:initials"),
            date: xml_attribute(opening, "w:date"),
            text: collect_word_text(block),
            anchor: None,
            parent_id: None,
            done: false,
            para_id: first_comment_para_id(block),
        });
        cursor = close;
    }
    Ok(comments)
}

fn xml_attribute(tag: &str, name: &str) -> Option<String> {
    let marker = format!("{}=\"", name);
    let start = tag.find(&marker)? + marker.len();
    let end = tag[start..].find('"')? + start;
    Some(xml_unescape(&tag[start..end]))
}

fn collect_word_text(xml: &str) -> String {
    let mut text = String::new();
    let mut cursor = 0;
    while let Some(relative) = xml[cursor..].find("<w:t") {
        let open = cursor + relative;
        let Some(open_end) = find_tag_close_after(xml, open) else {
            break;
        };
        let content_start = open_end + 1;
        let Some(close_relative) = xml[content_start..].find("</w:t>") else {
            break;
        };
        let content_end = content_start + close_relative;
        text.push_str(&xml_unescape(&xml[content_start..content_end]));
        cursor = content_end + "</w:t>".len();
    }
    text
}

fn first_comment_para_id(comment_xml: &str) -> Option<String> {
    let p_start = comment_xml.find("<w:p")?;
    let p_end = find_tag_close_after(comment_xml, p_start)?;
    xml_attribute(&comment_xml[p_start..=p_end], "w14:paraId")
}

#[derive(Debug, Clone)]
struct CommentExtension {
    parent_para_id: Option<String>,
    done: bool,
}

fn read_comment_extensions(
    package: &OxmlPackage,
) -> Result<HashMap<String, CommentExtension>, HandlerError> {
    let Ok(xml) = package.read_part_xml(DOCX_COMMENTS_EXT_PART) else {
        return Ok(HashMap::new());
    };
    let mut result = HashMap::new();
    let mut cursor = 0;
    while let Some(relative) = xml[cursor..].find("<w15:commentEx") {
        let start = cursor + relative;
        let Some(end) = find_tag_close_after(&xml, start) else {
            return Err(HandlerError::OperationFailed(
                "malformed commentsExtended.xml".to_string(),
            ));
        };
        let tag = &xml[start..=end];
        if let Some(para_id) = xml_attribute(tag, "w15:paraId") {
            let done = xml_attribute(tag, "w15:done")
                .as_ref()
                .map(|value| is_truthy(Some(value)))
                .unwrap_or(false);
            result.insert(
                para_id,
                CommentExtension {
                    parent_para_id: xml_attribute(tag, "w15:paraIdParent"),
                    done,
                },
            );
        }
        cursor = end + 1;
    }
    Ok(result)
}

fn ensure_comment_extension_part(package: &mut OxmlPackage) -> Result<String, HandlerError> {
    let xml = if package.has_part(DOCX_COMMENTS_EXT_PART) {
        package
            .read_part_xml(DOCX_COMMENTS_EXT_PART)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?
    } else {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w15:commentsEx xmlns:w15=\"{}\"></w15:commentsEx>",
            W15_NS
        );
        package
            .write_part_xml(DOCX_COMMENTS_EXT_PART, &xml)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
        xml
    };
    let rels = package
        .read_part_xml(DOCX_DOCUMENT_RELS_PART)
        .unwrap_or_default();
    if !rels.contains(DOCX_COMMENTS_EXT_REL_TYPE) {
        let rid = next_docx_rel_id(package, DOCX_DOCUMENT_RELS_PART);
        let relationship = format!(
            "<Relationship Id=\"{}\" Type=\"{}\" Target=\"commentsExtended.xml\"/>",
            rid, DOCX_COMMENTS_EXT_REL_TYPE
        );
        inject_docx_relationship(package, DOCX_DOCUMENT_RELS_PART, &relationship)?;
    }
    ensure_content_type_override(
        package,
        "/word/commentsExtended.xml",
        DOCX_COMMENTS_EXT_CONTENT_TYPE,
    )?;
    Ok(xml)
}

/// Prepare the package for a whole-part `/commentsExtended` replacement.
///
/// Word keys `w15:commentEx` records by the first body paragraph's
/// `w14:paraId`, not by `w:comment/@w:id`.  C# stamps absent paragraph ids
/// before replaying a dumped commentsExtended part; do the same here so raw
/// replay cannot create a silently unthreaded comment graph.
pub(crate) fn prepare_comments_extended_raw_replace(
    package: &mut OxmlPackage,
) -> Result<(), HandlerError> {
    if package.has_part(DOCX_COMMENTS_PART) {
        let mut comments_xml = package
            .read_part_xml(DOCX_COMMENTS_PART)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        let comment_ids: Vec<String> = parse_comments(&comments_xml)?
            .into_iter()
            .map(|comment| comment.id)
            .collect();
        for id in comment_ids {
            let (updated, _) = assign_comment_para_id(&comments_xml, &id)?;
            comments_xml = updated;
        }
        package
            .write_part_xml(DOCX_COMMENTS_PART, &comments_xml)
            .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    }
    let _ = ensure_comment_extension_part(package)?;
    Ok(())
}

fn ensure_content_type_override(
    package: &mut OxmlPackage,
    part_name: &str,
    content_type: &str,
) -> Result<(), HandlerError> {
    let xml = package
        .read_part_xml("[Content_Types].xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    if xml.contains(&format!("PartName=\"{}\"", part_name)) {
        return Ok(());
    }
    let types_open = xml.find("<Types").ok_or_else(|| {
        HandlerError::OperationFailed("malformed [Content_Types].xml".to_string())
    })?;
    let insertion = find_tag_close_after(&xml, types_open).ok_or_else(|| {
        HandlerError::OperationFailed("malformed [Content_Types].xml".to_string())
    })? + 1;
    let override_xml = format!(
        "<Override PartName=\"{}\" ContentType=\"{}\"/>",
        part_name, content_type
    );
    let mut updated = String::with_capacity(xml.len() + override_xml.len());
    updated.push_str(&xml[..insertion]);
    updated.push_str(&override_xml);
    updated.push_str(&xml[insertion..]);
    package
        .write_part_xml("[Content_Types].xml", &updated)
        .map_err(|e| HandlerError::SaveError(e.to_string()))
}

fn upsert_comment_extension(
    package: &mut OxmlPackage,
    para_id: &str,
    parent_para_id: Option<&str>,
    done: Option<bool>,
) -> Result<(), HandlerError> {
    let xml = ensure_comment_extension_part(package)?;
    let existing = format!("w15:paraId=\"{}\"", para_id);
    let updated = if let Some(match_pos) = xml.find(&existing) {
        let start = xml[..match_pos].rfind("<w15:commentEx").ok_or_else(|| {
            HandlerError::OperationFailed("malformed commentsExtended.xml".to_string())
        })?;
        let end = tag_end_after(&xml, start).ok_or_else(|| {
            HandlerError::OperationFailed("malformed commentsExtended.xml".to_string())
        })?;
        let mut tag = xml[start..end].to_string();
        if let Some(parent) = parent_para_id {
            set_attr_on_open_tag(&mut tag, "w15:paraIdParent", parent);
        }
        if let Some(done) = done {
            set_attr_on_open_tag(&mut tag, "w15:done", if done { "1" } else { "0" });
        }
        format!("{}{}{}", &xml[..start], tag, &xml[end..])
    } else {
        let close = xml.rfind("</w15:commentsEx>").ok_or_else(|| {
            HandlerError::OperationFailed("malformed commentsExtended.xml".to_string())
        })?;
        let parent = parent_para_id
            .map(|value| format!(" w15:paraIdParent=\"{}\"", escape_attr(value)))
            .unwrap_or_default();
        let done = done.unwrap_or(false);
        let entry = format!(
            "<w15:commentEx w15:paraId=\"{}\"{} w15:done=\"{}\"/>",
            escape_attr(para_id),
            parent,
            if done { "1" } else { "0" }
        );
        format!("{}{}{}", &xml[..close], entry, &xml[close..])
    };
    package
        .write_part_xml(DOCX_COMMENTS_EXT_PART, &updated)
        .map_err(|e| HandlerError::SaveError(e.to_string()))
}

fn tag_end_after(xml: &str, tag_start: usize) -> Option<usize> {
    let close = find_tag_close_after(xml, tag_start)?;
    if xml.as_bytes().get(close) == Some(&b'/') {
        Some(close + 2)
    } else {
        Some(close + 1)
    }
}

fn xml_unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

fn find_comment_anchor(document_xml: &str, id: &str) -> Option<String> {
    let marker = format!("<w:commentRangeStart w:id=\"{}\"", id);
    let marker_pos = document_xml.find(&marker)?;
    // Word paths are 1-based. This is intentionally based on parsed document
    // order rather than comment ids, so sparse ids remain stable.
    let para_count = document_xml[..marker_pos]
        .match_indices("<w:p")
        .filter(|(index, _)| {
            matches!(
                document_xml.as_bytes().get(index + 4),
                Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n')
            )
        })
        .count();
    Some(format!("/body/p[{}]", para_count))
}

fn remove_comment_block(xml: &str, id: &str) -> Result<String, HandlerError> {
    let needle = format!("w:id=\"{}\"", id);
    let id_offset = xml
        .find(&needle)
        .ok_or_else(|| HandlerError::PathNotFound(format!("comment id '{}' not found", id)))?;
    let open = xml[..id_offset]
        .rfind("<w:comment")
        .ok_or_else(|| HandlerError::OperationFailed("malformed comments.xml".to_string()))?;
    let close = find_matching_close(xml, open, "<w:comment", "</w:comment>").ok_or_else(|| {
        HandlerError::OperationFailed("unterminated comment in comments.xml".to_string())
    })?;
    let mut out = String::with_capacity(xml.len());
    out.push_str(&xml[..open]);
    out.push_str(&xml[close..]);
    Ok(out)
}

fn remove_comment_markers(xml: &str, id: &str) -> String {
    let mut output = xml.to_string();
    for tag in ["commentRangeStart", "commentRangeEnd"] {
        let needle = format!("<w:{} w:id=\"{}\"", tag, id);
        while let Some(start) = output.find(&needle) {
            let Some(end) = find_tag_close_after(&output, start) else {
                break;
            };
            output.replace_range(start..=end, "");
        }
    }
    // The reference normally occupies a dedicated run. Remove that whole run
    // when it contains this comment reference, otherwise leave surrounding
    // text/run properties untouched.
    let reference = format!("<w:commentReference w:id=\"{}\"", id);
    while let Some(reference_pos) = output.find(&reference) {
        let run_open = output[..reference_pos].rfind("<w:r");
        let run_close = output[reference_pos..]
            .find("</w:r>")
            .map(|n| reference_pos + n + "</w:r>".len());
        if let (Some(start), Some(end)) = (run_open, run_close) {
            output.replace_range(start..end, "");
        } else if let Some(end) = find_tag_close_after(&output, reference_pos) {
            output.replace_range(reference_pos..=end, "");
        } else {
            break;
        }
    }
    output
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

/// Escape XML text content (not attribute values): minimal but enough for
/// chart titles / category labels we generate locally.
fn xml_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Add an image to a paragraph in word/document.xml, embedding it as a
/// full OOXML picture: writes `word/media/imageN.<ext>`, wires
/// `word/_rels/document.xml.rels` to the image, updates `[Content_Types].xml`
/// with the extension's MIME type, and inserts a `<w:drawing>` with an inline
/// `<wp:inline>` anchor referencing the relationship.
///
/// Supported properties: src (path on disk, optional), payloadBase64 /
/// payloadHex (alternative binary source), format/ext, width/height (EMU,
/// "4in", "10cm", "200px"), alt/description, name.
pub fn add_image_part_aware(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    use std::path::Path;

    // Resolve extension and content type.
    let ext = properties
        .get("format")
        .or_else(|| properties.get("ext"))
        .map(|s| s.as_str())
        .or_else(|| {
            // Try to derive from src filename extension.
            properties
                .get("src")
                .or_else(|| properties.get("path"))
                .and_then(|p| Path::new(p).extension())
                .and_then(|e| e.to_str())
        })
        .unwrap_or("png");
    let (ext_norm, content_type) = match ext.to_lowercase().as_str() {
        "png" => ("png", "image/png"),
        "jpg" | "jpeg" => ("jpeg", "image/jpeg"),
        "gif" => ("gif", "image/gif"),
        "bmp" => ("bmp", "image/bmp"),
        "tiff" | "tif" => ("tiff", "image/tiff"),
        "webp" => ("webp", "image/webp"),
        "svg" => ("svg", "image/svg+xml"),
        "ico" => ("ico", "image/x-icon"),
        "emf" => ("emf", "image/x-emf"),
        "wmf" => ("wmf", "image/x-wmf"),
        _ => ("png", "image/png"),
    };

    // Dimensions in EMU. Default 4in × 3in.
    let (width_emu, height_emu) = parse_image_dimensions_emu(properties);
    let alt = properties
        .get("alt")
        .or_else(|| properties.get("description"))
        .map(|s| s.as_str())
        .unwrap_or("");
    let name = properties
        .get("name")
        .cloned()
        .unwrap_or_else(|| format!("Image {}", ext_norm));

    // Probe for next free image index.
    let image_idx = next_docx_image_index(package, ext_norm);
    let media_path = format!("word/media/image{}.{}", image_idx, ext_norm);

    // Write image binary — priority: src file > payloadBase64 > payloadHex > empty stub.
    let bytes_written = if let Some(src) = properties.get("src").or_else(|| properties.get("path"))
    {
        std::fs::read(src).ok()
    } else if let Some(b64) = properties.get("payloadBase64") {
        docx_base64_decode(b64).ok()
    } else if let Some(hex) = properties.get("payloadHex") {
        docx_hex_decode(hex).ok()
    } else {
        Some(Vec::new())
    };
    if let Some(bytes) = bytes_written {
        let _ = package.write_part(&media_path, bytes);
    }

    // Wire document.xml.rels → image relationship.
    let doc_rels_path = "word/_rels/document.xml.rels";
    let image_rel_id = next_docx_rel_id(package, doc_rels_path);
    let rel_target = format!("media/image{}.{}", image_idx, ext_norm);
    let rel_xml = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"{}\"/>",
        image_rel_id, rel_target
    );
    inject_docx_relationship(package, doc_rels_path, &rel_xml)?;

    // Update [Content_Types].xml with the image extension's Default entry.
    update_docx_content_types_for_image(package, ext_norm, content_type)?;

    // Insert <w:drawing> into the target paragraph (or body) of word/document.xml.
    let doc_xml = package
        .read_part_xml("word/document.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // docPr id — use the image index so it stays unique across the document.
    let doc_pr_id = image_idx;
    let drawing_xml = format!(
        r#"<w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{w}" cy="{h}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:docPr id="{id}" name="{name}" descr="{alt}"/><wp:cNvGraphicFramePr><a:graphicFrameLocks xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" noChangeAspect="1"/></wp:cNvGraphicFramePr><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="{id}" name="{name}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{w}" cy="{h}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing>"#,
        w = width_emu,
        h = height_emu,
        id = doc_pr_id,
        name = escape_attr(&name),
        alt = escape_attr(alt),
        rid = image_rel_id,
    );

    let new_doc_xml = ensure_document_root_namespaces(&insert_drawing_in_paragraph(
        &doc_xml,
        parent,
        &drawing_xml,
    )?);
    package
        .write_part_xml("word/document.xml", &new_doc_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    Ok(format!("{}/drawing[{}]", parent, image_idx))
}

/// Add a chart to a Word document. Mirrors `WordHandler.Add.Misc.cs /
/// WordHandler.Helpers.Chart.cs` from the C# upstream: writes
/// `word/charts/chartN.xml` (ChartSpace with inline literal data so the
/// chart is self-contained), wires `word/_rels/document.xml.rels`, adds the
/// chart's Override entry to `[Content_Types].xml`, and injects a
/// `<w:drawing>` containing `<wp:inline>` + `<a:graphic>` + `<c:chart>`
/// reference into the target paragraph.
///
/// Supported properties:
///   type=bar|column|line|pie     (default: column)
///   title=<chart title>          (default: "Chart")
///   categories=A,B,C             (CSV literal; default Cat A/B/C)
///   values=1,2,3                 (CSV literal of numbers — required)
///   width, height                (EMU or "1in"/"2cm"; default 4in × 3in)
pub fn add_chart_part_aware(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let chart_type = properties
        .get("type")
        .map(|s| s.as_str())
        .unwrap_or("column")
        .to_lowercase();
    let title = properties
        .get("title")
        .cloned()
        .unwrap_or_else(|| "Chart".to_string());
    let categories = properties
        .get("categories")
        .or_else(|| properties.get("cat"))
        .cloned()
        .unwrap_or_else(|| "Cat A,Cat B,Cat C".to_string());
    let values = properties
        .get("values")
        .or_else(|| properties.get("val"))
        .cloned()
        .unwrap_or_else(|| "1,2,3".to_string());

    let cats: Vec<&str> = categories
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let vals: Vec<f64> = values
        .split(',')
        .filter_map(|s| s.trim().parse::<f64>().ok())
        .collect();
    if vals.is_empty() {
        return Err(HandlerError::InvalidArgument(
            "chart requires 'values' as CSV of numbers (e.g. values=1,2,3)".to_string(),
        ));
    }

    let chart_idx = next_docx_chart_index(package);
    let chart_path = format!("word/charts/chart{}.xml", chart_idx);

    let chart_xml = build_docx_chart_xml(&chart_type, &title, &cats, &vals)?;
    package
        .write_part_xml(&chart_path, &chart_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    // document.xml.rels → chart relationship.
    let doc_rels_path = "word/_rels/document.xml.rels";
    let chart_rel_id = next_docx_rel_id(package, doc_rels_path);
    let chart_target = format!("charts/chart{}.xml", chart_idx);
    let rel_xml = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart\" Target=\"{}\"/>",
        chart_rel_id, chart_target
    );
    inject_docx_relationship(package, doc_rels_path, &rel_xml)?;

    // [Content_Types].xml Override for the chart part.
    update_docx_content_types_for_chart(package, &chart_path)?;

    // Inject <w:drawing> referencing the chart into the target paragraph.
    let doc_xml = package
        .read_part_xml("word/document.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let (width_emu, height_emu) = parse_image_dimensions_emu(properties);
    let doc_pr_id = chart_idx;
    let drawing_xml = format!(
        r#"<w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{w}" cy="{h}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:docPr id="{id}" name="Chart {idx}"/><wp:cNvGraphicFramePr><a:graphicFrameLocks xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" noChangeAspect="1"/></wp:cNvGraphicFramePr><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" r:id="{rid}"/></a:graphicData></a:graphic></wp:inline></w:drawing>"#,
        w = width_emu,
        h = height_emu,
        id = doc_pr_id,
        idx = chart_idx,
        rid = chart_rel_id,
    );
    let new_doc_xml = ensure_document_root_namespaces(&insert_drawing_in_paragraph(
        &doc_xml,
        parent,
        &drawing_xml,
    )?);
    package
        .write_part_xml("word/document.xml", &new_doc_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    Ok(format!("{}/drawing[{}]", parent, chart_idx))
}

/// Add a floating DrawingML `wps:wsp` shape or textbox.  These objects live in
/// a non-WordprocessingML namespace, so they deliberately bypass the regular
/// `WordDom` add path just like pictures and charts do.
pub fn add_drawing_shape_part_aware(
    package: &mut OxmlPackage,
    parent: &str,
    element_type: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let kind = match element_type.to_ascii_lowercase().as_str() {
        "shape" | "sp" => DrawingShapeKind::Shape,
        "textbox" | "txbx" => DrawingShapeKind::Textbox,
        _ => unreachable!("caller only routes shape/textbox element types"),
    };
    if parent != "/body" && parent != "/" && parse_paragraph_index_from_parent(parent).is_none() {
        return Err(HandlerError::InvalidPath(format!(
            "{} add expects /body or /body/p[N], got '{}'",
            element_type, parent
        )));
    }

    let doc_xml = package
        .read_part_xml(DOCX_DOCUMENT_PART)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let index = count_drawing_shapes(&doc_xml, kind) + 1;
    let doc_pr_id = count_all_drawing_shapes(&doc_xml) + 1;
    let width = properties
        .get("width")
        .map(|value| parse_emu(value))
        .unwrap_or(if kind == DrawingShapeKind::Textbox {
            2_286_000
        } else {
            914_400
        });
    let height = properties
        .get("height")
        .map(|value| parse_emu(value))
        .unwrap_or(914_400);
    if width <= 0 || height <= 0 {
        return Err(HandlerError::InvalidArgument(
            "shape width and height must be positive".to_string(),
        ));
    }
    let geometry = properties
        .get("geometry")
        .or_else(|| properties.get("preset"))
        .or_else(|| properties.get("shape"))
        .map(|value| sanitize_geometry(value))
        .unwrap_or("rect");
    let fill = drawing_fill_xml(
        properties
            .get("fill")
            .or_else(|| properties.get("fillcolor")),
    );
    let line = drawing_line_xml(properties);
    let text = properties.get("text").map(String::as_str).unwrap_or("");
    let name = properties
        .get("alt")
        .or_else(|| properties.get("name"))
        .map(String::as_str)
        .unwrap_or(if kind == DrawingShapeKind::Textbox {
            "Text Box"
        } else {
            "Shape"
        });
    let wrap = properties.get("wrap").map(String::as_str).unwrap_or(
        if kind == DrawingShapeKind::Textbox {
            "square"
        } else {
            "none"
        },
    );
    let wrap_xml = drawing_wrap_xml(wrap)?;
    let x = properties
        .get("anchor.x")
        .or_else(|| properties.get("hposition"))
        .map(|value| parse_emu(value))
        .unwrap_or(0);
    let y = properties
        .get("anchor.y")
        .or_else(|| properties.get("vposition"))
        .map(|value| parse_emu(value))
        .unwrap_or(0);
    let text_body = if kind == DrawingShapeKind::Textbox {
        format!("<wps:txbx><w:txbxContent><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr lIns=\"91440\" tIns=\"45720\" rIns=\"91440\" bIns=\"45720\"/>", xml_escape_text(text))
    } else {
        "<wps:bodyPr/>".to_string()
    };
    let tx_box = if kind == DrawingShapeKind::Textbox {
        " txBox=\"1\""
    } else {
        ""
    };
    let drawing = format!(
        r#"<w:drawing><wp:anchor distT="0" distB="0" distL="114300" distR="114300" simplePos="0" relativeHeight="251{index:03}" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:posOffset>{x}</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>{y}</wp:posOffset></wp:positionV><wp:extent cx="{width}" cy="{height}"/><wp:effectExtent l="0" t="0" r="0" b="0"/>{wrap_xml}<wp:docPr id="{doc_pr_id}" name="{name}"/><wp:cNvGraphicFramePr/><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr{tx_box}/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{width}" cy="{height}"/></a:xfrm><a:prstGeom prst="{geometry}"><a:avLst/></a:prstGeom>{fill}{line}</wps:spPr>{text_body}</wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing>"#,
        name = escape_attr(name),
    );
    let new_xml =
        ensure_document_root_namespaces(&insert_floating_drawing(&doc_xml, parent, &drawing)?);
    package
        .write_part_xml(DOCX_DOCUMENT_PART, &new_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(format!(
        "/body/{}[{}]",
        if kind == DrawingShapeKind::Textbox {
            "textbox"
        } else {
            "shape"
        },
        index
    ))
}

/// Emit a native, editable DrawingML group for Mermaid's common flowchart
/// subset.  The result is one `wpg:wgp` anchor, keeping the diagram movable as
/// a unit and leaving every node and connector editable in Word.
pub fn add_diagram_part_aware(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    if parent != "/body" && parent != "/" && parse_paragraph_index_from_parent(parent).is_none() {
        return Err(HandlerError::InvalidPath(format!(
            "diagram add expects /body or /body/p[N], got '{}'",
            parent
        )));
    }
    let source = if let Some(value) = properties
        .get("mermaid")
        .or_else(|| properties.get("text"))
        .or_else(|| properties.get("dsl"))
    {
        value.clone()
    } else if let Some(path) = properties.get("src").or_else(|| properties.get("path")) {
        std::fs::read_to_string(path).map_err(|e| {
            HandlerError::InvalidArgument(format!("diagram source file '{}': {}", path, e))
        })?
    } else {
        return Err(HandlerError::InvalidArgument(
            "diagram requires 'mermaid' (aliases: text, dsl) or src/path".to_string(),
        ));
    };
    let layout = parse_flowchart_layout(&source)?;
    let doc_xml = package
        .read_part_xml(DOCX_DOCUMENT_PART)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let group_index = count_drawing_groups(&doc_xml) + 1;
    let drawing_id = count_all_drawing_shapes(&doc_xml) + count_drawing_groups(&doc_xml) + 1;
    let requested_width = properties.get("width").map(|value| parse_emu(value));
    let requested_height = properties.get("height").map(|value| parse_emu(value));
    let scale = match (requested_width, requested_height) {
        (Some(width), Some(height)) => {
            (width as f64 / layout.width as f64).min(height as f64 / layout.height as f64)
        }
        (Some(width), None) => width as f64 / layout.width as f64,
        (None, Some(height)) => height as f64 / layout.height as f64,
        (None, None) => 1.0,
    }
    .max(0.01);
    let width = (layout.width as f64 * scale).round() as i64;
    let height = (layout.height as f64 * scale).round() as i64;
    let mut children = String::new();
    let mut next_id = drawing_id + 1;
    for node in &layout.nodes {
        let x = (node.x as f64 * scale).round() as i64;
        let y = (node.y as f64 * scale).round() as i64;
        let w = (node.width as f64 * scale).round() as i64;
        let h = (node.height as f64 * scale).round() as i64;
        children.push_str(&diagram_node_xml(next_id, node, x, y, w, h));
        next_id += 1;
    }
    for edge in &layout.edges {
        let x1 = (edge.x1 as f64 * scale).round() as i64;
        let y1 = (edge.y1 as f64 * scale).round() as i64;
        let x2 = (edge.x2 as f64 * scale).round() as i64;
        let y2 = (edge.y2 as f64 * scale).round() as i64;
        children.push_str(&diagram_edge_xml(next_id, x1, y1, x2, y2, edge.dashed));
        next_id += 1;
        if !edge.label.is_empty() {
            let label_width =
                720_000i64.min((edge.label.chars().count() as i64 * 95_250 + 144_000).max(360_000));
            let label_height = 187_325i64;
            children.push_str(&diagram_label_xml(
                next_id,
                &edge.label,
                (x1 + x2) / 2 - label_width / 2,
                (y1 + y2) / 2 - label_height,
                label_width,
                label_height,
            ));
            next_id += 1;
        }
    }
    let group_xml = format!(
        r#"<w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="2510000" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="margin"><wp:posOffset>0</wp:posOffset></wp:positionH><wp:positionV relativeFrom="margin"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="{width}" cy="{height}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapNone/><wp:docPr id="{drawing_id}" name="Diagram {group_index}"/><wp:cNvGraphicFramePr/><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingGroup"><wpg:wgp><wpg:cNvGrpSpPr/><wpg:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{width}" cy="{height}"/><a:chOff x="0" y="0"/><a:chExt cx="{width}" cy="{height}"/></a:xfrm></wpg:grpSpPr>{children}</wpg:wgp></a:graphicData></a:graphic></wp:anchor></w:drawing>"#
    );
    let new_xml =
        ensure_document_root_namespaces(&insert_floating_drawing(&doc_xml, parent, &group_xml)?);
    package
        .write_part_xml(DOCX_DOCUMENT_PART, &new_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(format!("/body/group[{}]", group_index))
}

/// Parse the synthetic public DrawingML paths used by C# compatibility.
pub fn is_drawing_shape_path(path: &str) -> bool {
    drawing_shape_path(path).is_some()
}

pub fn get_drawing_shape(package: &OxmlPackage, path: &str) -> Result<DocumentNode, HandlerError> {
    let (kind, wanted) =
        drawing_shape_path(path).ok_or_else(|| HandlerError::InvalidPath(path.to_string()))?;
    let xml = package
        .read_part_xml(DOCX_DOCUMENT_PART)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let doc = roxmltree::Document::parse(&xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let wsp = drawing_shape_nodes(&doc, kind)
        .into_iter()
        .nth(wanted - 1)
        .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?;
    let mut result = DocumentNode::new(
        path,
        if kind == DrawingShapeKind::Textbox {
            "textbox"
        } else {
            "shape"
        },
    );
    let text: String = wsp
        .descendants()
        .filter(|node| node.has_tag_name((W_NS, "t")))
        .filter_map(|node| node.text())
        .collect();
    if !text.is_empty() {
        result = result.with_preview(&text).with_text(&text);
    }
    if let Some(geometry) = wsp
        .descendants()
        .find(|node| node.has_tag_name((A_NS, "prstGeom")))
        .and_then(|node| node.attribute("prst"))
    {
        result = result.with_format("geometry", serde_json::Value::String(geometry.to_string()));
    }
    if let Some(anchor) = wsp
        .ancestors()
        .find(|node| node.has_tag_name((WP_NS, "anchor")) || node.has_tag_name((WP_NS, "inline")))
    {
        if let Some(extent) = anchor
            .children()
            .find(|node| node.has_tag_name((WP_NS, "extent")))
        {
            for key in ["cx", "cy"] {
                if let Some(value) = extent.attribute(key) {
                    result = result.with_format(
                        if key == "cx" { "width" } else { "height" },
                        serde_json::Value::String(value.to_string()),
                    );
                }
            }
        }
    }
    Ok(result)
}

pub fn set_drawing_shape(
    package: &mut OxmlPackage,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let (kind, wanted) =
        drawing_shape_path(path).ok_or_else(|| HandlerError::InvalidPath(path.to_string()))?;
    let xml = package
        .read_part_xml(DOCX_DOCUMENT_PART)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let mut dom = crate::handler::parse_document_xml(&xml)?;
    let width_update = properties
        .get("width")
        .map(|value| parse_emu(value).to_string());
    let height_update = properties
        .get("height")
        .map(|value| parse_emu(value).to_string());
    let wsp = find_drawing_shape_mut(&mut dom.root, kind, wanted)
        .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?;
    let mut unsupported = Vec::new();
    for (key, value) in properties {
        match key.as_str() {
            "geometry" | "preset" | "shape" => {
                if let Some(geometry) = find_descendant_mut(wsp, A_NS, "prstGeom") {
                    geometry
                        .attributes
                        .insert("prst".to_string(), sanitize_geometry(value).to_string());
                }
            }
            "width" | "height" => {
                let attr = if key == "width" { "cx" } else { "cy" };
                let value = parse_emu(value).to_string();
                if let Some(ext) = find_descendant_mut(wsp, A_NS, "ext") {
                    ext.attributes.insert(attr.to_string(), value);
                }
                // The wrapper's wp:extent is updated below in a separate pass.
            }
            "fill" | "fillcolor" | "line" | "line.style" | "linestyle" | "line.width"
            | "linewidth" | "line.color" | "linecolor" => unsupported.push(key.clone()),
            _ => unsupported.push(key.clone()),
        }
    }
    // Rebuild recognized fill/line values atomically when any were supplied.
    let has_fill = properties.contains_key("fill") || properties.contains_key("fillcolor");
    let has_line = properties.keys().any(|key| {
        matches!(
            key.as_str(),
            "line"
                | "line.style"
                | "linestyle"
                | "line.width"
                | "linewidth"
                | "line.color"
                | "linecolor"
        )
    });
    if has_fill || has_line {
        let sp_pr = find_descendant_mut(wsp, WPS_NS, "spPr").ok_or_else(|| {
            HandlerError::OperationFailed("DrawingML shape has no wps:spPr".to_string())
        })?;
        if has_fill {
            replace_drawing_child(
                sp_pr,
                &["solidFill", "noFill"],
                drawing_fill_node(
                    properties
                        .get("fill")
                        .or_else(|| properties.get("fillcolor")),
                ),
            );
        }
        if has_line {
            replace_drawing_child(sp_pr, &["ln"], drawing_line_node(properties));
        }
        unsupported.retain(|key| {
            !matches!(
                key.as_str(),
                "fill"
                    | "fillcolor"
                    | "line"
                    | "line.style"
                    | "linestyle"
                    | "line.width"
                    | "linewidth"
                    | "line.color"
                    | "linecolor"
            )
        });
    }
    let _ = wsp;
    if width_update.is_some() || height_update.is_some() {
        let anchor =
            find_drawing_shape_anchor_mut(&mut dom.root, kind, wanted).ok_or_else(|| {
                HandlerError::OperationFailed("DrawingML shape has no wp:anchor".to_string())
            })?;
        if let Some(extent) = anchor.children.iter_mut().find(|child| {
            child.namespace.as_deref() == Some(WP_NS)
                && matches!(&child.element_type, WordElementType::Unknown(name) if name == "extent")
        }) {
            if let Some(width) = width_update {
                extent.attributes.insert("cx".to_string(), width);
            }
            if let Some(height) = height_update {
                extent.attributes.insert("cy".to_string(), height);
            }
        }
    }
    let output = crate::handler::serialize_dom(&dom);
    package
        .write_part_xml(DOCX_DOCUMENT_PART, &output)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(unsupported)
}

pub fn remove_drawing_shape(package: &mut OxmlPackage, path: &str) -> Result<(), HandlerError> {
    let (kind, wanted) =
        drawing_shape_path(path).ok_or_else(|| HandlerError::InvalidPath(path.to_string()))?;
    let xml = package
        .read_part_xml(DOCX_DOCUMENT_PART)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let mut dom = crate::handler::parse_document_xml(&xml)?;
    let mut seen = 0;
    if !remove_shape_host_paragraph(&mut dom.root, kind, wanted, &mut seen) {
        return Err(HandlerError::PathNotFound(path.to_string()));
    }
    package
        .write_part_xml(DOCX_DOCUMENT_PART, &crate::handler::serialize_dom(&dom))
        .map_err(|e| HandlerError::SaveError(e.to_string()))
}

pub fn is_drawing_group_path(path: &str) -> bool {
    drawing_group_path(path).is_some()
}

pub fn get_drawing_group(package: &OxmlPackage, path: &str) -> Result<DocumentNode, HandlerError> {
    let wanted =
        drawing_group_path(path).ok_or_else(|| HandlerError::InvalidPath(path.to_string()))?;
    let xml = package
        .read_part_xml(DOCX_DOCUMENT_PART)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let doc = roxmltree::Document::parse(&xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let group = doc
        .descendants()
        .filter(|node| node.has_tag_name((WPG_NS, "wgp")))
        .nth(wanted - 1)
        .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?;
    let mut result = DocumentNode::new(path, "group");
    let text: String = group
        .descendants()
        .filter(|node| node.has_tag_name((W_NS, "t")))
        .filter_map(|node| node.text())
        .collect();
    if !text.is_empty() {
        result = result.with_text(&text).with_preview(&text);
    }
    if let Some(anchor) = group
        .ancestors()
        .find(|node| node.has_tag_name((WP_NS, "anchor")))
    {
        if let Some(extent) = anchor
            .children()
            .find(|node| node.has_tag_name((WP_NS, "extent")))
        {
            for (attr, key) in [("cx", "width"), ("cy", "height")] {
                if let Some(value) = extent.attribute(attr) {
                    result = result.with_format(key, serde_json::Value::String(value.to_string()));
                }
            }
        }
    }
    Ok(result)
}

pub fn set_drawing_group(
    package: &mut OxmlPackage,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let wanted =
        drawing_group_path(path).ok_or_else(|| HandlerError::InvalidPath(path.to_string()))?;
    let width = properties
        .get("width")
        .map(|value| parse_emu(value).to_string());
    let height = properties
        .get("height")
        .map(|value| parse_emu(value).to_string());
    let unsupported: Vec<String> = properties
        .keys()
        .filter(|key| key.as_str() != "width" && key.as_str() != "height")
        .cloned()
        .collect();
    if width.is_none() && height.is_none() {
        return Ok(unsupported);
    }
    let xml = package
        .read_part_xml(DOCX_DOCUMENT_PART)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let mut dom = crate::handler::parse_document_xml(&xml)?;
    let group = find_drawing_group_mut(&mut dom.root, wanted)
        .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?;
    if let Some(ext) = find_descendant_mut(group, A_NS, "ext") {
        if let Some(width) = &width {
            ext.attributes.insert("cx".to_string(), width.clone());
        }
        if let Some(height) = &height {
            ext.attributes.insert("cy".to_string(), height.clone());
        }
    }
    let _ = group;
    let anchor = find_drawing_group_anchor_mut(&mut dom.root, wanted).ok_or_else(|| {
        HandlerError::OperationFailed("DrawingML group has no wp:anchor".to_string())
    })?;
    if let Some(extent) = anchor.children.iter_mut().find(|child| {
        child.namespace.as_deref() == Some(WP_NS)
            && matches!(&child.element_type, WordElementType::Unknown(name) if name == "extent")
    }) {
        if let Some(width) = width {
            extent.attributes.insert("cx".to_string(), width);
        }
        if let Some(height) = height {
            extent.attributes.insert("cy".to_string(), height);
        }
    }
    package
        .write_part_xml(DOCX_DOCUMENT_PART, &crate::handler::serialize_dom(&dom))
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(unsupported)
}

pub fn remove_drawing_group(package: &mut OxmlPackage, path: &str) -> Result<(), HandlerError> {
    let wanted =
        drawing_group_path(path).ok_or_else(|| HandlerError::InvalidPath(path.to_string()))?;
    let xml = package
        .read_part_xml(DOCX_DOCUMENT_PART)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let mut dom = crate::handler::parse_document_xml(&xml)?;
    let mut seen = 0;
    if !remove_group_host_paragraph(&mut dom.root, wanted, &mut seen) {
        return Err(HandlerError::PathNotFound(path.to_string()));
    }
    package
        .write_part_xml(DOCX_DOCUMENT_PART, &crate::handler::serialize_dom(&dom))
        .map_err(|e| HandlerError::SaveError(e.to_string()))
}

fn drawing_shape_path(path: &str) -> Option<(DrawingShapeKind, usize)> {
    let path = path.trim();
    for (suffix, kind) in [
        ("textbox", DrawingShapeKind::Textbox),
        ("txbx", DrawingShapeKind::Textbox),
        ("shape", DrawingShapeKind::Shape),
        ("sp", DrawingShapeKind::Shape),
    ] {
        let marker = format!("/{}[", suffix);
        if let Some(offset) = path.rfind(&marker) {
            let value = path
                .get(offset + marker.len()..)?
                .strip_suffix(']')?
                .parse::<usize>()
                .ok()?;
            if value > 0 {
                return Some((kind, value));
            }
        }
    }
    None
}

fn drawing_group_path(path: &str) -> Option<usize> {
    let path = path.trim();
    let marker = "/group[";
    let offset = path.rfind(marker)?;
    let value = path
        .get(offset + marker.len()..)?
        .strip_suffix(']')?
        .parse::<usize>()
        .ok()?;
    (value > 0).then_some(value)
}

fn drawing_shape_nodes<'a>(
    doc: &'a roxmltree::Document<'a>,
    kind: DrawingShapeKind,
) -> Vec<roxmltree::Node<'a, 'a>> {
    doc.descendants()
        .filter(|node| {
            node.has_tag_name((WPS_NS, "wsp"))
                && (node
                    .children()
                    .any(|child| child.has_tag_name((WPS_NS, "txbx")))
                    == (kind == DrawingShapeKind::Textbox))
        })
        .collect()
}

fn count_drawing_shapes(xml: &str, kind: DrawingShapeKind) -> usize {
    roxmltree::Document::parse(xml)
        .map(|doc| drawing_shape_nodes(&doc, kind).len())
        .unwrap_or(0)
}

fn count_all_drawing_shapes(xml: &str) -> usize {
    roxmltree::Document::parse(xml)
        .map(|doc| {
            doc.descendants()
                .filter(|node| node.has_tag_name((WPS_NS, "wsp")))
                .count()
        })
        .unwrap_or(0)
}

fn count_drawing_groups(xml: &str) -> usize {
    const WPG_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup";
    roxmltree::Document::parse(xml)
        .map(|doc| {
            doc.descendants()
                .filter(|node| node.has_tag_name((WPG_NS, "wgp")))
                .count()
        })
        .unwrap_or(0)
}

#[derive(Clone)]
struct DiagramNodeLayout {
    label: String,
    geometry: &'static str,
    fill: &'static str,
    line: &'static str,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
}

struct DiagramEdgeLayout {
    x1: i64,
    y1: i64,
    x2: i64,
    y2: i64,
    dashed: bool,
    label: String,
}

struct DiagramLayout {
    nodes: Vec<DiagramNodeLayout>,
    edges: Vec<DiagramEdgeLayout>,
    width: i64,
    height: i64,
}

#[derive(Clone)]
struct DiagramSemanticNode {
    label: String,
    geometry: &'static str,
    fill: &'static str,
    line: &'static str,
}

/// A deliberately local port of the C# Mermaid flowchart front-end. It accepts
/// the high-value syntax (header direction, shape wrappers, chained links,
/// pipe labels and dashed links) and maps it into a layered editable drawing.
fn parse_flowchart_layout(source: &str) -> Result<DiagramLayout, HandlerError> {
    if source
        .lines()
        .flat_map(|line| line.split(';'))
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("%%"))
        .is_some_and(|line| line.eq_ignore_ascii_case("sequenceDiagram"))
    {
        return parse_sequence_diagram_layout(source);
    }
    let mut left_to_right = false;
    let mut nodes: Vec<DiagramSemanticNode> = Vec::new();
    let mut node_index: HashMap<String, usize> = HashMap::new();
    let mut edges: Vec<(String, String, bool, String)> = Vec::new();
    for statement in source.lines().flat_map(|line| line.split(';')) {
        let statement = statement.trim();
        if statement.is_empty() || statement.starts_with("%%") {
            continue;
        }
        let lower = statement.to_ascii_lowercase();
        if lower.starts_with("flowchart") || lower.starts_with("graph") {
            left_to_right = lower
                .split_whitespace()
                .nth(1)
                .is_some_and(|direction| matches!(direction, "lr" | "rl"));
            continue;
        }
        if lower.starts_with("subgraph")
            || matches!(lower.as_str(), "end")
            || lower.starts_with("direction")
            || lower.starts_with("style")
            || lower.starts_with("class")
            || lower.starts_with("click")
            || lower.starts_with("linkstyle")
        {
            continue;
        }
        let operators = find_mermaid_link_operators(statement);
        if operators.is_empty() {
            parse_diagram_node_token(statement, &mut nodes, &mut node_index);
            continue;
        }
        let mut fragments = Vec::new();
        let mut offset = 0;
        for (start, end, dashed) in &operators {
            fragments.push(statement[offset..*start].trim());
            offset = *end;
            let _ = dashed;
        }
        fragments.push(statement[offset..].trim());
        let ids: Vec<Option<String>> = fragments
            .iter()
            .map(|fragment| parse_diagram_node_token(fragment, &mut nodes, &mut node_index))
            .collect();
        for index in 0..ids.len().saturating_sub(1) {
            if let (Some(from), Some(to)) = (&ids[index], &ids[index + 1]) {
                edges.push((
                    from.clone(),
                    to.clone(),
                    operators[index].2,
                    mermaid_link_label(&statement[operators[index].0..operators[index].1]),
                ));
            }
        }
    }
    if nodes.is_empty() {
        return Err(HandlerError::InvalidArgument(
            "diagram has no nodes; use e.g. 'flowchart TD; A[Start] --> B[End]'".to_string(),
        ));
    }
    let mut ranks = vec![0usize; nodes.len()];
    for _ in 0..nodes.len() {
        let mut changed = false;
        for (from, to, _, _) in &edges {
            let from_index = node_index[from];
            let to_index = node_index[to];
            if from_index != to_index && ranks[to_index] <= ranks[from_index] {
                ranks[to_index] = ranks[from_index] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut slots: HashMap<usize, usize> = HashMap::new();
    let node_width = 1_440_000i64; // 4cm
    let node_height = 576_000i64; // 1.6cm
    let main_gap = 864_000i64; // 2.4cm
    let cross_gap = 720_000i64; // 2cm
    let margin = 288_000i64; // 0.8cm
    let mut placed = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        let slot = slots.entry(ranks[index]).or_insert(0);
        let (x, y) = if left_to_right {
            (
                margin + ranks[index] as i64 * (node_width + main_gap),
                margin + *slot as i64 * (node_height + cross_gap),
            )
        } else {
            (
                margin + *slot as i64 * (node_width + cross_gap),
                margin + ranks[index] as i64 * (node_height + main_gap),
            )
        };
        *slot += 1;
        placed.push(DiagramNodeLayout {
            label: node.label.clone(),
            geometry: node.geometry,
            fill: node.fill,
            line: node.line,
            x,
            y,
            width: node_width,
            height: node_height,
        });
    }
    let mut routed = Vec::new();
    for (from, to, dashed, label) in edges {
        let source = &placed[node_index[&from]];
        let target = &placed[node_index[&to]];
        let (x1, y1, x2, y2) = if left_to_right {
            (
                source.x + source.width,
                source.y + source.height / 2,
                target.x,
                target.y + target.height / 2,
            )
        } else {
            (
                source.x + source.width / 2,
                source.y + source.height,
                target.x + target.width / 2,
                target.y,
            )
        };
        routed.push(DiagramEdgeLayout {
            x1,
            y1,
            x2,
            y2,
            dashed,
            label,
        });
    }
    let width = placed
        .iter()
        .map(|node| node.x + node.width)
        .max()
        .unwrap_or(margin)
        + margin;
    let height = placed
        .iter()
        .map(|node| node.y + node.height)
        .max()
        .unwrap_or(margin)
        + margin;
    Ok(DiagramLayout {
        nodes: placed,
        edges: routed,
        width,
        height,
    })
}

/// Compact port of C# `SequenceLayout`: participants become boxes, their
/// lifelines are dashed vertical edges, and messages are stacked horizontally.
fn parse_sequence_diagram_layout(source: &str) -> Result<DiagramLayout, HandlerError> {
    let mut participants: Vec<(String, String)> = Vec::new();
    let mut participant_index: HashMap<String, usize> = HashMap::new();
    let mut messages: Vec<(String, String, String, bool)> = Vec::new();
    let mut see = |id: &str, label: Option<&str>| {
        if let Some(index) = participant_index.get(id).copied() {
            if let Some(label) = label {
                participants[index].1 = label.trim().to_string();
            }
        } else {
            participant_index.insert(id.to_string(), participants.len());
            participants.push((id.to_string(), label.unwrap_or(id).trim().to_string()));
        }
    };
    for statement in source.lines().flat_map(|line| line.split(';')) {
        let statement = statement.trim();
        if statement.is_empty()
            || statement.starts_with("%%")
            || statement.eq_ignore_ascii_case("sequenceDiagram")
        {
            continue;
        }
        let lower = statement.to_ascii_lowercase();
        if lower.starts_with("participant ") || lower.starts_with("actor ") {
            let declaration = statement
                .split_once(char::is_whitespace)
                .map(|(_, rest)| rest.trim())
                .unwrap_or("");
            let (id, label) = declaration
                .split_once(" as ")
                .unwrap_or((declaration, declaration));
            if !id.is_empty() {
                see(id.trim(), Some(label));
            }
            continue;
        }
        let Some((left, label)) = statement.split_once(':') else {
            continue;
        };
        let operator = ["-->>", "->>", "-->", "->", "--x", "-x"]
            .iter()
            .find_map(|operator| left.find(operator).map(|offset| (offset, *operator)));
        if let Some((offset, operator)) = operator {
            let from = left[..offset].trim();
            let to = left[offset + operator.len()..]
                .trim()
                .trim_start_matches(['+', '-']);
            if !from.is_empty() && !to.is_empty() {
                see(from, None);
                see(to, None);
                messages.push((
                    from.to_string(),
                    to.to_string(),
                    label.trim().to_string(),
                    operator.starts_with("--"),
                ));
            }
        }
    }
    if participants.is_empty() {
        return Err(HandlerError::InvalidArgument(
            "sequence diagram has no participants; use e.g. 'sequenceDiagram; Alice->>Bob: hello'"
                .to_string(),
        ));
    }
    let margin = 288_000i64;
    let box_w = 864_000i64;
    let box_h = 396_000i64;
    let gap = 504_000i64;
    let body_top = margin + box_h + 324_000;
    let row = 414_000i64;
    let bottom = body_top + (messages.len().max(1) as i64) * row + 216_000;
    let mut nodes = Vec::new();
    let mut centers = HashMap::new();
    for (index, (id, label)) in participants.iter().enumerate() {
        let x = margin + index as i64 * (box_w + gap);
        centers.insert(id.clone(), x + box_w / 2);
        nodes.push(DiagramNodeLayout {
            label: label.clone(),
            geometry: "rect",
            fill: "DAE8FC",
            line: "6C8EBF",
            x,
            y: margin,
            width: box_w,
            height: box_h,
        });
    }
    let mut edges = Vec::new();
    for (id, _) in &participants {
        let x = centers[id];
        edges.push(DiagramEdgeLayout {
            x1: x,
            y1: margin + box_h,
            x2: x,
            y2: bottom,
            dashed: true,
            label: String::new(),
        });
    }
    for (index, (from, to, label, dashed)) in messages.iter().enumerate() {
        let y = body_top + index as i64 * row;
        let x1 = centers[from];
        let x2 = centers[to];
        edges.push(DiagramEdgeLayout {
            x1,
            y1: y,
            x2,
            y2: y,
            dashed: *dashed,
            label: label.clone(),
        });
    }
    let width = margin * 2
        + participants.len() as i64 * box_w
        + participants.len().saturating_sub(1) as i64 * gap;
    Ok(DiagramLayout {
        nodes,
        edges,
        width,
        height: bottom + margin,
    })
}

fn find_mermaid_link_operators(value: &str) -> Vec<(usize, usize, bool)> {
    let bytes = value.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index + 2 < bytes.len() {
        if matches!(bytes[index], b'-' | b'=' | b'.')
            && matches!(bytes[index + 1], b'-' | b'=' | b'.')
        {
            let start = index;
            let dashed = bytes[index] == b'.' || (bytes[index] == b'-' && bytes[index + 2] == b'-');
            while index < bytes.len() && matches!(bytes[index], b'-' | b'=' | b'.') {
                index += 1;
            }
            if index < bytes.len() && matches!(bytes[index], b'>' | b'x' | b'o') {
                index += 1;
            }
            if index < bytes.len() && bytes[index] == b'|' {
                index += 1;
                while index < bytes.len() && bytes[index] != b'|' {
                    index += 1;
                }
                if index < bytes.len() {
                    index += 1;
                }
            }
            found.push((start, index, dashed));
        } else {
            index += 1;
        }
    }
    found
}

fn mermaid_link_label(operator: &str) -> String {
    let Some(start) = operator.find('|') else {
        return String::new();
    };
    operator[start + 1..]
        .find('|')
        .map(|end| operator[start + 1..start + 1 + end].trim().to_string())
        .unwrap_or_default()
}

fn parse_diagram_node_token(
    token: &str,
    nodes: &mut Vec<DiagramSemanticNode>,
    index: &mut HashMap<String, usize>,
) -> Option<String> {
    let token = token.trim().trim_matches('|').trim();
    let id_end = token
        .find(|character: char| !character.is_alphanumeric() && character != '_')
        .unwrap_or(token.len());
    let id = token[..id_end].trim();
    if id.is_empty() {
        return None;
    }
    let suffix = token[id_end..].trim().trim_end_matches(":::class");
    let (label, geometry, fill, line) = if let Some(label) = suffix
        .strip_prefix("{{")
        .and_then(|value| value.strip_suffix("}}"))
    {
        (label, "hexagon", "FFF2CC", "D6B656")
    } else if let Some(label) = suffix
        .strip_prefix("((")
        .and_then(|value| value.strip_suffix("))"))
    {
        (label, "ellipse", "F8CECC", "B85450")
    } else if let Some(label) = suffix
        .strip_prefix("[")
        .and_then(|value| value.strip_suffix("]"))
    {
        (label, "rect", "DAE8FC", "6C8EBF")
    } else if let Some(label) = suffix
        .strip_prefix("{")
        .and_then(|value| value.strip_suffix("}"))
    {
        (label, "diamond", "FFF2CC", "D6B656")
    } else if let Some(label) = suffix
        .strip_prefix("(")
        .and_then(|value| value.strip_suffix(")"))
    {
        (label, "roundRect", "D5E8D4", "82B366")
    } else {
        (id, "rect", "DAE8FC", "6C8EBF")
    };
    let label = label
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();
    let id = id.to_string();
    if let Some(existing) = index.get(&id).copied() {
        if label != id {
            nodes[existing].label = label;
        }
    } else {
        index.insert(id.clone(), nodes.len());
        nodes.push(DiagramSemanticNode {
            label,
            geometry,
            fill,
            line,
        });
    }
    Some(id)
}

fn diagram_node_xml(
    id: usize,
    node: &DiagramNodeLayout,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
) -> String {
    format!(
        r#"<wps:wsp><wps:cNvPr id="{id}" name="Diagram node {id}"/><wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{width}" cy="{height}"/></a:xfrm><a:prstGeom prst="{geometry}"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="{fill}"/></a:solidFill><a:ln w="12700"><a:solidFill><a:srgbClr val="{line}"/></a:solidFill></a:ln></wps:spPr><wps:txbx><w:txbxContent><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:rPr><w:sz w:val="24"/></w:rPr><w:t xml:space="preserve">{text}</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr lIns="0" tIns="0" rIns="0" bIns="0" anchor="ctr" anchorCtr="1"><a:normAutofit/></wps:bodyPr></wps:wsp>"#,
        geometry = node.geometry,
        fill = node.fill,
        line = node.line,
        text = xml_escape_text(&node.label)
    )
}

fn diagram_edge_xml(id: usize, x1: i64, y1: i64, x2: i64, y2: i64, dashed: bool) -> String {
    let x = x1.min(x2);
    let y = y1.min(y2);
    let width = (x1 - x2).unsigned_abs().max(12_700) as i64;
    let height = (y1 - y2).unsigned_abs().max(12_700) as i64;
    let p1x = x1 - x;
    let p1y = y1 - y;
    let p2x = x2 - x;
    let p2y = y2 - y;
    let dash = if dashed {
        "<a:prstDash val=\"dash\"/>"
    } else {
        ""
    };
    format!(
        r#"<wps:wsp><wps:cNvPr id="{id}" name="Diagram edge {id}"/><wps:cNvSpPr/><wps:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{width}" cy="{height}"/></a:xfrm><a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l="0" t="0" r="{width}" b="{height}"/><a:pathLst><a:path w="{width}" h="{height}"><a:moveTo><a:pt x="{p1x}" y="{p1y}"/></a:moveTo><a:lnTo><a:pt x="{p2x}" y="{p2y}"/></a:lnTo></a:path></a:pathLst></a:custGeom><a:noFill/><a:ln w="12700"><a:solidFill><a:srgbClr val="4D4D4D"/></a:solidFill>{dash}<a:tailEnd type="triangle"/></a:ln></wps:spPr><wps:bodyPr/></wps:wsp>"#
    )
}

fn diagram_label_xml(id: usize, text: &str, x: i64, y: i64, width: i64, height: i64) -> String {
    format!(
        r#"<wps:wsp><wps:cNvPr id="{id}" name="Diagram label {id}"/><wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{width}" cy="{height}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:ln><a:noFill/></a:ln></wps:spPr><wps:txbx><w:txbxContent><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:rPr><w:sz w:val="18"/></w:rPr><w:t xml:space="preserve">{text}</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr lIns="0" tIns="0" rIns="0" bIns="0" anchor="ctr" anchorCtr="1"><a:noAutofit/></wps:bodyPr></wps:wsp>"#,
        text = xml_escape_text(text)
    )
}

fn sanitize_geometry(value: &str) -> &str {
    match value.trim() {
        "rect" | "roundRect" | "ellipse" | "triangle" | "rtTriangle" | "diamond"
        | "parallelogram" | "trapezoid" | "hexagon" | "octagon" | "star5" | "rightArrow"
        | "leftArrow" | "upArrow" | "downArrow" | "line" => value.trim(),
        _ => "rect",
    }
}

fn drawing_fill_xml(fill: Option<&String>) -> String {
    match fill.map(|value| value.trim()).filter(|value| {
        !value.is_empty()
            && !value.eq_ignore_ascii_case("none")
            && !value.eq_ignore_ascii_case("transparent")
    }) {
        Some(color) => format!(
            "<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>",
            sanitize_drawing_color(color)
        ),
        None => "<a:noFill/>".to_string(),
    }
}

fn drawing_line_xml(properties: &HashMap<String, String>) -> String {
    let compact = properties.get("line").map(String::as_str);
    if compact.is_some_and(|value| value.eq_ignore_ascii_case("none")) {
        return "<a:ln><a:noFill/></a:ln>".to_string();
    }
    let parts: Vec<&str> = compact.unwrap_or("").split(';').collect();
    let style = properties
        .get("line.style")
        .or_else(|| properties.get("linestyle"))
        .map(String::as_str)
        .or_else(|| parts.first().copied())
        .filter(|value| !value.is_empty())
        .unwrap_or("solid");
    let width = properties
        .get("line.width")
        .or_else(|| properties.get("linewidth"))
        .map(String::as_str)
        .or_else(|| parts.get(1).copied())
        .filter(|value| !value.is_empty())
        .map(parse_emu)
        .unwrap_or(12_700);
    let color = properties
        .get("line.color")
        .or_else(|| properties.get("linecolor"))
        .map(String::as_str)
        .or_else(|| parts.get(2).copied())
        .filter(|value| !value.is_empty())
        .unwrap_or("000000");
    let dash = if style.eq_ignore_ascii_case("solid") {
        "".to_string()
    } else {
        format!("<a:prstDash val=\"{}\"/>", escape_attr(style))
    };
    format!(
        "<a:ln w=\"{}\"><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>{}</a:ln>",
        width,
        sanitize_drawing_color(color),
        dash
    )
}

fn drawing_wrap_xml(wrap: &str) -> Result<&'static str, HandlerError> {
    match wrap.to_ascii_lowercase().as_str() {
        "none" | "front" => Ok("<wp:wrapNone/>"),
        "behind" => Ok("<wp:wrapNone/>"),
        "square" => Ok("<wp:wrapSquare wrapText=\"bothSides\"/>"),
        "tight" => Ok("<wp:wrapTight wrapText=\"bothSides\"/>"),
        "through" => Ok("<wp:wrapThrough wrapText=\"bothSides\"/>"),
        "topandbottom" => Ok("<wp:wrapTopAndBottom/>"),
        value => Err(HandlerError::InvalidArgument(format!(
            "unsupported DrawingML wrap '{}'",
            value
        ))),
    }
}

fn sanitize_drawing_color(value: &str) -> String {
    let value = value.trim().trim_start_matches('#');
    if value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        value.to_ascii_uppercase()
    } else {
        "000000".to_string()
    }
}

fn insert_floating_drawing(
    doc_xml: &str,
    parent: &str,
    drawing: &str,
) -> Result<String, HandlerError> {
    if parent == "/body" || parent == "/" || parent.is_empty() {
        let paragraph = format!("<w:p>{}</w:p>", drawing);
        let target = doc_xml
            .find("<w:sectPr")
            .or_else(|| doc_xml.find("</w:body>"))
            .ok_or_else(|| {
                HandlerError::OperationFailed("could not locate document body".to_string())
            })?;
        let mut output = String::with_capacity(doc_xml.len() + paragraph.len());
        output.push_str(&doc_xml[..target]);
        output.push_str(&paragraph);
        output.push_str(&doc_xml[target..]);
        Ok(output)
    } else {
        insert_drawing_in_paragraph(doc_xml, parent, drawing)
    }
}

fn node_is_drawing_shape(node: &WordNode, kind: DrawingShapeKind) -> bool {
    node.namespace.as_deref() == Some(WPS_NS)
        && matches!(&node.element_type, WordElementType::Unknown(name) if name == "wsp")
        && (node.children.iter().any(|child| {
            child.namespace.as_deref() == Some(WPS_NS)
                && matches!(&child.element_type, WordElementType::Unknown(name) if name == "txbx")
        }) == (kind == DrawingShapeKind::Textbox))
}

fn find_drawing_shape_mut(
    node: &mut WordNode,
    kind: DrawingShapeKind,
    wanted: usize,
) -> Option<&mut WordNode> {
    fn walk<'a>(
        node: &'a mut WordNode,
        kind: DrawingShapeKind,
        wanted: usize,
        seen: &mut usize,
    ) -> Option<&'a mut WordNode> {
        if node_is_drawing_shape(node, kind) {
            *seen += 1;
            if *seen == wanted {
                return Some(node);
            }
        }
        for child in &mut node.children {
            if let Some(found) = walk(child, kind, wanted, seen) {
                return Some(found);
            }
        }
        None
    }
    walk(node, kind, wanted, &mut 0)
}

fn find_descendant_mut<'a>(
    node: &'a mut WordNode,
    namespace: &str,
    local: &str,
) -> Option<&'a mut WordNode> {
    if node.namespace.as_deref() == Some(namespace)
        && matches!(&node.element_type, WordElementType::Unknown(name) if name == local)
    {
        return Some(node);
    }
    for child in &mut node.children {
        if let Some(found) = find_descendant_mut(child, namespace, local) {
            return Some(found);
        }
    }
    None
}

fn contains_drawing_shape(node: &WordNode, kind: DrawingShapeKind) -> bool {
    node_is_drawing_shape(node, kind)
        || node
            .children
            .iter()
            .any(|child| contains_drawing_shape(child, kind))
}

fn find_drawing_shape_anchor_mut(
    node: &mut WordNode,
    kind: DrawingShapeKind,
    wanted: usize,
) -> Option<&mut WordNode> {
    fn walk<'a>(
        node: &'a mut WordNode,
        kind: DrawingShapeKind,
        wanted: usize,
        seen: &mut usize,
    ) -> Option<&'a mut WordNode> {
        let is_anchor = node.namespace.as_deref() == Some(WP_NS)
            && matches!(&node.element_type, WordElementType::Unknown(name) if name == "anchor" || name == "inline");
        if is_anchor && contains_drawing_shape(node, kind) {
            *seen += 1;
            if *seen == wanted {
                return Some(node);
            }
        }
        for child in &mut node.children {
            if let Some(found) = walk(child, kind, wanted, seen) {
                return Some(found);
            }
        }
        None
    }
    walk(node, kind, wanted, &mut 0)
}

fn node_is_drawing_group(node: &WordNode) -> bool {
    node.namespace.as_deref() == Some(WPG_NS)
        && matches!(&node.element_type, WordElementType::Unknown(name) if name == "wgp")
}

fn find_drawing_group_mut(node: &mut WordNode, wanted: usize) -> Option<&mut WordNode> {
    fn walk<'a>(
        node: &'a mut WordNode,
        wanted: usize,
        seen: &mut usize,
    ) -> Option<&'a mut WordNode> {
        if node_is_drawing_group(node) {
            *seen += 1;
            if *seen == wanted {
                return Some(node);
            }
        }
        for child in &mut node.children {
            if let Some(found) = walk(child, wanted, seen) {
                return Some(found);
            }
        }
        None
    }
    walk(node, wanted, &mut 0)
}

fn contains_drawing_group(node: &WordNode) -> bool {
    node_is_drawing_group(node) || node.children.iter().any(contains_drawing_group)
}

fn find_drawing_group_anchor_mut(node: &mut WordNode, wanted: usize) -> Option<&mut WordNode> {
    fn walk<'a>(
        node: &'a mut WordNode,
        wanted: usize,
        seen: &mut usize,
    ) -> Option<&'a mut WordNode> {
        let is_anchor = node.namespace.as_deref() == Some(WP_NS)
            && matches!(&node.element_type, WordElementType::Unknown(name) if name == "anchor");
        if is_anchor && contains_drawing_group(node) {
            *seen += 1;
            if *seen == wanted {
                return Some(node);
            }
        }
        for child in &mut node.children {
            if let Some(found) = walk(child, wanted, seen) {
                return Some(found);
            }
        }
        None
    }
    walk(node, wanted, &mut 0)
}

fn namespace_node(namespace: &str, local: &str) -> WordNode {
    let mut node = WordNode::new(WordElementType::Unknown(local.to_string()));
    node.namespace = Some(namespace.to_string());
    node
}

fn drawing_fill_node(fill: Option<&String>) -> WordNode {
    if let Some(color) = fill.map(String::as_str).filter(|value| {
        !value.eq_ignore_ascii_case("none") && !value.eq_ignore_ascii_case("transparent")
    }) {
        let mut srgb = namespace_node(A_NS, "srgbClr");
        srgb.attributes
            .insert("val".to_string(), sanitize_drawing_color(color));
        namespace_node(A_NS, "solidFill").with_children(vec![srgb])
    } else {
        namespace_node(A_NS, "noFill")
    }
}

fn drawing_line_node(properties: &HashMap<String, String>) -> WordNode {
    let xml = format!(
        "<root xmlns:a=\"{}\">{}</root>",
        A_NS,
        drawing_line_xml(properties)
    );
    crate::handler::parse_document_xml(&xml)
        .ok()
        .and_then(|dom| dom.root.children.into_iter().next())
        .unwrap_or_else(|| namespace_node(A_NS, "ln"))
}

fn replace_drawing_child(parent: &mut WordNode, locals: &[&str], replacement: WordNode) {
    parent.children.retain(|child| !(child.namespace.as_deref() == Some(A_NS) && matches!(&child.element_type, WordElementType::Unknown(name) if locals.contains(&name.as_str()))));
    parent.children.push(replacement);
}

fn remove_shape_host_paragraph(
    node: &mut WordNode,
    kind: DrawingShapeKind,
    wanted: usize,
    seen: &mut usize,
) -> bool {
    let mut index = 0;
    while index < node.children.len() {
        if node.children[index].element_type == WordElementType::Paragraph {
            let mut count_in_paragraph = 0;
            count_shapes_in_node(&node.children[index], kind, &mut count_in_paragraph);
            if *seen + count_in_paragraph >= wanted && count_in_paragraph > 0 {
                node.children.remove(index);
                return true;
            }
            *seen += count_in_paragraph;
        } else if remove_shape_host_paragraph(&mut node.children[index], kind, wanted, seen) {
            return true;
        }
        index += 1;
    }
    false
}

fn count_shapes_in_node(node: &WordNode, kind: DrawingShapeKind, count: &mut usize) {
    if node_is_drawing_shape(node, kind) {
        *count += 1;
    }
    for child in &node.children {
        count_shapes_in_node(child, kind, count);
    }
}

fn remove_group_host_paragraph(node: &mut WordNode, wanted: usize, seen: &mut usize) -> bool {
    let mut index = 0;
    while index < node.children.len() {
        if node.children[index].element_type == WordElementType::Paragraph {
            let count = count_groups_in_node(&node.children[index]);
            if count > 0 && *seen + count >= wanted {
                node.children.remove(index);
                return true;
            }
            *seen += count;
        } else if remove_group_host_paragraph(&mut node.children[index], wanted, seen) {
            return true;
        }
        index += 1;
    }
    false
}

fn count_groups_in_node(node: &WordNode) -> usize {
    usize::from(node_is_drawing_group(node))
        + node
            .children
            .iter()
            .map(count_groups_in_node)
            .sum::<usize>()
}

/// Find next free chart index in word/charts/chartN.xml.
fn next_docx_chart_index(package: &OxmlPackage) -> usize {
    let mut i = 1;
    loop {
        let path = format!("word/charts/chart{}.xml", i);
        if package.read_part_xml(&path).is_err() {
            return i;
        }
        i += 1;
        // Sanity ceiling — no document should legitimately reach this.
        if i > 9999 {
            return i;
        }
    }
}

/// Build a self-contained ChartSpace XML for a docx chart. The chart embeds
/// its categories and values via `strCache` / `numCache` so viewers don't
/// need a backing spreadsheet.
fn build_docx_chart_xml(
    chart_type: &str,
    title: &str,
    cats: &[&str],
    vals: &[f64],
) -> Result<String, HandlerError> {
    let (bar_dir, chart_kind) = match chart_type {
        "bar" => ("bar", "barChart"),
        "column" => ("col", "barChart"),
        "line" => ("", "lineChart"),
        "pie" => ("", "pieChart"),
        other => {
            return Err(HandlerError::InvalidArgument(format!(
                "unsupported chart type '{}': expected bar/column/line/pie",
                other
            )))
        }
    };

    let mut xml = String::with_capacity(2048);
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    xml.push_str(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" ",
    );
    xml.push_str("xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" ");
    xml.push_str(
        "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">",
    );
    xml.push_str("<c:chart>");
    if !title.is_empty() {
        xml.push_str(&format!(
            "<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang=\"en-US\"/><a:t>{}</a:t></a:r></a:p></c:rich></c:tx></c:title>",
            xml_escape_text(title)
        ));
    }
    xml.push_str("<c:autoTitleDeleted val=\"0\"/>");
    xml.push_str("<c:plotArea><c:layout/>");
    xml.push_str(&format!("<c:{}>", chart_kind));
    if chart_kind == "barChart" {
        xml.push_str(&format!("<c:barDir val=\"{}\"/>", bar_dir));
        xml.push_str("<c:grouping val=\"clustered\"/>");
        xml.push_str("<c:varyColors val=\"1\"/>");
    } else if chart_kind == "lineChart" {
        xml.push_str("<c:grouping val=\"standard\"/>");
        xml.push_str("<c:varyColors val=\"1\"/>");
        xml.push_str("<c:smooth val=\"0\"/>");
    } else {
        xml.push_str("<c:varyColors val=\"1\"/>");
    }
    xml.push_str("<c:ser>");
    xml.push_str("<c:idx val=\"0\"/>");
    xml.push_str("<c:order val=\"0\"/>");
    if !cats.is_empty() {
        xml.push_str("<c:cat><c:strRef><c:f>categories</c:f><c:strCache>");
        xml.push_str(&format!("<c:ptCount val=\"{}\"/>", cats.len()));
        for (i, c) in cats.iter().enumerate() {
            xml.push_str(&format!(
                "<c:pt idx=\"{}\"><c:v>{}</c:v></c:pt>",
                i,
                xml_escape_text(c)
            ));
        }
        xml.push_str("</c:strCache></c:strRef></c:cat>");
    }
    xml.push_str("<c:val><c:numRef><c:f>values</c:f><c:numCache>");
    xml.push_str("<c:formatCode>General</c:formatCode>");
    xml.push_str(&format!("<c:ptCount val=\"{}\"/>", vals.len()));
    for (i, v) in vals.iter().enumerate() {
        xml.push_str(&format!("<c:pt idx=\"{}\"><c:v>{}</c:v></c:pt>", i, v));
    }
    xml.push_str("</c:numCache></c:numRef></c:val>");
    xml.push_str("</c:ser>");
    if chart_kind == "barChart" {
        xml.push_str("<c:axId val=\"1\"/><c:axId val=\"2\"/>");
        xml.push_str("</c:barChart>");
        xml.push_str("<c:catAx><c:axId val=\"1\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"bottom\"/></c:catAx>");
        xml.push_str("<c:valAx><c:axId val=\"2\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"left\"/></c:valAx>");
    } else if chart_kind == "lineChart" {
        xml.push_str("<c:axId val=\"1\"/><c:axId val=\"2\"/>");
        xml.push_str("</c:lineChart>");
        xml.push_str("<c:catAx><c:axId val=\"1\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"bottom\"/></c:catAx>");
        xml.push_str("<c:valAx><c:axId val=\"2\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"left\"/></c:valAx>");
    } else {
        xml.push_str("</c:pieChart>");
    }
    xml.push_str("</c:plotArea>");
    xml.push_str("</c:chart>");
    xml.push_str("</c:chartSpace>");
    Ok(xml)
}

/// Add a chart part Override entry to [Content_Types].xml. Chart parts
/// are unique per part-path so we use Override rather than Default.
fn update_docx_content_types_for_chart(
    package: &mut OxmlPackage,
    chart_path: &str,
) -> Result<(), HandlerError> {
    let xml = package
        .read_part_xml("[Content_Types].xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let part_name_attr = format!("PartName=\"/{}\"", chart_path);
    if xml.contains(&part_name_attr) {
        return Ok(());
    }
    let override_xml = format!(
        "<Override PartName=\"/{}\" ContentType=\"application/vnd.openxmlformats-officedocument.drawingml.chart+xml\"/>",
        chart_path
    );
    let new_xml = if let Some(close) = xml.find('>') {
        let mut out = String::with_capacity(xml.len() + override_xml.len());
        out.push_str(&xml[..close + 1]);
        out.push_str(&override_xml);
        out.push_str(&xml[close + 1..]);
        out
    } else {
        format!("<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">{}</Types>", override_xml)
    };
    package
        .write_part_xml("[Content_Types].xml", &new_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(())
}
fn next_docx_image_index(package: &OxmlPackage, ext: &str) -> usize {
    let mut i = 1;
    loop {
        let path = format!("word/media/image{}.{}", i, ext);
        if package.read_part_xml(&path).is_err() {
            return i;
        }
        i += 1;
    }
}

/// Find the next free rIdN in a relationships part. Defaults to rId1 if absent.
fn next_docx_rel_id(package: &OxmlPackage, rels_path: &str) -> String {
    let xml = package.read_part_xml(rels_path).unwrap_or_default();
    let mut max_id = 0usize;
    for part in xml.split("Id=\"rId") {
        if let Some(end) = part.find('"') {
            if let Ok(id) = part[..end].parse::<usize>() {
                if id > max_id {
                    max_id = id;
                }
            }
        }
    }
    format!("rId{}", max_id + 1)
}

/// Inject a Relationship element into a .rels part, creating the wrapper if missing.
fn inject_docx_relationship(
    package: &mut OxmlPackage,
    rels_path: &str,
    rel_xml: &str,
) -> Result<(), HandlerError> {
    let xml = package.read_part_xml(rels_path).unwrap_or_else(|_| {
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"/>".to_string()
    });

    let new_xml = if let Some(pos) = xml.find("</Relationships>") {
        let mut r = xml.clone();
        r.insert_str(pos, rel_xml);
        r
    } else if let Some(close) = xml.find("/>") {
        // Preserve the original root attributes (especially the package
        // relationships namespace) when expanding a self-closing root.
        // `create` writes an XML declaration before this element, so exact
        // string comparisons against `<Relationships .../>` are insufficient.
        let root_start = xml[..close].rfind("<Relationships");
        if root_start.is_some() {
            let mut r = String::with_capacity(xml.len() + rel_xml.len() + 16);
            r.push_str(&xml[..close]);
            r.push('>');
            r.push_str(rel_xml);
            r.push_str("</Relationships>");
            r.push_str(&xml[close + 2..]);
            r
        } else {
            let mut r = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">".to_string();
            r.push_str(rel_xml);
            r.push_str("</Relationships>");
            r
        }
    } else if xml.trim().is_empty() {
        let mut r = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">".to_string();
        r.push_str(rel_xml);
        r.push_str("</Relationships>");
        r
    } else {
        let mut r =
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships>"
                .to_string();
        r.push_str(rel_xml);
        r.push_str("</Relationships>");
        r
    };

    package
        .write_part_xml(rels_path, &new_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(())
}

/// Add Default extension entry to [Content_Types].xml if the extension isn't registered.
fn update_docx_content_types_for_image(
    package: &mut OxmlPackage,
    ext: &str,
    content_type: &str,
) -> Result<(), HandlerError> {
    let xml = package
        .read_part_xml("[Content_Types].xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let ext_attr = format!("Extension=\"{}\"", ext);
    if xml.contains(&ext_attr) {
        return Ok(());
    }
    let default_xml = format!(
        "<Default Extension=\"{}\" ContentType=\"{}\"/>",
        ext, content_type
    );
    let new_xml = if let Some(close) = xml.find('>') {
        // Insert Default right after <Types ...>.
        let mut out = String::with_capacity(xml.len() + default_xml.len());
        out.push_str(&xml[..close + 1]);
        out.push_str(&default_xml);
        out.push_str(&xml[close + 1..]);
        out
    } else {
        xml.replace("</Types>", &format!("{}{}</Types>", default_xml, ""))
    };
    package
        .write_part_xml("[Content_Types].xml", &new_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(())
}

/// Insert a `<w:drawing>` element inside the paragraph at `parent` path. If the
/// path is "/" or "/body", appends to the last paragraph in the body (or
/// creates one if none exists).
fn insert_drawing_in_paragraph(
    doc_xml: &str,
    parent: &str,
    drawing_xml: &str,
) -> Result<String, HandlerError> {
    let target = if parent == "/" || parent == "/body" || parent.is_empty() {
        // Append to last <w:p>...</w:p>, creating one if none exists.
        if let Some(close_idx) = doc_xml.rfind("</w:p>") {
            let mut out = String::with_capacity(doc_xml.len() + drawing_xml.len());
            out.push_str(&doc_xml[..close_idx]);
            out.push_str(drawing_xml);
            out.push_str(&doc_xml[close_idx..]);
            return Ok(out);
        }
        // No paragraphs: inject one right before </w:body>.
        let wrap = format!("<w:p>{}{}</w:p>", drawing_xml, "");
        if let Some(body_end) = doc_xml.find("</w:body>") {
            let mut out = String::with_capacity(doc_xml.len() + wrap.len());
            out.push_str(&doc_xml[..body_end]);
            out.push_str(&wrap);
            out.push_str(&doc_xml[body_end..]);
            return Ok(out);
        }
        return Err(HandlerError::OperationFailed(
            "could not locate body for image insertion".to_string(),
        ));
    } else {
        // Parent is like /body/p[N] — find the Nth <w:p>.
        let p_idx = parse_paragraph_index_from_parent(parent).ok_or_else(|| {
            HandlerError::InvalidPath(format!(
                "image add expects '/body/p[N]' parent, got '{}'",
                parent
            ))
        })?;
        locate_nth_w_p(doc_xml, p_idx).ok_or_else(|| {
            HandlerError::PathNotFound(format!("paragraph index {} not found", p_idx))
        })?
    };

    // target points at the position right before the matching </w:p>.
    let mut out = String::with_capacity(doc_xml.len() + drawing_xml.len());
    out.push_str(&doc_xml[..target]);
    out.push_str(drawing_xml);
    out.push_str(&doc_xml[target..]);
    Ok(out)
}

/// Ensure a direct XML insertion can be parsed even when the source document
/// was created with Word's minimal namespace set.  Part-aware image/chart
/// additions introduce DrawingML prefixes before the document is next passed
/// through the DOM serializer, so their declarations must be available now.
fn ensure_document_root_namespaces(xml: &str) -> String {
    let Some(root_start) = xml.find("<w:document") else {
        return xml.to_string();
    };
    let Some(root_end_relative) = xml[root_start..].find('>') else {
        return xml.to_string();
    };
    let root_end = root_start + root_end_relative;
    let root = &xml[root_start..=root_end];
    let namespaces = [
        ("a", "http://schemas.openxmlformats.org/drawingml/2006/main"),
        (
            "c",
            "http://schemas.openxmlformats.org/drawingml/2006/chart",
        ),
        (
            "pic",
            "http://schemas.openxmlformats.org/drawingml/2006/picture",
        ),
        (
            "r",
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
        ),
        (
            "wp",
            "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
        ),
        (
            "wps",
            "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
        ),
        (
            "wpg",
            "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup",
        ),
    ];

    let mut missing = String::new();
    for (prefix, uri) in namespaces {
        if !root.contains(&format!("xmlns:{}=", prefix)) {
            missing.push_str(&format!(" xmlns:{}=\"{}\"", prefix, uri));
        }
    }
    if missing.is_empty() {
        return xml.to_string();
    }

    let mut output = String::with_capacity(xml.len() + missing.len());
    output.push_str(&xml[..root_end]);
    output.push_str(&missing);
    output.push_str(&xml[root_end..]);
    output
}

/// Parse "/body/p[3]" → Some(3).
fn parse_paragraph_index_from_parent(parent: &str) -> Option<usize> {
    let lower = parent.to_lowercase();
    let pos = lower.find("/p[")?;
    let rest = &parent[pos + 3..];
    let end = rest.find(']')?;
    rest[..end].parse::<usize>().ok()
}

/// Return the byte offset of `</w:p>` for the Nth <w:p> element (1-based).
/// Matches both bare `<w:p>` and attributed `<w:p paraId="..." ...>`.
fn locate_nth_w_p(xml: &str, n: usize) -> Option<usize> {
    let bytes = xml.as_bytes();
    let mut count = 0;
    let mut i = 0;
    while i + 3 < bytes.len() {
        // Look for `<w:p` followed by `>` or ` `.
        if bytes[i] == b'<' && bytes[i + 1] == b'w' && bytes[i + 2] == b':' && bytes[i + 3] == b'p'
        {
            let next = bytes.get(i + 4).copied().unwrap_or(0);
            if next == b'>' || next == b' ' || next == b'\t' || next == b'\n' {
                count += 1;
                if count == n {
                    // Find the corresponding </w:p> after this opening tag.
                    return xml[i..].find("</w:p>").map(|p| i + p);
                }
            }
        }
        i += 1;
    }
    None
}

/// Parse dimension properties (width / height) in EMU. Accepts numeric EMU
/// or unit suffixes like "4in", "10cm", "200px", "300pt". Default 4in × 3in.
fn parse_image_dimensions_emu(props: &HashMap<String, String>) -> (i64, i64) {
    let width = props
        .get("width")
        .or_else(|| props.get("w"))
        .map(|s| parse_emu(s))
        .unwrap_or(3_657_600); // 4 inches
    let height = props
        .get("height")
        .or_else(|| props.get("h"))
        .map(|s| parse_emu(s))
        .unwrap_or(2_743_200); // 3 inches
    (width, height)
}

/// Convert a measurement string into EMU (English Metric Units: 914400/inch).
fn parse_emu(s: &str) -> i64 {
    let s = s.trim();
    if let Some(v) = s.strip_suffix("in") {
        v.trim()
            .parse::<f64>()
            .map(|n| (n * 914400.0) as i64)
            .unwrap_or(3_657_600)
    } else if let Some(v) = s.strip_suffix("cm") {
        v.trim()
            .parse::<f64>()
            .map(|n| (n * 360000.0) as i64)
            .unwrap_or(3_657_600)
    } else if let Some(v) = s.strip_suffix("mm") {
        v.trim()
            .parse::<f64>()
            .map(|n| (n * 36000.0) as i64)
            .unwrap_or(3_657_600)
    } else if let Some(v) = s.strip_suffix("pt") {
        v.trim()
            .parse::<f64>()
            .map(|n| (n * 12700.0) as i64)
            .unwrap_or(3_657_600)
    } else if let Some(v) = s.strip_suffix("px") {
        v.trim()
            .parse::<f64>()
            .map(|n| (n * 9525.0) as i64)
            .unwrap_or(3_657_600)
    } else {
        s.parse::<i64>().unwrap_or(3_657_600)
    }
}

fn docx_base64_decode(s: &str) -> Result<Vec<u8>, ()> {
    let mut bits: u32 = 0;
    let mut nbits: u32 = 0;
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for c in s.chars().filter(|c| !c.is_whitespace()) {
        let v: u32 = match c {
            'A'..='Z' => (c as u32) - ('A' as u32),
            'a'..='z' => (c as u32) - ('a' as u32) + 26,
            '0'..='9' => (c as u32) - ('0' as u32) + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '=' => break,
            _ => return Err(()),
        };
        bits = (bits << 6) | v;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((bits >> nbits) as u8);
        }
    }
    Ok(out)
}

fn docx_hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !cleaned.len().is_multiple_of(2) {
        return Err(());
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    let bytes = cleaned.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let byte = u8::from_str_radix(&format!("{}{}", bytes[i] as char, bytes[i + 1] as char), 16)
            .map_err(|_| ())?;
        out.push(byte);
        i += 2;
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────
// Document Defaults / Settings / Compatibility
//
// These targets live in `word/styles.xml` (`<w:docDefaults>`) and
// `word/settings.xml` (`<w:defaultTabStop>`, `<w:compat>`, etc.) — not in
// the document body, so the body DOM is the wrong layer. They mirror the C#
// `WordHandler.Set.DocDefaults.cs` / `Set.DocSettings.cs` / `Set.Compatibility.cs`
// partial classes but with a flat key namespace (no `rPr.` / `pPr.` nesting)
// since callers are humans/agents, not OOXML authors.

/// Keys accepted by `set /docDefaults`. The `r.` / `run.` prefix is optional;
/// `p.` / `para.` likewise.
const DOC_DEFAULTS_RUN_KEYS: &[&str] = &[
    "r.font",
    "run.font",
    "r.size",
    "run.size",
    "r.color",
    "run.color",
    "r.bold",
    "run.bold",
    "r.italic",
    "run.italic",
    "r.lang",
    "run.lang",
];
const DOC_DEFAULTS_PARA_KEYS: &[&str] = &[
    "p.spacing",
    "para.spacing",
    "p.align",
    "para.align",
    "p.ind",
    "para.ind",
];

pub fn set_doc_defaults_on_part(
    package: &mut OxmlPackage,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let xml = package
        .read_part_xml("word/styles.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Locate or synthesize <w:docDefaults>...</w:docDefaults>.
    let dd_open_marker = "<w:docDefaults";
    let dd_close_marker = "</w:docDefaults>";
    let doc_defaults_block = match xml.find(dd_open_marker) {
        Some(open) => {
            let close = match find_matching_close(&xml, open, dd_open_marker, dd_close_marker) {
                Some(c) => c,
                None => {
                    return Err(HandlerError::OperationFailed(
                        "malformed <w:docDefaults> block in styles.xml".into(),
                    ))
                }
            };
            xml[open..close].to_string()
        }
        None => {
            r#"<w:docDefaults><w:rPrDefault><w:rPr/></w:rPrDefault><w:pPrDefault><w:pPr/></w:pPrDefault></w:docDefaults>"#
                .to_string()
        }
    };

    let mut new_block = doc_defaults_block.clone();
    let mut unsupported = Vec::new();

    // Route each property into the rPr (inside <w:rPrDefault>) or pPr
    // (inside <w:pPrDefault>) sub-block of docDefaults. The wrapped block
    // includes `<w:rPr>...</w:rPr>` or `<w:pPr>...</w:pPr>` tags so that
    // set_or_replace_attr_child / toggle_flag_child still anchor on the
    // opening tag's `>`.
    for (k, v) in properties {
        let key = k.as_str();
        if DOC_DEFAULTS_RUN_KEYS.contains(&key) {
            let mut rpr = extract_or_synthesize_wrapped(&mut new_block, "w:rPrDefault", "w:rPr");
            apply_run_property(&mut rpr, key_to_run_attr(key), v);
            splice_wrapped_back(&mut new_block, "w:rPrDefault", "w:rPr", &rpr);
        } else if DOC_DEFAULTS_PARA_KEYS.contains(&key) {
            let mut ppr = extract_or_synthesize_wrapped(&mut new_block, "w:pPrDefault", "w:pPr");
            apply_para_property(&mut ppr, key_to_para_attr(key), v);
            splice_wrapped_back(&mut new_block, "w:pPrDefault", "w:pPr", &ppr);
        } else {
            unsupported.push(k.clone());
        }
    }

    if new_block != doc_defaults_block {
        let new_xml = if xml.contains(dd_open_marker) {
            let open = xml.find(dd_open_marker).unwrap();
            let close = find_matching_close(&xml, open, dd_open_marker, dd_close_marker).unwrap();
            let mut out = String::with_capacity(xml.len() + new_block.len());
            out.push_str(&xml[..open]);
            out.push_str(&new_block);
            out.push_str(&xml[close..]);
            out
        } else {
            // Insert docDefaults right after the root <w:styles ...> opening tag.
            let open = match xml
                .find("<w:styles")
                .and_then(|p| xml[p..].find('>').map(|q| p + q + 1))
            {
                Some(p) => p,
                None => {
                    return Err(HandlerError::OperationFailed(
                        "styles.xml missing <w:styles> root".into(),
                    ))
                }
            };
            let mut out = String::with_capacity(xml.len() + new_block.len() + 2);
            out.push_str(&xml[..open]);
            out.push_str(&new_block);
            out.push_str(&xml[open..]);
            out
        };

        package
            .write_part_xml("word/styles.xml", &new_xml)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    }

    Ok(unsupported)
}

/// Extract the wrapped `<w:{inner}>…</w:{inner}>` block from inside the
/// `<w:{wrapper}>…</w:{wrapper}>` element of `block`. The returned string
/// contains the wrapper's child element with its open+close tags so that
/// helpers (which anchor on an opening tag's `>`) work unchanged.
/// If either wrapper or inner is missing, synthesize an empty
/// `<w:{wrapper}><w:{inner}></w:{inner}></w:{wrapper}>` fragment.
fn extract_or_synthesize_wrapped(block: &mut String, wrapper: &str, inner: &str) -> String {
    let wrapper_open_marker = format!("<{}", wrapper);
    let wrapper_open = match block.find(wrapper_open_marker.as_str()) {
        Some(p) => p,
        None => {
            let fragment = format!("<{}><{}></{}></{}>", wrapper, inner, inner, wrapper);
            let insert_at = find_inner_insertion_point(block);
            block.insert_str(insert_at, &fragment);
            return format!("<{}></{}>", inner, inner);
        }
    };
    let wrapper_tag_close = match block[wrapper_open..].find('>') {
        Some(q) => wrapper_open + q,
        None => return format!("<{}></{}>", inner, inner),
    };
    if block.as_bytes().get(wrapper_tag_close.saturating_sub(1)) == Some(&b'/') {
        // `<w:{wrapper}/>` self-closed — replace with explicit open/close
        // containing an empty inner element.
        let replacement = format!("<{}><{}></{}></{}>", wrapper, inner, inner, wrapper);
        block.replace_range(wrapper_open..wrapper_tag_close + 1, &replacement);
        return format!("<{}></{}>", inner, inner);
    }
    let after_open = wrapper_tag_close + 1;
    let inner_open_marker = format!("<{}", inner);
    let inner_open = match block[after_open..].find(inner_open_marker.as_str()) {
        Some(p) => after_open + p,
        None => {
            // Insert an empty inner element right after wrapper open.
            let empty_inner = format!("<{}></{}>", inner, inner);
            block.insert_str(after_open, &empty_inner);
            return empty_inner;
        }
    };
    let inner_tag_close = match block[inner_open..].find('>') {
        Some(q) => inner_open + q,
        None => return format!("<{}></{}>", inner, inner),
    };
    if block.as_bytes().get(inner_tag_close.saturating_sub(1)) == Some(&b'/') {
        // Self-closed `<w:{inner}/>`. Replace with `<w:{inner}></w:{inner}>`.
        let replacement = format!("<{}></{}>", inner, inner);
        block.replace_range(inner_open..inner_tag_close + 1, &replacement);
        return replacement;
    }
    // Inner has explicit open/close. Find the close.
    let inner_close_marker = format!("</{}>", inner);
    let close_from = inner_tag_close + 1;
    let inner_close = match block[close_from..].find(inner_close_marker.as_str()) {
        Some(p) => close_from + p,
        None => return format!("<{}></{}>", inner, inner),
    };
    let end_after_close = inner_close + inner_close_marker.len();
    block[inner_open..end_after_close].to_string()
}

/// Splice the (possibly modified) wrapped inner block back into `block`.
/// Inverse of `extract_or_synthesize_wrapped`.
fn splice_wrapped_back(block: &mut String, wrapper: &str, inner: &str, new_wrapped: &str) {
    let wrapper_open_marker = format!("<{}", wrapper);
    let inner_open_marker = format!("<{}", inner);
    let wrapper_open = match block.find(wrapper_open_marker.as_str()) {
        Some(p) => p,
        None => return,
    };
    let inner_search_from = match block[wrapper_open..].find('>') {
        Some(q) => wrapper_open + q + 1,
        None => return,
    };
    let inner_open = match block[inner_search_from..].find(inner_open_marker.as_str()) {
        Some(p) => inner_search_from + p,
        None => return,
    };
    let inner_tag_close = match block[inner_open..].find('>') {
        Some(q) => inner_open + q,
        None => return,
    };
    if block.as_bytes().get(inner_tag_close.saturating_sub(1)) == Some(&b'/') {
        block.replace_range(inner_open..inner_tag_close + 1, new_wrapped);
        return;
    }
    let inner_close_marker = format!("</{}>", inner);
    let close_from = inner_tag_close + 1;
    let inner_close = match block[close_from..].find(inner_close_marker.as_str()) {
        Some(p) => close_from + p,
        None => return,
    };
    let end_after_close = inner_close + inner_close_marker.len();
    block.replace_range(inner_open..end_after_close, new_wrapped);
}

/// Insertion point inside the outermost <w:docDefaults>...</w:docDefaults>
/// block — right after `<w:docDefaults...>` opening tag.
fn find_inner_insertion_point(block: &str) -> usize {
    let open_marker = "<w:docDefaults";
    if let Some(p) = block.find(open_marker) {
        if let Some(q) = block[p..].find('>') {
            return p + q + 1;
        }
    }
    0
}

/// Translate a CLI key (`r.font`, `run.size`, …) to its OOXML child-element
/// local name (`w:rFonts`, `w:sz`, …).
fn key_to_run_attr(key: &str) -> &'static str {
    let bare = key.split('.').next_back().unwrap_or(key);
    match bare {
        "font" => "w:rFonts",
        "size" => "w:sz",
        "color" => "w:color",
        "bold" => "w:b",
        "italic" => "w:i",
        "lang" => "w:lang",
        _ => "",
    }
}

fn key_to_para_attr(key: &str) -> &'static str {
    let bare = key.split('.').next_back().unwrap_or(key);
    match bare {
        "spacing" => "w:spacing",
        "align" => "w:jc",
        "ind" => "w:ind",
        _ => "",
    }
}

/// Apply a property by rewriting the OOXML child inside the rPr/rPrDefault
/// parent. Mutates `block` in place.
fn apply_run_property(block: &mut String, child_tag: &str, value: &str) {
    if child_tag.is_empty() {
        return;
    }
    match child_tag {
        "w:rFonts" => set_or_replace_attr_child(block, "w:rFonts", "w:ascii", value),
        "w:sz" => set_or_replace_attr_child(block, "w:sz", "w:val", value),
        "w:color" => set_or_replace_attr_child(block, "w:color", "w:val", value),
        "w:b" | "w:i" => toggle_flag_child(block, child_tag, value),
        "w:lang" => set_or_replace_attr_child(block, "w:lang", "w:val", value),
        _ => {}
    }
}

fn apply_para_property(block: &mut String, child_tag: &str, value: &str) {
    if child_tag.is_empty() {
        return;
    }
    match child_tag {
        "w:spacing" => set_or_replace_attr_child(block, "w:spacing", "w:after", value),
        "w:jc" => set_or_replace_attr_child(block, "w:jc", "w:val", value),
        "w:ind" => set_or_replace_attr_child(block, "w:ind", "w:left", value),
        _ => {}
    }
}

// ─────────────────────────────────────────────────────────────────────────
// word/settings.xml — `<w:defaultTabStop>`, `<w:compat>` flags, and other
// top-level settings elements. This is a thin, op-aware wrapper that mirrors
// the C# WordHandler.Set.DocSettings.cs / Set.Compatibility.cs.

/// Keys that toggle a `<w:compat>` flag (present = true, absent = false).
const COMPAT_FLAGS: &[&str] = &[
    "useFELayout",
    "doNotExpandShiftReturn",
    "noLineBreaksAfter",
    "noLineBreaksBefore",
    "saveIfXMLInvalid",
    "doNotUseEastAsianBreakRules",
    "useWord2013",
    "compatExp",
];

/// Keys that map to a single self-closing settings element.
const SETTINGS_ELEMENT_KEYS: &[(&str, &str, &str)] = &[
    // (cli key, child element name, attribute name for value)
    ("defaultTabStop", "w:defaultTabStop", "w:val"),
    (
        "characterSpacingControl",
        "w:characterSpacingControl",
        "w:val",
    ),
    ("trackChanges", "w:trackChanges", ""),
    ("defaultDateFormat", "w:date", "w:val"),
    ("linkStyles", "w:linkStyles", ""),
    ("alignBordersAndEdges", "w:alignBordersAndEdges", ""),
    ("autoFormatOverride", "w:autoFormatOverride", ""),
    ("displayBackgroundShape", "w:displayBackgroundShape", ""),
    (
        "doNotDisplayPageBoundaries",
        "w:doNotDisplayPageBoundaries",
        "",
    ),
    ("embedSystemFonts", "w:embedSystemFonts", ""),
    ("zoomPercent", "w:zoom", "w:percent"),
    ("evenAndOddHeaders", "w:evenAndOddHeaders", ""),
];

pub fn set_settings_on_part(
    package: &mut OxmlPackage,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let mut xml = package
        .read_part_xml("word/settings.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let mut unsupported = Vec::new();

    for (key, value) in properties {
        // A CLI key may use plain form (`compat.useFELayout`) or a bare form
        // (`useFELayout`). The `compat.` / `settings.` prefix is stripped
        // before lookup.
        let bare = key
            .strip_prefix("compat.")
            .or_else(|| key.strip_prefix("settings."))
            .unwrap_or(key);

        if COMPAT_FLAGS.contains(&bare) {
            xml = update_compat_flag(&xml, bare, value);
            continue;
        }

        if let Some((_, elem, attr)) = SETTINGS_ELEMENT_KEYS
            .iter()
            .find(|(k, _, _)| k.eq_ignore_ascii_case(bare))
        {
            xml = update_settings_child(&xml, elem, attr, value);
            continue;
        }

        unsupported.push(key.clone());
    }

    package
        .write_part_xml("word/settings.xml", &xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    Ok(unsupported)
}

/// Insert, replace, or remove a `<w:compat>` flag child of `<w:compat>`.
fn update_compat_flag(xml: &str, flag: &str, value: &str) -> String {
    let tag = format!("w:{}", flag);
    let truthy = matches!(
        value.to_ascii_lowercase().as_str(),
        "true" | "1" | "on" | "yes" | ""
    );
    let current = strip_named_element_pub(xml, &tag);
    if !truthy {
        return current;
    }
    // Insert the new flag inside <w:compat>…</w:compat>. If the wrapper is
    // missing, synthesize it just before </w:settings>.
    let compat_marker = "<w:compat>";
    if let Some(open) = current.find(compat_marker) {
        let insert_pos = open + compat_marker.len();
        let mut out = String::with_capacity(current.len() + tag.len() + 4);
        out.push_str(&current[..insert_pos]);
        out.push_str(&format!("<{}/>", tag));
        out.push_str(&current[insert_pos..]);
        return out;
    }
    let settings_close = "</w:settings>";
    if let Some(p) = current.rfind(settings_close) {
        let insertion = format!("<w:compat><{}/></w:compat>", tag);
        let mut out = String::with_capacity(current.len() + insertion.len());
        out.push_str(&current[..p]);
        out.push_str(&insertion);
        out.push_str(&current[p..]);
        return out;
    }
    current
}

/// Insert or replace a single self-closing `<w:NAME w:ATTR="VAL"/>` (or just
/// `<w:NAME/>` when ATTR is empty) at the start of `<w:settings>`. Removes any
/// existing same-named child first.
fn update_settings_child(xml: &str, elem: &str, attr: &str, value: &str) -> String {
    let current = strip_named_element_pub(xml, elem);
    // Build the new element fragment.
    let fragment = if attr.is_empty() {
        format!("<{}/>", elem)
    } else {
        format!("<{} {}=\"{}\"/>", elem, attr, escape_attr(value))
    };
    // Inject right after the opening <w:settings ...> tag.
    let open_close = match current
        .find("<w:settings")
        .and_then(|p| find_tag_close_after(&current, p).map(|q| q + 1))
    {
        Some(p) => p,
        None => return current, // malformed; give up gracefully
    };
    let mut out = String::with_capacity(current.len() + fragment.len() + 2);
    out.push_str(&current[..open_close]);
    out.push_str(&fragment);
    out.push_str(&current[open_close..]);
    out
}

/// Strip every `<prefix:name …>…</prefix:name>` or `<prefix:name …/>` from `xml`.
/// Walks the opening tag char-by-char to find its real close `>`, then either
/// consumes a self-closing form or scans to the matching close tag.
fn strip_named_element_pub(xml: &str, qualified_name: &str) -> String {
    let mut out = xml.to_string();
    let open_tag_pat = format!("<{}", qualified_name);
    let close_tag_pat = format!("</{}>", qualified_name);
    loop {
        let Some(open) = out.find(&open_tag_pat) else {
            break;
        };
        // Ensure the match is the start of a real tag (not a longer name).
        let next = out.as_bytes().get(open + open_tag_pat.len()).copied();
        if !matches!(
            next,
            Some(b' ') | Some(b'/') | Some(b'>') | Some(b'\t') | Some(b'\n') | Some(b'\r')
        ) {
            break;
        }
        let opening_close = match find_tag_close_after(&out, open) {
            Some(p) => p,
            None => break,
        };
        let opening_close_end = opening_close + 1;
        let self_closing = out.as_bytes().get(opening_close).copied() == Some(b'/');
        if self_closing {
            out.replace_range(open..opening_close_end, "");
            continue;
        }
        let Some(close_rel) = out[opening_close_end..].find(&close_tag_pat) else {
            break;
        };
        let close_start = opening_close_end + close_rel;
        let close_end = close_start + close_tag_pat.len();
        out.replace_range(open..close_end, "");
    }
    out
}

/// Find the byte index of the `>` (or `/` for self-close) that closes the
/// opening tag starting at `tag_open`. Walks through attribute values
/// respecting single/double quotes.
fn find_tag_close_after(s: &str, tag_open: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = tag_open;
    let mut in_single = false;
    let mut in_double = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_single {
            if b == b'\'' {
                in_single = false;
            }
        } else if in_double {
            if b == b'"' {
                in_double = false;
            }
        } else {
            match b {
                b'\'' => in_single = true,
                b'"' => in_double = true,
                b'/' => {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'>' {
                        return Some(i);
                    }
                }
                b'>' => return Some(i),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod doc_settings_tests {
    use super::*;

    #[test]
    fn settings_key_lookup() {
        let table = SETTINGS_ELEMENT_KEYS;
        assert!(table.iter().any(|(k, _, _)| *k == "defaultTabStop"));
        assert!(table
            .iter()
            .any(|(k, _, _)| *k == "characterSpacingControl"));
    }

    #[test]
    fn compat_flag_toggle_inserts_into_compat_block() {
        let xml = r#"<w:settings xmlns:w="w"><w:compat/></w:settings>"#;
        let out = update_compat_flag(xml, "useFELayout", "true");
        assert!(out.contains("<w:useFELayout/>"));
        assert!(out.contains("<w:compat><w:useFELayout/>"));
    }

    #[test]
    fn compat_flag_false_strips_existing() {
        let xml = r#"<w:settings><w:compat><w:useFELayout/></w:compat></w:settings>"#;
        let out = update_compat_flag(xml, "useFELayout", "false");
        assert!(!out.contains("useFELayout"));
    }

    #[test]
    fn compat_flag_synthesizes_compat_block_when_missing() {
        let xml = r#"<w:settings></w:settings>"#;
        let out = update_compat_flag(xml, "useFELayout", "true");
        assert!(out.contains("<w:compat><w:useFELayout/></w:compat>"));
    }

    #[test]
    fn settings_inserts_default_tab_stop() {
        let xml = r#"<w:settings xmlns:w="w"></w:settings>"#;
        let out = update_settings_child(xml, "w:defaultTabStop", "w:val", "720");
        assert!(out.contains("<w:defaultTabStop w:val=\"720\"/>"));
    }

    #[test]
    fn settings_replaces_existing() {
        let xml = r#"<w:settings><w:defaultTabStop w:val="360"/></w:settings>"#;
        let out = update_settings_child(xml, "w:defaultTabStop", "w:val", "720");
        assert!(out.contains("720"));
        assert!(!out.contains("360"));
    }

    #[test]
    fn key_to_run_attr_maps_known_keys() {
        assert_eq!(key_to_run_attr("r.font"), "w:rFonts");
        assert_eq!(key_to_run_attr("run.size"), "w:sz");
        assert_eq!(key_to_run_attr("r.color"), "w:color");
        assert_eq!(key_to_run_attr("r.bold"), "w:b");
    }

    #[test]
    fn key_to_para_attr_maps_known_keys() {
        assert_eq!(key_to_para_attr("p.spacing"), "w:spacing");
        assert_eq!(key_to_para_attr("para.align"), "w:jc");
        assert_eq!(key_to_para_attr("p.ind"), "w:ind");
    }
}

#[cfg(test)]
mod chart_tests {
    use super::*;

    #[test]
    fn column_chart_has_axes_and_categories() {
        let cats = vec!["Q1", "Q2", "Q3"];
        let vals = vec![10.0, 20.0, 30.0];
        let xml = build_docx_chart_xml("column", "Revenue", &cats, &vals).unwrap();
        assert!(xml.contains("<c:barChart>"));
        assert!(xml.contains("<c:barDir val=\"col\"/>"));
        assert!(xml.contains("<c:title>"));
        assert!(xml.contains("Revenue</a:t>"));
        assert!(xml.contains("Q1</c:v>"));
        assert!(xml.contains("30</c:v>"));
        assert!(xml.contains("<c:catAx>"));
        assert!(xml.contains("<c:valAx>"));
    }

    #[test]
    fn bar_chart_uses_horizontal_dir() {
        let empty: [&str; 0] = [];
        let xml = build_docx_chart_xml("bar", "", &empty, &[1.0]).unwrap_or_default();
        // No title when empty.
        assert!(!xml.contains("<c:title>"));
        assert!(xml.contains("<c:barDir val=\"bar\"/>"));
    }

    #[test]
    fn line_chart_has_two_axes_no_bar_dir() {
        let xml = build_docx_chart_xml("line", "Trend", &["A", "B"], &[5.0, 9.0]).unwrap();
        assert!(xml.contains("<c:lineChart>"));
        assert!(!xml.contains("<c:barDir"));
        assert!(xml.contains("<c:grouping val=\"standard\"/>"));
    }

    #[test]
    fn pie_chart_has_no_axes() {
        let xml = build_docx_chart_xml("pie", "Share", &["A", "B"], &[1.0, 2.0]).unwrap();
        assert!(xml.contains("<c:pieChart>"));
        assert!(!xml.contains("<c:catAx>"));
        assert!(!xml.contains("<c:valAx>"));
    }

    #[test]
    fn unknown_chart_type_rejected() {
        let err = build_docx_chart_xml("radar", "x", &["a"], &[1.0]).unwrap_err();
        match err {
            HandlerError::InvalidArgument(msg) => assert!(msg.contains("radar")),
            other => panic!("expected InvalidArgument, got {:?}", other),
        }
    }

    #[test]
    fn text_escaping_in_titles() {
        let xml = build_docx_chart_xml("pie", "A & B < C >", &["A"], &[1.0]).unwrap();
        assert!(xml.contains("A &amp; B &lt; C &gt;</a:t>"));
    }
}

#[cfg(test)]
mod comment_tests {
    use super::*;

    fn comment_test_package() -> OxmlPackage {
        let mut package = OxmlPackage::create("comment-test.docx");
        package.add_part(
            "[Content_Types].xml",
            br#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
        );
        package.add_part(
            "word/_rels/document.xml.rels",
            br#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#,
        );
        package.add_part(
            "word/document.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>first</w:t></w:r><w:r><w:t>second</w:t></w:r></w:p><w:sectPr/></w:body></w:document>"#,
        );
        package
    }

    #[test]
    fn comment_lifecycle_wires_package_and_cleans_anchors() {
        let mut package = comment_test_package();
        let properties = HashMap::from([
            ("text".to_string(), "Review this".to_string()),
            ("author".to_string(), "Alice Example".to_string()),
            ("date".to_string(), "2026-08-03T10:30:00Z".to_string()),
        ]);

        let path = add_comment_part_aware(&mut package, "/body/p[1]/r[2]", &properties)
            .expect("add comment");
        assert_eq!(path, "/comments/comment[@commentId=0]");
        let comments = list_comments(&package).expect("list comments");
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].text, "Review this");
        assert_eq!(comments[0].author.as_deref(), Some("Alice Example"));
        assert_eq!(comments[0].anchor.as_deref(), Some("/body/p[1]"));
        assert!(package
            .read_part_xml("word/_rels/document.xml.rels")
            .unwrap()
            .contains(DOCX_COMMENTS_REL_TYPE));
        assert!(package
            .read_part_xml("[Content_Types].xml")
            .unwrap()
            .contains("/word/comments.xml"));
        let document = package.read_part_xml(DOCX_DOCUMENT_PART).unwrap();
        assert!(document.contains("commentRangeStart"));
        assert!(document.contains("commentRangeEnd"));
        assert!(document.contains("commentReference"));

        let set = HashMap::from([("text".to_string(), "Updated comment".to_string())]);
        assert!(
            set_comment_on_part(&mut package, DOCX_COMMENTS_PART, &path, &set)
                .expect("set comment")
                .is_empty()
        );
        assert_eq!(
            get_comment(&package, &path).unwrap().text,
            "Updated comment"
        );

        remove_comment_part_aware(&mut package, &path).expect("remove comment");
        assert!(list_comments(&package).unwrap().is_empty());
        let document = package.read_part_xml(DOCX_DOCUMENT_PART).unwrap();
        assert!(!document.contains("commentRangeStart"));
        assert!(!document.contains("commentRangeEnd"));
        assert!(!document.contains("commentReference"));
    }

    #[test]
    fn comment_positional_paths_use_collection_order_not_ooxml_ids() {
        let mut package = comment_test_package();
        let first = HashMap::from([
            ("text".to_string(), "first comment".to_string()),
            ("commentId".to_string(), "7".to_string()),
        ]);
        let second = HashMap::from([
            ("text".to_string(), "second comment".to_string()),
            ("commentId".to_string(), "42".to_string()),
        ]);
        add_comment_part_aware(&mut package, "/body/p[1]/r[1]", &first).unwrap();
        add_comment_part_aware(&mut package, "/body/p[1]/r[2]", &second).unwrap();
        assert_eq!(
            get_comment(&package, "/comments/comment[1]").unwrap().id,
            "7"
        );
        assert_eq!(
            get_comment(&package, "/comments/comment[@commentId=42]")
                .unwrap()
                .text,
            "second comment"
        );
    }

    #[test]
    fn point_and_cross_paragraph_comment_ranges_preserve_marker_shape() {
        let mut package = comment_test_package();
        let point = HashMap::from([
            ("text".to_string(), "point".to_string()),
            ("pointRef".to_string(), "true".to_string()),
        ]);
        add_comment_part_aware(&mut package, "/body/p[1]/r[1]", &point).unwrap();
        let document = package.read_part_xml(DOCX_DOCUMENT_PART).unwrap();
        assert_eq!(document.matches("commentReference").count(), 1);
        assert!(!document.contains("commentRangeStart"));

        let open = HashMap::from([
            ("text".to_string(), "spanning".to_string()),
            ("rangeOpen".to_string(), "true".to_string()),
        ]);
        add_comment_part_aware(&mut package, "/body/p[1]/r[2]", &open).unwrap();
        let close = HashMap::from([("rangeEnd".to_string(), "true".to_string())]);
        close_open_comment_range(&mut package, "/body/p[1]/r[2]", &close).unwrap();
        let document = package.read_part_xml(DOCX_DOCUMENT_PART).unwrap();
        assert_eq!(document.matches("commentRangeStart").count(), 1);
        assert_eq!(document.matches("commentRangeEnd").count(), 1);
        assert_eq!(document.matches("commentReference").count(), 2);
    }

    #[test]
    fn modern_comment_metadata_round_trips_reply_and_resolved_state() {
        let mut package = comment_test_package();
        let root = HashMap::from([
            ("text".to_string(), "root".to_string()),
            ("done".to_string(), "true".to_string()),
        ]);
        add_comment_part_aware(&mut package, "/body/p[1]/r[1]", &root).unwrap();
        let reply = HashMap::from([
            ("text".to_string(), "reply".to_string()),
            ("parentId".to_string(), "0".to_string()),
        ]);
        add_comment_part_aware(&mut package, "/body/p[1]/r[2]", &reply).unwrap();

        let comments = list_comments(&package).unwrap();
        assert_eq!(comments.len(), 2);
        assert!(comments[0].done);
        assert_eq!(comments[1].parent_id.as_deref(), Some("0"));
        let extended = package.read_part_xml(DOCX_COMMENTS_EXT_PART).unwrap();
        assert!(extended.contains("w15:commentEx"));
        assert!(extended.contains("w15:paraIdParent"));

        let text_update = HashMap::from([("text".to_string(), "updated reply".to_string())]);
        set_comment_on_part(
            &mut package,
            DOCX_COMMENTS_PART,
            "/comments/comment[@commentId=1]",
            &text_update,
        )
        .unwrap();
        let updated = get_comment(&package, "/comments/comment[@commentId=1]").unwrap();
        assert_eq!(updated.text, "updated reply");
        assert_eq!(updated.parent_id.as_deref(), Some("0"));

        let update = HashMap::from([("resolved".to_string(), "true".to_string())]);
        set_comment_on_part(
            &mut package,
            DOCX_COMMENTS_PART,
            "/comments/comment[@commentId=1]",
            &update,
        )
        .unwrap();
        assert!(
            get_comment(&package, "/comments/comment[@commentId=1]")
                .unwrap()
                .done
        );
    }

    #[test]
    fn preparing_comments_extended_replace_stamps_existing_comment_para_ids() {
        let mut package = comment_test_package();
        let definition_only = HashMap::from([
            ("text".to_string(), "existing comment".to_string()),
            ("id".to_string(), "42".to_string()),
            ("range".to_string(), "none".to_string()),
        ]);
        add_comment_part_aware(&mut package, "/body/p[1]", &definition_only).unwrap();
        prepare_comments_extended_raw_replace(&mut package).unwrap();

        let comments = package.read_part_xml(DOCX_COMMENTS_PART).unwrap();
        assert!(comments.contains("w14:paraId="));
        let rels = package.read_part_xml(DOCX_DOCUMENT_RELS_PART).unwrap();
        assert!(rels.contains(DOCX_COMMENTS_EXT_REL_TYPE));
        let types = package.read_part_xml("[Content_Types].xml").unwrap();
        assert!(types.contains("/word/commentsExtended.xml"));
        assert!(package.has_part(DOCX_COMMENTS_EXT_PART));
    }

    #[test]
    fn definition_only_comment_does_not_rewrite_the_document_part() {
        let mut package = comment_test_package();
        let before = package.read_part_xml(DOCX_DOCUMENT_PART).unwrap();
        let properties = HashMap::from([
            ("text".to_string(), "definition".to_string()),
            ("id".to_string(), "99".to_string()),
            ("range".to_string(), "none".to_string()),
        ]);
        add_comment_part_aware(&mut package, "/body/p[1]", &properties).unwrap();
        assert_eq!(package.read_part_xml(DOCX_DOCUMENT_PART).unwrap(), before);
        assert_eq!(
            get_comment(&package, "/comments/comment[@commentId=99]")
                .unwrap()
                .text,
            "definition"
        );
    }
}
