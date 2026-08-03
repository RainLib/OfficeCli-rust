/// Add operations for xlsx documents: add cells, rows, sheets.
use crate::dom_types::*;
use crate::formula;
use crate::helpers;
use crate::rich_value_image;
use handler_common::{HandlerError, InsertPosition};
use oxml::OxmlPackage;
use std::collections::HashMap;

/// Add a new element to the workbook.
/// Supported types (expanded vocabulary matching C# ExcelHandler.Add):
///   cell — add a cell to a sheet (parent = /SheetName, requires "ref" and "value")
///   sheet — add a new sheet (parent = /, requires "name")
///   row — add a row of cells (requires "row" index or uses "ref" as anchor)
///   column — add a column of cells
///   table — create a defined Excel Table (ListObject) over a range
///   chart — add a chart (bar/column/line/pie) embedded via drawing+graphicFrame
///   conditionalFormat | conditional-format | cf — add a conditional format rule
///   dataValidation | validation — add a data validation rule
///   hyperLink | hyperlink — add a hyperlink to a cell
///   image | picture — add an embedded image via drawing anchor
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
        "row" => add_row(package, parent, position, properties),
        "column" | "col" => add_column(package, parent, position, properties),
        "table" => add_table(package, parent, position, properties),
        "chart" => add_chart_real(package, parent, properties),
        "conditionalFormat" | "conditional-format" | "cf" => {
            add_conditional_format(package, parent, position, properties)
        }
        "dataValidation" | "validation" => {
            add_data_validation(package, parent, position, properties)
        }
        "hyperlink" => add_hyperlink(package, parent, position, properties),
        "image" | "picture" => add_image_real(package, parent, properties),
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
    let formula = properties
        .get("formula")
        .map(|value| formula::qualify_for_ooxml(value))
        .transpose()
        .map_err(HandlerError::InvalidArgument)?;
    let image_vm = properties
        .get("image")
        .filter(|source| !source.is_empty() && !source.eq_ignore_ascii_case("none"))
        .map(|source| {
            let alt = properties
                .get("alt")
                .or_else(|| properties.get("altText"))
                .or_else(|| properties.get("alttext"))
                .or_else(|| properties.get("description"))
                .or_else(|| properties.get("image.alt"));
            rich_value_image::add_image(package, source, alt.map(String::as_str))
        })
        .transpose()?;

    // Validate the cell reference
    let cr = CellRef::parse(ref_str).ok_or_else(|| {
        HandlerError::InvalidArgument(format!("invalid cell reference '{}'", ref_str))
    })?;

    // Find the sheet part path
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

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
    let cell_xml = if let Some(vm) = image_vm {
        format!(
            "<c r=\"{}\" t=\"e\" vm=\"{}\"><v>#VALUE!</v></c>",
            ref_str, vm
        )
    } else if let Some(f) = &formula {
        let mut cell = format!("<c r=\"{}\"", ref_str);
        if !t_attr.is_empty() {
            cell.push_str(&format!(" {}", t_attr));
        }
        let spill = if formula::is_dynamic_array_formula(f) {
            format!(" t=\"array\" ref=\"{}\"", ref_str)
        } else {
            String::new()
        };
        cell.push_str(&format!("><f{spill}>{}</f>", escape_xml(f)));
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
    position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let name = properties.get("name").ok_or_else(|| {
        HandlerError::InvalidArgument("sheet requires 'name' property".to_string())
    })?;

    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    // Check for duplicate name
    if model.sheets.iter().any(|s| s.name == *name) {
        return Err(HandlerError::InvalidArgument(format!(
            "sheet '{}' already exists",
            name
        )));
    }

    let workbook_xml = package
        .read_part_xml("xl/workbook.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let workbook_rels_path = "xl/_rels/workbook.xml.rels";
    let workbook_rels = package.read_part_xml(workbook_rels_path).map_err(|e| {
        HandlerError::OperationFailed(format!("failed to read workbook rels: {}", e))
    })?;
    let entries = crate::mutations::workbook_sheet_entries(&workbook_xml)?;
    let insert_index = resolve_sheet_insert_index(&entries, &position)?;
    let part_number = next_worksheet_part_number(package);
    let sheet_id = next_workbook_sheet_id(&workbook_xml)?;
    let relationship_id = next_workbook_relationship_id(&workbook_rels)?;
    let part_path = format!("xl/worksheets/sheet{}.xml", part_number);

    // Create minimal worksheet XML
    let sheet_xml = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
         <sheetData/></worksheet>"
        .to_string();

    // Add the new sheet part to the package
    package
        .write_part_xml(&part_path, &sheet_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    let new_sheet_entry = format!(
        "<sheet name=\"{}\" sheetId=\"{}\" r:id=\"{}\"/>",
        escape_xml_attribute(name),
        sheet_id,
        relationship_id
    );
    let shifted_workbook = crate::mutations::rewrite_defined_name_scopes(&workbook_xml, |scope| {
        if scope as usize >= insert_index {
            Some(scope + 1)
        } else {
            Some(scope)
        }
    })?;
    let modified_workbook = insert_sheet_entry(&shifted_workbook, insert_index, &new_sheet_entry)?;

    let new_rel = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{}.xml\"/>",
        relationship_id, part_number
    );
    let modified_rels = if let Some(rels_end) = workbook_rels.find("</Relationships>") {
        let mut result = workbook_rels[..rels_end].to_string();
        result.push_str(&new_rel);
        result.push_str(&workbook_rels[rels_end..]);
        result
    } else {
        return Err(HandlerError::OperationFailed(
            "no </Relationships> in workbook rels".to_string(),
        ));
    };

    let content_types_path = "[Content_Types].xml";
    let content_types = package
        .read_part_xml(content_types_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let modified_content_types = register_worksheet_content_type(&content_types, &part_path)?;

    package
        .write_part_xml("xl/workbook.xml", &modified_workbook)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    package
        .write_part_xml(workbook_rels_path, &modified_rels)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    package
        .write_part_xml(content_types_path, &modified_content_types)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    Ok(format!("/{}", name))
}

fn resolve_sheet_insert_index(
    entries: &[crate::mutations::WorkbookSheetEntry],
    position: &InsertPosition,
) -> Result<usize, HandlerError> {
    let anchor_name = |path: &str| path.trim().trim_start_matches('/').to_string();
    match position {
        InsertPosition::AtIndex(index) => Ok((*index).min(entries.len())),
        InsertPosition::BeforeElement(anchor) => {
            let anchor = anchor_name(anchor);
            entries
                .iter()
                .position(|entry| entry.name == anchor)
                .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", anchor)))
        }
        InsertPosition::AfterElement(anchor) => {
            let anchor = anchor_name(anchor);
            entries
                .iter()
                .position(|entry| entry.name == anchor)
                .map(|index| index + 1)
                .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", anchor)))
        }
        InsertPosition::Append => Ok(entries.len()),
    }
}

fn insert_sheet_entry(
    workbook_xml: &str,
    insert_index: usize,
    entry_xml: &str,
) -> Result<String, HandlerError> {
    let entries = crate::mutations::workbook_sheet_entries(workbook_xml)?;
    let insert_at = if insert_index < entries.len() {
        entries[insert_index].range.start
    } else {
        let doc = roxmltree::Document::parse(workbook_xml)
            .map_err(|e| HandlerError::OperationFailed(format!("invalid workbook.xml: {}", e)))?;
        let sheets = doc
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "sheets")
            .ok_or_else(|| {
                HandlerError::OperationFailed("workbook has no sheets list".to_string())
            })?;
        let range = sheets.range();
        let container = &workbook_xml[range.clone()];
        range.start
            + container
                .rfind("</")
                .ok_or_else(|| HandlerError::OperationFailed("malformed sheets list".to_string()))?
    };
    let mut result = workbook_xml.to_string();
    result.insert_str(insert_at, entry_xml);
    Ok(result)
}

fn next_worksheet_part_number(package: &OxmlPackage) -> usize {
    package
        .list_parts()
        .into_iter()
        .filter_map(|path| {
            path.strip_prefix("xl/worksheets/sheet")
                .and_then(|value| value.strip_suffix(".xml"))
                .and_then(|value| value.parse::<usize>().ok())
        })
        .max()
        .unwrap_or(0)
        + 1
}

fn next_workbook_sheet_id(workbook_xml: &str) -> Result<u64, HandlerError> {
    let doc = roxmltree::Document::parse(workbook_xml)
        .map_err(|e| HandlerError::OperationFailed(format!("invalid workbook.xml: {}", e)))?;
    Ok(doc
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "sheet")
        .filter_map(|node| node.attribute("sheetId"))
        .filter_map(|value| value.parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        + 1)
}

fn next_workbook_relationship_id(rels_xml: &str) -> Result<String, HandlerError> {
    let doc = roxmltree::Document::parse(rels_xml).map_err(|e| {
        HandlerError::OperationFailed(format!("invalid workbook relationships: {}", e))
    })?;
    let next = doc
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "Relationship")
        .filter_map(|node| node.attribute("Id"))
        .filter_map(|value| value.strip_prefix("rId"))
        .filter_map(|value| value.parse::<usize>().ok())
        .max()
        .unwrap_or(0)
        + 1;
    Ok(format!("rId{}", next))
}

