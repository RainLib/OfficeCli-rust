use crate::HcdError;
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
                | "break-before"
                | "width"
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
                    || matches!(character, '#' | '.' | '-' | '_' | ',' | '%' | '\'' | '"')
            })
        {
            return Err(HcdError::InvalidBundle(format!(
                "inline CSS value for {property} is unsafe"
            )));
        }
        let normalized_value = value.to_ascii_lowercase();
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
            "table-layout:fixed;min-height:18pt;break-inside:avoid;border-top:1pt solid #123456;padding-left:5pt"
        )
        .is_ok());
        assert!(validate_inline_style("position:fixed").is_err());
        assert!(validate_inline_style("overflow:scroll").is_err());
        assert!(validate_inline_style("table-layout:inherit").is_err());
        assert!(validate_inline_style("break-inside:page").is_err());
        assert!(validate_inline_style("color:expression(alert(1))").is_err());
        assert!(validate_inline_style("background-color:url(javascript:x)").is_err());
    }
}
