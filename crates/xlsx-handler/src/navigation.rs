/// Path navigation for xlsx documents.
/// Path system:
///   /                  -> workbook root
///   /SheetName          -> sheet root
///   /SheetName/A1       -> cell at column A, row 1
///   /SheetName/A1:value -> cell value property
///   /SheetName/A1:formula -> cell formula property
///   /SheetName/A1:style -> cell style property
use crate::dom_types::*;
use crate::helpers;
use handler_common::DocumentNode;
use handler_common::HandlerError;
use oxml::OxmlPackage;

/// Parse a path string into its components.
pub struct PathComponents {
    /// Sheet name (e.g. "Sheet1")
    pub sheet_name: Option<String>,
    /// Cell reference (e.g. "A1") parsed as CellRef
    pub cell_ref: Option<CellRef>,
    /// Property suffix (e.g. "value", "formula", "style")
    pub property: Option<String>,
}

pub fn parse_path(path: &str) -> Result<PathComponents, HandlerError> {
    // Normalize path: strip leading slashes
    let path = path.trim_start_matches('/');

    if path.is_empty() {
        return Ok(PathComponents {
            sheet_name: None,
            cell_ref: None,
            property: None,
        });
    }

    // Split on : to extract property
    let (main_path, property) = if path.contains(':') {
        let idx = path.find(':').unwrap();
        let prop = &path[idx + 1..];
        // Validate property names
        if !["value", "formula", "style", "type", "ref"].contains(&prop) {
            return Err(HandlerError::InvalidPath(format!(
                "unknown property '{}': path '{}'",
                prop, path
            )));
        }
        (&path[..idx], Some(prop.to_string()))
    } else {
        (path, None)
    };

    // Split main_path into sheet name and cell reference
    // Sheet name consists of the alphabetic/unicode prefix, cell ref is at the end
    // Strategy: find the cell ref pattern (letter(s) followed by digits) at the end
    let cell_ref_end = main_path
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .count();

    if cell_ref_end == 0 {
        // No cell reference — just a sheet name
        return Ok(PathComponents {
            sheet_name: Some(main_path.to_string()),
            cell_ref: None,
            property,
        });
    }

    // Find where the column letters start (immediately before the digits)
    let digits_end_from_right = cell_ref_end;
    let _col_start_from_right = digits_end_from_right;
    let col_letters_len = main_path
        .chars()
        .rev()
        .skip(digits_end_from_right)
        .take_while(|c| c.is_ascii_uppercase())
        .count();

    if col_letters_len == 0 {
        // No column letters before digits — treat as just a sheet name
        return Ok(PathComponents {
            sheet_name: Some(main_path.to_string()),
            cell_ref: None,
            property,
        });
    }

    let cell_ref_len = col_letters_len + digits_end_from_right;
    let total_len = main_path.len();

    if total_len == cell_ref_len {
        // The entire path is just a cell reference (no sheet name)
        // This is invalid — need a sheet name
        let ref_str = main_path;
        let cr = CellRef::parse(ref_str).ok_or_else(|| {
            HandlerError::InvalidPath(format!("invalid cell reference '{}'", ref_str))
        })?;

        return Ok(PathComponents {
            sheet_name: None,
            cell_ref: Some(cr),
            property,
        });
    }

    // Split: sheet_name = prefix, cell_ref = suffix
    // Strip trailing '/' from sheet_name (the / delimiter between sheet and cell)
    let sheet_name = main_path[..total_len - cell_ref_len].trim_end_matches('/');
    let ref_str = &main_path[total_len - cell_ref_len..];

    let cr = CellRef::parse(ref_str).ok_or_else(|| {
        HandlerError::InvalidPath(format!("invalid cell reference '{}'", ref_str))
    })?;

    Ok(PathComponents {
        sheet_name: Some(sheet_name.to_string()),
        cell_ref: Some(cr),
        property,
    })
}