fn register_worksheet_content_type(xml: &str, part_path: &str) -> Result<String, HandlerError> {
    let part_name = format!("/{}", part_path.trim_start_matches('/'));
    if xml.contains(&format!("PartName=\"{}\"", part_name)) {
        return Ok(xml.to_string());
    }
    let entry = format!(
        "<Override PartName=\"{}\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>",
        part_name
    );
    let close = xml.find("</Types>").ok_or_else(|| {
        HandlerError::OperationFailed("invalid [Content_Types].xml: missing </Types>".to_string())
    })?;
    let mut result = xml.to_string();
    result.insert_str(close, &entry);
    Ok(result)
}

fn escape_xml_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ─── New Element Types ─────────────────────────────────────────────────

/// Add a row of cells. Uses "row" property for the row index and either a
/// comma-separated "values" list or numbered r1c1, r1c2, ... properties.
fn add_row(
    package: &mut OxmlPackage,
    parent: &str,
    _position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let parent_trimmed = parent.trim_start_matches('/');
    let sheet_name = parent_trimmed;

    let row_idx: usize = properties
        .get("row")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| {
            HandlerError::InvalidArgument(
                "row add requires 'row' property (1-based row number)".to_string(),
            )
        })?;

    // Find the sheet part path
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;
    let ws = model
        .sheets
        .iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;
    let part_path = ws.part_path.clone();

    // Try values="A,B,C" first; fall back to r1c1, r1c2, ... properties.
    if let Some(values_csv) = properties.get("values") {
        for (col_idx, value) in values_csv.split(',').enumerate() {
            let col_letter = col_index_to_letter(col_idx + 1);
            let cell_ref = format!("{}{}", col_letter, row_idx);
            let mut cell_props = HashMap::new();
            cell_props.insert("ref".to_string(), cell_ref);
            cell_props.insert("value".to_string(), value.trim().to_string());
            add_cell(package, parent, InsertPosition::Append, &cell_props)?;
        }
    } else {
        // Look for r1c1, r1c2, ... properties matching the row index
        for col_idx in 1..=256 {
            let key = format!("r{}c{}", row_idx, col_idx);
            if let Some(value) = properties.get(&key) {
                let col_letter = col_index_to_letter(col_idx);
                let cell_ref = format!("{}{}", col_letter, row_idx);
                let mut cell_props = HashMap::new();
                cell_props.insert("ref".to_string(), cell_ref);
                cell_props.insert("value".to_string(), value.clone());
                add_cell(package, parent, InsertPosition::Append, &cell_props)?;
            } else {
                break; // Stop at first missing column
            }
        }
    }

    let _ = part_path; // Part path used implicitly via add_cell
    Ok(format!("/{}/row[{}]", sheet_name, row_idx))
}

