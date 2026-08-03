use crate::dom_types::{WordDom, WordElementType, WordNode};
use handler_common::{DocumentNode, HandlerError, Selector};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
struct RevisionInfo {
    native_path: String,
    id: Option<String>,
    revision_type: String,
    author: Option<String>,
    date: Option<String>,
    text: String,
}

enum RevisionTarget {
    Selector(Selector),
    NativePath(String),
}

/// Return C#-compatible synthetic revision nodes.  Revision wrappers stay in
/// the normal DOM so all Rust-only raw and text operations retain their
/// existing behavior.
pub fn query_revisions(dom: &WordDom, selector: &Selector) -> Vec<DocumentNode> {
    let mut revisions = Vec::new();
    if let Some(body) = dom.body() {
        collect_revisions(body, "/body", &mut revisions);
    }
    let id_counts = revisions.iter().filter_map(|item| item.id.as_ref()).fold(
        HashMap::<&str, usize>::new(),
        |mut counts, id| {
            *counts.entry(id.as_str()).or_default() += 1;
            counts
        },
    );
    revisions
        .iter()
        .enumerate()
        .filter(|(_, item)| revision_matches_selector(item, selector))
        .map(|(index, item)| revision_to_document_node(item, index + 1, &id_counts))
        .collect()
}

pub fn get_revision(dom: &WordDom, path: &str) -> Result<DocumentNode, HandlerError> {
    let selector = parse_revision_path(path)?;
    let mut candidates = query_revisions(dom, &selector);
    if let Some(index) = revision_positional_index(path) {
        return candidates
            .into_iter()
            .find(|node| node.path == format!("/revision[{}]", index))
            .ok_or_else(|| HandlerError::PathNotFound(path.to_string()));
    }
    if let Some(node) = candidates.iter().find(|node| node.path == path) {
        return Ok(node.clone());
    }
    // A moveFrom/moveTo pair deliberately shares its id.  C# accepts the
    // unqualified id path as a pair-wise action address; for Get expose one
    // endpoint while preserving the caller's requested stable path.
    if selector.attributes.len() == 1 && selector.attributes[0].0 == "id" {
        if let Some(mut node) = candidates.drain(..).next() {
            node.path = path.to_string();
            return Ok(node);
        }
    }
    Err(HandlerError::PathNotFound(path.to_string()))
}

pub fn apply_revision_action(
    dom: &mut WordDom,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let action = properties
        .get("revision.action")
        .map(|value| value.to_ascii_lowercase())
        .ok_or_else(|| HandlerError::InvalidArgument("revision.action is required".to_string()))?;
    if action != "accept" && action != "reject" {
        return Err(HandlerError::InvalidArgument(
            "revision.action must be accept or reject".to_string(),
        ));
    }
    let unsupported: Vec<String> = properties
        .keys()
        .filter(|key| key.as_str() != "revision.action")
        .cloned()
        .collect();
    if !unsupported.is_empty() {
        return Err(HandlerError::InvalidArgument(
            "revision.action cannot be mixed with other properties".to_string(),
        ));
    }

    let target = if path.starts_with("/revision") {
        let selector = parse_revision_path(path)?;
        if revision_positional_index(path).is_some()
            || selector.attributes.iter().any(|(key, _)| key == "type")
            || (selector.attributes.iter().any(|(key, _)| key == "id")
                && selector.attributes.len() > 1)
        {
            RevisionTarget::NativePath(
                get_revision(dom, path)?
                    .format
                    .get("nativePath")
                    .and_then(|value| value.as_ref())
                    .and_then(Value::as_str)
                    .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?
                    .to_string(),
            )
        } else {
            RevisionTarget::Selector(selector)
        }
    } else {
        let all_revisions = query_revisions(
            dom,
            &Selector::parse("revision").map_err(|error| {
                HandlerError::OperationFailed(format!("invalid revision selector: {}", error))
            })?,
        );
        let selected = all_revisions.into_iter().find(|node| {
            node.format
                .get("nativePath")
                .and_then(|value| value.as_ref())
                .and_then(Value::as_str)
                == Some(path)
        });
        let is_move = selected.as_ref().and_then(|node| {
            node.format
                .get("type")
                .and_then(|value| value.as_ref())
                .and_then(Value::as_str)
        });
        let move_id = selected.as_ref().and_then(|node| {
            node.format
                .get("id")
                .and_then(|value| value.as_ref())
                .and_then(Value::as_str)
        });
        if matches!(is_move, Some("moveFrom" | "moveTo")) {
            let id = move_id.ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?;
            RevisionTarget::Selector(Selector::parse(&format!("revision[@id={}]", id)).map_err(
                |error| {
                    HandlerError::OperationFailed(format!("invalid revision selector: {}", error))
                },
            )?)
        } else {
            RevisionTarget::NativePath(path.to_string())
        }
    };
    let body_index = dom
        .root
        .children
        .iter()
        .position(|node| node.element_type == WordElementType::Body)
        .ok_or_else(|| HandlerError::PathNotFound("body element not found".to_string()))?;
    let mut matched = 0;
    let mut resolved_move_ids = HashSet::new();
    apply_action_to_children(
        &mut dom.root.children[body_index].children,
        "/body",
        &target,
        &action,
        &mut matched,
        &mut resolved_move_ids,
    );
    if matched == 0 {
        return Err(HandlerError::PathNotFound(path.to_string()));
    }
    if !resolved_move_ids.is_empty() {
        remove_move_range_markers(&mut dom.root, &resolved_move_ids);
    }
    Ok(Vec::new())
}

