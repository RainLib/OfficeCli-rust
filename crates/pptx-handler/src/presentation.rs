//! Presentation-level settings stored in `ppt/presentation.xml` and presProps.

use handler_common::{DocumentNode, HandlerError};
use oxml::OxmlPackage;
use serde_json::Value;
use std::collections::HashMap;

const PRESENTATION: &str = "ppt/presentation.xml";
const PRESENTATION_RELS: &str = "ppt/_rels/presentation.xml.rels";
const PRES_PROPS: &str = "ppt/presProps.xml";
const PRES_PROPS_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/presProps";
const PRES_PROPS_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presProps+xml";

pub fn get(package: &OxmlPackage, depth: usize) -> Result<DocumentNode, HandlerError> {
    let presentation = read(package, PRESENTATION)?;
    let document = roxmltree::Document::parse(&presentation).map_err(|error| {
        HandlerError::OperationFailed(format!("invalid presentation.xml: {error}"))
    })?;
    let root = document.root_element();
    let mut node = DocumentNode::new("/presentation", "presentation");
    for (attr, key) in [
        ("firstSlideNum", "firstSlideNum"),
        ("rtl", "direction"),
        ("compatMode", "compatMode"),
        ("removePersonalInfoOnSave", "removePersonalInfo"),
    ] {
        if let Some(value) = root.attribute(attr) {
            if key == "direction" {
                if truthy(value) {
                    node = node.with_format(key, Value::String("rtl".to_string()));
                }
            } else if matches!(key, "compatMode" | "removePersonalInfo") {
                if truthy(value) {
                    node = node.with_format(key, Value::Bool(true));
                }
            } else {
                node = node.with_format(key, Value::String(value.to_string()));
            }
        }
    }
    if let Ok(xml) = package.read_part_xml(PRES_PROPS) {
        let props = roxmltree::Document::parse(&xml).map_err(|error| {
            HandlerError::OperationFailed(format!("invalid presentation properties: {error}"))
        })?;
        for (tag, mappings) in [
            (
                "prnPr",
                [
                    ("prnWhat", "print.what"),
                    ("clr", "print.colorMode"),
                    ("hiddenSlides", "print.hiddenSlides"),
                    ("scaleToFitPaper", "print.scaleToFitPaper"),
                    ("frameSlides", "print.frameSlides"),
                ]
                .as_slice(),
            ),
            (
                "showPr",
                [
                    ("loop", "show.loop"),
                    ("showNarration", "show.narration"),
                    ("showAnimation", "show.animation"),
                    ("useTimings", "show.useTimings"),
                ]
                .as_slice(),
            ),
        ] {
            if let Some(element) = props.descendants().find(|item| item.has_tag_name(tag)) {
                for (attr, key) in mappings {
                    if let Some(value) = element.attribute(*attr) {
                        node = node.with_format(
                            key,
                            if key.ends_with("what") || key.ends_with("colorMode") {
                                Value::String(value.to_string())
                            } else {
                                Value::Bool(truthy(value))
                            },
                        );
                    }
                }
            }
        }
    }
    if depth > 0 {
        node.text = Some("presentation settings".to_string());
    }
    Ok(node)
}

