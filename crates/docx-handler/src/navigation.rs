use crate::dom_types::{WordDom, WordElementType, WordNode};
use handler_common::{HandlerError, PathAliases, PathSegment};

/// Parse a document path string into a list of PathSegments.
/// Path format: /body/p[3]/r[1] or /body/tbl[2]/tr[1]/tc[3]
/// Also supports aliases: /body/paragraph[3]/run[1]
pub fn parse_path(path: &str) -> Result<Vec<PathSegment>, HandlerError> {
    if !path.starts_with('/') {
        return Err(HandlerError::InvalidPath(format!(
            "path must start with /: {}",
            path
        )));
    }

    let aliases = PathAliases::new();
    let segments_str = path.split('/').filter(|s| !s.is_empty());
    let mut segments = Vec::new();

    for seg in segments_str {
        let mut name = String::new();
        let mut index: Option<usize> = None;
        let mut attribute: Option<(String, String)> = None;

        // Parse [N] or [@attr=val]
        if let Some(bracket_start) = seg.find('[') {
            name = seg[..bracket_start].to_string();
            let remaining = &seg[bracket_start..];
            let mut pos = 0;
            while pos < remaining.len() && remaining[pos..].starts_with('[') {
                if let Some(end) = remaining[pos..].find(']') {
                    let content = &remaining[pos + 1..pos + end];
                    if content.starts_with('@') {
                        let attr_content = &content[1..];
                        if let Some(eq) = attr_content.find('=') {
                            attribute = Some((
                                attr_content[..eq].to_string(),
                                attr_content[eq + 1..].to_string(),
                            ));
                        }
                    } else if let Ok(idx) = content.parse::<usize>() {
                        index = Some(idx);
                    }
                    pos += end + 1;
                } else {
                    break;
                }
            }
        } else {
            name = seg.to_string();
        }

        // Resolve alias (paragraph -> p, run -> r, etc.)
        name = aliases.resolve(&name);

        let ps = PathSegment::new(&name);
        let ps = if let Some(idx) = index {
            ps.with_index(idx)
        } else {
            ps
        };
        let ps = if let Some((k, v)) = attribute {
            ps.with_attribute(&k, &v)
        } else {
            ps
        };

        segments.push(ps);
    }

    Ok(segments)
}

/// Navigate the Word DOM tree to find the node at a given path.
pub fn navigate_to_element<'a>(dom: &'a WordDom, path: &str) -> Result<&'a WordNode, HandlerError> {
    let segments = parse_path(path)?;
    if segments.is_empty() {
        return Err(HandlerError::InvalidPath("empty path".to_string()));
    }

    let first = &segments[0];
    if first.name != "body" {
        return Err(HandlerError::InvalidPath(format!(
            "Word paths must start with /body, got: /{}",
            first.name
        )));
    }

    let body = dom
        .body()
        .ok_or_else(|| HandlerError::PathNotFound("body element not found".to_string()))?;

    if segments.len() == 1 {
        return Ok(body);
    }

    navigate_segments(body, &segments[1..], path)
}

/// Navigate mutable Word DOM tree to find the node at a given path.
pub fn navigate_to_element_mut<'a>(
    dom: &'a mut WordDom,
    path: &str,
) -> Result<&'a mut WordNode, HandlerError> {
    let segments = parse_path(path)?;
    if segments.is_empty() {
        return Err(HandlerError::InvalidPath("empty path".to_string()));
    }

    let first = &segments[0];
    if first.name != "body" {
        return Err(HandlerError::InvalidPath(format!(
            "Word paths must start with /body, got: /{}",
            first.name
        )));
    }

    // Find body in root children
    let body_idx = dom
        .root
        .children
        .iter()
        .position(|c| c.element_type == WordElementType::Body)
        .ok_or_else(|| HandlerError::PathNotFound("body element not found".to_string()))?;

    if segments.len() == 1 {
        return Ok(&mut dom.root.children[body_idx]);
    }

    navigate_segments_mut(&mut dom.root.children[body_idx], &segments[1..], path)
}

/// Navigate through segments on a given node.
fn navigate_segments<'a>(
    node: &'a WordNode,
    segments: &[PathSegment],
    full_path: &str,
) -> Result<&'a WordNode, HandlerError> {
    if segments.is_empty() {
        return Ok(node);
    }

    let seg = &segments[0];
    let target_type = resolve_element_type_from_name(&seg.name);

    let matching: Vec<&WordNode> = node
        .children
        .iter()
        .filter(|c| element_matches_type(c, &target_type, seg))
        .collect();

    if matching.is_empty() {
        return Err(HandlerError::PathNotFound(format!(
            "no {} children at path {}",
            seg.name, full_path
        )));
    }

    let idx = seg.index.unwrap_or(1);
    if idx == 0 || idx > matching.len() {
        return Err(HandlerError::PathNotFound(format!(
            "index {} out of range for {} at path {} (max: {})",
            idx,
            seg.name,
            full_path,
            matching.len()
        )));
    }

    navigate_segments(matching[idx - 1], &segments[1..], full_path)
}

