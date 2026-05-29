use crate::dom_types::{WordDom, WordElementType};
use handler_common::HandlerError;

/// Resolve a paraId-based path to an index-based path.
/// E.g., given "p[@paraId=12345678]" find the paragraph index and return "/body/p[3]".
pub fn resolve_para_id_path(dom: &WordDom, para_id: &str) -> Result<String, HandlerError> {
    let body = dom
        .body()
        .ok_or_else(|| HandlerError::OperationFailed("body element not found".to_string()))?;

    let mut para_idx = 0;
    for child in &body.children {
        if child.element_type == WordElementType::Paragraph {
            para_idx += 1;
            if child.attributes.get("paraId").map(|s| s.as_str()) == Some(para_id) {
                return Ok(format!("/body/p[{}]", para_idx));
            }
        }
    }

    Err(HandlerError::PathNotFound(format!(
        "paragraph with paraId={} not found",
        para_id
    )))
}

/// Find all paragraphs that have paraId attributes.
pub fn collect_para_ids(dom: &WordDom) -> Vec<(usize, String)> {
    let body = dom.body();
    if let Some(body) = body {
        let mut result = Vec::new();
        let mut para_idx = 0;
        for child in &body.children {
            if child.element_type == WordElementType::Paragraph {
                para_idx += 1;
                if let Some(para_id) = child.attributes.get("paraId") {
                    result.push((para_idx, para_id.clone()));
                }
            }
        }
        result
    } else {
        Vec::new()
    }
}

/// Generate a unique paragraph ID (8 hex chars).
pub fn generate_para_id() -> String {
    use uuid::Uuid;
    let uuid = Uuid::new_v4();
    uuid.to_string().replace('-', "")[..8].to_string()
}
