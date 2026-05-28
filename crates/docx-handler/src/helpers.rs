use crate::dom_types::{WordNode, WordElementType};
use handler_common::HandlerError;

/// Split a run at a text offset.
/// Given a run node and a byte offset within its text content, split it into two runs:
/// - Left run: text from 0..offset (preserving original rPr)
/// - Right run: text from offset..end (preserving original rPr)
///
/// Returns (left_run, right_run). If offset is 0, left is None.
/// If offset equals text length, right is None.
pub fn split_run_at_offset(run: &WordNode, offset: usize) -> (Option<WordNode>, Option<WordNode>) {
    // Find the w:t element and its text
    let text_node = run.children.iter().find(|c| c.element_type == WordElementType::Text);
    if text_node.is_none() {
        // No text in this run; can't split
        return (Some(run.clone()), None);
    }
    let text_node = text_node.unwrap();
    let text = text_node.text_content.as_deref().unwrap_or("");

    if offset == 0 {
        return (None, Some(run.clone()));
    }
    if offset >= text.len() {
        return (Some(run.clone()), None);
    }

    // Get run properties to clone into both halves
    let rpr = run.run_properties().cloned();

    // Left run: text[0..offset]
    let left_text = text[..offset].to_string();
    let left_preserve = if text_node.has_preserve_space() || left_text.ends_with(' ') || left_text.starts_with(' ') {
        true
    } else {
        false
    };
    let left_t = WordNode::new(WordElementType::Text)
        .with_text(&left_text);
    let mut left_t = left_t;
    if left_preserve {
        left_t.attributes.insert("xml:space".to_string(), "preserve".to_string());
        left_t.preserve_space = true;
    }

    let mut left_children = Vec::new();
    if let Some(rpr) = &rpr {
        left_children.push(rpr.clone());
    }
    left_children.push(left_t);

    let left_run = WordNode::new(WordElementType::Run).with_children(left_children);

    // Right run: text[offset..end]
    let right_text = text[offset..].to_string();
    let _right_preserve = true; // Always preserve space on the right part after split
    let mut right_t = WordNode::new(WordElementType::Text)
        .with_text(&right_text);
    right_t.attributes.insert("xml:space".to_string(), "preserve".to_string());
    right_t.preserve_space = true;

    let mut right_children = Vec::new();
    if let Some(rpr) = &rpr {
        right_children.push(rpr.clone());
    }
    right_children.push(right_t);

    let right_run = WordNode::new(WordElementType::Run).with_children(right_children);

    (Some(left_run), Some(right_run))
}

/// Find the run and offset within a paragraph that corresponds to a global text offset.
/// Returns (run_index_0based, offset_within_run_text).
pub fn find_run_at_offset(para: &WordNode, para_text_offset: usize) -> Result<(usize, usize), HandlerError> {
    let mut current_offset = 0;
    let runs = para.runs();

    for (i, run) in runs.iter().enumerate() {
        let run_text = run.paragraph_text();
        let run_len = run_text.len();

        if current_offset + run_len > para_text_offset {
            // The target offset falls within this run
            let offset_in_run = para_text_offset - current_offset;
            return Ok((i, offset_in_run));
        }
        current_offset += run_len;
    }

    Err(HandlerError::PathNotFound(
        format!("offset {} beyond paragraph text length {}", para_text_offset, current_offset),
    ))
}

/// Count body-level content elements (paragraphs + tables) for path indexing.
pub fn count_body_content_elements(body: &WordNode) -> usize {
    body.children.iter()
        .filter(|c| c.element_type.is_body_child())
        .count()
}

/// Find the 1-based index of a body child element among its siblings of the same type.
pub fn find_sibling_index(parent: &WordNode, target: &WordNode) -> usize {
    let mut idx = 0;
    for child in &parent.children {
        if child.element_type == target.element_type {
            idx += 1;
            if std::ptr::eq(child, target) {
                return idx;
            }
        }
    }
    idx
}

/// Find a paragraph by its paraId attribute.
pub fn find_paragraph_by_para_id(dom: &crate::dom_types::WordDom, para_id: &str) -> Option<usize> {
    let body = dom.body()?;
    let mut para_idx = 0;
    for child in &body.children {
        if child.element_type == WordElementType::Paragraph {
            para_idx += 1;
            if child.attributes.get("paraId").map(|s| s.as_str()) == Some(para_id) {
                return Some(para_idx);
            }
        }
    }
    None
}

/// Generate a unique paragraph ID (hex string, 8 chars).
pub fn generate_para_id() -> String {
    use uuid::Uuid;
    let uuid = Uuid::new_v4();
    // Take first 8 hex chars for a short paraId
    uuid.to_string().replace('-', "")[..8].to_string()
}

