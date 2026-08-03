//! Workbook-level settings backed by `xl/workbook.xml`.

use handler_common::{DocumentNode, HandlerError};
use oxml::OxmlPackage;
use serde_json::Value;
use std::collections::HashMap;

const WORKBOOK: &str = "xl/workbook.xml";

pub fn populate_node(package: &OxmlPackage, node: &mut DocumentNode) -> Result<(), HandlerError> {
    let xml = read(package)?;
    let document = roxmltree::Document::parse(&xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid workbook.xml: {error}")))?;
    if let Some(props) = document
        .descendants()
        .find(|item| item.has_tag_name("workbookPr"))
    {
        for (attr, key) in [
            ("date1904", "workbook.date1904"),
            ("codeName", "workbook.codeName"),
            ("filterPrivacy", "workbook.filterPrivacy"),
            ("showObjects", "workbook.showObjects"),
            ("backupFile", "workbook.backupFile"),
            ("dateCompatibility", "workbook.dateCompatibility"),
        ] {
            if let Some(value) = props.attribute(attr) {
                let value = if matches!(attr, "codeName" | "showObjects") {
                    Value::String(value.to_string())
                } else {
                    Value::Bool(truthy(value))
                };
                *node = std::mem::replace(node, DocumentNode::new("", "")).with_format(key, value);
            }
        }
    }
    let calc = document
        .descendants()
        .find(|item| item.has_tag_name("calcPr"));
    *node = std::mem::replace(node, DocumentNode::new("", "")).with_format(
        "calc.fullPrecision",
        Value::Bool(
            calc.and_then(|item| item.attribute("fullPrecision"))
                .map(truthy)
                .unwrap_or(true),
        ),
    );
    if let Some(calc) = calc {
        for (attr, key, bool_value) in [
            ("calcMode", "calc.mode", false),
            ("iterate", "calc.iterate", true),
            ("iterateCount", "calc.iterateCount", false),
            ("iterateDelta", "calc.iterateDelta", false),
            ("fullCalcOnLoad", "calc.fullCalcOnLoad", true),
            ("refMode", "calc.refMode", false),
        ] {
            if let Some(value) = calc.attribute(attr) {
                let value = if bool_value {
                    Value::Bool(truthy(value))
                } else {
                    Value::String(value.to_string())
                };
                *node = std::mem::replace(node, DocumentNode::new("", "")).with_format(key, value);
            }
        }
    }
    if let Some(view) = document
        .descendants()
        .find(|item| item.has_tag_name("workbookView"))
    {
        for (attr, key) in [("activeTab", "activeTab"), ("firstSheet", "firstSheet")] {
            if let Some(value) = view.attribute(attr) {
                *node = std::mem::replace(node, DocumentNode::new("", ""))
                    .with_format(key, Value::String(value.to_string()));
            }
        }
    }
    if let Some(protection) = document
        .descendants()
        .find(|item| item.has_tag_name("workbookProtection"))
    {
        if truthy(protection.attribute("lockStructure").unwrap_or("0")) {
            *node = std::mem::replace(node, DocumentNode::new("", ""))
                .with_format("workbook.lockStructure", Value::Bool(true));
        }
        if truthy(protection.attribute("lockWindows").unwrap_or("0")) {
            *node = std::mem::replace(node, DocumentNode::new("", ""))
                .with_format("workbook.lockWindows", Value::Bool(true));
        }
        if let Some(hash) = protection.attribute("workbookPassword") {
            *node = std::mem::replace(node, DocumentNode::new("", ""))
                .with_format("workbook.passwordHash", Value::String(hash.to_string()));
            *node = std::mem::replace(node, DocumentNode::new("", ""))
                .with_format("workbook.password", Value::String("***".to_string()));
        }
    }
    Ok(())
}

pub fn set(
    package: &mut OxmlPackage,
    properties: &HashMap<String, String>,
    lock_structure_explicit: &mut bool,
) -> Result<Vec<String>, HandlerError> {
    let mut xml = read(package)?;
    let sheets = sheet_names(&xml)?;
    // HashMap property order is deliberately unspecified. Apply an explicit
    // lock intent before handling a password clear so a single Set request is
    // deterministic, and retain it for later calls on this handler just like
    // C#'s `_workbookLockStructureExplicit` field.
    if let Some(value) = properties.iter().find_map(|(key, value)| {
        matches!(
            key.to_ascii_lowercase().as_str(),
            "workbook.lockstructure" | "lockstructure"
        )
        .then_some(value)
    }) {
        *lock_structure_explicit = truthy(value);
    }
    let mut unsupported = Vec::new();
    for (key, value) in properties {
        match key.to_ascii_lowercase().as_str() {
            "workbook.date1904" | "date1904" => {
                xml = set_child_attr(
                    &xml,
                    "workbookPr",
                    "date1904",
                    bool_xml(value)?,
                    Some("sheets"),
                )?
            }
            "workbook.codename" | "codename" => {
                xml = set_child_attr(&xml, "workbookPr", "codeName", value, Some("sheets"))?
            }
            "workbook.filterprivacy" | "filterprivacy" => {
                xml = set_child_attr(
                    &xml,
                    "workbookPr",
                    "filterPrivacy",
                    bool_xml(value)?,
                    Some("sheets"),
                )?
            }
            "workbook.showobjects" | "showobjects" => {
                let normalized = match value.to_ascii_lowercase().as_str() {
                    "all" | "placeholders" | "none" => value.to_ascii_lowercase(),
                    _ => {
                        return Err(HandlerError::InvalidArgument(format!(
                            "invalid showObjects '{value}'"
                        )))
                    }
                };
                xml = set_child_attr(
                    &xml,
                    "workbookPr",
                    "showObjects",
                    &normalized,
                    Some("sheets"),
                )?;
            }
            "workbook.backupfile" | "backupfile" => {
                xml = set_child_attr(
                    &xml,
                    "workbookPr",
                    "backupFile",
                    bool_xml(value)?,
                    Some("sheets"),
                )?
            }
            "workbook.datecompatibility" | "datecompatibility" => {
                xml = set_child_attr(
                    &xml,
                    "workbookPr",
                    "dateCompatibility",
                    bool_xml(value)?,
                    Some("sheets"),
                )?
            }
            "calc.mode" | "calcmode" => {
                let normalized = match value.to_ascii_lowercase().as_str() {
                    "auto" | "automatic" => "auto",
                    "manual" => "manual",
                    "autonoexcepttables" | "autoexcepttables" | "autonotable" => "autoNoTable",
                    _ => {
                        return Err(HandlerError::InvalidArgument(format!(
                            "invalid calc.mode '{value}'"
                        )))
                    }
                };
                xml = set_child_attr(&xml, "calcPr", "calcMode", normalized, None)?;
            }
            "calc.iterate" | "iterate" => {
                xml = set_child_attr(&xml, "calcPr", "iterate", bool_xml(value)?, None)?
            }
            "calc.iteratecount" | "iteratecount" => {
                parse_u32(value, key)?;
                xml = set_child_attr(&xml, "calcPr", "iterateCount", value, None)?;
            }
            "calc.iteratedelta" | "iteratedelta" => {
                parse_f64(value, key)?;
                xml = set_child_attr(&xml, "calcPr", "iterateDelta", value, None)?;
            }
            "calc.fullprecision" | "fullprecision" => {
                xml = set_child_attr(&xml, "calcPr", "fullPrecision", bool_xml(value)?, None)?
            }
            "calc.fullcalconload" | "fullcalconload" => {
                xml = set_child_attr(&xml, "calcPr", "fullCalcOnLoad", bool_xml(value)?, None)?
            }
            "calc.refmode" | "refmode" => {
                let normalized = match value.to_ascii_lowercase().as_str() {
                    "a1" => "A1",
                    "r1c1" => "R1C1",
                    _ => {
                        return Err(HandlerError::InvalidArgument(format!(
                            "invalid calc.refMode '{value}'"
                        )))
                    }
                };
                xml = set_child_attr(&xml, "calcPr", "refMode", normalized, None)?;
            }
            "activetab" | "workbook.activetab" => {
                xml = set_view_attr(&xml, "activeTab", sheet_index(value, &sheets)?)?
            }
            "firstsheet" | "workbook.firstsheet" => {
                xml = set_view_attr(&xml, "firstSheet", sheet_index(value, &sheets)?)?
            }
            "workbook.protection" | "workbookprotection" => {
                if value.eq_ignore_ascii_case("none") || !truthy(value) {
                    xml = remove_child(&xml, "workbookProtection")?;
                } else {
                    xml = set_child_attr(
                        &xml,
                        "workbookProtection",
                        "lockStructure",
                        "1",
                        Some("bookViews"),
                    )?;
                    xml = set_child_attr(
                        &xml,
                        "workbookProtection",
                        "lockWindows",
                        "1",
                        Some("bookViews"),
                    )?;
                }
            }
            "workbook.lockstructure" | "lockstructure" => {
                xml = set_protection_attr(&xml, "lockStructure", value)?;
                *lock_structure_explicit = truthy(value);
            }
            "workbook.lockwindows" | "lockwindows" => {
                xml = set_protection_attr(&xml, "lockWindows", value)?;
            }
            "workbook.passwordhash" => {
                if value.is_empty() || value.eq_ignore_ascii_case("none") {
                    xml = remove_protection_attr(&xml, "workbookPassword")?;
                    if !*lock_structure_explicit {
                        xml = remove_protection_attr(&xml, "lockStructure")?;
                    }
                } else {
                    if !value.chars().all(|item| item.is_ascii_hexdigit()) {
                        return Err(HandlerError::InvalidArgument(
                            "workbook.passwordHash must be hexadecimal".to_string(),
                        ));
                    }
                    xml = set_child_attr(
                        &xml,
                        "workbookProtection",
                        "workbookPassword",
                        &value.to_ascii_uppercase(),
                        Some("bookViews"),
                    )?;
                    if !has_protection_lock(&xml)? {
                        xml = set_child_attr(
                            &xml,
                            "workbookProtection",
                            "lockStructure",
                            "1",
                            Some("bookViews"),
                        )?;
                    }
                }
            }
            "workbook.password" | "workbookpassword" => {
                if value.is_empty() || value.eq_ignore_ascii_case("none") {
                    xml = remove_protection_attr(&xml, "workbookPassword")?;
                    if !*lock_structure_explicit {
                        xml = remove_protection_attr(&xml, "lockStructure")?;
                    }
                } else {
                    let hash = legacy_password_hash(value);
                    xml = set_child_attr(
                        &xml,
                        "workbookProtection",
                        "workbookPassword",
                        &hash,
                        Some("bookViews"),
                    )?;
                    if !has_protection_lock(&xml)? {
                        xml = set_child_attr(
                            &xml,
                            "workbookProtection",
                            "lockStructure",
                            "1",
                            Some("bookViews"),
                        )?;
                    }
                }
            }
            _ => unsupported.push(key.clone()),
        }
    }
    package
        .write_part_xml(WORKBOOK, &xml)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    Ok(unsupported)
}

fn read(package: &OxmlPackage) -> Result<String, HandlerError> {
    package
        .read_part_xml(WORKBOOK)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))
}
fn truthy(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
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
fn parse_u32(value: &str, key: &str) -> Result<(), HandlerError> {
    value
        .parse::<u32>()
        .map(|_| ())
        .map_err(|_| HandlerError::InvalidArgument(format!("{key} must be an unsigned integer")))
}
fn parse_f64(value: &str, key: &str) -> Result<(), HandlerError> {
    match value.parse::<f64>() {
        Ok(item) if item.is_finite() => Ok(()),
        _ => Err(HandlerError::InvalidArgument(format!(
            "{key} must be finite"
        ))),
    }
}
fn sheet_names(xml: &str) -> Result<Vec<String>, HandlerError> {
    Ok(roxmltree::Document::parse(xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?
        .descendants()
        .filter(|item| item.has_tag_name("sheet"))
        .filter_map(|item| item.attribute("name").map(str::to_string))
        .collect())
}
fn sheet_index(value: &str, sheets: &[String]) -> Result<String, HandlerError> {
    if value.parse::<u32>().is_ok() {
        return Ok(value.to_string());
    }
    sheets
        .iter()
        .position(|item| item.eq_ignore_ascii_case(value))
        .map(|index| index.to_string())
        .ok_or_else(|| HandlerError::InvalidArgument(format!("'{value}' is not a workbook sheet")))
}
fn set_view_attr(xml: &str, attr: &str, value: String) -> Result<String, HandlerError> {
    set_child_attr(xml, "workbookView", attr, &value, Some("sheets"))
}
fn set_child_attr(
    xml: &str,
    tag: &str,
    attr: &str,
    value: &str,
    before: Option<&str>,
) -> Result<String, HandlerError> {
    if let Some(start) = xml.find(&format!("<{tag}")) {
        return upsert_attr(xml, start, attr, value);
    }
    let element = if tag == "workbookView" {
        format!("<bookViews><workbookView {attr}=\"{value}\"/></bookViews>")
    } else {
        format!("<{tag} {attr}=\"{value}\"/>")
    };
    let pos = before
        .and_then(|tag| xml.find(&format!("<{tag}")))
        .or_else(|| xml.rfind("</workbook>"))
        .ok_or_else(|| HandlerError::OperationFailed("invalid workbook.xml".to_string()))?;
    let mut updated = xml.to_string();
    updated.insert_str(pos, &element);
    Ok(updated)
}
fn upsert_attr(xml: &str, start: usize, attr: &str, value: &str) -> Result<String, HandlerError> {
    let end = xml[start..]
        .find('>')
        .map(|offset| start + offset)
        .ok_or_else(|| HandlerError::OperationFailed("unterminated workbook XML".to_string()))?;
    let open = &xml[start..=end];
    let needle = format!(" {attr}=\"");
    let replacement = if let Some(offset) = open.find(&needle) {
        let from = offset + needle.len();
        let to = open[from..]
            .find('"')
            .map(|offset| from + offset)
            .ok_or_else(|| {
                HandlerError::OperationFailed("invalid workbook attribute".to_string())
            })?;
        format!("{}{}{}", &open[..from], value, &open[to..])
    } else {
        let suffix = if open.ends_with("/>") { 2 } else { 1 };
        format!(
            "{} {attr}=\"{value}\"{}",
            &open[..open.len() - suffix],
            &open[open.len() - suffix..]
        )
    };
    let mut updated = xml.to_string();
    updated.replace_range(start..=end, &replacement);
    Ok(updated)
}

fn set_protection_attr(xml: &str, attr: &str, value: &str) -> Result<String, HandlerError> {
    if truthy(value) {
        set_child_attr(xml, "workbookProtection", attr, "1", Some("bookViews"))
    } else {
        remove_protection_attr(xml, attr)
    }
}

fn remove_protection_attr(xml: &str, attr: &str) -> Result<String, HandlerError> {
    let Some(start) = xml.find("<workbookProtection") else {
        return Ok(xml.to_string());
    };
    let end = xml[start..]
        .find('>')
        .map(|offset| start + offset)
        .ok_or_else(|| {
            HandlerError::OperationFailed("unterminated workbookProtection".to_string())
        })?;
    let open = &xml[start..=end];
    let needle = format!(" {attr}=\"");
    let Some(offset) = open.find(&needle) else {
        return Ok(xml.to_string());
    };
    let value_end = open[offset + needle.len()..]
        .find('"')
        .map(|item| offset + needle.len() + item + 1)
        .ok_or_else(|| {
            HandlerError::OperationFailed("invalid workbookProtection attribute".to_string())
        })?;
    let mut updated = xml.to_string();
    updated.replace_range(start + offset..start + value_end, "");
    cleanup_protection(&updated)
}

fn remove_child(xml: &str, tag: &str) -> Result<String, HandlerError> {
    let Some(start) = xml.find(&format!("<{tag}")) else {
        return Ok(xml.to_string());
    };
    let end = xml[start..]
        .find('>')
        .map(|offset| start + offset + 1)
        .ok_or_else(|| HandlerError::OperationFailed(format!("unterminated {tag}")))?;
    let mut updated = xml.to_string();
    updated.replace_range(start..end, "");
    Ok(updated)
}

fn cleanup_protection(xml: &str) -> Result<String, HandlerError> {
    let Some(start) = xml.find("<workbookProtection") else {
        return Ok(xml.to_string());
    };
    let end = xml[start..]
        .find('>')
        .map(|offset| start + offset + 1)
        .ok_or_else(|| {
            HandlerError::OperationFailed("unterminated workbookProtection".to_string())
        })?;
    xml[start..end]
        .contains('=')
        .then(|| xml.to_string())
        .ok_or_else(|| HandlerError::OperationFailed("invalid workbookProtection".to_string()))
        .or_else(|_| remove_child(xml, "workbookProtection"))
}

fn has_protection_lock(xml: &str) -> Result<bool, HandlerError> {
    Ok(roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?
        .descendants()
        .find(|node| node.has_tag_name("workbookProtection"))
        .is_some_and(|node| {
            truthy(node.attribute("lockStructure").unwrap_or("0"))
                || truthy(node.attribute("lockWindows").unwrap_or("0"))
        }))
}

fn legacy_password_hash(value: &str) -> String {
    let mut hash: u16 = 0;
    let units: Vec<u16> = value.encode_utf16().collect();
    for item in units.iter().rev() {
        hash = ((hash >> 14) & 1) | ((hash << 1) & 0x7FFF);
        hash ^= *item;
    }
    hash = ((hash >> 14) & 1) | ((hash << 1) & 0x7FFF);
    hash ^= units.len() as u16;
    hash ^= 0xCE4B;
    format!("{hash:04X}")
}