/// Convert a 1-based column index to a letter (1 → "A", 27 → "AA").
fn col_index_to_letter(idx: usize) -> String {
    let mut result = String::new();
    let mut n = idx;
    while n > 0 {
        n -= 1;
        let ch = (b'A' + (n % 26) as u8) as char;
        result.insert(0, ch);
        n /= 26;
    }
    result
}

/// Add a column of cells (vertical fill).
fn add_column(
    package: &mut OxmlPackage,
    parent: &str,
    _position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let parent_trimmed = parent.trim_start_matches('/');
    let sheet_name = parent_trimmed;

    let col_letter = properties
        .get("column")
        .or_else(|| properties.get("col"))
        .ok_or_else(|| {
            HandlerError::InvalidArgument(
                "column add requires 'column' property (e.g. column=B)".to_string(),
            )
        })?
        .to_uppercase();

    let start_row: usize = properties
        .get("startRow")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);

    if let Some(values_csv) = properties.get("values") {
        for (offset, value) in values_csv.split(',').enumerate() {
            let row_idx = start_row + offset;
            let cell_ref = format!("{}{}", col_letter, row_idx);
            let mut cell_props = HashMap::new();
            cell_props.insert("ref".to_string(), cell_ref);
            cell_props.insert("value".to_string(), value.trim().to_string());
            add_cell(package, parent, InsertPosition::Append, &cell_props)?;
        }
    }

    Ok(format!("/{}/col[{}]", sheet_name, col_letter))
}

/// Add a defined Excel Table (ListObject) over a range.
fn add_table(
    package: &mut OxmlPackage,
    parent: &str,
    _position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let parent_trimmed = parent.trim_start_matches('/');
    let sheet_name = parent_trimmed;

    let name = properties
        .get("name")
        .cloned()
        .unwrap_or_else(|| "Table1".to_string());
    let range = properties
        .get("range")
        .or_else(|| properties.get("ref"))
        .ok_or_else(|| {
            HandlerError::InvalidArgument(
                "table add requires 'range' property (e.g. range=A1:C10)".to_string(),
            )
        })?;

    // Extract first/last cell from range like "A1:C10"
    let (first_cell, last_cell) = if let Some(colon) = range.find(':') {
        (range[..colon].to_string(), range[colon + 1..].to_string())
    } else {
        (range.clone(), range.clone())
    };

    // Find the next table part number
    let mut next_num = 1;
    while package
        .read_part_xml(&format!("xl/tables/table{}.xml", next_num))
        .is_ok()
    {
        next_num += 1;
    }
    let table_path = format!("xl/tables/table{}.xml", next_num);

    let table_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<table xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" id="{}" name="{}" displayName="{}" ref="{}" totalsRowShown="0">
  <tableStyleInfo name="TableStyleMedium2" showFirstColumn="0" showLastColumn="0" showRowStripes="1" showColumnStripes="0"/>
</table>"#,
        next_num, name, name, range
    );

    package
        .write_part_xml(&table_path, &table_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    // Update workbook.xml.rels to register the table part
    let rels_path = "xl/_rels/workbook.xml.rels";
    let rels_xml = package
        .read_part_xml(rels_path)
        .unwrap_or_else(|_| "<Relationships/>".to_string());
    let next_rel_id = format!("rId{}", max_rel_id(&rels_xml) + 1);
    let new_rel = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/table\" Target=\"tables/table{}.xml\"/>",
        next_rel_id, next_num
    );
    let modified_rels = if let Some(pos) = rels_xml.find("</Relationships>") {
        let mut result = rels_xml.clone();
        result.insert_str(pos, &new_rel);
        result
    } else {
        format!("<Relationships>{}</Relationships>", new_rel)
    };
    package
        .write_part_xml(rels_path, &modified_rels)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    let _ = (first_cell, last_cell);
    Ok(format!("/{}/table[{}]", sheet_name, next_num))
}

/// Find the max rId in a relationships XML.
fn max_rel_id(xml: &str) -> usize {
    let mut max_id = 0;
    for part in xml.split("Id=\"rId") {
        if let Some(end) = part.find('"') {
            if let Ok(id) = part[..end].parse::<usize>() {
                if id > max_id {
                    max_id = id;
                }
            }
        }
    }
    max_id
}