/// Build a minimal w:rPr (run properties) XML element from format properties.
pub fn build_run_properties(props: &std::collections::HashMap<String, String>) -> Option<WordNode> {
    if props.is_empty() {
        return None;
    }

    let mut rpr = WordNode::new(WordElementType::RunProperties);
    let mut children = Vec::new();

    for (key, value) in props {
        match key.as_str() {
            "bold" | "b" => {
                if value == "true" || value == "1" {
                    let b_node = WordNode::new(WordElementType::Unknown("b".to_string()));
                    children.push(b_node);
                }
            }
            "italic" | "i" => {
                if value == "true" || value == "1" {
                    let i_node = WordNode::new(WordElementType::Unknown("i".to_string()));
                    children.push(i_node);
                }
            }
            "underline" | "u" => {
                let val = if value.is_empty() { "single" } else { value.as_str() };
                let u_node = WordNode::new(WordElementType::Unknown("u".to_string()))
                    .with_attribute("val", val);
                children.push(u_node);
            }
            "strike" | "strikeout" => {
                if value == "true" || value == "1" {
                    let strike_node = WordNode::new(WordElementType::Unknown("strike".to_string()));
                    children.push(strike_node);
                }
            }
            "font" | "fontFamily" => {
                let rfonts = WordNode::new(WordElementType::Unknown("rFonts".to_string()))
                    .with_attribute("ascii", value.as_str())
                    .with_attribute("hAnsi", value.as_str());
                children.push(rfonts);
            }
            "size" | "fontSize" => {
                // OOXML font size is in half-points (24 = 12pt)
                let half_points = if let Ok(pt) = value.parse::<f32>() {
                    (pt * 2.0) as usize
                } else {
                    24 // default 12pt
                };
                let sz_node = WordNode::new(WordElementType::Unknown("sz".to_string()))
                    .with_attribute("val", half_points.to_string().as_str());
                children.push(sz_node);
                let sz_cs = WordNode::new(WordElementType::Unknown("szCs".to_string()))
                    .with_attribute("val", half_points.to_string().as_str());
                children.push(sz_cs);
            }
            "color" | "fontColor" => {
                let color_val = if value.starts_with('#') {
                    // Convert #RRGGBB to RRGGBB
                    &value[1..]
                } else {
                    value.as_str()
                };
                let color_node = WordNode::new(WordElementType::Unknown("color".to_string()))
                    .with_attribute("val", color_val);
                children.push(color_node);
            }
            "bgColor" | "highlight" | "bg" => {
                let color_val = if value.starts_with('#') {
                    &value[1..]
                } else {
                    value.as_str()
                };
                if matches!(color_val.to_lowercase().as_str(), "yellow" | "green" | "cyan" | "magenta" | "blue" | "red" | "darkblue" | "darkcyan" | "darkgreen" | "darkmagenta" | "darkred" | "darkyellow" | "white" | "lightgray" | "darkgray" | "black" | "none") {
                    let hl_node = WordNode::new(WordElementType::Unknown("highlight".to_string()))
                        .with_attribute("val", &color_val.to_lowercase());
                    children.push(hl_node);
                } else {
                    let shd_node = WordNode::new(WordElementType::Unknown("shd".to_string()))
                        .with_attribute("val", "clear")
                        .with_attribute("color", "auto")
                        .with_attribute("fill", color_val);
                    children.push(shd_node);
                }
            }
            _ => {} // Ignore unknown properties
        }
    }

    if children.is_empty() {
        return None;
    }

    rpr.children = children;
    Some(rpr)
}

/// Build paragraph properties from format properties.
pub fn build_paragraph_properties(props: &std::collections::HashMap<String, String>) -> Option<WordNode> {
    if props.is_empty() {
        return None;
    }

    let mut ppr = WordNode::new(WordElementType::ParagraphProperties);
    let mut children = Vec::new();

    for (key, value) in props {
        match key.as_str() {
            "style" | "pStyle" => {
                let pstyle = WordNode::new(WordElementType::Unknown("pStyle".to_string()))
                    .with_attribute("val", value.as_str());
                children.push(pstyle);
            }
            "alignment" | "jc" => {
                let jc = WordNode::new(WordElementType::Unknown("jc".to_string()))
                    .with_attribute("val", value.as_str());
                children.push(jc);
            }
            "indentLeft" => {
                let ind = WordNode::new(WordElementType::Unknown("ind".to_string()))
                    .with_attribute("left", value.as_str());
                children.push(ind);
            }
            "indentRight" => {
                // We might need to combine with existing ind node
                // For simplicity, create separate ind nodes per property
                // (OOXML has a single ind element with multiple attrs, but this is simpler)
                let ind = WordNode::new(WordElementType::Unknown("ind".to_string()))
                    .with_attribute("right", value.as_str());
                children.push(ind);
            }
            "spacingBefore" => {
                let spacing = WordNode::new(WordElementType::Unknown("spacing".to_string()))
                    .with_attribute("before", value.as_str());
                children.push(spacing);
            }
            "spacingAfter" => {
                let spacing = WordNode::new(WordElementType::Unknown("spacing".to_string()))
                    .with_attribute("after", value.as_str());
                children.push(spacing);
            }
            _ => {} // Ignore unknown properties
        }
    }

    if children.is_empty() {
        return None;
    }

    ppr.children = children;
    Some(ppr)
}