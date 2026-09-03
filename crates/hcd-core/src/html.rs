use crate::{hash_bytes, HcdError, ImageGeometry, ImageGeometryUnit};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::BTreeMap;

/// Extracts canonical editable text from one bounded HCD HTML fragment.
///
/// Structural elements may carry `data-hcd-id`; canonical text nodes are the
/// elements that carry both that attribute and `data-hcd-node-hash`.
pub fn extract_html_text_nodes(html: &str) -> Result<BTreeMap<String, String>, HcdError> {
    let mut reader = Reader::from_str(html);
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::with_capacity(16 * 1024);
    let mut current: Option<(String, Vec<u8>, String)> = None;
    let mut nodes = BTreeMap::new();

    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| HcdError::InvalidBundle(format!("HTML XML parse error: {error}")))?;
        match event {
            Event::Start(start) => {
                let (node_id, has_node_hash) = node_attributes(&reader, &start)?;
                if has_node_hash {
                    let node_id = node_id.ok_or_else(|| {
                        HcdError::InvalidBundle(
                            "text node hash is present without data-hcd-id".to_string(),
                        )
                    })?;
                    if current.is_some() {
                        return Err(HcdError::InvalidBundle(
                            "nested canonical text nodes are forbidden".to_string(),
                        ));
                    }
                    current = Some((node_id, start.name().as_ref().to_vec(), String::new()));
                }
            }
            Event::Empty(empty) => {
                let (node_id, has_node_hash) = node_attributes(&reader, &empty)?;
                if has_node_hash {
                    insert_unique(
                        &mut nodes,
                        node_id.ok_or_else(|| {
                            HcdError::InvalidBundle(
                                "text node hash is present without data-hcd-id".to_string(),
                            )
                        })?,
                        String::new(),
                    )?;
                }
            }
            Event::Text(text) => {
                if let Some((_, _, value)) = &mut current {
                    value.push_str(&text.unescape().map_err(|error| {
                        HcdError::InvalidBundle(format!("invalid HTML text: {error}"))
                    })?);
                }
            }
            Event::CData(text) => {
                if let Some((_, _, value)) = &mut current {
                    value.push_str(&reader.decoder().decode(text.as_ref()).map_err(|error| {
                        HcdError::InvalidBundle(format!("invalid HTML CDATA: {error}"))
                    })?);
                }
            }
            Event::End(end) => {
                if current
                    .as_ref()
                    .is_some_and(|(_, name, _)| name.as_slice() == end.name().as_ref())
                {
                    let (node_id, _, text) = current.take().expect("checked current text node");
                    insert_unique(&mut nodes, node_id, text)?;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if let Some((node_id, _, _)) = current {
        return Err(HcdError::InvalidBundle(format!(
            "canonical text node {node_id} was not closed"
        )));
    }
    Ok(nodes)
}

/// Canonical, hashable state for mapped image nodes in one bounded fragment.
#[derive(Debug, Clone, PartialEq)]
pub struct HtmlImageNode {
    pub visual_hash: String,
    pub asset_hash: Option<String>,
    pub geometry: Option<ImageGeometry>,
}

pub fn image_visual_hash(asset_hash: Option<&str>, geometry: Option<&ImageGeometry>) -> String {
    let geometry = geometry.map_or_else(
        || "none".to_string(),
        |value| {
            format!(
                "{},{},{},{},{}",
                canonical_number(value.x),
                canonical_number(value.y),
                canonical_number(value.width),
                canonical_number(value.height),
                match value.unit {
                    ImageGeometryUnit::Emu => "emu",
                    ImageGeometryUnit::Pt => "pt",
                }
            )
        },
    );
    hash_bytes(
        format!(
            "officecli-hcd-image/1\0asset={}\0geometry={geometry}",
            asset_hash.unwrap_or("none")
        )
        .as_bytes(),
    )
}

pub fn extract_html_image_nodes(html: &str) -> Result<BTreeMap<String, HtmlImageNode>, HcdError> {
    let mut reader = Reader::from_str(html);
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::with_capacity(16 * 1024);
    let mut nodes = BTreeMap::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| HcdError::InvalidBundle(format!("HTML XML parse error: {error}")))?;
        match event {
            Event::Start(start) | Event::Empty(start) => {
                let attributes = decoded_attributes(&reader, &start)?;
                let Some(node_id) = attributes.get("data-hcd-id") else {
                    buffer.clear();
                    continue;
                };
                let Some(declared_hash) = attributes.get("data-hcd-visual-hash") else {
                    buffer.clear();
                    continue;
                };
                let asset_hash = attributes.get("data-hcd-asset-hash").cloned();
                let geometry = image_geometry_from_attributes(&attributes)?;
                let actual_hash = image_visual_hash(asset_hash.as_deref(), geometry.as_ref());
                if actual_hash != *declared_hash {
                    return Err(HcdError::InvalidBundle(format!(
                        "image node {node_id} visual hash {declared_hash} does not match {actual_hash}"
                    )));
                }
                if nodes
                    .insert(
                        node_id.clone(),
                        HtmlImageNode {
                            visual_hash: actual_hash,
                            asset_hash,
                            geometry,
                        },
                    )
                    .is_some()
                {
                    return Err(HcdError::InvalidBundle(format!(
                        "duplicate canonical image node {node_id}"
                    )));
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(nodes)
}

fn decoded_attributes<B: std::io::BufRead>(
    reader: &Reader<B>,
    start: &quick_xml::events::BytesStart<'_>,
) -> Result<BTreeMap<String, String>, HcdError> {
    let mut output = BTreeMap::new();
    for attribute in start.attributes().with_checks(true) {
        let attribute = attribute
            .map_err(|error| HcdError::InvalidBundle(format!("invalid HTML attribute: {error}")))?;
        let key = std::str::from_utf8(attribute.key.as_ref())
            .map_err(|error| {
                HcdError::InvalidBundle(format!("invalid HTML attribute name: {error}"))
            })?
            .to_string();
        let value = attribute
            .decode_and_unescape_value(reader.decoder())
            .map_err(|error| HcdError::InvalidBundle(format!("invalid HTML attribute: {error}")))?
            .into_owned();
        output.insert(key, value);
    }
    Ok(output)
}

fn image_geometry_from_attributes(
    attributes: &BTreeMap<String, String>,
) -> Result<Option<ImageGeometry>, HcdError> {
    let values = [
        attributes.get("data-hcd-x"),
        attributes.get("data-hcd-y"),
        attributes.get("data-hcd-width"),
        attributes.get("data-hcd-height"),
    ];
    if values.iter().all(|value| value.is_none()) {
        return Ok(None);
    }
    if values.iter().any(|value| value.is_none()) {
        return Err(HcdError::InvalidBundle(
            "image geometry must contain x, y, width, and height".to_string(),
        ));
    }
    let parse = |name: &str, value: &str| {
        value.parse::<f64>().map_err(|error| {
            HcdError::InvalidBundle(format!("invalid image {name} value {value}: {error}"))
        })
    };
    let unit = match attributes.get("data-hcd-geometry-unit").map(String::as_str) {
        Some("emu") => ImageGeometryUnit::Emu,
        Some("pt") => ImageGeometryUnit::Pt,
        Some(value) => {
            return Err(HcdError::InvalidBundle(format!(
                "unsupported image geometry unit {value}"
            )))
        }
        None => {
            return Err(HcdError::InvalidBundle(
                "image geometry has no data-hcd-geometry-unit".to_string(),
            ))
        }
    };
    let geometry = ImageGeometry {
        x: parse("x", values[0].expect("checked geometry"))?,
        y: parse("y", values[1].expect("checked geometry"))?,
        width: parse("width", values[2].expect("checked geometry"))?,
        height: parse("height", values[3].expect("checked geometry"))?,
        unit,
    };
    validate_image_geometry(&geometry)?;
    Ok(Some(geometry))
}

pub(crate) fn validate_image_geometry(geometry: &ImageGeometry) -> Result<(), HcdError> {
    if !geometry.x.is_finite()
        || !geometry.y.is_finite()
        || !geometry.width.is_finite()
        || !geometry.height.is_finite()
        || geometry.x.abs() > 1_000_000_000_000.0
        || geometry.y.abs() > 1_000_000_000_000.0
        || !(0.0..=1_000_000_000_000.0).contains(&geometry.width)
        || !(0.0..=1_000_000_000_000.0).contains(&geometry.height)
        || geometry.width == 0.0
        || geometry.height == 0.0
    {
        return Err(HcdError::InvalidPatch(
            "image geometry must be finite, bounded, and have positive width/height".to_string(),
        ));
    }
    Ok(())
}

fn canonical_number(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    let formatted = format!("{value:.6}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn node_attributes<B: std::io::BufRead>(
    reader: &Reader<B>,
    start: &quick_xml::events::BytesStart<'_>,
) -> Result<(Option<String>, bool), HcdError> {
    let mut node_id = None;
    let mut has_node_hash = false;
    for attribute in start.attributes().with_checks(true) {
        let attribute = attribute
            .map_err(|error| HcdError::InvalidBundle(format!("invalid HTML attribute: {error}")))?;
        match attribute.key.as_ref() {
            b"data-hcd-id" => {
                node_id = Some(
                    attribute
                        .decode_and_unescape_value(reader.decoder())
                        .map_err(|error| {
                            HcdError::InvalidBundle(format!("invalid data-hcd-id: {error}"))
                        })?
                        .into_owned(),
                );
            }
            b"data-hcd-node-hash" => has_node_hash = true,
            _ => {}
        }
    }
    Ok((node_id, has_node_hash))
}

fn insert_unique(
    nodes: &mut BTreeMap<String, String>,
    node_id: String,
    text: String,
) -> Result<(), HcdError> {
    if nodes.insert(node_id.clone(), text).is_some() {
        return Err(HcdError::InvalidBundle(format!(
            "duplicate canonical text node {node_id}"
        )));
    }
    Ok(())
}

pub fn validate_css_text(css: &str) -> Result<(), HcdError> {
    let compact: String = css
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| !character.is_ascii_whitespace() && !character.is_control())
        .collect();
    for forbidden in [
        "url(",
        "@import",
        "expression(",
        "behavior:",
        "-moz-binding",
    ] {
        if compact.contains(forbidden) {
            return Err(HcdError::InvalidBundle(format!(
                "CSS contains forbidden construct {forbidden}"
            )));
        }
    }
    Ok(())
}

pub(crate) fn validate_inline_style(style: &str) -> Result<(), HcdError> {
    validate_css_text(style)?;
    for declaration in style.split(';') {
        let declaration = declaration.trim();
        if declaration.is_empty() {
            continue;
        }
        let (property, value) = declaration.split_once(':').ok_or_else(|| {
            HcdError::InvalidBundle(format!("invalid inline CSS declaration {declaration}"))
        })?;
        let property = property.trim().to_ascii_lowercase();
        if !matches!(
            property.as_str(),
            "font-weight"
                | "font-style"
                | "font-size"
                | "font-family"
                | "text-decoration"
                | "color"
                | "background-color"
                | "vertical-align"
                | "direction"
                | "unicode-bidi"
                | "display"
                | "text-align"
                | "margin-left"
                | "margin-right"
                | "margin-top"
                | "margin-bottom"
                | "text-indent"
                | "line-height"
                | "letter-spacing"
                | "break-before"
                | "width"
                | "min-width"
                | "height"
                | "min-height"
                | "table-layout"
                | "break-inside"
                | "border-top"
                | "border-right"
                | "border-bottom"
                | "border-left"
                | "padding-top"
                | "padding-right"
                | "padding-bottom"
                | "padding-left"
                | "position"
                | "left"
                | "top"
                | "overflow"
                | "z-index"
                | "transform"
                | "transform-origin"
                | "background-image"
                | "border-radius"
                | "box-shadow"
                | "writing-mode"
                | "text-orientation"
                | "flex-direction"
                | "justify-content"
        ) {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS property {property} is not allowed"
            )));
        }
        let value = value.trim();
        if value.is_empty()
            || value.len() > 256
            || !value.chars().all(|character| {
                character.is_alphanumeric()
                    || character.is_ascii_whitespace()
                    || matches!(
                        character,
                        '#' | '.' | '-' | '_' | ',' | '%' | '\'' | '"' | '(' | ')'
                    )
            })
        {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS value for {property} is unsafe"
            )));
        }
        let normalized_value = value.to_ascii_lowercase();
        if normalized_value.contains("url(") {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS value for {property} is unsafe"
            )));
        }
        if property == "position" && !matches!(normalized_value.as_str(), "relative" | "absolute") {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS position {value} is not allowed"
            )));
        }
        if property == "overflow"
            && !matches!(normalized_value.as_str(), "hidden" | "visible" | "clip")
        {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS overflow {value} is not allowed"
            )));
        }
        if property == "table-layout" && !matches!(normalized_value.as_str(), "fixed" | "auto") {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS table-layout {value} is not allowed"
            )));
        }
        if property == "break-inside" && !matches!(normalized_value.as_str(), "avoid" | "auto") {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS break-inside {value} is not allowed"
            )));
        }
        if property == "writing-mode"
            && !matches!(
                normalized_value.as_str(),
                "horizontal-tb" | "vertical-rl" | "vertical-lr"
            )
        {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS writing-mode {value} is not allowed"
            )));
        }
        if property == "text-orientation"
            && !matches!(normalized_value.as_str(), "mixed" | "upright" | "sideways")
        {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS text-orientation {value} is not allowed"
            )));
        }
        if property == "flex-direction" && !matches!(normalized_value.as_str(), "row" | "column") {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS flex-direction {value} is not allowed"
            )));
        }
        if property == "justify-content"
            && !matches!(
                normalized_value.as_str(),
                "flex-start" | "center" | "flex-end" | "space-between" | "space-around"
            )
        {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS justify-content {value} is not allowed"
            )));
        }
        if property == "transform-origin"
            && !matches!(
                normalized_value.as_str(),
                "center" | "top" | "bottom" | "left" | "right"
            )
        {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS transform-origin {value} is not allowed"
            )));
        }
        if property == "transform"
            && !(normalized_value.starts_with("rotate(")
                && normalized_value.ends_with("deg)")
                && normalized_value[7..normalized_value.len() - 4]
                    .parse::<f64>()
                    .is_ok())
        {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS transform {value} is not allowed"
            )));
        }
        if property == "background-image"
            && !(normalized_value.starts_with("linear-gradient(")
                && normalized_value.ends_with(')')
                && normalized_value.matches('#').count() >= 2)
        {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS background-image {value} is not allowed"
            )));
        }
        if property == "z-index" && value.parse::<i32>().is_err() {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS z-index {value} is not allowed"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_canonical_text_nodes() {
        let html = r#"<section data-hcd-id="block"><span data-hcd-id="n_1" data-hcd-node-hash="h">甲&amp;😀</span><span data-hcd-id="n_2" data-hcd-node-hash="e"></span></section>"#;
        let nodes = extract_html_text_nodes(html).unwrap();
        assert_eq!(nodes.get("n_1").unwrap(), "甲&😀");
        assert_eq!(nodes.get("n_2").unwrap(), "");
        assert!(!nodes.contains_key("block"));
    }

    #[test]
    fn dangerous_css_is_rejected_case_and_space_insensitively() {
        assert!(validate_css_text("a { background: U R L (javascript:x) }").is_err());
        assert!(validate_css_text("@IMPORT 'https://example.test/x.css'").is_err());
        assert!(validate_css_text("a { color: #123; }").is_ok());
    }

    #[test]
    fn inline_css_uses_a_small_property_and_value_allowlist() {
        assert!(validate_inline_style("font-weight:700;color:#ff0000;margin-left:36.00pt").is_ok());
        assert!(validate_inline_style(
            "position:absolute;left:10.50px;top:0;width:100px;height:50px;overflow:hidden"
        )
        .is_ok());
        assert!(validate_inline_style(
            "table-layout:fixed;min-width:18pt;min-height:18pt;break-inside:avoid;border-top:1pt solid #123456;padding-left:5pt"
        )
        .is_ok());
        assert!(validate_inline_style(
            "z-index:7;transform:rotate(45deg);transform-origin:center;background-image:linear-gradient(90deg,#FF0000,#FFFF00);border-radius:12px;writing-mode:vertical-rl;text-orientation:mixed;flex-direction:column;justify-content:center"
        )
        .is_ok());
        assert!(validate_inline_style("position:fixed").is_err());
        assert!(validate_inline_style("overflow:scroll").is_err());
        assert!(validate_inline_style("table-layout:inherit").is_err());
        assert!(validate_inline_style("break-inside:page").is_err());
        assert!(validate_inline_style("color:expression(alert(1))").is_err());
        assert!(validate_inline_style("background-color:url(javascript:x)").is_err());
        assert!(validate_inline_style("background-image:url(relative.png)").is_err());
        assert!(validate_inline_style("transform:translate(10px)").is_err());
        assert!(validate_inline_style("writing-mode:sideways-lr").is_err());
    }
}