fn collect_revisions(node: &WordNode, path: &str, output: &mut Vec<RevisionInfo>) {
    let mut counts = HashMap::<String, usize>::new();
    for child in &node.children {
        let name = child.element_type.to_path_name().to_string();
        let count = counts.entry(name.clone()).or_default();
        *count += 1;
        let child_path = format!("{}/{}[{}]", path.trim_end_matches('/'), name, count);
        if let Some(revision_type) = revision_type(child) {
            output.push(RevisionInfo {
                native_path: child_path.clone(),
                id: child.attributes.get("id").cloned(),
                revision_type: revision_type.to_string(),
                author: child.attributes.get("author").cloned(),
                date: child.attributes.get("date").cloned(),
                text: revision_text(child),
            });
        }
        collect_revisions(child, &child_path, output);
    }
}

fn revision_to_document_node(
    item: &RevisionInfo,
    positional_index: usize,
    id_counts: &HashMap<&str, usize>,
) -> DocumentNode {
    let path = match item.id.as_deref() {
        Some(id) if id_counts.get(id) == Some(&1) => format!("/revision[@id={}]", id),
        Some(id) => format!("/revision[@id={}][@type={}]", id, item.revision_type),
        None => format!("/revision[{}]", positional_index),
    };
    let mut node = DocumentNode::new(&path, "revision")
        .with_format("type", Value::String(item.revision_type.clone()))
        .with_format("nativePath", Value::String(item.native_path.clone()));
    if let Some(id) = &item.id {
        node = node.with_format("id", Value::String(id.clone()));
    }
    if let Some(author) = &item.author {
        node = node.with_format("author", Value::String(author.clone()));
    }
    if let Some(date) = &item.date {
        node = node.with_format("date", Value::String(date.clone()));
    }
    if !item.text.is_empty() {
        node = node
            .with_text(item.text.clone())
            .with_preview(item.text.clone());
    }
    node
}

fn revision_matches_selector(item: &RevisionInfo, selector: &Selector) -> bool {
    selector
        .attributes
        .iter()
        .all(|(key, value)| match key.as_str() {
            "id" => item.id.as_deref() == Some(value),
            "type" => item.revision_type.eq_ignore_ascii_case(value),
            "author" => item.author.as_deref() == Some(value),
            _ => false,
        })
}

fn parse_revision_path(path: &str) -> Result<Selector, HandlerError> {
    let selector = path
        .strip_prefix('/')
        .ok_or_else(|| HandlerError::InvalidPath(path.to_string()))?;
    let selector = Selector::parse(selector)
        .map_err(|error| HandlerError::InvalidPath(format!("{}: {}", path, error)))?;
    if selector.element_type.as_deref() != Some("revision") {
        return Err(HandlerError::InvalidPath(path.to_string()));
    }
    Ok(selector)
}