/// Navigate through segments on a mutable node.
fn navigate_segments_mut<'a>(
    node: &'a mut WordNode,
    segments: &[PathSegment],
    full_path: &str,
) -> Result<&'a mut WordNode, HandlerError> {
    if segments.is_empty() {
        return Ok(node);
    }

    let seg = &segments[0];
    let target_type = resolve_element_type_from_name(&seg.name);
    let idx = seg.index.unwrap_or(1);

    // First pass: collect matching child indices (non-mutable scan)
    let matching_indices: Vec<usize> = node
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| element_matches_type(c, &target_type, seg))
        .map(|(i, _)| i)
        .collect();

    if matching_indices.is_empty() {
        return Err(HandlerError::PathNotFound(format!(
            "no {} children at path {}",
            seg.name, full_path
        )));
    }

    if idx == 0 || idx > matching_indices.len() {
        return Err(HandlerError::PathNotFound(format!(
            "index {} out of range for {} at path {} (max: {})",
            idx,
            seg.name,
            full_path,
            matching_indices.len()
        )));
    }

    let child_idx = matching_indices[idx - 1];
    let child = &mut node.children[child_idx];

    navigate_segments_mut(child, &segments[1..], full_path)
}

/// Resolve a path segment name to a WordElementType.
pub fn resolve_element_type_from_name(name: &str) -> WordElementType {
    match name {
        "body" => WordElementType::Body,
        "p" => WordElementType::Paragraph,
        "r" => WordElementType::Run,
        "t" => WordElementType::Text,
        "tbl" => WordElementType::Table,
        "tr" => WordElementType::TableRow,
        "tc" => WordElementType::TableCell,
        "hyperlink" => WordElementType::Hyperlink,
        "drawing" => WordElementType::Drawing,
        "br" => WordElementType::Break,
        "tab" => WordElementType::Tab,
        "pPr" => WordElementType::ParagraphProperties,
        "rPr" => WordElementType::RunProperties,
        "tblPr" => WordElementType::TableProperties,
        "trPr" => WordElementType::TableRowProperties,
        "tcPr" => WordElementType::TableCellProperties,
        "sdt" => WordElementType::Sdt,
        "sdtContent" => WordElementType::SdtContent,
        "bookmarkStart" => WordElementType::BookmarkStart,
        "bookmarkEnd" => WordElementType::BookmarkEnd,
        "sectPr" => WordElementType::SectionProperties,
        "footnoteRef" => WordElementType::FootnoteReference,
        "endnoteRef" => WordElementType::EndnoteReference,
        "commentRef" => WordElementType::CommentReference,
        "moveFrom" => WordElementType::MoveFrom,
        "moveTo" => WordElementType::MoveTo,
        other => WordElementType::Unknown(other.to_string()),
    }
}

/// Check if a node matches the target type and optional attribute filter.
fn element_matches_type(node: &WordNode, target: &WordElementType, seg: &PathSegment) -> bool {
    if node.element_type != *target {
        // Also match Unknown by name
        if let WordElementType::Unknown(ref name) = node.element_type {
            if name != &seg.name {
                return false;
            }
        } else {
            return false;
        }
    }

    // Check attribute filter
    if let Some((attr_key, attr_val)) = &seg.attribute {
        match node.attributes.get(attr_key) {
            Some(val) if val != attr_val => return false,
            None => return false,
            _ => {}
        }
    }

    true
}

/// Build the path string for a body-level child element.
pub fn build_body_child_path(index: usize, element_type: &WordElementType) -> String {
    let name = element_type.to_path_name();
    format!("/body/{}[{}]", name, index)
}

/// Build the path string for a run within a paragraph.
pub fn build_run_path(para_index: usize, run_index: usize) -> String {
    format!("/body/p[{}]/r[{}]", para_index, run_index)
}

/// Build the path string for a table cell.
pub fn build_cell_path(tbl_index: usize, row_index: usize, cell_index: usize) -> String {
    format!(
        "/body/tbl[{}]/tr[{}]/tc[{}]",
        tbl_index, row_index, cell_index
    )
}

/// Given a path, return the parent path.
pub fn parent_path(path: &str) -> Option<String> {
    let segments = parse_path(path).ok()?;
    if segments.len() <= 1 {
        return None;
    }
    Some(format_path(&segments[..segments.len() - 1]))
}

/// Format a list of PathSegments back into a path string.
fn format_path(segments: &[PathSegment]) -> String {
    let mut result = String::new();
    for seg in segments {
        result.push('/');
        result.push_str(&seg.to_path_fragment());
    }
    result
}

/// Find the 0-based child index of the element at a given path.
pub fn find_child_index(dom: &WordDom, path: &str) -> usize {
    let segments = parse_path(path).ok();
    if segments.is_none() || segments.as_ref().unwrap().len() < 2 {
        return 0;
    }
    let segments = segments.unwrap();

    let body = dom.body();
    if body.is_none() {
        return 0;
    }
    let mut current = body.unwrap();

    // Walk all but last segment to get to the parent
    for i in 1..segments.len() - 1 {
        let seg = &segments[i];
        let target_type = resolve_element_type_from_name(&seg.name);
        let matching_indices: Vec<usize> = current
            .children
            .iter()
            .enumerate()
            .filter(|(_, c)| element_matches_type(c, &target_type, seg))
            .map(|(i, _)| i)
            .collect();
        let idx = seg.index.unwrap_or(1);
        if idx > matching_indices.len() {
            return 0;
        }
        current = &current.children[matching_indices[idx - 1]];
    }

    let last_seg = &segments[segments.len() - 1];
    let target_type = resolve_element_type_from_name(&last_seg.name);
    let idx = last_seg.index.unwrap_or(1);

    let mut count = 0;
    for child in &current.children {
        if element_matches_type(child, &target_type, last_seg) {
            count += 1;
            if count == idx {
                return count - 1;
            }
        }
    }
    0
}