pub fn set(
    package: &mut OxmlPackage,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let mut presentation = read(package, PRESENTATION)?;
    let mut props_xml: Option<String> = None;
    let mut unsupported = Vec::new();
    for (key, value) in properties {
        match key.to_ascii_lowercase().as_str() {
            "firstslidenum" | "firstslidenumber" => {
                value.parse::<i32>().map_err(|_| {
                    HandlerError::InvalidArgument(format!(
                        "firstSlideNum must be an integer (got '{value}')"
                    ))
                })?;
                presentation = upsert_root_attribute(&presentation, "firstSlideNum", value)?;
            }
            "rtl" | "direction" => {
                presentation = upsert_root_attribute(&presentation, "rtl", bool_xml(value)?)?;
            }
            "compatmode" | "compatibilitymode" => {
                presentation =
                    upsert_root_attribute(&presentation, "compatMode", bool_xml(value)?)?;
            }
            "removepersonalinfoonsave" | "removepersonalinfo" => {
                presentation = upsert_root_attribute(
                    &presentation,
                    "removePersonalInfoOnSave",
                    bool_xml(value)?,
                )?;
            }
            "print.what" | "printwhat" => {
                let normalized = match value.to_ascii_lowercase().as_str() {
                    "slides" | "handouts1" | "handouts2" | "handouts3" | "handouts4"
                    | "handouts6" | "handouts9" | "notes" | "outline" => value.to_ascii_lowercase(),
                    "handouts" | "handout" => "handouts1".to_string(),
                    _ => {
                        return Err(HandlerError::InvalidArgument(format!(
                            "invalid print.what '{value}'"
                        )))
                    }
                };
                set_pres_prop(package, &mut props_xml, "prnPr", "prnWhat", &normalized)?;
            }
            "print.colormode" | "printcolormode" => {
                let normalized = match value.to_ascii_lowercase().as_str() {
                    "color" | "clr" => "clr",
                    "grayscale" | "gray" => "gray",
                    "blackandwhite" | "bw" => "bw",
                    _ => {
                        return Err(HandlerError::InvalidArgument(format!(
                            "invalid print.colorMode '{value}'"
                        )))
                    }
                };
                set_pres_prop(package, &mut props_xml, "prnPr", "clr", normalized)?;
            }
            "print.hiddenslides" => set_pres_prop(
                package,
                &mut props_xml,
                "prnPr",
                "hiddenSlides",
                bool_xml(value)?,
            )?,
            "print.scaletofitpaper" => set_pres_prop(
                package,
                &mut props_xml,
                "prnPr",
                "scaleToFitPaper",
                bool_xml(value)?,
            )?,
            "print.frameslides" => set_pres_prop(
                package,
                &mut props_xml,
                "prnPr",
                "frameSlides",
                bool_xml(value)?,
            )?,
            "show.loop" | "showloop" => {
                set_pres_prop(package, &mut props_xml, "showPr", "loop", bool_xml(value)?)?
            }
            "show.narration" | "shownarration" => set_pres_prop(
                package,
                &mut props_xml,
                "showPr",
                "showNarration",
                bool_xml(value)?,
            )?,
            "show.animation" | "showanimation" => set_pres_prop(
                package,
                &mut props_xml,
                "showPr",
                "showAnimation",
                bool_xml(value)?,
            )?,
            "show.usetimings" | "usetimings" => set_pres_prop(
                package,
                &mut props_xml,
                "showPr",
                "useTimings",
                bool_xml(value)?,
            )?,
            _ => unsupported.push(key.clone()),
        }
    }
    write(package, PRESENTATION, &presentation)?;
    if let Some(xml) = props_xml {
        write(package, PRES_PROPS, &xml)?;
    }
    Ok(unsupported)
}

fn set_pres_prop(
    package: &mut OxmlPackage,
    xml: &mut Option<String>,
    tag: &str,
    attr: &str,
    value: &str,
) -> Result<(), HandlerError> {
    if xml.is_none() {
        ensure_pres_props_wiring(package)?;
        *xml = Some(package.read_part_xml(PRES_PROPS).unwrap_or_else(|_| {
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><p:presProps xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"/>".to_string()
        }));
    }
    let current = xml.as_ref().expect("initialized presentation properties");
    let current = expand_pres_props_root(current)?;
    *xml = Some(upsert_child_attribute(&current, tag, attr, value)?);
    Ok(())
}

fn expand_pres_props_root(xml: &str) -> Result<String, HandlerError> {
    let start = xml.find("<p:presProps").ok_or_else(|| {
        HandlerError::OperationFailed("missing presentation properties root".to_string())
    })?;
    let end = xml[start..]
        .find('>')
        .map(|offset| start + offset)
        .ok_or_else(|| {
            HandlerError::OperationFailed("unterminated presentation properties root".to_string())
        })?;
    if !xml[..=end].ends_with("/>") {
        return Ok(xml.to_string());
    }
    let mut updated = xml.to_string();
    updated.replace_range(end - 1..=end, "></p:presProps>");
    Ok(updated)
}