/// Get a node from the workbook model at the given path.
pub fn get_node_at_path(
    package: &OxmlPackage,
    path: &str,
    depth: usize,
) -> Result<DocumentNode, HandlerError> {
    let model =
        helpers::build_workbook_model(package).map_err(|e| HandlerError::OperationFailed(e))?;

    let pc = parse_path(path)?;

    match (pc.sheet_name, pc.cell_ref, pc.property) {
        // Root: / -> workbook
        (None, None, None) => {
            let mut root = DocumentNode::new("/", "workbook");
            let children: Vec<DocumentNode> = model
                .sheets
                .iter()
                .map(|ws| {
                    DocumentNode::new(&format!("/{}", ws.name), "sheet")
                        .with_preview(format!("{} cells", ws.cells.len()))
                        .with_format("maxCol", serde_json::Value::Number(ws.max_col.into()))
                        .with_format("maxRow", serde_json::Value::Number(ws.max_row.into()))
                })
                .collect();
            root = root.with_children(children);
            Ok(root)
        }
        // Sheet root: /SheetName
        (Some(sheet_name), None, None) => {
            let ws = model
                .sheets
                .iter()
                .find(|s| s.name == sheet_name)
                .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

            let mut sheet_node =
                DocumentNode::new(&format!("/{}", ws.name), "sheet").with_preview(format!(
                    "{} cells, cols 1-{}, rows 1-{}",
                    ws.cells.len(),
                    ws.max_col,
                    ws.max_row
                ));

            if depth > 0 {
                // Show cells as children
                let max_row_display = ws.max_row.min(50); // limit display
                let max_col_display = ws.max_col.min(20);
                let cell_nodes: Vec<DocumentNode> = (1..=max_row_display)
                    .flat_map(|row| {
                        (1..=max_col_display).filter_map(move |col| {
                            ws.cells.get(&(row, col)).map(|cell| {
                                DocumentNode::new(&format!("/{}/{}", ws.name, cell.ref_str), "cell")
                                    .with_text(cell.display_value.clone())
                                    .with_preview(cell.formula.clone().unwrap_or_default())
                            })
                        })
                    })
                    .collect();
                sheet_node = sheet_node.with_children(cell_nodes);
            }

            Ok(sheet_node)
        }
        // Cell: /SheetName/A1
        (Some(sheet_name), Some(cell_ref), None) => {
            let ws = model
                .sheets
                .iter()
                .find(|s| s.name == sheet_name)
                .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

            let cell = ws.cells.get(&(cell_ref.row, cell_ref.col)).ok_or_else(|| {
                HandlerError::PathNotFound(format!(
                    "cell {}{}",
                    sheet_name,
                    cell_ref.to_string_ref()
                ))
            })?;

            let mut cell_node =
                DocumentNode::new(&format!("/{}/{}", ws.name, cell.ref_str), "cell")
                    .with_text(cell.display_value.clone())
                    .with_preview(cell.formula.clone().unwrap_or_default())
                    .with_format(
                        "value",
                        serde_json::Value::String(cell.raw_value.clone().unwrap_or_default()),
                    )
                    .with_format(
                        "formula",
                        serde_json::Value::String(cell.formula.clone().unwrap_or_default()),
                    )
                    .with_format(
                        "type",
                        serde_json::Value::String(cell_type_label(&cell.value_type).to_string()),
                    )
                    .with_format("ref", serde_json::Value::String(cell.ref_str.clone()));

            if let Some(si) = cell.style_index {
                cell_node =
                    cell_node.with_format("styleIndex", serde_json::Value::Number(si.into()));
            }

            Ok(cell_node)
        }
        // Cell property: /SheetName/A1:value etc.
        (Some(sheet_name), Some(cell_ref), Some(prop)) => {
            let ws = model
                .sheets
                .iter()
                .find(|s| s.name == sheet_name)
                .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

            let cell = ws.cells.get(&(cell_ref.row, cell_ref.col)).ok_or_else(|| {
                HandlerError::PathNotFound(format!(
                    "cell {}{}",
                    sheet_name,
                    cell_ref.to_string_ref()
                ))
            })?;

            let path_str = format!("/{}{}:{}", ws.name, cell.ref_str, prop);
            let value = match prop.as_str() {
                "value" => cell.display_value.clone(),
                "formula" => cell.formula.clone().unwrap_or_default(),
                "style" => cell.style_index.map(|s| s.to_string()).unwrap_or_default(),
                "type" => cell_type_label(&cell.value_type).to_string(),
                "ref" => cell.ref_str.clone(),
                _ => return Err(HandlerError::UnsupportedProperty(prop)),
            };

            Ok(DocumentNode::new(&path_str, "property").with_text(value))
        }
        // Cell without sheet name is ambiguous
        (None, Some(cell_ref), prop) => Err(HandlerError::InvalidPath(format!(
            "cell reference {} requires a sheet name (e.g. /Sheet1/{}{})",
            cell_ref.to_string_ref(),
            cell_ref.to_string_ref(),
            prop.map(|p| format!(":{}", p)).unwrap_or_default()
        ))),
        // Property without cell reference is invalid
        (Some(sheet_name), None, Some(prop)) => Err(HandlerError::InvalidPath(format!(
            "property ':{}' requires a cell reference (e.g. /{}A1:{})",
            prop, sheet_name, prop
        ))),
        // Property at root level (no sheet or cell) is invalid
        (None, None, Some(prop)) => Err(HandlerError::InvalidPath(format!(
            "property ':{}' requires a path like /Sheet1/A1:{}",
            prop, prop
        ))),
    }
}

fn cell_type_label(vt: &CellValueType) -> &'static str {
    match vt {
        CellValueType::Number => "number",
        CellValueType::SharedString => "sharedString",
        CellValueType::InlineString => "inlineString",
        CellValueType::Boolean => "boolean",
        CellValueType::Error => "error",
    }
}