/// Build and embed a chart in an xlsx workbook.
///
/// Supported properties:
///   type=bar|column|line|pie   (default: column)
///   title=<chart title>
///   sheet=<sheet name>          (default: first sheet)
///   categories=A1:A5            (cell range for x-axis labels)
///   values=B1:B5                (cell range for data)
///   seriesName=B1               (cell with series name; optional)
///   anchor=E2                   (cell where chart top-left anchors; default E2)
fn add_chart_real(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    // Parent can be "/" (workbook) or "/<SheetName>" — extract sheet name.
    let sheet = properties
        .get("sheet")
        .cloned()
        .or_else(|| {
            let p = parent.trim_start_matches('/');
            if p.is_empty() {
                None
            } else {
                Some(p.to_string())
            }
        })
        .ok_or_else(|| {
            HandlerError::InvalidArgument("chart add requires 'sheet' property".to_string())
        })?;

    let chart_type = properties
        .get("type")
        .map(|s| s.as_str())
        .unwrap_or("column")
        .to_lowercase();
    let title = properties.get("title").cloned();
    let categories = properties
        .get("categories")
        .or_else(|| properties.get("cat"))
        .cloned()
        .unwrap_or_else(|| "A1:A5".to_string());
    let values = properties
        .get("values")
        .or_else(|| properties.get("val"))
        .cloned()
        .unwrap_or_else(|| "B1:B5".to_string());
    let series_name = properties.get("seriesName").cloned();
    let anchor = properties
        .get("anchor")
        .cloned()
        .unwrap_or_else(|| "E2".to_string());

    // Allocate chart + drawing numbers by scanning existing parts.
    let chart_idx = next_part_index(package, "xl/charts/chart");
    let drawing_idx = next_part_index(package, "xl/drawings/drawing");

    let chart_path = format!("xl/charts/chart{}.xml", chart_idx);
    let drawing_path = format!("xl/drawings/drawing{}.xml", drawing_idx);

    // Build chart XML.
    let chart_xml = build_chart_xml(
        &chart_type,
        title.as_deref(),
        &sheet,
        &categories,
        &values,
        series_name.as_deref(),
    )?;

    package
        .write_part_xml(&chart_path, &chart_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    // Build drawing XML with a one-cell anchor at `anchor`.
    let drawing_xml = build_drawing_xml(&drawing_path, &chart_path, &sheet, &anchor);
    package
        .write_part_xml(&drawing_path, &drawing_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    // Wire up worksheet→drawing rels + drawing→chart rels.
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;
    let ws = model
        .sheets
        .iter()
        .find(|s| s.name == sheet)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet)))?;
    let ws_part = ws
        .part_path
        .strip_prefix('/')
        .unwrap_or(&ws.part_path)
        .to_string();
    let ws_dir = part_dir(&ws_part);

    // worksheet.xml.rels — link drawing.
    let ws_rels_path = format!("xl/_rels/{}.rels", strip_xl_prefix(&ws_part));
    let drawing_target = relative_path(&ws_dir, &drawing_path);
    let drawing_rel_id = next_rel_id_in_part(package, &ws_rels_path);
    let drawing_rel_xml = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing\" Target=\"{}\"/>",
        drawing_rel_id,
        drawing_target
    );
    inject_relationship(package, &ws_rels_path, &drawing_rel_xml)?;

    // Inject <drawing r:id="..."/> into worksheet.
    let ws_xml = package
        .read_part_xml(&ws_part)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let drawing_element = format!(
        "<drawing xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" r:id=\"{}\"/>",
        drawing_rel_id
    );
    let new_ws_xml = if ws_xml.contains("</worksheet>") {
        ws_xml.replace("</worksheet>", &format!("{}</worksheet>", drawing_element))
    } else {
        format!("{}{}", ws_xml, drawing_element)
    };
    package
        .write_part_xml(&ws_part, &new_ws_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    // drawing.xml.rels — link chart.
    let drawing_rels_path = format!("xl/_rels/{}.rels", strip_xl_prefix(&drawing_path));
    let chart_target = relative_path("xl/drawings", &chart_path);
    let chart_rel_id = "rId1".to_string();
    let chart_rel_xml = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart\" Target=\"{}\"/>",
        chart_rel_id,
        chart_target
    );
    inject_relationship(package, &drawing_rels_path, &chart_rel_xml)?;

    // Update content types so the new parts are recognized.
    update_content_types_for_chart(package, &chart_path, &drawing_path)?;

    Ok(format!("/{}", chart_path))
}

/// Find the next 1-based index for a part family (e.g. "xl/charts/chart" → 1, 2, ...).
fn next_part_index(package: &OxmlPackage, family: &str) -> usize {
    // Best-effort scan of part paths. We only need an unused index, so iterate
    // until we find one not present.
    let mut i = 1;
    loop {
        let candidate = format!("{}.xml", i);
        let full = format!("{}{}.xml", family, i);
        if !package_has_part(package, &full) {
            return i;
        }
        let _ = &candidate;
        i += 1;
    }
}

/// Heuristic part-presence check (we don't have a public iterator, so probe).
fn package_has_part(package: &OxmlPackage, part: &str) -> bool {
    package.read_part_xml(part).is_ok() || package.read_part_bytes(part).is_ok()
}

/// Extract the directory portion of a part path.
fn part_dir(part: &str) -> String {
    match part.rfind('/') {
        Some(i) => part[..i].to_string(),
        None => String::new(),
    }
}

/// Strip leading "xl/" if present.
fn strip_xl_prefix(part: &str) -> String {
    part.strip_prefix("xl/").unwrap_or(part).to_string()
}

/// Compute a relative path from `from_dir` to `to_part`.
fn relative_path(from_dir: &str, to_part: &str) -> String {
    // Simplified: both live under xl/, so we go up to xl/ then back down.
    // Count the number of '/' segments in from_dir to know how many ../ to add.
    let segs = from_dir.matches('/').count();
    let stripped = to_part.strip_prefix("xl/").unwrap_or(to_part);
    format!("{}{}", "../".repeat(segs), stripped)
}

/// Insert a <Relationship/> into a .rels part, creating the part if missing.
fn inject_relationship(
    package: &mut OxmlPackage,
    rels_path: &str,
    rel_xml: &str,
) -> Result<(), HandlerError> {
    let existing = package.read_part_xml(rels_path).ok();
    let new = match existing {
        Some(xml) => {
            if xml.contains("</Relationships>") {
                xml.replace("</Relationships>", &format!("{}</Relationships>", rel_xml))
            } else {
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{}</Relationships>",
                    rel_xml
                )
            }
        }
        None => format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{}</Relationships>",
            rel_xml
        ),
    };
    package
        .write_part_xml(rels_path, &new)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(())
}

/// Find the next free rId in a .rels part (returns "rId1" if part missing).
fn next_rel_id_in_part(package: &OxmlPackage, rels_path: &str) -> String {
    let Ok(xml) = package.read_part_xml(rels_path) else {
        return "rId1".to_string();
    };
    let mut max = 0;
    for hit in xml.match_indices("Id=\"rId") {
        let after = &xml[hit.0 + "Id=\"rId".len()..];
        if let Some(end) = after.find('"') {
            if let Ok(n) = after[..end].parse::<usize>() {
                if n > max {
                    max = n;
                }
            }
        }
    }
    format!("rId{}", max + 1)
}

