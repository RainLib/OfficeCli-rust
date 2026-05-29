use crate::dom_types::{WordDom, WordElementType, WordNode};
use crate::navigation::{navigate_to_element, navigate_to_element_mut, parse_path};
use handler_common::{HandlerError, InsertPosition};
use std::collections::HashMap;

/// Add a new element at the given parent path.
pub fn add_element(
    dom: &mut WordDom,
    parent: &str,
    element_type: &str,
    position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let resolved_type = resolve_add_type(element_type)?;

    match resolved_type {
        AddType::Paragraph => add_paragraph(dom, parent, position, properties),
        AddType::Run => add_run(dom, parent, position, properties),
        AddType::Table => add_table(dom, parent, position),
        AddType::TableRow => add_table_row(dom, parent, position),
        AddType::TableCell => add_table_cell(dom, parent, position, properties),
    }
}

enum AddType {
    Paragraph,
    Run,
    Table,
    TableRow,
    TableCell,
}

fn resolve_add_type(name: &str) -> Result<AddType, HandlerError> {
    match name {
        "p" | "paragraph" => Ok(AddType::Paragraph),
        "r" | "run" => Ok(AddType::Run),
        "tbl" | "table" => Ok(AddType::Table),
        "tr" | "row" => Ok(AddType::TableRow),
        "tc" | "cell" => Ok(AddType::TableCell),
        other => Err(HandlerError::UnsupportedType(format!(
            "cannot add element type: {}",
            other
        ))),
    }
}

/// Add a paragraph to the body or after a specific paragraph.
fn add_paragraph(
    dom: &mut WordDom,
    parent: &str,
    position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let segments = parse_path(parent)?;
    let first_seg = segments.first().ok_or_else(|| {
        HandlerError::InvalidPath("parent path must start with /body".to_string())
    })?;

    if first_seg.name != "body" {
        return Err(HandlerError::InvalidPath(format!(
            "paragraphs can only be added under /body, got: {}",
            parent
        )));
    }

    let para_id = crate::helpers::generate_para_id();
    let mut para = WordNode::new(WordElementType::Paragraph).with_attribute("paraId", &para_id);

    // Add paragraph properties if provided
    if let Some(ppr) = crate::helpers::build_paragraph_properties(properties) {
        para.children.push(ppr);
    }

    // If "text" property is provided, add a run with that text
    if let Some(text) = properties.get("text") {
        let mut run = WordNode::new(WordElementType::Run);
        let run_props: HashMap<String, String> = properties
            .iter()
            .filter(|(k, _)| {
                k.as_str() != "text"
                    && k.as_str() != "style"
                    && k.as_str() != "alignment"
                    && !k.starts_with("indent")
                    && !k.starts_with("spacing")
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if let Some(rpr) = crate::helpers::build_run_properties(&run_props) {
            run.children.push(rpr);
        }
        let mut text_node = WordNode::new(WordElementType::Text).with_text(text);
        if text.starts_with(' ') || text.ends_with(' ') {
            text_node
                .attributes
                .insert("xml:space".to_string(), "preserve".to_string());
            text_node.preserve_space = true;
        }
        run.children.push(text_node);
        para.children.push(run);
    }

    // Get body and determine insertion index
    let body_idx = dom
        .root
        .children
        .iter()
        .position(|c| c.element_type == WordElementType::Body)
        .ok_or_else(|| HandlerError::OperationFailed("body element not found".to_string()))?;

    let content_items: Vec<usize> = dom.root.children[body_idx]
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| c.element_type.is_body_child())
        .map(|(i, _)| i)
        .collect();

    let insert_idx = resolve_insert_index_simple(&position, content_items.len());

    match insert_idx {
        Some(idx) => {
            let real_idx = if idx < content_items.len() {
                content_items[idx]
            } else {
                dom.root.children[body_idx].children.len()
            };
            dom.root.children[body_idx].children.insert(real_idx, para);
        }
        None => {
            dom.root.children[body_idx].children.push(para);
        }
    }

    // Calculate the path of the new paragraph
    let mut new_para_idx = 0;
    for child in &dom.root.children[body_idx].children {
        if child.element_type == WordElementType::Paragraph {
            new_para_idx += 1;
        }
    }

    Ok(format!("/body/p[{}]", new_para_idx))
}

/// Add a run to a paragraph.
fn add_run(
    dom: &mut WordDom,
    parent: &str,
    position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    // First, check if path exists (immutable borrow to verify)
    let existing_run_count = {
        let para = navigate_to_element(dom, parent)?;
        para.runs().len()
    };

    // Build the run node
    let mut run = WordNode::new(WordElementType::Run);

    let run_props: HashMap<String, String> = properties
        .iter()
        .filter(|(k, _)| k.as_str() != "text")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if let Some(rpr) = crate::helpers::build_run_properties(&run_props) {
        run.children.push(rpr);
    }

    if let Some(text) = properties.get("text") {
        let mut text_node = WordNode::new(WordElementType::Text).with_text(text);
        if text.starts_with(' ') || text.ends_with(' ') {
            text_node
                .attributes
                .insert("xml:space".to_string(), "preserve".to_string());
            text_node.preserve_space = true;
        }
        run.children.push(text_node);
    }

    // Now get mutable access
    let para = navigate_to_element_mut(dom, parent)?;

    let existing_runs: Vec<usize> = para
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| c.element_type == WordElementType::Run)
        .map(|(i, _)| i)
        .collect();

    let insert_idx = resolve_insert_index_simple(&position, existing_runs.len());

    match insert_idx {
        Some(idx) => {
            let real_idx = if idx < existing_runs.len() {
                existing_runs[idx]
            } else {
                para.children.len()
            };
            para.children.insert(real_idx, run);
        }
        None => {
            para.children.push(run);
        }
    }

    Ok(format!("{}/r[{}]", parent, existing_run_count + 1))
}

