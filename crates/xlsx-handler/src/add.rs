/// Add operations for xlsx documents: add cells, rows, sheets.
use crate::dom_types::*;
use crate::helpers;
use handler_common::{HandlerError, InsertPosition};
use oxml::OxmlPackage;
use std::collections::HashMap;

/// Add a new element to the workbook.
/// Supported types:
///   "cell" — add a cell to a sheet (parent = /SheetName, requires "ref" and "value" properties)
///   "sheet" — add a new sheet (parent = /, requires "name" property)
pub fn add_element(
    package: &mut OxmlPackage,
    parent: &str,
    element_type: &str,
    position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    match element_type {
        "cell" => add_cell(package, parent, position, properties),
        "sheet" => add_sheet(package, parent, position, properties),
        _ => Err(HandlerError::UnsupportedType(element_type.to_string())),
    }
}

/// Add a cell to a worksheet.
fn add_cell(
    package: &mut OxmlPackage,
    parent: &str,
    _position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    // Parent should be /SheetName
    let parent_trimmed = parent.trim_start_matches('/');
    let sheet_name = parent_trimmed;

    let ref_str = properties.get("ref").ok_or_else(|| {
        HandlerError::InvalidArgument("cell requires 'ref' property (e.g. ref=B2)".to_string())
    })?;

    let value = properties.get("value").cloned().unwrap_or_default();
    let formula = properties.get("formula").cloned();

    // Validate the cell reference
    let cr = CellRef::parse(ref_str).ok_or_else(|| {
        HandlerError::InvalidArgument(format!("invalid cell reference '{}'", ref_str))
    })?;

    // Find the sheet part path
    let model =
        helpers::build_workbook_model(package).map_err(|e| HandlerError::OperationFailed(e))?;

    let ws = model
        .sheets
        .iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

    let part_path = ws.part_path.clone();

    // Read the worksheet XML
    let xml = package
        .read_part_xml(&part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Determine cell type and value content
    let ss_idx = model.shared_strings.iter().position(|s| s == &value);
    let (t_attr, v_content) = if let Some(idx) = ss_idx {
        ("t=\"s\"".to_string(), idx.to_string())
    } else if value == "TRUE" || value == "FALSE" {
        (
            "t=\"b\"".to_string(),
            if value == "TRUE" {
                "1".to_string()
            } else {
                "0".to_string()
            },
        )
    } else if value.parse::<f64>().is_ok() {
        ("".to_string(), value.clone())
    } else if value.is_empty() && formula.is_none() {
        ("".to_string(), "".to_string())
    } else if !value.is_empty() {
        ("t=\"str\"".to_string(), value.clone())
    } else {
        ("".to_string(), "".to_string())
    };

    // Build the cell XML
    let cell_xml = if let Some(f) = &formula {
        let mut cell = format!("<c r=\"{}\"", ref_str);
        if !t_attr.is_empty() {
            cell.push_str(&format!(" {}", t_attr));
        }
        cell.push_str(&format!("><f>{}</f>", f));
        if !v_content.is_empty() {
            cell.push_str(&format!("<v>{}</v>", v_content));
        }
        cell.push_str("</c>");
        cell
    } else if v_content.is_empty() {
        format!("<c r=\"{}\"/>", ref_str)
    } else {
        let mut cell = format!("<c r=\"{}\"", ref_str);
        if !t_attr.is_empty() {
            cell.push_str(&format!(" {}", t_attr));
        }
        cell.push_str(&format!("><v>{}</v></c>", v_content));
        cell
    };

    // Insert the cell into the sheetData
    let row_num = cr.row;
    let row_pattern = format!("<row r=\"{}\"", row_num);

    let modified_xml = if let Some(row_start) = xml.find(&row_pattern) {
        // Existing row — insert cell at end of row
        // Find end of row opening tag
        let row_gt = xml[row_start..]
            .find('>')
            .map(|pos| row_start + pos + 1)
            .ok_or_else(|| HandlerError::OperationFailed("malformed row element".to_string()))?;

        let mut result = xml[..row_gt].to_string();
        result.push_str(&cell_xml);
        result.push_str(&xml[row_gt..]);
        result
    } else {
        // No existing row — create new row
        let new_row = format!("<row r=\"{}\">{}</row>", row_num, cell_xml);

        // Insert before </sheetData>
        let sd_end = xml
            .find("</sheetData>")
            .ok_or_else(|| HandlerError::OperationFailed("no </sheetData> element".to_string()))?;

        let mut result = xml[..sd_end].to_string();
        result.push_str(&new_row);
        result.push('\n');
        result.push_str(&xml[sd_end..]);
        result
    };

    package
        .write_part_xml(&part_path, &modified_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    Ok(format!("/{}{}", sheet_name, ref_str))
}

/// Add a new sheet to the workbook.
fn add_sheet(
    package: &mut OxmlPackage,
    _parent: &str,
    _position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let name = properties.get("name").ok_or_else(|| {
        HandlerError::InvalidArgument("sheet requires 'name' property".to_string())
    })?;

    let model =
        helpers::build_workbook_model(package).map_err(|e| HandlerError::OperationFailed(e))?;

    // Check for duplicate name
    if model.sheets.iter().any(|s| s.name == *name) {
        return Err(HandlerError::InvalidArgument(format!(
            "sheet '{}' already exists",
            name
        )));
    }

    let new_sheet_index = model.sheets.len() + 1;
    let part_path = format!("xl/worksheets/sheet{}.xml", new_sheet_index);

    // Create minimal worksheet XML
    let sheet_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <sheetData/></worksheet>"
    );

    // Add the new sheet part to the package
    package
        .write_part_xml(&part_path, &sheet_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    // Update workbook.xml to include the new sheet
    let wb_xml = package
        .read_part_xml("xl/workbook.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Find </sheets> and insert before it
    let new_sheet_entry = format!(
        "<sheet name=\"{}\" sheetId=\"{}\" r:id=\"rId{}\"/>",
        name, new_sheet_index, new_sheet_index
    );

    let modified_wb = if let Some(sheets_end) = wb_xml.find("</sheets>") {
        let mut result = wb_xml[..sheets_end].to_string();
        result.push_str(&new_sheet_entry);
        result.push_str(&wb_xml[sheets_end..]);
        result
    } else {
        return Err(HandlerError::OperationFailed(
            "no </sheets> in workbook.xml".to_string(),
        ));
    };

    package
        .write_part_xml("xl/workbook.xml", &modified_wb)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    // Update workbook relationships
    let rels_xml = package
        .read_part_xml("xl/_rels/workbook.xml.rels")
        .map_err(|e| {
            HandlerError::OperationFailed(format!("failed to read workbook rels: {}", e))
        })?;

    let new_rel = format!(
        "<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{}.xml\"/>",
        new_sheet_index, new_sheet_index
    );

    let modified_rels = if let Some(rels_end) = rels_xml.find("</Relationships>") {
        let mut result = rels_xml[..rels_end].to_string();
        result.push_str(&new_rel);
        result.push_str(&rels_xml[rels_end..]);
        result
    } else {
        return Err(HandlerError::OperationFailed(
            "no </Relationships> in workbook rels".to_string(),
        ));
    };

    package
        .write_part_xml("xl/_rels/workbook.xml.rels", &modified_rels)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    Ok(format!("/{}", name))
}