/// Append chart/drawing override entries to [Content_Types].xml if missing.
fn update_content_types_for_chart(
    package: &mut OxmlPackage,
    chart_path: &str,
    drawing_path: &str,
) -> Result<(), HandlerError> {
    let xml = package
        .read_part_xml("[Content_Types].xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let mut new_xml = xml.clone();

    let chart_override = format!(
        "<Override PartName=\"/{}\" ContentType=\"application/vnd.openxmlformats-officedocument.drawingml.chart+xml\"/>",
        chart_path
    );
    if !new_xml.contains(&chart_override) {
        new_xml = new_xml.replace("</Types>", &format!("{}</Types>", chart_override));
    }

    let drawing_override = format!(
        "<Override PartName=\"/{}\" ContentType=\"application/vnd.openxmlformats-officedocument.drawing+xml\"/>",
        drawing_path
    );
    if !new_xml.contains(&drawing_override) {
        new_xml = new_xml.replace("</Types>", &format!("{}</Types>", drawing_override));
    }

    if new_xml != xml {
        package
            .write_part_xml("[Content_Types].xml", &new_xml)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    }
    Ok(())
}

/// Build chart1.xml content for the requested chart type.
fn build_chart_xml(
    chart_type: &str,
    title: Option<&str>,
    sheet: &str,
    categories: &str,
    values: &str,
    series_name: Option<&str>,
) -> Result<String, HandlerError> {
    let bar_dir = match chart_type {
        "bar" => "bar",
        "column" => "col",
        "line" => "line",
        "pie" => "pie",
        other => {
            return Err(HandlerError::InvalidArgument(format!(
                "unsupported chart type '{}'; supported: bar, column, line, pie",
                other
            )))
        }
    };

    let title_xml = match title {
        Some(t) => format!(
            "<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz=\"1400\"/></a:pPr><a:r><a:t>{}</a:t></a:r></a:p></c:rich></c:tx><c:overlay val=\"0\"/></c:title>",
            escape_xml(t)
        ),
        None => String::new(),
    };

    let series_name_xml = match series_name {
        Some(name_cell) => format!(
            "<c:tx><c:strRef><c:f>{}!{}</c:f></c:strRef></c:tx>",
            sheet, name_cell
        ),
        None => "<c:tx><c:v>Series 1</c:v></c:tx>".to_string(),
    };

    let plot_type_xml = if chart_type == "pie" {
        format!(
            "<c:pieChart>{}{}<c:firstSliceAng val=\"0\"/></c:pieChart>",
            series_xml(series_name_xml.as_str(), sheet, categories, values),
            ""
        )
    } else {
        format!(
            "<c:{}Chart><c:barDir val=\"{}\"/><c:grouping val=\"{}\"/><c:varyColors val=\"0\"/>{}</c:{}Chart>",
            bar_dir,
            if chart_type == "bar" { "bar" } else { "col" },
            if chart_type == "line" { "standard" } else { "clustered" },
            series_xml(series_name_xml.as_str(), sheet, categories, values),
            bar_dir
        )
    };

    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    xml.push_str(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" ",
    );
    xml.push_str("xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" ");
    xml.push_str(
        "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">",
    );
    xml.push_str("<c:chart>");
    xml.push_str(&title_xml);
    xml.push_str("<c:autoTitleDeleted val=\"0\"/>");
    xml.push_str(&plot_type_xml);
    xml.push_str("<c:catAx><c:axId val=\"1\"/><c:scaling/><c:delete val=\"0\"/><c:axPos val=\"b\"/></c:catAx>");
    xml.push_str("<c:valAx><c:axId val=\"2\"/><c:scaling/><c:delete val=\"0\"/><c:axPos val=\"l\"/></c:valAx>");
    xml.push_str("<c:plotVisOnly val=\"1\"/>");
    xml.push_str("</c:chart>");
    xml.push_str("</c:chartSpace>");

    Ok(xml)
}

fn series_xml(name_xml: &str, sheet: &str, categories: &str, values: &str) -> String {
    format!(
        "<c:ser>{}<c:cat><c:numRef><c:f>{}!{}</c:f></c:numRef></c:cat><c:val><c:numRef><c:f>{}!{}</c:f></c:numRef></c:val></c:ser>",
        name_xml, sheet, categories, sheet, values
    )
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Build xl/drawings/drawingN.xml with a one-cell anchor pointing at the chart.
fn build_drawing_xml(_drawing_path: &str, _chart_path: &str, _sheet: &str, anchor: &str) -> String {
    // Convert "E2" → col=5, row=2 (1-based).
    let (col, row) = parse_cell_ref(anchor);

    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    xml.push_str("<xdr:wsDr xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\" ");
    xml.push_str("xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" ");
    xml.push_str(
        "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">",
    );
    xml.push_str(&format!(
        "<xdr:twoCellAnchor><xdr:from><xdr:col>{}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>",
        col - 1,
        row - 1
    ));
    xml.push_str("<xdr:to><xdr:col>10</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>22</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>");
    xml.push_str("<xdr:graphicFrame macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"2\" name=\"Chart 1\"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr>");
    xml.push_str("<xdr:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/></xdr:xfrm>");
    xml.push_str("<a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">");
    xml.push_str("<a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\">");
    xml.push_str("<c:chart xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" r:id=\"rId1\"/>");
    xml.push_str("</a:graphicData></a:graphic></xdr:graphicFrame>");
    xml.push_str("<xdr:clientData/>");
    xml.push_str("</xdr:twoCellAnchor></xdr:wsDr>");
    xml
}

/// Parse "A1" or "BC23" → (col=1-based, row=1-based).
fn parse_cell_ref(s: &str) -> (usize, usize) {
    let bytes = s.as_bytes();
    let mut col = 0usize;
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        col = col * 26 + (bytes[i].to_ascii_uppercase() as usize - b'A' as usize + 1);
        i += 1;
    }
    let row: usize = s[i..].parse().unwrap_or(1);
    (col, row)
}

/// Add a conditional formatting rule to a range.
fn add_conditional_format(
    package: &mut OxmlPackage,
    parent: &str,
    _position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let parent_trimmed = parent.trim_start_matches('/');
    let sheet_name = parent_trimmed;

    let range = properties
        .get("range")
        .or_else(|| properties.get("ref"))
        .ok_or_else(|| {
            HandlerError::InvalidArgument(
                "conditionalFormat add requires 'range' property".to_string(),
            )
        })?;

    let rule_type = properties
        .get("type")
        .or_else(|| properties.get("ruleType"))
        .map(|s| s.as_str())
        .unwrap_or("cellIs");

    let operator = properties
        .get("operator")
        .map(|s| s.as_str())
        .unwrap_or("greaterThan");
    let formula = properties.get("formula").map(|s| s.as_str()).unwrap_or("0");
    let fill_color = properties
        .get("fill")
        .or_else(|| properties.get("fillColor"))
        .map(|c| c.strip_prefix('#').unwrap_or(c))
        .unwrap_or("FFEB9C");

    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;
    let ws = model
        .sheets
        .iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;
    let part_path = ws.part_path.clone();

    let xml = package
        .read_part_xml(&part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Build the conditional formatting XML block
    let cf_block = format!(
        r#"<conditionalFormatting sqref="{}">
  <cfRule type="{}" operator="{}" priority="1">
    <formula>{}</formula>
    <dxf><fill><patternFill><bgColor rgb="FF{}"/></patternFill></fill></dxf>
  </cfRule>
</conditionalFormatting>"#,
        range, rule_type, operator, formula, fill_color
    );

    // Insert before </worksheet>
    let modified = if let Some(pos) = xml.find("</worksheet>") {
        let mut result = xml.clone();
        result.insert_str(pos, &cf_block);
        result
    } else {
        xml.clone()
    };

    package
        .write_part_xml(&part_path, &modified)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(format!("/{}/conditionalFormat[{}]", sheet_name, range))
}

/// Add a data validation rule to a range.
fn add_data_validation(
    package: &mut OxmlPackage,
    parent: &str,
    _position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let parent_trimmed = parent.trim_start_matches('/');
    let sheet_name = parent_trimmed;

    let range = properties
        .get("range")
        .or_else(|| properties.get("ref"))
        .ok_or_else(|| {
            HandlerError::InvalidArgument(
                "dataValidation add requires 'range' property".to_string(),
            )
        })?;

    let validation_type = properties
        .get("type")
        .map(|s| s.as_str())
        .unwrap_or("whole");
    let operator = properties
        .get("operator")
        .map(|s| s.as_str())
        .unwrap_or("between");
    let formula1 = properties
        .get("formula1")
        .or_else(|| properties.get("min"))
        .map(|s| s.as_str())
        .unwrap_or("0");
    let formula2 = properties
        .get("formula2")
        .or_else(|| properties.get("max"))
        .map(|s| s.as_str())
        .unwrap_or("100");

    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;
    let ws = model
        .sheets
        .iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;
    let part_path = ws.part_path.clone();

    let xml = package
        .read_part_xml(&part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let dv_block = format!(
        r#"<dataValidations count="1">
  <dataValidation type="{}" operator="{}" allowBlank="1" sqref="{}">
    <formula1>{}</formula1>
    <formula2>{}</formula2>
  </dataValidation>
</dataValidations>"#,
        validation_type, operator, range, formula1, formula2
    );

    let modified = if let Some(pos) = xml.find("</worksheet>") {
        let mut result = xml.clone();
        result.insert_str(pos, &dv_block);
        result
    } else {
        xml.clone()
    };

    package
        .write_part_xml(&part_path, &modified)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(format!("/{}/validation[{}]", sheet_name, range))
}

/// Add a hyperlink to a cell.
fn add_hyperlink(
    package: &mut OxmlPackage,
    parent: &str,
    _position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let parent_trimmed = parent.trim_start_matches('/');
    let sheet_name = parent_trimmed;

    let cell_ref = properties.get("ref").ok_or_else(|| {
        HandlerError::InvalidArgument("hyperlink add requires 'ref' (cell reference)".to_string())
    })?;
    let url = properties
        .get("url")
        .or_else(|| properties.get("target"))
        .ok_or_else(|| HandlerError::InvalidArgument("hyperlink requires 'url'".to_string()))?;
    // Reject unsafe schemes (javascript:, data:, vbscript:) before they
    // round-trip into a sheet rels file. See handler_common::hyperlink_validator.
    if let Err(msg) = handler_common::hyperlink_validator::require_safe_scheme(url, "hyperlink") {
        return Err(HandlerError::InvalidArgument(msg));
    }

    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;
    let ws = model
        .sheets
        .iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;
    let part_path = ws.part_path.clone();

    let xml = package
        .read_part_xml(&part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Add relationship for the URL
    let sheet_rels_path = part_path
        .replace("xl/", "xl/_rels/")
        .replace(".xml", ".xml.rels");
    let rels_xml = package
        .read_part_xml(&sheet_rels_path)
        .unwrap_or_else(|_| "<Relationships/>".to_string());
    let next_rid = format!("rId{}", max_rel_id(&rels_xml) + 1);
    let new_rel = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"{}\" TargetMode=\"External\"/>",
        next_rid, url
    );
    let modified_rels = if let Some(pos) = rels_xml.find("</Relationships>") {
        let mut result = rels_xml.clone();
        result.insert_str(pos, &new_rel);
        result
    } else {
        format!("<Relationships>{}</Relationships>", new_rel)
    };
    package
        .write_part_xml(&sheet_rels_path, &modified_rels)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    // Insert <hyperlink> element before </worksheet>
    let hl_block = format!("<hyperlink ref=\"{}\" r:id=\"{}\"/>", cell_ref, next_rid);
    let modified = if let Some(pos) = xml.find("</worksheet>") {
        let mut result = xml.clone();
        result.insert_str(pos, &hl_block);
        result
    } else {
        xml.clone()
    };
    package
        .write_part_xml(&part_path, &modified)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    Ok(format!("/{}/hyperlink[{}]", sheet_name, cell_ref))
}

/// Add an embedded image. Writes the image binary (from `payloadBase64` or
/// `payloadHex`, or an empty stub), creates `xl/drawings/drawingN.xml` with a
/// two-cell anchor, wires worksheet→drawing→image rels, and updates
/// [Content_Types].xml with the image extension.
fn add_image_real(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let sheet = properties
        .get("sheet")
        .cloned()
        .or_else(|| {
            let p = parent.trim_start_matches('/');
            if p.is_empty() {
                None
            } else {
                Some(p.to_string())
            }
        })
        .ok_or_else(|| {
            HandlerError::InvalidArgument("image add requires 'sheet' property".to_string())
        })?;

    let ext = properties
        .get("format")
        .or_else(|| properties.get("ext"))
        .map(|s| s.as_str())
        .unwrap_or("png");
    let (ext_norm, content_type) = match ext.to_lowercase().as_str() {
        "png" => ("png", "image/png"),
        "jpg" | "jpeg" => ("jpeg", "image/jpeg"),
        "gif" => ("gif", "image/gif"),
        "bmp" => ("bmp", "image/bmp"),
        "tiff" | "tif" => ("tiff", "image/tiff"),
        "webp" => ("webp", "image/webp"),
        "svg" => ("svg", "image/svg+xml"),
        "ico" => ("ico", "image/x-icon"),
        _ => ("png", "image/png"),
    };

    let anchor = properties
        .get("anchor")
        .or_else(|| properties.get("ref"))
        .cloned()
        .unwrap_or_else(|| "B2".to_string());
    let (col, row) = parse_cell_ref(&anchor);

    let name = properties
        .get("name")
        .cloned()
        .unwrap_or_else(|| format!("Image {}", ext_norm));
    let alt = properties
        .get("alt")
        .or_else(|| properties.get("description"))
        .map(|s| s.as_str())
        .unwrap_or("");

    // Width / height in EMU (default 4x3 inches = 3657600 x 2743200).
    let (width_emu, height_emu) = parse_image_dimensions(properties);

    // Probe for free indices.
    let image_idx = next_image_index(package, ext_norm);
    let drawing_idx = next_part_index(package, "xl/drawings/drawing");

    let media_path = format!("xl/media/image{}.{}", image_idx, ext_norm);
    let drawing_path = format!("xl/drawings/drawing{}.xml", drawing_idx);

    // Write image binary.
    if let Some(b64) = properties.get("payloadBase64") {
        if let Ok(bytes) = base64_decode(b64) {
            let _ = package.write_part(&media_path, bytes);
        }
    } else if let Some(hex) = properties.get("payloadHex") {
        if let Ok(bytes) = hex_decode(hex) {
            let _ = package.write_part(&media_path, bytes);
        }
    } else {
        // Empty stub so the part exists; caller must overwrite with real bytes.
        let _ = package.write_part(&media_path, Vec::new());
    }

    // Build the drawing XML with a two-cell anchor hosting <xdr:pic>.
    let drawing_xml = build_image_drawing_xml(
        &drawing_path,
        &media_path,
        col,
        row,
        width_emu,
        height_emu,
        &name,
        alt,
    );
    package
        .write_part_xml(&drawing_path, &drawing_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    // Resolve worksheet part path.
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;
    let ws = model
        .sheets
        .iter()
        .find(|s| s.name == sheet)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet)))?;
    let ws_part = ws
        .part_path
        .strip_prefix('/')
        .unwrap_or(&ws.part_path)
        .to_string();
    let ws_dir = part_dir(&ws_part);

    // worksheet.xml.rels → drawing.
    let ws_rels_path = format!("xl/_rels/{}.rels", strip_xl_prefix(&ws_part));
    let drawing_target = relative_path(&ws_dir, &drawing_path);
    let drawing_rel_id = next_rel_id_in_part(package, &ws_rels_path);
    let drawing_rel_xml = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing\" Target=\"{}\"/>",
        drawing_rel_id, drawing_target
    );
    inject_relationship(package, &ws_rels_path, &drawing_rel_xml)?;

    // Inject <drawing r:id=.../> into the worksheet (before </worksheet>).
    let ws_xml = package
        .read_part_xml(&ws_part)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let drawing_element = format!(
        "<drawing xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" r:id=\"{}\"/>",
        drawing_rel_id
    );
    let new_ws_xml = if ws_xml.contains("</worksheet>") {
        ws_xml.replace("</worksheet>", &format!("{}</worksheet>", drawing_element))
    } else {
        format!("{}{}", ws_xml, drawing_element)
    };
    package
        .write_part_xml(&ws_part, &new_ws_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    // drawing.xml.rels → image.
    let drawing_rels_path = format!("xl/_rels/{}.rels", strip_xl_prefix(&drawing_path));
    let image_target = relative_path("xl/drawings", &media_path);
    let image_rel_id = "rId1".to_string();
    let image_rel_xml = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"{}\"/>",
        image_rel_id, image_target
    );
    inject_relationship(package, &drawing_rels_path, &image_rel_xml)?;

    // Update [Content_Types].xml.
    update_content_types_for_image(package, ext_norm, content_type, &drawing_path)?;

    Ok(format!("/{}", media_path))
}

/// Find next free image index in xl/media/imageN.<ext>.
fn next_image_index(package: &OxmlPackage, ext: &str) -> usize {
    let mut i = 1;
    loop {
        if package_has_part(package, &format!("xl/media/image{}.{}", i, ext)) {
            i += 1;
        } else {
            return i;
        }
    }
}

/// Parse dimension properties (width / height) in EMU. Accepts numeric EMU
/// or unit suffixes like "4in", "10cm", "200px", "300pt".
fn parse_image_dimensions(props: &HashMap<String, String>) -> (i64, i64) {
    let width = props
        .get("width")
        .or_else(|| props.get("w"))
        .map(|s| parse_emu(s))
        .unwrap_or(3_657_600); // 4 inches
    let height = props
        .get("height")
        .or_else(|| props.get("h"))
        .map(|s| parse_emu(s))
        .unwrap_or(2_743_200); // 3 inches
    (width, height)
}

/// Convert a measurement string into EMU (English Metric Units: 914400/inch).
fn parse_emu(s: &str) -> i64 {
    let s = s.trim();
    if let Some(v) = s.strip_suffix("in") {
        v.trim()
            .parse::<f64>()
            .map(|n| (n * 914400.0) as i64)
            .unwrap_or(3_657_600)
    } else if let Some(v) = s.strip_suffix("cm") {
        v.trim()
            .parse::<f64>()
            .map(|n| (n * 360000.0) as i64)
            .unwrap_or(3_657_600)
    } else if let Some(v) = s.strip_suffix("mm") {
        v.trim()
            .parse::<f64>()
            .map(|n| (n * 36000.0) as i64)
            .unwrap_or(3_657_600)
    } else if let Some(v) = s.strip_suffix("pt") {
        v.trim()
            .parse::<f64>()
            .map(|n| (n * 12700.0) as i64)
            .unwrap_or(3_657_600)
    } else if let Some(v) = s.strip_suffix("px") {
        v.trim()
            .parse::<f64>()
            .map(|n| (n * 9525.0) as i64)
            .unwrap_or(3_657_600)
    } else {
        s.parse::<i64>().unwrap_or(3_657_600)
    }
}

/// Build xl/drawings/drawingN.xml with a twoCellAnchor containing <xdr:pic>.
#[allow(clippy::too_many_arguments)]
fn build_image_drawing_xml(
    _drawing_path: &str,
    _media_path: &str,
    col: usize,
    row: usize,
    width_emu: i64,
    height_emu: i64,
    name: &str,
    alt: &str,
) -> String {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    xml.push_str("<xdr:wsDr xmlns:xdr=\"http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing\" ");
    xml.push_str("xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" ");
    xml.push_str(
        "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" ",
    );
    xml.push_str("xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">");
    xml.push_str("<xdr:twoCellAnchor>");
    xml.push_str(&format!(
        "<xdr:from><xdr:col>{}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>",
        col.saturating_sub(1),
        row.saturating_sub(1)
    ));
    xml.push_str(&format!(
        "<xdr:to><xdr:col>{}</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>{}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>",
        col.saturating_sub(1).saturating_add(((width_emu / 9525) / 96) as usize + 1),
        row.saturating_sub(1).saturating_add(((height_emu / 9525) / 96) as usize + 1)
    ));
    xml.push_str(&format!(
        "<xdr:pic><xdr:nvPicPr><xdr:cNvPr id=\"2\" name=\"{}\" descr=\"{}\"/><xdr:cNvPicPr><a:picLocks noChangeAspect=\"1\"/></xdr:cNvPicPr><xdr:nvPr/></xdr:nvPicPr>",
        escape_xml(name),
        escape_xml(alt)
    ));
    xml.push_str("<xdr:blipFill><a:blip xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" r:embed=\"rId1\"/><a:stretch><a:fillRect/></a:stretch></xdr:blipFill>");
    xml.push_str(&format!(
        "<xdr:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></xdr:spPr>",
        width_emu, height_emu
    ));
    xml.push_str("</xdr:pic>");
    xml.push_str("<xdr:clientData/>");
    xml.push_str("</xdr:twoCellAnchor></xdr:wsDr>");
    xml
}

/// Add Default entry for image extension and Override for drawing part.
fn update_content_types_for_image(
    package: &mut OxmlPackage,
    ext: &str,
    content_type: &str,
    drawing_path: &str,
) -> Result<(), HandlerError> {
    let xml = package
        .read_part_xml("[Content_Types].xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let ext_attr = format!("Extension=\"{}\"", ext);
    let default_xml = format!(
        "<Default Extension=\"{}\" ContentType=\"{}\"/>",
        ext, content_type
    );
    let override_xml = format!(
        "<Override PartName=\"/{}\" ContentType=\"application/vnd.openxmlformats-officedocument.drawing+xml\"/>",
        drawing_path
    );

    let mut out = String::with_capacity(xml.len() + default_xml.len() + override_xml.len());
    let has_ext = xml.contains(&ext_attr);
    let has_drawing = xml.contains(&format!("PartName=\"/{}\"", drawing_path));

    if has_ext && has_drawing {
        return Ok(());
    }

    // Insert Default after the opening <Types ...> tag.
    if let Some(close) = xml.find('>') {
        out.push_str(&xml[..close + 1]);
        if !has_ext {
            out.push_str(&default_xml);
        }
        // Insert Override before </Types>.
        let body = &xml[close + 1..];
        if let Some(end) = body.rfind("</Types>") {
            let (head, tail) = body.split_at(end);
            out.push_str(head);
            if !has_drawing {
                out.push_str(&override_xml);
            }
            out.push_str(tail);
        } else {
            out.push_str(body);
        }
    } else {
        out.push_str(&xml);
    }

    package
        .write_part_xml("[Content_Types].xml", &out)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(())
}

fn base64_decode(s: &str) -> Result<Vec<u8>, ()> {
    let mut bits: u32 = 0;
    let mut nbits: u32 = 0;
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for c in s.chars().filter(|c| !c.is_whitespace()) {
        let v: u32 = match c {
            'A'..='Z' => (c as u32) - ('A' as u32),
            'a'..='z' => (c as u32) - ('a' as u32) + 26,
            '0'..='9' => (c as u32) - ('0' as u32) + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '=' => break,
            _ => return Err(()),
        };
        bits = (bits << 6) | v;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((bits >> nbits) as u8);
        }
    }
    Ok(out)
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !cleaned.len().is_multiple_of(2) {
        return Err(());
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    let bytes = cleaned.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let byte = u8::from_str_radix(&format!("{}{}", bytes[i] as char, bytes[i + 1] as char), 16)
            .map_err(|_| ())?;
        out.push(byte);
        i += 2;
    }
    Ok(out)
}