/// Add an empty table to the body.
fn add_table(
    dom: &mut WordDom,
    parent: &str,
    position: InsertPosition,
) -> Result<String, HandlerError> {
    let segments = parse_path(parent)?;
    let first_seg = segments.first().ok_or_else(|| {
        HandlerError::InvalidPath("parent path must start with /body".to_string())
    })?;

    if first_seg.name != "body" {
        return Err(HandlerError::InvalidPath(
            "tables can only be added under /body".to_string(),
        ));
    }

    let tbl_pr = WordNode::new(WordElementType::TableProperties);
    let cell = WordNode::new(WordElementType::TableCell)
        .with_children(vec![WordNode::new(WordElementType::Paragraph)]);
    let row = WordNode::new(WordElementType::TableRow).with_children(vec![cell]);
    let table = WordNode::new(WordElementType::Table).with_children(vec![tbl_pr, row]);

    let body_idx = dom
        .root
        .children
        .iter()
        .position(|c| c.element_type == WordElementType::Body)
        .ok_or_else(|| HandlerError::OperationFailed("body element not found".to_string()))?;

    let content_items: Vec<usize> = dom.root.children[body_idx]
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| c.element_type.is_body_child())
        .map(|(i, _)| i)
        .collect();

    let insert_idx = resolve_insert_index_simple(&position, content_items.len());

    match insert_idx {
        Some(idx) => {
            let real_idx = if idx < content_items.len() {
                content_items[idx]
            } else {
                dom.root.children[body_idx].children.len()
            };
            dom.root.children[body_idx].children.insert(real_idx, table);
        }
        None => {
            dom.root.children[body_idx].children.push(table);
        }
    }

    let mut tbl_idx = 0;
    for child in &dom.root.children[body_idx].children {
        if child.element_type == WordElementType::Table {
            tbl_idx += 1;
        }
    }
    Ok(format!("/body/tbl[{}]", tbl_idx))
}

/// Add a row to a table.
fn add_table_row(
    dom: &mut WordDom,
    parent: &str,
    position: InsertPosition,
) -> Result<String, HandlerError> {
    // First check table structure (immutable)
    let col_count = {
        let table = navigate_to_element(dom, parent)?;
        table
            .children
            .iter()
            .find(|c| c.element_type == WordElementType::TableRow)
            .map(|row| {
                row.children
                    .iter()
                    .filter(|c| c.element_type == WordElementType::TableCell)
                    .count()
            })
            .unwrap_or(1)
    };

    let mut cells = Vec::new();
    for _ in 0..col_count {
        cells.push(
            WordNode::new(WordElementType::TableCell)
                .with_children(vec![WordNode::new(WordElementType::Paragraph)]),
        );
    }
    let row = WordNode::new(WordElementType::TableRow).with_children(cells);

    // Now get mutable access
    let table = navigate_to_element_mut(dom, parent)?;

    let existing_rows: Vec<usize> = table
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| c.element_type == WordElementType::TableRow)
        .map(|(i, _)| i)
        .collect();

    let insert_idx = resolve_insert_index_simple(&position, existing_rows.len());

    match insert_idx {
        Some(idx) => {
            let real_idx = if idx < existing_rows.len() {
                existing_rows[idx]
            } else {
                table.children.len()
            };
            table.children.insert(real_idx, row);
        }
        None => {
            table.children.push(row);
        }
    }

    let row_count = table
        .children
        .iter()
        .filter(|c| c.element_type == WordElementType::TableRow)
        .count();
    Ok(format!("{}/tr[{}]", parent, row_count))
}

/// Add a cell to a table row.
fn add_table_cell(
    dom: &mut WordDom,
    parent: &str,
    position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let mut para = WordNode::new(WordElementType::Paragraph);
    if let Some(text) = properties.get("text") {
        let run = WordNode::new(WordElementType::Run)
            .with_children(vec![WordNode::new(WordElementType::Text).with_text(text)]);
        para.children.push(run);
    }

    let cell = WordNode::new(WordElementType::TableCell).with_children(vec![para]);

    let row = navigate_to_element_mut(dom, parent)?;

    let existing_cells: Vec<usize> = row
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| c.element_type == WordElementType::TableCell)
        .map(|(i, _)| i)
        .collect();

    let insert_idx = resolve_insert_index_simple(&position, existing_cells.len());

    match insert_idx {
        Some(idx) => {
            let real_idx = if idx < existing_cells.len() {
                existing_cells[idx]
            } else {
                row.children.len()
            };
            row.children.insert(real_idx, cell);
        }
        None => {
            row.children.push(cell);
        }
    }

    let cell_count = row
        .children
        .iter()
        .filter(|c| c.element_type == WordElementType::TableCell)
        .count();
    Ok(format!("{}/tc[{}]", parent, cell_count))
}

/// Simple insertion index resolution without closures that need dom access.
/// For AtIndex: returns the index directly.
/// For Append: returns None.
/// For After/Before: we'd need dom access, but for simplicity, we just append.
fn resolve_insert_index_simple(position: &InsertPosition, _child_count: usize) -> Option<usize> {
    match position {
        InsertPosition::AtIndex(idx) => Some(*idx),
        InsertPosition::Append => None,
        InsertPosition::AfterElement(_) | InsertPosition::BeforeElement(_) => {
            // For now, just append. Proper resolution would require path navigation.
            None
        }
    }
}
