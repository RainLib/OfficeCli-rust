/// Query operations for xlsx documents.
use crate::dom_types::*;
use crate::helpers;
use handler_common::DocumentNode;
use handler_common::HandlerError;
use oxml::OxmlPackage;

/// Query cells matching a selector pattern.
/// Supported selectors:
///   "sheet=SheetName" — all cells in a sheet
///   "formula" — all cells with formulas
///   "type=sharedString" — all cells of a specific type
///   "range=A1:C10" — cells in a range on the first sheet
///   "Sheet1!A1:C10" — cells in a range on a specific sheet
pub fn query_cells(
    package: &OxmlPackage,
    selector: &str,
) -> Result<Vec<DocumentNode>, HandlerError> {
    let model =
        helpers::build_workbook_model(package).map_err(|e| HandlerError::OperationFailed(e))?;

    let mut results = Vec::new();

    // Parse the selector
    if selector.starts_with("sheet=") {
        // Sheet selector
        let sheet_name = &selector[6..];
        let ws = model
            .sheets
            .iter()
            .find(|s| s.name == sheet_name)
            .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

        for cell in ws.cells.values() {
            results.push(make_cell_node(ws, cell));
        }
    } else if selector == "formula" {
        // All formula cells
        for ws in &model.sheets {
            for cell in ws.cells.values() {
                if cell.formula.is_some() {
                    results.push(make_cell_node(ws, cell));
                }
            }
        }
    } else if selector.starts_with("type=") {
        // Type selector
        let type_name = &selector[5..];
        let target_type = match type_name {
            "number" => CellValueType::Number,
            "sharedString" => CellValueType::SharedString,
            "inlineString" => CellValueType::InlineString,
            "boolean" => CellValueType::Boolean,
            "error" => CellValueType::Error,
            _ => {
                return Err(HandlerError::InvalidArgument(format!(
                    "unknown cell type '{}'",
                    type_name
                )))
            }
        };

        for ws in &model.sheets {
            for cell in ws.cells.values() {
                if cell.value_type == target_type {
                    results.push(make_cell_node(ws, cell));
                }
            }
        }
    } else if selector.contains(':') || selector.contains('!') {
        // Range selector: "A1:C10" or "Sheet1!A1:C10"
        let (sheet_name, range_str) = if selector.contains('!') {
            let idx = selector.find('!').unwrap();
            (&selector[..idx], &selector[idx + 1..])
        } else {
            // Default to first sheet
            (
                model
                    .sheets
                    .first()
                    .map(|s| s.name.as_str())
                    .unwrap_or("Sheet1"),
                selector,
            )
        };

        let ws = model
            .sheets
            .iter()
            .find(|s| s.name == sheet_name)
            .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

        // Parse range: "A1:C10"
        let parts: Vec<&str> = range_str.split(':').collect();
        if parts.len() != 2 {
            return Err(HandlerError::InvalidArgument(format!(
                "invalid range '{}'",
                range_str
            )));
        }

        let start_ref = CellRef::parse(parts[0]).ok_or_else(|| {
            HandlerError::InvalidArgument(format!("invalid cell ref '{}'", parts[0]))
        })?;
        let end_ref = CellRef::parse(parts[1]).ok_or_else(|| {
            HandlerError::InvalidArgument(format!("invalid cell ref '{}'", parts[1]))
        })?;

        for row in start_ref.row..=end_ref.row {
            for col in start_ref.col..=end_ref.col {
                if let Some(cell) = ws.cells.get(&(row, col)) {
                    results.push(make_cell_node(ws, cell));
                }
            }
        }
    } else {
        return Err(HandlerError::InvalidArgument(format!(
            "unsupported selector '{}'",
            selector
        )));
    }

    Ok(results)
}

fn make_cell_node(ws: &Worksheet, cell: &Cell) -> DocumentNode {
    let path = format!("/{}{}", ws.name, cell.ref_str);
    let mut node = DocumentNode::new(&path, "cell").with_text(cell.display_value.clone());

    if let Some(f) = &cell.formula {
        node = node.with_preview(f.clone());
    }

    node
}
