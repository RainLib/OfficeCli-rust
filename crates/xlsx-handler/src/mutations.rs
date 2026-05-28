/// Mutation operations for xlsx documents: set cell values, formulas, remove, move, copy.
use crate::dom_types::*;
use crate::helpers;
use crate::navigation;
use handler_common::HandlerError;
use oxml::OxmlPackage;
use std::collections::HashMap;

/// Remove an element from the workbook.
/// Supported paths:
///   /SheetName/A1 — remove a cell (clear its content from the worksheet XML)
///   /SheetName     — remove a sheet (remove part + update workbook.xml)
pub fn remove_element(
    package: &mut OxmlPackage,
    path: &str,
) -> Result<Option<String>, HandlerError> {
    let pc = navigation::parse_path(path)?;

    match (pc.sheet_name, pc.cell_ref) {
        (Some(sheet_name), Some(cell_ref)) => {
            remove_cell(package, &sheet_name, &cell_ref)?;
            Ok(Some(format!("removed cell {}{}", sheet_name, cell_ref.to_string_ref())))
        }
        (Some(sheet_name), None) => {
            remove_sheet(package, &sheet_name)?;
            Ok(Some(format!("removed sheet {}", sheet_name)))
        }
        (None, None) => Err(HandlerError::InvalidPath("remove requires a sheet or cell path".to_string())),
        (None, Some(_)) => Err(HandlerError::InvalidPath("cell path requires a sheet name".to_string())),
    }
}

