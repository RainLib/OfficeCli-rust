/// Text offset mapping for xlsx documents.
/// Maps each cell's display text to a path for AI agent navigation.
use crate::dom_types::*;
use crate::helpers;
use handler_common::HandlerError;
use handler_common::TextOffsetMap;
use oxml::OxmlPackage;

/// Build a TextOffsetMap for the workbook.
/// Each cell gets a span: path = "/SheetName/A1", element_type = "cell".
/// Cells are ordered sheet-by-sheet, row-by-row, col-by-col.
pub fn build_text_offset_map_internal(
    package: &OxmlPackage,
) -> Result<TextOffsetMap, HandlerError> {
    let model =
        helpers::build_workbook_model(package).map_err(|e| HandlerError::OperationFailed(e))?;

    let mut map = TextOffsetMap::empty("xlsx");

    for ws in &model.sheets {
        // Sort cells by (row, col) for consistent ordering
        let cell_refs: Vec<&Cell> = ws.cells.values().collect();
        let mut sorted_cells = cell_refs;
        sorted_cells.sort_by(|a, b| (a.row, a.col).cmp(&(b.row, b.col)));

        // Sheet header
        let sheet_header = format!("[{}]\n", ws.name);
        map.push_span(&sheet_header, &format!("/{}", ws.name), "sheet-header");

        for cell in sorted_cells {
            let path = format!("/{}{}", ws.name, cell.ref_str);
            let text = format!("{}: {}\n", cell.ref_str, cell.display_value);

            // Cell content span
            map.push_span(&text, &path, "cell");

            // Formula span (if present)
            if let Some(formula) = &cell.formula {
                let formula_text = format!("  ={}\n", formula);
                map.push_span(&formula_text, &format!("{}:formula", path), "cell-formula");
            }
        }

        // Row separator between sheets
        map.push_span("\n", &format!("/{}", ws.name), "sheet-separator");
    }

    Ok(map)
}
