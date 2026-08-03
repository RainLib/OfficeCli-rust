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
const THEME_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme";
const SLIDE_MASTER_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster";

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
    let mut theme_xml: Option<(String, String)> = None;
    let mut unsupported = Vec::new();
    for (key, value) in properties {
        match key.to_ascii_lowercase().as_str() {
            "defaultfont" | "font" => {
                if !set_theme_font(package, &mut theme_xml, "majorFont", "latin", value, false)? {
                    unsupported.push(key.clone());
                } else {
                    set_theme_font(package, &mut theme_xml, "minorFont", "latin", value, false)?;
                }
            }
            key if key.starts_with("theme.color.") => {
                if !set_theme_color(package, &mut theme_xml, &key[12..], value)? {
                    unsupported.push(key.to_string());
                }
            }
            key if key.starts_with("theme.font.") => {
                if !set_dotted_theme_font(package, &mut theme_xml, key, value)? {
                    unsupported.push(key.to_string());
                }
            }
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
    if let Some((path, xml)) = theme_xml {
        write(package, &path, &xml)?;
    }
    Ok(unsupported)
}

/// Read the C#-compatible singleton `/theme` node.
pub fn get_theme(package: &OxmlPackage) -> Result<DocumentNode, HandlerError> {
    let mut node = DocumentNode::new("/theme", "theme");
    let Some(path) = theme_path(package)? else {
        return Ok(node);
    };
    let xml = read(package, &path)?;
    let document = roxmltree::Document::parse(&xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid theme XML: {error}")))?;
    let Some(elements) = document
        .descendants()
        .find(|item| item.has_tag_name("themeElements"))
    else {
        return Ok(node);
    };
    if let Some(scheme) = elements
        .children()
        .find(|item| item.has_tag_name("clrScheme"))
    {
        if let Some(name) = scheme.attribute("name") {
            node = node.with_format("name", Value::String(name.to_string()));
        }
        for (tag, key) in theme_color_slots() {
            if let Some(slot) = scheme.children().find(|item| item.has_tag_name(tag)) {
                if let Some(value) = slot
                    .descendants()
                    .find(|item| item.has_tag_name("srgbClr"))
                    .and_then(|item| item.attribute("val"))
                    .or_else(|| {
                        slot.descendants()
                            .find(|item| item.has_tag_name("sysClr"))
                            .and_then(|item| item.attribute("lastClr").or(item.attribute("val")))
                    })
                {
                    node = node.with_format(key, Value::String(value.to_ascii_uppercase()));
                }
            }
        }
    }
    if let Some(font_scheme) = elements
        .children()
        .find(|item| item.has_tag_name("fontScheme"))
    {
        for (font_tag, short) in [("majorFont", "headingFont"), ("minorFont", "bodyFont")] {
            if let Some(font) = font_scheme
                .children()
                .find(|item| item.has_tag_name(font_tag))
            {
                for (script, suffix) in [("latin", ""), ("ea", ".ea"), ("cs", ".cs")] {
                    if let Some(typeface) = font
                        .children()
                        .find(|item| item.has_tag_name(script))
                        .and_then(|item| item.attribute("typeface"))
                        .filter(|value| !value.is_empty())
                    {
                        let key = format!("{short}{suffix}");
                        node = node.with_format(&key, Value::String(typeface.to_string()));
                    }
                }
            }
        }
    }
    Ok(node)
}

/// Merge the dotted `theme.*` readback C# exposes at the presentation root.
pub fn populate_root_theme(
    package: &OxmlPackage,
    node: &mut DocumentNode,
) -> Result<(), HandlerError> {
    let Some(path) = theme_path(package)? else {
        return Ok(());
    };
    let xml = read(package, &path)?;
    let document = roxmltree::Document::parse(&xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid theme XML: {error}")))?;
    if let Some(name) = document.root_element().attribute("name") {
        node.format.insert(
            "theme.name".to_string(),
            Some(Value::String(name.to_string())),
        );
    }
    let theme = get_theme(package)?;
    for (tag, _) in theme_color_slots() {
        let source = match tag {
            "hlink" => "hyperlink",
            "folHlink" => "followedhyperlink",
            _ => tag,
        };
        if let Some(Some(value)) = theme.format.get(source) {
            node.format
                .insert(format!("theme.color.{tag}"), Some(value.clone()));
        }
    }
    for (short, major_minor) in [("headingFont", "major"), ("bodyFont", "minor")] {
        if let Some(Some(value)) = theme.format.get(short) {
            node.format.insert(
                format!("theme.font.{major_minor}.latin"),
                Some(value.clone()),
            );
        }
        if let Some(Some(value)) = theme.format.get(&format!("{short}.ea")) {
            node.format.insert(
                format!("theme.font.{major_minor}.eastAsia"),
                Some(value.clone()),
            );
        }
    }
    Ok(())
}

/// Set properties using C#'s `/theme` surface (short color/font names and aliases).
pub fn set_theme(
    package: &mut OxmlPackage,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let mut theme_xml = None;
    let mut unsupported = Vec::new();
    for (key, value) in properties {
        let key = key.to_ascii_lowercase();
        let handled = match key.as_str() {
            "accent1" | "accent2" | "accent3" | "accent4" | "accent5" | "accent6" | "dk1"
            | "dk2" | "lt1" | "lt2" | "hyperlink" | "hlink" | "followedhyperlink" | "folhlink" => {
                set_theme_color(package, &mut theme_xml, &key, value)?
            }
            "dark1" => set_theme_color(package, &mut theme_xml, "dk1", value)?,
            "dark2" => set_theme_color(package, &mut theme_xml, "dk2", value)?,
            "light1" => set_theme_color(package, &mut theme_xml, "lt1", value)?,
            "light2" => set_theme_color(package, &mut theme_xml, "lt2", value)?,
            "headingfont" | "majorfont" => {
                set_theme_font(package, &mut theme_xml, "majorFont", "latin", value, true)?
            }
            "bodyfont" | "minorfont" => {
                set_theme_font(package, &mut theme_xml, "minorFont", "latin", value, true)?
            }
            "headingfont.ea" | "majorfont.ea" => {
                set_theme_font(package, &mut theme_xml, "majorFont", "ea", value, true)?
            }
            "bodyfont.ea" | "minorfont.ea" => {
                set_theme_font(package, &mut theme_xml, "minorFont", "ea", value, true)?
            }
            "headingfont.cs" | "majorfont.cs" => {
                set_theme_font(package, &mut theme_xml, "majorFont", "cs", value, true)?
            }
            "bodyfont.cs" | "minorfont.cs" => {
                set_theme_font(package, &mut theme_xml, "minorFont", "cs", value, true)?
            }
            "name" => set_theme_name(package, &mut theme_xml, value)?,
            _ => false,
        };
        if !handled {
            unsupported.push(key);
        }
    }
    if let Some((path, xml)) = theme_xml {
        write(package, &path, &xml)?;
    }
    Ok(unsupported)
}

fn theme_color_slots() -> [(&'static str, &'static str); 12] {
    [
        ("dk1", "dk1"),
        ("lt1", "lt1"),
        ("dk2", "dk2"),
        ("lt2", "lt2"),
        ("accent1", "accent1"),
        ("accent2", "accent2"),
        ("accent3", "accent3"),
        ("accent4", "accent4"),
        ("accent5", "accent5"),
        ("accent6", "accent6"),
        ("hlink", "hyperlink"),
        ("folHlink", "followedhyperlink"),
    ]
}

pub(crate) fn theme_path(package: &OxmlPackage) -> Result<Option<String>, HandlerError> {
    let rels = package
        .part_rels(PRESENTATION)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    if let Some(theme) = rels.all().values().find(|rel| rel.type_uri == THEME_REL) {
        return Ok(Some(
            package.resolve_rel_target(PRESENTATION, &theme.target),
        ));
    }
    for master in rels
        .all()
        .values()
        .filter(|rel| rel.type_uri == SLIDE_MASTER_REL)
    {
        let master_path = package.resolve_rel_target(PRESENTATION, &master.target);
        let master_rels = package
            .part_rels(&master_path)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        if let Some(theme) = master_rels
            .all()
            .values()
            .find(|rel| rel.type_uri == THEME_REL)
        {
            return Ok(Some(
                package.resolve_rel_target(&master_path, &theme.target),
            ));
        }
    }
    Ok(None)
}

fn load_theme<'a>(
    package: &OxmlPackage,
    state: &'a mut Option<(String, String)>,
) -> Result<Option<&'a mut (String, String)>, HandlerError> {
    if state.is_none() {
        let Some(path) = theme_path(package)? else {
            return Ok(None);
        };
        *state = Some((path.clone(), read(package, &path)?));
    }
    Ok(state.as_mut())
}

fn set_theme_color(
    package: &OxmlPackage,
    state: &mut Option<(String, String)>,
    key: &str,
    value: &str,
) -> Result<bool, HandlerError> {
    let lowercase_key = key.to_ascii_lowercase();
    let key = match lowercase_key.as_str() {
        "hlink" | "hyperlink" => "hlink",
        "folhlink" | "followedhyperlink" => "folHlink",
        key => key,
    };
    if !theme_color_slots().iter().any(|(tag, _)| *tag == key) {
        return Ok(false);
    }
    let color = normalize_theme_color(value)?;
    let Some((_, xml)) = load_theme(package, state)? else {
        return Ok(false);
    };
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid theme XML: {error}")))?;
    let Some(slot) = document
        .descendants()
        .find(|item| item.has_tag_name("clrScheme"))
        .and_then(|scheme| scheme.children().find(|item| item.has_tag_name(key)))
    else {
        return Ok(false);
    };
    let replacement = format!("<a:{key}><a:srgbClr val=\"{color}\"/></a:{key}>");
    xml.replace_range(slot.range(), &replacement);
    Ok(true)
}

fn set_theme_name(
    package: &OxmlPackage,
    state: &mut Option<(String, String)>,
    value: &str,
) -> Result<bool, HandlerError> {
    let Some((_, xml)) = load_theme(package, state)? else {
        return Ok(false);
    };
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid theme XML: {error}")))?;
    let Some(scheme) = document
        .descendants()
        .find(|item| item.has_tag_name("clrScheme"))
    else {
        return Ok(false);
    };
    *xml = upsert_attribute_at(xml, scheme.range().start, "name", &escape_xml_attr(value))?;
    Ok(true)
}

fn set_dotted_theme_font(
    package: &OxmlPackage,
    state: &mut Option<(String, String)>,
    key: &str,
    value: &str,
) -> Result<bool, HandlerError> {
    let (font, script) = match key {
        "theme.font.major.latin" => ("majorFont", "latin"),
        "theme.font.major.eastasia" => ("majorFont", "ea"),
        "theme.font.minor.latin" => ("minorFont", "latin"),
        "theme.font.minor.eastasia" => ("minorFont", "ea"),
        _ => return Ok(false),
    };
    set_theme_font(package, state, font, script, value, false)
}

fn set_theme_font(
    package: &OxmlPackage,
    state: &mut Option<(String, String)>,
    font_tag: &str,
    script_tag: &str,
    value: &str,
    normalize_clear: bool,
) -> Result<bool, HandlerError> {
    let value = if normalize_clear
        && (value.is_empty()
            || value.eq_ignore_ascii_case("none")
            || value.eq_ignore_ascii_case("default"))
    {
        ""
    } else {
        value
    };
    let Some((_, xml)) = load_theme(package, state)? else {
        return Ok(false);
    };
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid theme XML: {error}")))?;
    let Some(font) = document
        .descendants()
        .find(|item| item.has_tag_name("fontScheme"))
        .and_then(|scheme| scheme.children().find(|item| item.has_tag_name(font_tag)))
    else {
        return Ok(false);
    };
    let escaped = escape_xml_attr(value);
    if let Some(script) = font.children().find(|item| item.has_tag_name(script_tag)) {
        *xml = upsert_attribute_at(xml, script.range().start, "typeface", &escaped)?;
    } else {
        let close = format!("</a:{font_tag}>");
        let offset = xml[font.range()]
            .rfind(&close)
            .ok_or_else(|| HandlerError::OperationFailed(format!("invalid theme {font_tag}")))?
            + font.range().start;
        xml.insert_str(offset, &format!("<a:{script_tag} typeface=\"{escaped}\"/>"));
    }
    Ok(true)
}

fn normalize_theme_color(value: &str) -> Result<String, HandlerError> {
    let value = value.strip_prefix('#').unwrap_or(value);
    let normalized = match value.len() {
        3 if value.chars().all(|char| char.is_ascii_hexdigit()) => value
            .chars()
            .flat_map(|char| [char, char])
            .collect::<String>(),
        6 if value.chars().all(|char| char.is_ascii_hexdigit()) => value.to_string(),
        _ => {
            return Err(HandlerError::InvalidArgument(format!(
                "theme color must be a 3- or 6-digit hex value (got '{value}')"
            )))
        }
    };
    Ok(normalized.to_ascii_uppercase())
}

fn escape_xml_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_path_uses_the_related_part_not_a_fixed_theme1_name() {
        let mut package = OxmlPackage::create("custom-theme.pptx");
        package.add_part(
            PRESENTATION,
            br#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"/>"#,
        );
        package.add_part(
            PRESENTATION_RELS,
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId7" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="themes/custom.xml"/></Relationships>"#,
        );
        package.add_part("ppt/themes/custom.xml", b"<a:theme/>");

        assert_eq!(
            theme_path(&package).unwrap().as_deref(),
            Some("ppt/themes/custom.xml")
        );
    }
}