/// Remove a cell from a worksheet by finding and deleting its <c> element.
fn remove_cell(
    package: &mut OxmlPackage,
    sheet_name: &str,
    cell_ref: &CellRef,
) -> Result<(), HandlerError> {
    let model = helpers::build_workbook_model(package)
        .map_err(|e| HandlerError::OperationFailed(e))?;

    let ws = model.sheets.iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

    let part_path = ws.part_path.clone();
    let ref_str = cell_ref.to_string_ref();

    let xml = package.read_part_xml(&part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let p = detect_namespace_prefix(&xml);

    // Find the cell element
    let cell_pattern = format!("<{}c r=\"{}\"", p, ref_str);
    if let Some(cell_start) = xml.find(&cell_pattern) {
        let cell_end = find_cell_element_end(&xml, cell_start, &p)?;
        let mut result = xml[..cell_start].to_string();
        result.push_str(&xml[cell_end..]);
        package.write_part_xml(&part_path, &result)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    }

    Ok(())
}

/// Remove a sheet from the workbook.
fn remove_sheet(
    package: &mut OxmlPackage,
    sheet_name: &str,
) -> Result<(), HandlerError> {
    let model = helpers::build_workbook_model(package)
        .map_err(|e| HandlerError::OperationFailed(e))?;

    let ws = model.sheets.iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

    // Remove the sheet part from the package
    if package.has_part(&ws.part_path) {
        package.write_part(&ws.part_path, Vec::<u8>::new())
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    }

    // Remove the <sheet> entry from workbook.xml
    let wb_xml = package.read_part_xml("xl/workbook.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Find the sheet entry by name
    let sheet_entry_pattern = format!("name=\"{}\"", sheet_name);
    if let Some(name_pos) = wb_xml.find(&sheet_entry_pattern) {
        // Find the <sheet .../> element containing this name
        let element_start = wb_xml[..name_pos].rfind("<sheet")
            .unwrap_or(0);
        let element_end = find_element_end(&wb_xml, element_start, "sheet");
        let mut result = wb_xml[..element_start].to_string();
        result.push_str(&wb_xml[element_end..]);

        package.write_part_xml("xl/workbook.xml", &result)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    }

    Ok(())
}

/// Move a cell's content from source to target.
/// Source: /SheetName/A1, Target: /SheetName/B1 (or different sheet)
pub fn move_cell(
    package: &mut OxmlPackage,
    source: &str,
    target_parent: Option<&str>,
) -> Result<String, HandlerError> {
    let source_pc = navigation::parse_path(source)?;

    let sheet_name = source_pc.sheet_name
        .ok_or_else(|| HandlerError::InvalidPath("move source requires a sheet name".to_string()))?;
    let source_ref = source_pc.cell_ref
        .ok_or_else(|| HandlerError::InvalidPath("move source requires a cell reference".to_string()))?;

    // Determine target
    let target_path = target_parent.unwrap_or("/");
    let target_pc = navigation::parse_path(target_path)?;

    let target_sheet = target_pc.sheet_name.unwrap_or(sheet_name.clone());
    let target_ref = target_pc.cell_ref
        .ok_or_else(|| HandlerError::InvalidPath("move target requires a cell reference".to_string()))?;

    // 1. Copy cell content to target
    let model = helpers::build_workbook_model(package)
        .map_err(|e| HandlerError::OperationFailed(e))?;

    let src_ws = model.sheets.iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

    let src_cell = src_ws.cells.get(&(source_ref.row, source_ref.col))
        .ok_or_else(|| HandlerError::PathNotFound(format!("cell {}{}", sheet_name, source_ref.to_string_ref())))?;

    // Build properties from source cell
    let mut props = HashMap::new();
    if let Some(v) = &src_cell.raw_value {
        props.insert("value".to_string(), v.clone());
    }
    if let Some(f) = &src_cell.formula {
        props.insert("formula".to_string(), f.clone());
    }
    if let Some(si) = src_cell.style_index {
        props.insert("style".to_string(), si.to_string());
    }

    // Set the target cell
    let target_path_str = format!("/{}/{}", target_sheet, target_ref.to_string_ref());
    set_cell_properties(package, &target_path_str, &props)?;

    // 2. Remove the source cell
    remove_cell(package, &sheet_name, &source_ref)?;

    Ok(target_path_str)
}

/// Copy a cell's content from source to target (keeping source intact).
pub fn copy_cell(
    package: &mut OxmlPackage,
    source: &str,
    target_parent: &str,
) -> Result<String, HandlerError> {
    let source_pc = navigation::parse_path(source)?;

    let sheet_name = source_pc.sheet_name
        .ok_or_else(|| HandlerError::InvalidPath("copy source requires a sheet name".to_string()))?;
    let source_ref = source_pc.cell_ref
        .ok_or_else(|| HandlerError::InvalidPath("copy source requires a cell reference".to_string()))?;

    let target_pc = navigation::parse_path(target_parent)?;

    let target_sheet = target_pc.sheet_name.unwrap_or(sheet_name.clone());
    let target_ref = target_pc.cell_ref
        .ok_or_else(|| HandlerError::InvalidPath("copy target requires a cell reference".to_string()))?;

    let model = helpers::build_workbook_model(package)
        .map_err(|e| HandlerError::OperationFailed(e))?;

    let src_ws = model.sheets.iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

    let src_cell = src_ws.cells.get(&(source_ref.row, source_ref.col))
        .ok_or_else(|| HandlerError::PathNotFound(format!("cell {}{}", sheet_name, source_ref.to_string_ref())))?;

    let mut props = HashMap::new();
    if let Some(v) = &src_cell.raw_value {
        props.insert("value".to_string(), v.clone());
    }
    if let Some(f) = &src_cell.formula {
        props.insert("formula".to_string(), f.clone());
    }
    if let Some(si) = src_cell.style_index {
        props.insert("style".to_string(), si.to_string());
    }

    let target_path_str = format!("/{}/{}", target_sheet, target_ref.to_string_ref());
    set_cell_properties(package, &target_path_str, &props)?;

    Ok(target_path_str)
}

/// Find the end position of an XML element (handles both self-closing and regular closing tags).
fn find_element_end(xml: &str, start: usize, tag: &str) -> usize {
    // Check if self-closing: look for /> before >
    let first_gt = xml[start..].find('>').map(|pos| start + pos).unwrap_or(xml.len());

    if first_gt > 0 && xml.as_bytes().get(first_gt - 1) == Some(&b'/') {
        // Self-closing element: <tag .../>
        first_gt + 1
    } else {
        // Regular element: find </tag>
        let close_tag = format!("</{}>", tag);
        xml[first_gt..].find(&close_tag)
            .map(|pos| first_gt + pos + close_tag.len())
            .unwrap_or(xml.len())
    }
}

/// Set properties on a cell identified by path like /Sheet1/A1.
pub fn set_cell_properties(
    package: &mut OxmlPackage,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let pc = navigation::parse_path(path)?;

    // Need both sheet name and cell reference for set operations
    let sheet_name = pc.sheet_name
        .ok_or_else(|| HandlerError::InvalidPath("set requires a sheet name in the path".to_string()))?;
    let cell_ref = pc.cell_ref
        .ok_or_else(|| HandlerError::InvalidPath("set requires a cell reference (e.g. /Sheet1/A1)".to_string()))?;

    // Parse the model to find the sheet part path
    let model = helpers::build_workbook_model(package)
        .map_err(|e| HandlerError::OperationFailed(e))?;

    let ws = model.sheets.iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

    let part_path = ws.part_path.clone();
    let cell_ref_str = cell_ref.to_string_ref();

    // Read the current worksheet XML
    let xml = package.read_part_xml(&part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let p = detect_namespace_prefix(&xml);

    let mut modified_xml = xml.clone();
    let mut unsupported = Vec::new();

    for (key, value) in properties {
        match key.as_str() {
            "value" => {
                modified_xml = set_cell_value(&modified_xml, &cell_ref_str, value, &model.shared_strings, &p)?;
            }
            "formula" => {
                modified_xml = set_cell_formula(&modified_xml, &cell_ref_str, value, &p)?;
            }
            "style" => {
                modified_xml = set_cell_style(&modified_xml, &cell_ref_str, value, &p)?;
            }
            _ => {
                unsupported.push(key.clone());
            }
        }
    }

    // Write back the modified XML
    package.write_part_xml(&part_path, &modified_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    Ok(unsupported)
}

/// Helper to detect namespacing prefix (e.g., "x:") used in worksheet XML.
fn detect_namespace_prefix(xml: &str) -> String {
    if let Some(pos) = xml.find("worksheet") {
        if let Some(lt_pos) = xml[..pos].rfind('<') {
            let prefix = &xml[lt_pos + 1..pos];
            if !prefix.is_empty() && prefix.ends_with(':') {
                return prefix.to_string();
            }
        }
    }
    "".to_string()
}

/// Set the value of a cell in the worksheet XML.
/// If the cell exists, update its <v> element. If not, insert a new <c> element.
fn set_cell_value(xml: &str, cell_ref: &str, value: &str, shared_strings: &[String], p: &str) -> Result<String, HandlerError> {
    // Check if the value matches an existing shared string
    let ss_idx = shared_strings.iter().position(|s| s == value);

    let (t_attr, v_content) = if let Some(idx) = ss_idx {
        // Use shared string reference
        ("t=\"s\"".to_string(), idx.to_string())
    } else if value == "TRUE" || value == "FALSE" {
        // Boolean
        ("t=\"b\"".to_string(), if value == "TRUE" { "1".to_string() } else { "0".to_string() })
    } else if value.parse::<f64>().is_ok() {
        // Numeric
        ("".to_string(), value.to_string())
    } else {
        // Inline string
        ("t=\"str\"".to_string(), value.to_string())
    };

    // Try to find and update existing cell
    let cell_pattern = format!("<{}c r=\"{}\"", p, cell_ref);
    if let Some(cell_start) = xml.find(&cell_pattern) {
        // Find the end of this <c> element
        let cell_end = find_cell_element_end(xml, cell_start, p)?;

        let cell_xml = &xml[cell_start..cell_end];

        // Build new cell XML
        let new_cell = build_cell_xml(cell_ref, &t_attr, &v_content, None, &extract_existing_style(cell_xml), p);

        let mut result = xml[..cell_start].to_string();
        result.push_str(&new_cell);
        result.push_str(&xml[cell_end..]);
        Ok(result)
    } else {
        // Insert new cell — find the <sheetData> element and insert
        insert_new_cell(xml, cell_ref, &t_attr, &v_content, None, "", p)
    }
}

/// Set the formula of a cell.
fn set_cell_formula(xml: &str, cell_ref: &str, formula: &str, p: &str) -> Result<String, HandlerError> {
    let cell_pattern = format!("<{}c r=\"{}\"", p, cell_ref);
    if let Some(cell_start) = xml.find(&cell_pattern) {
        let cell_end = find_cell_element_end(xml, cell_start, p)?;

        let cell_xml = &xml[cell_start..cell_end];
        let existing_style = extract_existing_style(cell_xml);
        let existing_type = extract_existing_type(cell_xml);
        let existing_value = extract_existing_value(cell_xml, p);

        // Formula cells: type should be empty (calculated) or "str" if result is inline string
        let new_cell = build_cell_xml(
            cell_ref,
            &existing_type,
            &existing_value,
            Some(formula),
            &existing_style,
            p,
        );

        let mut result = xml[..cell_start].to_string();
        result.push_str(&new_cell);
        result.push_str(&xml[cell_end..]);
        Ok(result)
    } else {
        // Insert new cell with formula (type defaults to calculated)
        insert_new_cell(xml, cell_ref, "", "", Some(formula), "", p)
    }
}

/// Set the style index of a cell.
fn set_cell_style(xml: &str, cell_ref: &str, style_index: &str, p: &str) -> Result<String, HandlerError> {
    let cell_pattern = format!("<{}c r=\"{}\"", p, cell_ref);
    if let Some(cell_start) = xml.find(&cell_pattern) {
        let cell_end = find_cell_element_end(xml, cell_start, p)?;
        let cell_xml = &xml[cell_start..cell_end];

        // Modify the s= attribute in the cell opening tag
        let new_cell_xml = modify_style_in_cell(cell_xml, style_index);

        let mut result = xml[..cell_start].to_string();
        result.push_str(&new_cell_xml);
        result.push_str(&xml[cell_end..]);
        Ok(result)
    } else {
        Err(HandlerError::PathNotFound(format!("cell {}", cell_ref)))
    }
}

/// Build a complete <c> element XML string.
fn build_cell_xml(
    ref_str: &str,
    t_attr: &str,
    v_content: &str,
    formula: Option<&str>,
    style_attr: &str,
    p: &str,
) -> String {
    let mut attrs = format!("r=\"{}\"", ref_str);
    if !t_attr.is_empty() {
        attrs.push_str(&format!(" {}", t_attr));
    }
    if !style_attr.is_empty() {
        attrs.push_str(&format!(" {}", style_attr));
    }

    if formula.is_none() && v_content.is_empty() {
        // Empty cell — self-closing
        return format!("<{}c {}/>", p, attrs);
    }

    let mut cell = format!("<{}c {}>", p, attrs);

    if let Some(f) = formula {
        cell.push_str(&format!("<{}f>{}</{}f>", p, f, p));
    }

    if !v_content.is_empty() {
        cell.push_str(&format!("<{}v>{}</{}v>", p, v_content, p));
    }

    cell.push_str(&format!("</{}c>", p));
    cell
}

/// Find the end position of a <c> element in XML text.
/// Handles both self-closing <c .../> and regular <c ...>...</c>.
fn find_cell_element_end(xml: &str, start: usize, p: &str) -> Result<usize, HandlerError> {
    // Check for regular closing tag — need to find matching </c>
    // Look for the next '>' after start to see if self-closing or not
    let first_gt = xml[start..].find('>')
        .map(|pos| start + pos)
        .ok_or_else(|| HandlerError::OperationFailed("malformed XML: no '>' in cell tag".to_string()))?;

    // Check if the character before '>' is '/' (self-closing)
    if xml.as_bytes().get(first_gt - 1) == Some(&b'/') {
        // Self-closing: end is at first_gt + 1
        Ok(first_gt + 1)
    } else {
        // Regular element: find </c>
        let close_tag = format!("</{}c>", p);
        let close_tag_pos = xml[first_gt..].find(&close_tag)
            .map(|pos| first_gt + pos + close_tag.len())
            .ok_or_else(|| HandlerError::OperationFailed(format!("malformed XML: no '{}' closing tag", close_tag)))?;
        Ok(close_tag_pos)
    }
}

/// Extract the s= attribute from an existing cell XML element.
fn extract_existing_style(cell_xml: &str) -> String {
    // Look for s="N" in the opening tag
    let s_pattern = "s=\"";
    if let Some(s_start) = cell_xml.find(s_pattern) {
        let val_start = s_start + s_pattern.len();
        if let Some(val_end) = cell_xml[val_start..].find('"') {
            return format!("s=\"{}\"", &cell_xml[val_start..val_start + val_end]);
        }
    }
    "".to_string()
}

/// Extract the t= attribute from an existing cell XML element.
fn extract_existing_type(cell_xml: &str) -> String {
    let t_pattern = "t=\"";
    if let Some(t_start) = cell_xml.find(t_pattern) {
        let val_start = t_start + t_pattern.len();
        if let Some(val_end) = cell_xml[val_start..].find('"') {
            return format!("t=\"{}\"", &cell_xml[val_start..val_start + val_end]);
        }
    }
    "".to_string()
}

/// Extract the value from the <v> element in an existing cell.
fn extract_existing_value(cell_xml: &str, p: &str) -> String {
    let v_start_pattern = format!("<{}v>", p);
    if let Some(v_start) = cell_xml.find(&v_start_pattern) {
        let content_start = v_start + v_start_pattern.len();
        let v_end_pattern = format!("</{}v>", p);
        if let Some(v_end) = cell_xml.find(&v_end_pattern) {
            if v_end > content_start {
                return cell_xml[content_start..v_end].to_string();
            }
        }
    }
    "".to_string()
}

/// Modify the s= attribute in a cell element's XML.
fn modify_style_in_cell(cell_xml: &str, new_style: &str) -> String {
    let s_pattern = "s=\"";
    if let Some(s_start) = cell_xml.find(s_pattern) {
        let val_start = s_start + s_pattern.len();
        if let Some(val_end) = cell_xml[val_start..].find('"') {
            let full_val_end = val_start + val_end;
            let mut result = cell_xml[..s_start].to_string();
            result.push_str(&format!("s=\"{}\"", new_style));
            result.push_str(&cell_xml[full_val_end + 1..]);
            return result;
        }
    }
    // No existing style — insert s= attribute into the opening tag
    // Find the first > or /> and insert before it
    let insert_pos = cell_xml.find("/>")
        .or_else(|| cell_xml.find('>'))
        .unwrap_or(cell_xml.len());
    let mut result = cell_xml[..insert_pos].to_string();
    result.push_str(&format!(" s=\"{}\"", new_style));
    result.push_str(&cell_xml[insert_pos..]);
    result
}

/// Insert a new <c> element into the <sheetData> section.
fn insert_new_cell(
    xml: &str,
    ref_str: &str,
    t_attr: &str,
    v_content: &str,
    formula: Option<&str>,
    style_attr: &str,
    p: &str,
) -> Result<String, HandlerError> {
    let new_cell = build_cell_xml(ref_str, t_attr, v_content, formula, style_attr, p);

    // Find <sheetData> opening tag
    let sd_start = xml.find(&format!("<{}sheetData", p))
        .ok_or_else(|| HandlerError::OperationFailed(format!("no <{}sheetData> element found", p)))?;

    // Find the first <row> inside sheetData, or the closing </sheetData>
    let after_sd = &xml[sd_start..];
    let sd_gt = after_sd.find('>')
        .map(|pos| sd_start + pos + 1)
        .ok_or_else(|| HandlerError::OperationFailed(format!("malformed <{}sheetData>", p)))?;

    // Determine the row number from the cell reference
    let row_num = CellRef::parse(ref_str)
        .ok_or_else(|| HandlerError::InvalidPath(format!("invalid cell ref '{}'", ref_str)))?
        .row;

    // Try to find the matching <row r="N"> element
    let row_pattern = format!("<{}row r=\"{}\"", p, row_num);
    if let Some(row_start) = xml[sd_gt..].find(&row_pattern) {
        let abs_row_start = sd_gt + row_start;

        // Find end of row element
        let row_gt = xml[abs_row_start..].find('>')
            .map(|pos| abs_row_start + pos + 1)
            .ok_or_else(|| HandlerError::OperationFailed(format!("malformed <{}row>", p)))?;

        // Insert cell after the row opening tag
        let mut result = xml[..row_gt].to_string();
        result.push_str(&new_cell);
        result.push_str(&xml[row_gt..]);
        Ok(result)
    } else {
        // No existing row — insert a new <row> with the cell
        let new_row = format!("<{}row r=\"{}\">{}  </{}row>", p, row_num, new_cell, p);

        // Insert before </sheetData>
        let sd_end_pattern = format!("</{}sheetData>", p);
        let sd_end = xml.find(&sd_end_pattern)
            .ok_or_else(|| HandlerError::OperationFailed(format!("no {} closing tag", sd_end_pattern)))?;

        let mut result = xml[..sd_end].to_string();
        result.push_str(&new_row);
        result.push('\n');
        result.push_str(&xml[sd_end..]);
        Ok(result)
    }
}