fn ensure_pres_props_wiring(package: &mut OxmlPackage) -> Result<(), HandlerError> {
    let content_types = read(package, "[Content_Types].xml")?;
    if !content_types.contains("PartName=\"/ppt/presProps.xml\"") {
        let pos = content_types.rfind("</Types>").ok_or_else(|| {
            HandlerError::OperationFailed("invalid [Content_Types].xml".to_string())
        })?;
        let mut updated = content_types;
        updated.insert_str(pos, &format!("<Override PartName=\"/ppt/presProps.xml\" ContentType=\"{PRES_PROPS_CONTENT_TYPE}\"/>"));
        write(package, "[Content_Types].xml", &updated)?;
    }
    let rels = package.read_part_xml(PRESENTATION_RELS).unwrap_or_else(|_| {
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"/>".to_string()
    });
    if !rels.contains(&format!("Type=\"{PRES_PROPS_REL}\"")) {
        let id = next_rel_id(&rels);
        let pos = rels.rfind("</Relationships>").ok_or_else(|| {
            HandlerError::OperationFailed("invalid presentation relationships".to_string())
        })?;
        let mut updated = rels;
        updated.insert_str(
            pos,
            &format!(
                "<Relationship Id=\"{id}\" Type=\"{PRES_PROPS_REL}\" Target=\"presProps.xml\"/>"
            ),
        );
        write(package, PRESENTATION_RELS, &updated)?;
    }
    if !package.has_part(PRES_PROPS) {
        write(package, PRES_PROPS, "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><p:presProps xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"/>")?;
    }
    Ok(())
}

fn upsert_root_attribute(xml: &str, attr: &str, value: &str) -> Result<String, HandlerError> {
    upsert_attribute_at(
        xml,
        xml.find("<p:presentation").ok_or_else(|| {
            HandlerError::OperationFailed("missing presentation root".to_string())
        })?,
        attr,
        value,
    )
}

fn upsert_child_attribute(
    xml: &str,
    tag: &str,
    attr: &str,
    value: &str,
) -> Result<String, HandlerError> {
    if let Some(start) = xml.find(&format!("<p:{tag}")) {
        return upsert_attribute_at(xml, start, attr, value);
    }
    let insertion = format!("<p:{tag} {attr}=\"{value}\"/>");
    let pos = if tag == "prnPr" {
        xml.find("<p:showPr")
            .or_else(|| xml.rfind("</p:presProps>"))
    } else {
        xml.rfind("</p:presProps>")
    }
    .ok_or_else(|| HandlerError::OperationFailed("invalid presentation properties".to_string()))?;
    let mut updated = xml.to_string();
    updated.insert_str(pos, &insertion);
    Ok(updated)
}

fn upsert_attribute_at(
    xml: &str,
    start: usize,
    attr: &str,
    value: &str,
) -> Result<String, HandlerError> {
    let end = xml[start..]
        .find('>')
        .map(|offset| start + offset)
        .ok_or_else(|| HandlerError::OperationFailed("unterminated XML element".to_string()))?;
    let open = &xml[start..=end];
    let replacement = if let Some(offset) = open.find(&format!(" {attr}=\"")) {
        let value_start = offset + attr.len() + 3;
        let value_end = open[value_start..]
            .find('"')
            .map(|offset| value_start + offset)
            .ok_or_else(|| HandlerError::OperationFailed("invalid XML attribute".to_string()))?;
        format!("{}{}{}", &open[..value_start], value, &open[value_end..])
    } else {
        let suffix_len = usize::from(open.ends_with("/>")) + 1;
        format!(
            "{} {attr}=\"{value}\"{}",
            &open[..open.len() - suffix_len],
            &open[open.len() - suffix_len..]
        )
    };
    let mut updated = xml.to_string();
    updated.replace_range(start..=end, &replacement);
    Ok(updated)
}

fn next_rel_id(rels: &str) -> String {
    let max = rels
        .match_indices("Id=\"rId")
        .filter_map(|(start, _)| {
            rels[start + 7..]
                .find('"')
                .and_then(|end| rels[start + 7..start + 7 + end].parse::<usize>().ok())
        })
        .max()
        .unwrap_or(0);
    format!("rId{}", max + 1)
}

fn bool_xml(value: &str) -> Result<&'static str, HandlerError> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok("1"),
        "0" | "false" | "no" | "off" | "" => Ok("0"),
        _ => Err(HandlerError::InvalidArgument(format!(
            "invalid boolean '{value}'"
        ))),
    }
}

fn truthy(value: &str) -> bool {
    matches!(value, "1" | "true" | "True")
}
fn read(package: &OxmlPackage, path: &str) -> Result<String, HandlerError> {
    package
        .read_part_xml(path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))
}
fn write(package: &mut OxmlPackage, path: &str, xml: &str) -> Result<(), HandlerError> {
    package
        .write_part_xml(path, xml)
        .map_err(|error| HandlerError::SaveError(error.to_string()))
}
