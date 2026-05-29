use crate::dom_types::{WordDom, WordElementType};
use handler_common::{HandlerError, TextOffsetMap};

/// Build a TextOffsetMap from the Word DOM.
/// Each paragraph contributes its text, and each run gets its own span.
/// Paragraph breaks are represented as separate "paragraph-break" spans.
pub fn extract_text_with_offsets(dom: &WordDom) -> Result<TextOffsetMap, HandlerError> {
    let mut map = TextOffsetMap::empty("docx");

    let body = dom
        .body()
        .ok_or_else(|| HandlerError::OperationFailed("body element not found".to_string()))?;

    let mut para_idx = 0;
    let mut tbl_idx = 0;

    for child in &body.children {
        match child.element_type {
            WordElementType::Paragraph => {
                para_idx += 1;
                let para_path = format!("/body/p[{}]", para_idx);

                if para_idx > 1 {
                    // Add paragraph separator (newline) between paragraphs
                    map.push_span(
                        "\n",
                        &format!("/body/p[{}]/break", para_idx),
                        "paragraph-break",
                    );
                }

                // Walk runs within the paragraph
                let mut run_idx = 0;
                for p_child in &child.children {
                    if p_child.element_type == WordElementType::Run {
                        run_idx += 1;
                        let run_path = format!("{}/r[{}]", para_path, run_idx);
                        let run_text = extract_run_text(p_child);
                        if !run_text.is_empty() {
                            map.push_span(&run_text, &run_path, "run");
                        }
                    } else if p_child.element_type == WordElementType::Hyperlink {
                        // Hyperlinks contain runs; treat them as nested
                        let mut hyperlink_run_idx = 0;
                        for hl_child in &p_child.children {
                            if hl_child.element_type == WordElementType::Run {
                                hyperlink_run_idx += 1;
                                let hl_path = format!(
                                    "{}/hyperlink[{}]/r[{}]",
                                    para_path,
                                    count_hyperlinks_before(&child.children, p_child),
                                    hyperlink_run_idx
                                );
                                let run_text = extract_run_text(hl_child);
                                if !run_text.is_empty() {
                                    map.push_span(&run_text, &hl_path, "run");
                                }
                            }
                        }
                    }
                }

                // If paragraph has no text, still add an empty span for navigation
                let para_text = child.paragraph_text();
                if para_text.is_empty() && run_idx == 0 {
                    map.push_span("", &para_path, "paragraph");
                }
            }
            WordElementType::Table => {
                tbl_idx += 1;
                let tbl_path = format!("/body/tbl[{}]", tbl_idx);

                if para_idx > 0 || tbl_idx > 1 {
                    map.push_span(
                        "\n",
                        &format!("/body/tbl[{}]/break", tbl_idx),
                        "paragraph-break",
                    );
                }

                let mut row_idx = 0;
                for tbl_child in &child.children {
                    if tbl_child.element_type == WordElementType::TableRow {
                        row_idx += 1;
                        let row_path = format!("{}/tr[{}]", tbl_path, row_idx);

                        let mut cell_idx = 0;
                        for tr_child in &tbl_child.children {
                            if tr_child.element_type == WordElementType::TableCell {
                                cell_idx += 1;
                                let cell_path = format!("{}/tc[{}]", row_path, cell_idx);

                                let cell_text = extract_cell_text(tr_child);
                                if !cell_text.is_empty() {
                                    map.push_span(&cell_text, &cell_path, "cell");
                                }

                                // Tab separator between cells in same row
                                if cell_idx < count_cells_in_row(&tbl_child.children) {
                                    map.push_span(
                                        "\t",
                                        &format!("{}/tc[{}]/sep", row_path, cell_idx),
                                        "cell-separator",
                                    );
                                }
                            }
                        }

                        // Newline between rows
                        if row_idx < count_rows_in_table(&child.children) {
                            map.push_span(
                                "\n",
                                &format!("{}/tr[{}]/break", tbl_path, row_idx),
                                "row-break",
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(map)
}

/// Extract text from a run element (w:r).
fn extract_run_text(run: &crate::dom_types::WordNode) -> String {
    let mut result = String::new();
    for child in &run.children {
        match child.element_type {
            WordElementType::Text => {
                if let Some(t) = &child.text_content {
                    result.push_str(t);
                }
            }
            WordElementType::Tab => {
                result.push('\t');
            }
            WordElementType::Break => {
                let break_type = child
                    .attributes
                    .get("type")
                    .map(|s| s.as_str())
                    .unwrap_or("");
                if break_type == "page" {
                    result.push('\n');
                } else {
                    result.push('\n');
                }
            }
            _ => {}
        }
    }
    result
}

/// Extract text from a table cell (w:tc).
fn extract_cell_text(cell: &crate::dom_types::WordNode) -> String {
    let mut result = String::new();
    let mut para_count = 0;
    for child in &cell.children {
        if child.element_type == WordElementType::Paragraph {
            if para_count > 0 {
                result.push('\n');
            }
            result.push_str(&child.paragraph_text());
            para_count += 1;
        }
    }
    result
}

/// Count hyperlink elements before the current one in the children list.
fn count_hyperlinks_before(
    children: &[crate::dom_types::WordNode],
    current: &crate::dom_types::WordNode,
) -> usize {
    let mut count = 0;
    for child in children {
        if child.element_type == WordElementType::Hyperlink {
            count += 1;
            if std::ptr::eq(child, current) {
                return count;
            }
        }
    }
    count
}

/// Count cells in a table row.
fn count_cells_in_row(children: &[crate::dom_types::WordNode]) -> usize {
    children
        .iter()
        .filter(|c| c.element_type == WordElementType::TableCell)
        .count()
}

/// Count rows in a table.
fn count_rows_in_table(children: &[crate::dom_types::WordNode]) -> usize {
    children
        .iter()
        .filter(|c| c.element_type == WordElementType::TableRow)
        .count()
}
