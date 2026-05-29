use crate::dom_types::{WordDom, WordElementType};
use handler_common::{DocumentNode, HandlerError, Selector};

/// Query the document DOM using a CSS-like selector.
/// Supports:
/// - Element type: "p" finds all paragraphs
/// - Attribute filter: "p[@style=Normal]" finds paragraphs with Normal style
/// - Style shorthand: "r[bold]" finds bold runs
pub fn query_elements(
    dom: &WordDom,
    selector_str: &str,
) -> Result<Vec<DocumentNode>, HandlerError> {
    let selector = Selector::parse(selector_str)
        .map_err(|e| HandlerError::InvalidArgument(format!("invalid selector: {}", e)))?;

    let body = dom
        .body()
        .ok_or_else(|| HandlerError::OperationFailed("body element not found".to_string()))?;

    let mut results = Vec::new();

    // Search through the entire body tree
    walk_and_match(body, &selector, "/body", &mut results, 0);

    Ok(results)
}

/// Walk the DOM tree and collect matching elements.
fn walk_and_match(
    node: &crate::dom_types::WordNode,
    selector: &Selector,
    current_path: &str,
    results: &mut Vec<DocumentNode>,
    depth: usize,
) {
    // Check if this node matches the selector
    if matches_selector(node, selector) {
        let doc_node = build_document_node(node, current_path);
        results.push(doc_node);
    }

    // Recurse into children (with path indexing)
    let mut type_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for child in &node.children {
        let name = child.element_type.to_path_name();
        let idx = type_counts.entry(name.to_string()).or_insert(0);
        *idx += 1;

        let child_path = format!("{}/{}[{}]", current_path, name, *idx);

        // Only recurse up to reasonable depth to avoid runaway recursion
        if depth < 20 {
            walk_and_match(child, selector, &child_path, results, depth + 1);
        }
    }
}

/// Check if a WordNode matches the selector criteria.
fn matches_selector(node: &crate::dom_types::WordNode, selector: &Selector) -> bool {
    // Check element type
    if let Some(ref et) = selector.element_type {
        let local_name = node.element_type.to_local_name();
        let path_name = node.element_type.to_path_name();
        if et != local_name && et != path_name {
            // Also check aliases (paragraph -> p)
            if et != "paragraph" || node.element_type != WordElementType::Paragraph {
                if et != "run" || node.element_type != WordElementType::Run {
                    if et != "table" || node.element_type != WordElementType::Table {
                        if et != "row" || node.element_type != WordElementType::TableRow {
                            if et != "cell" || node.element_type != WordElementType::TableCell {
                                return false;
                            }
                        }
                    }
                }
            }
        }
    }

    // Check attribute filters
    for (attr_key, attr_val) in &selector.attributes {
        match attr_key.as_str() {
            "style" | "pStyle" => {
                // Check paragraph style
                let ppr = node.paragraph_properties();
                if let Some(ppr) = ppr {
                    let mut found = false;
                    for child in &ppr.children {
                        if child.element_type == WordElementType::Unknown("pStyle".into()) {
                            if let Some(val) = child.attributes.get("val") {
                                if val == attr_val {
                                    found = true;
                                }
                            }
                        }
                    }
                    if !found {
                        return false;
                    }
                } else {
                    return false;
                }
            }
            "paraId" => {
                if node.attributes.get("paraId").map(|s| s.as_str()) != Some(attr_val.as_str()) {
                    return false;
                }
            }
            _ => {
                // Generic attribute check
                if node.attributes.get(attr_key).map(|s| s.as_str()) != Some(attr_val.as_str()) {
                    return false;
                }
            }
        }
    }

    // Check style shorthand filters
    for shorthand in &selector.style_shorthands {
        match shorthand.as_str() {
            "bold" | "b" => {
                if !node.is_bold() {
                    // Check if any run in this paragraph is bold
                    if node.element_type == WordElementType::Paragraph {
                        let has_bold_run = node.runs().iter().any(|r| r.is_bold());
                        if !has_bold_run {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
            }
            "italic" | "i" => {
                if !node.is_italic() {
                    if node.element_type == WordElementType::Paragraph {
                        let has_italic_run = node.runs().iter().any(|r| r.is_italic());
                        if !has_italic_run {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
            }
            "heading" => {
                if node.heading_level().is_none() {
                    return false;
                }
            }
            _ => {} // Ignore unknown shorthands
        }
    }

    true
}

/// Build a DocumentNode from a WordNode at the given path.
fn build_document_node(node: &crate::dom_types::WordNode, path: &str) -> DocumentNode {
    let element_type = node.element_type.to_path_name();
    let text = node.paragraph_text();
    let preview = if text.chars().count() > 80 {
        Some(format!("{}...", text.chars().take(80).collect::<String>()))
    } else if !text.is_empty() {
        Some(text.clone())
    } else {
        None
    };

    let style = node.heading_level().map(|l| {
        if l == 0 {
            "Title".to_string()
        } else {
            format!("Heading{}", l)
        }
    });

    let child_count = node.children.len();

    let mut doc_node =
        DocumentNode::new(path, element_type).with_preview(preview.unwrap_or_default());

    if !text.is_empty() {
        doc_node = doc_node.with_text(&text);
    }
    if let Some(s) = style {
        doc_node = doc_node.with_style(&s);
    }
    doc_node.child_count = child_count;

    doc_node
}