fn revision_positional_index(path: &str) -> Option<usize> {
    path.strip_prefix("/revision[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

fn apply_action_to_children(
    children: &mut Vec<WordNode>,
    parent_path: &str,
    target: &RevisionTarget,
    action: &str,
    matched: &mut usize,
    resolved_move_ids: &mut HashSet<String>,
) {
    let mut counts = HashMap::<String, usize>::new();
    let mut index = children.len();
    while index > 0 {
        index -= 1;
        let name = children[index].element_type.to_path_name().to_string();
        let count = counts.entry(name.clone()).or_default();
        *count += 1;
        let forward_index = children[..=index]
            .iter()
            .filter(|child| child.element_type.to_path_name() == name)
            .count();
        let path = format!(
            "{}/{}[{}]",
            parent_path.trim_end_matches('/'),
            name,
            forward_index
        );
        if let Some(revision_type) = revision_type(&children[index]).map(str::to_string) {
            let item = RevisionInfo {
                native_path: path.clone(),
                id: children[index].attributes.get("id").cloned(),
                revision_type: revision_type.clone(),
                author: children[index].attributes.get("author").cloned(),
                date: children[index].attributes.get("date").cloned(),
                text: revision_text(&children[index]),
            };
            if revision_matches_target(&item, target) {
                *matched += 1;
                if revision_type == "format" {
                    if action == "accept" {
                        children.remove(index);
                    } else {
                        reject_format_revision(children, index);
                    }
                    // Reject replaces the whole rPr child vector with its
                    // snapshot, invalidating every sibling index in this
                    // traversal. A run has one rPr, so returning here is both
                    // safe and prevents an out-of-bounds follow-up visit.
                    return;
                }
                if matches!(revision_type.as_str(), "moveFrom" | "moveTo") {
                    if let Some(id) = &item.id {
                        resolved_move_ids.insert(id.clone());
                    }
                }
                let remove = matches!(
                    (revision_type.as_str(), action),
                    ("del" | "moveFrom", "accept") | ("ins" | "moveTo", "reject")
                );
                if remove {
                    children.remove(index);
                } else {
                    let mut replacement = std::mem::take(&mut children[index].children);
                    if matches!(revision_type.as_str(), "del" | "moveFrom") && action == "reject" {
                        for node in &mut replacement {
                            restore_deleted_text(node);
                        }
                    }
                    children.splice(index..=index, replacement);
                }
                continue;
            }
        }
        if children[index].element_type == WordElementType::TableRow
            && apply_row_revision_action(&mut children[index], &path, target, action, matched)
        {
            children.remove(index);
            continue;
        }
        if children[index].element_type == WordElementType::TableCell
            && apply_cell_revision_action(&mut children[index], &path, target, action, matched)
        {
            children.remove(index);
            continue;
        }
        match apply_paragraph_mark_action(children, index, &path, target, action, matched) {
            ParagraphMarkAction::NoMatch => {}
            ParagraphMarkAction::KeepParagraph => continue,
            ParagraphMarkAction::RemovedInPlace => continue,
        }
        apply_action_to_children(
            &mut children[index].children,
            &path,
            target,
            action,
            matched,
            resolved_move_ids,
        );
    }
}

enum ParagraphMarkAction {
    NoMatch,
    KeepParagraph,
    RemovedInPlace,
}

/// Paragraph-mark revisions are stored in `pPr/rPr`, but accepting a marked
/// deletion or rejecting a marked insertion joins adjacent paragraphs.  This
/// is structurally different from a run wrapper and must be resolved while
/// the sibling paragraph list is available.
fn apply_paragraph_mark_action(
    siblings: &mut Vec<WordNode>,
    index: usize,
    paragraph_path: &str,
    target: &RevisionTarget,
    action: &str,
    matched: &mut usize,
) -> ParagraphMarkAction {
    if siblings[index].element_type != WordElementType::Paragraph {
        return ParagraphMarkAction::NoMatch;
    }
    let marker_info = {
        let paragraph = &siblings[index];
        let Some(ppr) = paragraph
            .children
            .iter()
            .find(|child| child.element_type == WordElementType::ParagraphProperties)
        else {
            return ParagraphMarkAction::NoMatch;
        };
        let Some(mark_rpr) = ppr
            .children
            .iter()
            .find(|child| child.element_type == WordElementType::RunProperties)
        else {
            return ParagraphMarkAction::NoMatch;
        };
        let Some(marker) = mark_rpr.children.iter().find(|child| {
            matches!(&child.element_type, WordElementType::Unknown(name) if name == "ins" || name == "del")
        }) else {
            return ParagraphMarkAction::NoMatch;
        };
        let kind = match &marker.element_type {
            WordElementType::Unknown(name) => name.clone(),
            _ => return ParagraphMarkAction::NoMatch,
        };
        RevisionInfo {
            native_path: format!("{}/pPr[1]/rPr[1]/{}[1]", paragraph_path, kind),
            id: marker.attributes.get("id").cloned(),
            revision_type: kind,
            author: marker.attributes.get("author").cloned(),
            date: marker.attributes.get("date").cloned(),
            text: String::new(),
        }
    };
    if !revision_matches_target(&marker_info, target) {
        return ParagraphMarkAction::NoMatch;
    }
    *matched += 1;
    let kind = marker_info.revision_type.as_str();
    let remove_and_join_previous = kind == "ins" && action == "reject";
    let remove_and_join_next = kind == "del" && action == "accept";
    let can_join = (remove_and_join_previous
        && index > 0
        && siblings[index - 1].element_type == WordElementType::Paragraph)
        || (remove_and_join_next
            && index + 1 < siblings.len()
            && siblings[index + 1].element_type == WordElementType::Paragraph);

    let paragraph = &mut siblings[index];
    let ppr = paragraph
        .children
        .iter_mut()
        .find(|child| child.element_type == WordElementType::ParagraphProperties)
        .expect("paragraph mark requires pPr");
    let mark_rpr = ppr
        .children
        .iter_mut()
        .find(|child| child.element_type == WordElementType::RunProperties)
        .expect("paragraph mark requires pPr/rPr");
    let marker_index = mark_rpr
        .children
        .iter()
        .position(
            |child| matches!(&child.element_type, WordElementType::Unknown(name) if name == kind),
        )
        .expect("paragraph mark disappeared during action");
    mark_rpr.children.remove(marker_index);

    if !can_join {
        return ParagraphMarkAction::KeepParagraph;
    }
    let mut removed = siblings.remove(index);
    let content: Vec<_> = removed
        .children
        .drain(..)
        .filter(|child| child.element_type != WordElementType::ParagraphProperties)
        .collect();
    if remove_and_join_previous {
        siblings[index - 1].children.extend(content);
    } else {
        siblings[index].children.splice(0..0, content);
    }
    ParagraphMarkAction::RemovedInPlace
}

/// A row marker lives in `<w:trPr>`, but accepting a row deletion or rejecting
/// a row insertion removes the enclosing `<w:tr>`.  Handle it before normal
/// recursion so the generic ins/del logic cannot leave an incorrect row behind.
fn apply_row_revision_action(
    row: &mut WordNode,
    row_path: &str,
    target: &RevisionTarget,
    action: &str,
    matched: &mut usize,
) -> bool {
    let Some(tr_pr) = row
        .children
        .iter_mut()
        .find(|child| child.element_type == WordElementType::TableRowProperties)
    else {
        return false;
    };
    let Some(marker_index) = tr_pr.children.iter().position(|child| {
        matches!(&child.element_type, WordElementType::Unknown(name) if name == "ins" || name == "del")
    }) else {
        return false;
    };
    let marker = &tr_pr.children[marker_index];
    let kind = match &marker.element_type {
        WordElementType::Unknown(name) => name.as_str(),
        _ => return false,
    };
    let item = RevisionInfo {
        native_path: format!("{}/trPr[1]/{}[1]", row_path, kind),
        id: marker.attributes.get("id").cloned(),
        revision_type: kind.to_string(),
        author: marker.attributes.get("author").cloned(),
        date: marker.attributes.get("date").cloned(),
        text: String::new(),
    };
    if !revision_matches_target(&item, target) {
        return false;
    }
    *matched += 1;
    let remove_row = matches!((kind, action), ("ins", "reject") | ("del", "accept"));
    tr_pr.children.remove(marker_index);
    remove_row
}

/// Cell insertion/deletion markers use `<w:tcPr>/<w:cellIns|w:cellDel>`.
/// As with row markers, the destructive branch applies to the enclosing cell.
fn apply_cell_revision_action(
    cell: &mut WordNode,
    cell_path: &str,
    target: &RevisionTarget,
    action: &str,
    matched: &mut usize,
) -> bool {
    let Some(tc_pr) = cell
        .children
        .iter_mut()
        .find(|child| child.element_type == WordElementType::TableCellProperties)
    else {
        return false;
    };
    let Some(marker_index) = tc_pr.children.iter().position(|child| {
        matches!(&child.element_type, WordElementType::Unknown(name) if name == "cellIns" || name == "cellDel")
    }) else {
        return false;
    };
    let marker = &tc_pr.children[marker_index];
    let kind = match &marker.element_type {
        WordElementType::Unknown(name) => name.as_str(),
        _ => return false,
    };
    let item = RevisionInfo {
        native_path: format!("{}/tcPr[1]/{}[1]", cell_path, kind),
        id: marker.attributes.get("id").cloned(),
        revision_type: kind.to_string(),
        author: marker.attributes.get("author").cloned(),
        date: marker.attributes.get("date").cloned(),
        text: String::new(),
    };
    if !revision_matches_target(&item, target) {
        return false;
    }
    *matched += 1;
    let remove_cell = matches!(
        (kind, action),
        ("cellIns", "reject") | ("cellDel", "accept")
    );
    tc_pr.children.remove(marker_index);
    remove_cell
}

fn revision_matches_target(item: &RevisionInfo, target: &RevisionTarget) -> bool {
    match target {
        RevisionTarget::Selector(selector) => revision_matches_selector(item, selector),
        RevisionTarget::NativePath(path) => item.native_path == *path,
    }
}

fn remove_move_range_markers(node: &mut WordNode, ids: &HashSet<String>) {
    node.children.retain(|child| {
        let marker = matches!(
            child.element_type,
            WordElementType::MoveFromRangeStart
                | WordElementType::MoveFromRangeEnd
                | WordElementType::MoveToRangeStart
                | WordElementType::MoveToRangeEnd
        );
        !marker
            || !child
                .attributes
                .get("id")
                .is_some_and(|id| ids.contains(id))
    });
    for child in &mut node.children {
        remove_move_range_markers(child, ids);
    }
}

fn revision_type(node: &WordNode) -> Option<&str> {
    match node.element_type {
        WordElementType::MoveFrom => Some("moveFrom"),
        WordElementType::MoveTo => Some("moveTo"),
        WordElementType::Unknown(ref local) if local == "ins" => Some("ins"),
        WordElementType::Unknown(ref local) if local == "del" => Some("del"),
        WordElementType::Unknown(ref local) if local == "cellIns" => Some("cellIns"),
        WordElementType::Unknown(ref local) if local == "cellDel" => Some("cellDel"),
        WordElementType::Unknown(ref local)
            if matches!(
                local.as_str(),
                "rPrChange" | "pPrChange" | "tblPrChange" | "trPrChange" | "tcPrChange"
            ) =>
        {
            Some("format")
        }
        _ => None,
    }
}

fn reject_format_revision(children: &mut Vec<WordNode>, index: usize) {
    let snapshot_type = match &children[index].element_type {
        WordElementType::Unknown(name) if name == "rPrChange" => WordElementType::RunProperties,
        WordElementType::Unknown(name) if name == "pPrChange" => {
            WordElementType::ParagraphProperties
        }
        WordElementType::Unknown(name) if name == "tblPrChange" => WordElementType::TableProperties,
        WordElementType::Unknown(name) if name == "trPrChange" => {
            WordElementType::TableRowProperties
        }
        WordElementType::Unknown(name) if name == "tcPrChange" => {
            WordElementType::TableCellProperties
        }
        _ => return,
    };
    let snapshot = children[index]
        .children
        .iter()
        .find(|child| child.element_type == snapshot_type)
        .map(|child| child.children.clone())
        .unwrap_or_default();
    // rPrChange's nested rPr is the pre-change snapshot. Replacing the outer
    // run-properties children implements reject without leaving an orphaned
    // rPrChange marker or the post-change formatting behind.
    *children = snapshot;
}

fn revision_text(node: &WordNode) -> String {
    let mut output = String::new();
    collect_revision_text(node, &mut output);
    output
}

fn collect_revision_text(node: &WordNode, output: &mut String) {
    if node.element_type == WordElementType::Text
        || matches!(&node.element_type, WordElementType::Unknown(local) if local == "delText")
    {
        if let Some(text) = &node.text_content {
            output.push_str(text);
        }
    }
    for child in &node.children {
        collect_revision_text(child, output);
    }
}

fn restore_deleted_text(node: &mut WordNode) {
    if matches!(&node.element_type, WordElementType::Unknown(local) if local == "delText") {
        node.element_type = WordElementType::Text;
    }
    for child in &mut node.children {
        restore_deleted_text(child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_run(text: &str, deleted: bool) -> WordNode {
        WordNode::new(WordElementType::Run).with_children(vec![WordNode::new(if deleted {
            WordElementType::Unknown("delText".to_string())
        } else {
            WordElementType::Text
        })
        .with_text(text)])
    }

    fn sample_dom() -> WordDom {
        let ins = WordNode::new(WordElementType::Unknown("ins".to_string()))
            .with_attribute("id", "1")
            .with_attribute("author", "Ada")
            .with_children(vec![text_run("added", false)]);
        let del = WordNode::new(WordElementType::Unknown("del".to_string()))
            .with_attribute("id", "2")
            .with_attribute("author", "Bea")
            .with_children(vec![text_run("removed", true)]);
        WordDom::new(WordNode::new(WordElementType::Document).with_children(vec![
            WordNode::new(WordElementType::Body).with_children(vec![
                WordNode::new(WordElementType::Paragraph).with_children(vec![ins, del]),
            ]),
        ]))
    }

    #[test]
    fn query_revisions_emits_stable_synthetic_paths_and_deleted_text() {
        let dom = sample_dom();
        let selector = Selector::parse("revision").unwrap();
        let revisions = query_revisions(&dom, &selector);
        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[0].path, "/revision[@id=1]");
        assert_eq!(revisions[0].text.as_deref(), Some("added"));
        assert_eq!(revisions[1].path, "/revision[@id=2]");
        assert_eq!(revisions[1].text.as_deref(), Some("removed"));
        assert_eq!(
            revisions[1]
                .format
                .get("nativePath")
                .and_then(|value| value.as_ref())
                .and_then(Value::as_str),
            Some("/body/p[1]/del[1]")
        );
    }

    #[test]
    fn accept_and_reject_apply_the_expected_revision_semantics() {
        let mut dom = sample_dom();
        apply_revision_action(
            &mut dom,
            "/revision[@id=1]",
            &HashMap::from([("revision.action".to_string(), "accept".to_string())]),
        )
        .unwrap();
        apply_revision_action(
            &mut dom,
            "/revision[@id=2]",
            &HashMap::from([("revision.action".to_string(), "reject".to_string())]),
        )
        .unwrap();

        let body = dom.body().unwrap();
        let paragraph = &body.children[0];
        assert_eq!(paragraph.children.len(), 2);
        assert!(paragraph
            .children
            .iter()
            .all(|child| child.element_type == WordElementType::Run));
        assert_eq!(paragraph.paragraph_text(), "addedremoved");
        assert!(query_revisions(&dom, &Selector::parse("revision").unwrap()).is_empty());
    }

    #[test]
    fn native_revision_path_can_target_one_marker() {
        let mut dom = sample_dom();
        apply_revision_action(
            &mut dom,
            "/body/p[1]/ins[1]",
            &HashMap::from([("revision.action".to_string(), "reject".to_string())]),
        )
        .unwrap();
        let revisions = query_revisions(&dom, &Selector::parse("revision").unwrap());
        assert_eq!(revisions.len(), 1);
        assert_eq!(revisions[0].path, "/revision[@id=2]");
    }
}
