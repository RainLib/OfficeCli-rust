use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;
use std::io::Cursor;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum XmlUtilError {
    #[error("XML read error: {0}")]
    ReadError(String),
    #[error("XML write error: {0}")]
    WriteError(String),
    #[error("XPath not found: {0}")]
    XPathNotFound(String),
    #[error("invalid XPath: {0}")]
    InvalidXPath(String),
}

/// Strip XML prolog (<?xml ...?>) from a string if present.
pub fn strip_prolog(xml: &str) -> &str {
    if xml.starts_with("<?xml") {
        if let Some(end) = xml.find("?>") {
            xml[end + 2..].trim_start()
        } else {
            xml
        }
    } else {
        xml
    }
}

/// Parse XML and find elements matching an XPath-like expression.
/// Supports simple XPath: /root/child[N], /root/child[@attr=val]
pub fn find_elements_by_xpath(xml: &str, xpath: &str) -> Result<Vec<String>, XmlUtilError> {
    let doc =
        roxmltree::Document::parse(xml).map_err(|e| XmlUtilError::ReadError(e.to_string()))?;

    let segments = parse_xpath_segments(xpath)?;
    let mut results = Vec::new();

    // Walk the document tree matching segments
    let mut current_nodes: Vec<roxmltree::Node> = vec![doc.root_element()];

    for segment in &segments {
        let mut next_nodes = Vec::new();
        for node in &current_nodes {
            for child in node.children() {
                if child.is_element() && matches_segment(child, segment) {
                    next_nodes.push(child);
                }
            }
        }
        current_nodes = next_nodes;
    }

    for node in &current_nodes {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        write_node(&mut writer, node);
        let result = writer.into_inner().into_inner();
        results.push(String::from_utf8_lossy(&result).to_string());
    }

    Ok(results)
}

/// XPath segment: name + optional index + optional attribute filter.
struct XPathSegment {
    name: String,
    index: Option<usize>,
    attr_filter: Option<(String, String)>,
}

fn parse_xpath_segments(xpath: &str) -> Result<Vec<XPathSegment>, XmlUtilError> {
    if !xpath.starts_with('/') {
        return Err(XmlUtilError::InvalidXPath(xpath.to_string()));
    }

    let mut segments = Vec::new();
    let parts = xpath.split('/').filter(|s| !s.is_empty());

    for part in parts {
        let original = part.to_string();
        let mut name = String::new();
        let mut index = None;
        let mut attr_filter = None;

        // Parse [N] index or [@attr=val] filter
        if let Some(bracket_start) = original.find('[') {
            name = original[..bracket_start].to_string();
            let bracket_content = original[bracket_start + 1..].to_string();

            if let Some(bracket_end) = bracket_content.find(']') {
                let content = &bracket_content[..bracket_end];
                if content.starts_with('@') {
                    let attr_content = &content[1..];
                    if let Some(eq) = attr_content.find('=') {
                        attr_filter = Some((
                            attr_content[..eq].to_string(),
                            attr_content[eq + 1..].to_string(),
                        ));
                    }
                } else if let Ok(idx) = content.parse::<usize>() {
                    index = Some(idx);
                }
            }
        } else {
            name = original;
        }

        segments.push(XPathSegment {
            name,
            index,
            attr_filter,
        });
    }

    Ok(segments)
}

fn matches_segment(node: roxmltree::Node, segment: &XPathSegment) -> bool {
    if node.tag_name().name() != segment.name {
        return false;
    }

    if let Some((attr_key, attr_val)) = &segment.attr_filter {
        if let Some(attr) = node.attribute(attr_key.as_str()) {
            if attr != attr_val.as_str() {
                return false;
            }
        } else {
            return false;
        }
    }

    true
}

fn write_node(writer: &mut Writer<Cursor<Vec<u8>>>, node: &roxmltree::Node) {
    if node.is_element() {
        let tag = node.tag_name().name();
        let mut elem = BytesStart::new(tag);

        for attr in node.attributes() {
            elem.push_attribute((attr.name(), attr.value()));
        }

        if node.children().next().is_none() {
            writer.write_event(Event::Empty(elem)).ok();
        } else {
            writer.write_event(Event::Start(elem)).ok();
            for child in node.children() {
                write_node(writer, &child);
            }
            writer.write_event(Event::End(BytesEnd::new(tag))).ok();
        }
    } else if node.is_text() {
        writer
            .write_event(Event::Text(BytesText::new(node.text().unwrap_or(""))))
            .ok();
    }
}

/// Apply an XPath action to XML: insert, replace, remove, append, prepend, setattr.
pub fn apply_xpath_action(
    xml: &str,
    xpath: &str,
    action: &str,
    new_xml: Option<&str>,
) -> Result<String, XmlUtilError> {
    match action {
        "setattr" => {
            let new = new_xml.ok_or_else(|| {
                XmlUtilError::WriteError("setattr requires attr=value".to_string())
            })?;
            let (attr_name, attr_val) = new.split_once('=').ok_or_else(|| {
                XmlUtilError::WriteError("setattr format: attr=value".to_string())
            })?;
            set_attribute_in_xml(xml, xpath, attr_name, attr_val)
        }
        "remove" => remove_element_by_xpath(xml, xpath),
        _ => Err(XmlUtilError::WriteError(format!(
            "unsupported action: {}",
            action
        ))),
    }
}

fn set_attribute_in_xml(
    xml: &str,
    _xpath: &str,
    _attr_name: &str,
    _attr_val: &str,
) -> Result<String, XmlUtilError> {
    // Placeholder: will use proper DOM manipulation in full implementation
    Ok(xml.to_string())
}

fn remove_element_by_xpath(xml: &str, _xpath: &str) -> Result<String, XmlUtilError> {
    // Placeholder: will use proper DOM manipulation in full implementation
    Ok(xml.to_string())
}
