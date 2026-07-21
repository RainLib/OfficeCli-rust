/// View modes for xlsx documents: text grid, outline, stats.
use crate::dom_types::*;
use crate::helpers;
use handler_common::output_format::ViewOptions;
use handler_common::{DocumentIssue, HandlerError, IssueSeverity, ValidationError};
use oxml::OxmlPackage;

/// ViewAsText: show cells as a formatted text grid.
pub fn view_as_text(package: &OxmlPackage, opts: &ViewOptions) -> Result<String, HandlerError> {
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    let mut output = String::new();

    for ws in &model.sheets {
        let sheet_lines = format_sheet_as_grid(ws, opts);
        output.push_str(&format!("=== {} ===\n", ws.name));
        output.push_str(&sheet_lines);
        output.push('\n');
    }

    Ok(output)
}

/// Format a single sheet as a text grid.
fn format_sheet_as_grid(ws: &Worksheet, opts: &ViewOptions) -> String {
    if ws.cells.is_empty() {
        return "(empty sheet)\n".to_string();
    }

    let start_row = opts.start_line.unwrap_or(1);
    let end_row = opts.end_line.unwrap_or(ws.max_row);
    let max_lines = opts.max_lines.unwrap_or(100);
    let effective_end_row = end_row.min(start_row + max_lines - 1).min(ws.max_row);

    // Determine column range
    let col_filter = opts.cols.as_ref();
    let min_col: usize = col_filter
        .and_then(|cols| {
            cols.first()
                .and_then(|c| CellRef::parse(c).map(|cr| cr.col))
        })
        .unwrap_or(1);
    let max_col: usize = col_filter
        .and_then(|cols| cols.last().and_then(|c| CellRef::parse(c).map(|cr| cr.col)))
        .unwrap_or(ws.max_col);

    // Build column headers
    let col_range: Vec<usize> = if let Some(cols) = col_filter {
        cols.iter()
            .filter_map(|c| CellRef::parse(c).map(|cr| cr.col))
            .collect()
    } else {
        (min_col..=max_col).collect()
    };

    if col_range.is_empty() {
        return "(no columns)\n".to_string();
    }

    // Calculate column widths
    let mut col_widths: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for col in &col_range {
        let header_len = col_num_to_letters(*col).len();
        let max_cell_len = (start_row..=effective_end_row)
            .filter_map(|row| ws.cells.get(&(row, *col)))
            .map(|cell| cell.display_value.chars().count())
            .max()
            .unwrap_or(0);
        col_widths.insert(*col, std::cmp::max(header_len, max_cell_len).min(30) + 2);
    }

    // Row number column width
    let row_num_width = std::cmp::max(effective_end_row.to_string().len(), 3) + 1;

    let mut grid = String::new();

    // Header row
    grid.push_str(&format!("{:>width$}", "", width = row_num_width));
    for col in &col_range {
        let w = col_widths.get(col).unwrap_or(&6);
        grid.push_str(&format!(
            "{:^>width$}",
            col_num_to_letters(*col),
            width = *w
        ));
    }
    grid.push('\n');

    // Data rows
    for row in start_row..=effective_end_row {
        grid.push_str(&format!("{:>width$}", row, width = row_num_width));
        for col in &col_range {
            let w = col_widths.get(col).unwrap_or(&6);
            let cell = ws.cells.get(&(row, *col));
            let display = cell.map(|c| c.display_value.as_str()).unwrap_or("");
            // For formula cells, display_value already contains the evaluated result
            // so we show it directly — the formula is visible in annotated view
            let truncated = truncate_cell_value(display, w.saturating_sub(2));
            grid.push_str(&format!("{:>width$}", truncated, width = *w));
        }
        grid.push('\n');
    }

    grid
}

fn truncate_cell_value(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut truncated: String = value.chars().take(max_chars - 1).collect();
    truncated.push('…');
    truncated
}

fn col_num_to_letters(num: usize) -> String {
    let mut letters = String::new();
    let mut n = num;
    while n > 0 {
        n -= 1;
        letters.push((b'A' + (n % 26) as u8) as char);
        n /= 26;
    }
    letters.chars().rev().collect()
}

/// ViewAsOutline: list sheets with summary info.
pub fn view_as_outline(package: &OxmlPackage) -> Result<String, HandlerError> {
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    let mut output = String::new();
    output.push_str(&format!("Workbook: {} sheets\n", model.sheets.len()));

    for ws in &model.sheets {
        output.push_str(&format!(
            "  /{} — {} cells, {} rows, {} cols\n",
            ws.name,
            ws.cells.len(),
            ws.max_row,
            ws.max_col
        ));
    }

    if !model.pivot_tables.is_empty() {
        output.push_str(&format!("  Pivot tables: {}\n", model.pivot_tables.len()));
        for pt in &model.pivot_tables {
            let range_info = pt
                .source_range
                .as_ref()
                .map(|r| format!(", source: {}", r))
                .unwrap_or_default();
            output.push_str(&format!(
                "    \"{}\" ({} fields){}\n",
                pt.name, pt.field_count, range_info
            ));
        }
    }

    Ok(output)
}

/// ViewAsOutline JSON representation.
pub fn view_as_outline_json(package: &OxmlPackage) -> Result<serde_json::Value, HandlerError> {
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    let sheets_json: Vec<serde_json::Value> = model
        .sheets
        .iter()
        .map(|ws| {
            serde_json::json!({
                "name": ws.name,
                "index": ws.index,
                "path": format!("/{}", ws.name),
                "cellCount": ws.cells.len(),
                "maxRow": ws.max_row,
                "maxCol": ws.max_col,
                "partPath": ws.part_path,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "format": "xlsx",
        "sheetCount": model.sheets.len(),
        "sheets": sheets_json,
    }))
}

/// ViewAsText JSON representation.
pub fn view_as_text_json(
    package: &OxmlPackage,
    _opts: &ViewOptions,
) -> Result<serde_json::Value, HandlerError> {
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    let sheets_json: Vec<serde_json::Value> = model
        .sheets
        .iter()
        .map(|ws| {
            let cells_json: Vec<serde_json::Value> = ws
                .cells
                .values()
                .map(|cell| {
                    let mut obj = serde_json::json!({
                        "ref": cell.ref_str,
                        "value": cell.display_value,
                        "type": cell_type_label(&cell.value_type),
                    });
                    if let Some(f) = &cell.formula {
                        obj["formula"] = serde_json::Value::String(f.clone());
                    }
                    if let Some(si) = cell.style_index {
                        obj["styleIndex"] = serde_json::Value::Number(si.into());
                    }
                    obj
                })
                .collect();

            serde_json::json!({
                "name": ws.name,
                "path": format!("/{}", ws.name),
                "maxRow": ws.max_row,
                "maxCol": ws.max_col,
                "cells": cells_json,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "format": "xlsx",
        "sheets": sheets_json,
    }))
}

/// ViewAsStats: summary statistics.
pub fn view_as_stats(package: &OxmlPackage) -> Result<String, HandlerError> {
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    let total_cells: usize = model.sheets.iter().map(|ws| ws.cells.len()).sum();
    let total_formulas: usize = model
        .sheets
        .iter()
        .flat_map(|ws| ws.cells.values())
        .filter(|c| c.formula.is_some())
        .count();
    let max_dimensions = model
        .sheets
        .iter()
        .map(|ws| format!("{}: {}R x {}C", ws.name, ws.max_row, ws.max_col))
        .collect::<Vec<_>>();

    let mut stats = format!(
        "Format: xlsx\n\
         Sheets: {}\n\
         Total cells: {}\n\
         Formulas: {}\n\
         Shared strings: {}\n\
         Dimensions:\n  {}\n",
        model.sheets.len(),
        total_cells,
        total_formulas,
        model.shared_strings.len(),
        max_dimensions.join("\n  "),
    );

    if !model.pivot_tables.is_empty() {
        stats.push_str(&format!("Pivot tables: {}\n", model.pivot_tables.len()));
    }

    Ok(stats)
}

/// ViewAsStats JSON.
pub fn view_as_stats_json(package: &OxmlPackage) -> Result<serde_json::Value, HandlerError> {
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    let total_cells: usize = model.sheets.iter().map(|ws| ws.cells.len()).sum();
    let total_formulas: usize = model
        .sheets
        .iter()
        .flat_map(|ws| ws.cells.values())
        .filter(|c| c.formula.is_some())
        .count();

    Ok(serde_json::json!({
        "format": "xlsx",
        "sheetCount": model.sheets.len(),
        "totalCells": total_cells,
        "totalFormulas": total_formulas,
        "sharedStringCount": model.shared_strings.len(),
        "pivotTableCount": model.pivot_tables.len(),
    }))
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

/// Detect issues in the workbook.
pub fn view_as_issues(
    package: &OxmlPackage,
    issue_type: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<DocumentIssue>, HandlerError> {
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;
    let mut issues = Vec::new();

    // Check for missing sheet parts
    for ws in &model.sheets {
        if !package.has_part(&ws.part_path) {
            issues.push(DocumentIssue {
                severity: IssueSeverity::Warning,
                issue_type: "missing-sheet".to_string(),
                description: format!("Sheet '{}' part '{}' is missing", ws.name, ws.part_path),
                path: Some(format!("/{}", ws.name)),
            });
        }

        // Check for cells with error values
        for cell in ws.cells.values() {
            if cell.value_type == CellValueType::Error {
                issues.push(DocumentIssue {
                    severity: IssueSeverity::Warning,
                    issue_type: "cell-error".to_string(),
                    description: format!(
                        "Cell {} has error value: {}",
                        cell.ref_str, cell.display_value
                    ),
                    path: Some(format!("/{}/{}", ws.name, cell.ref_str)),
                });
            }
        }

        // Check for orphan shared string references (index out of bounds)
        for cell in ws.cells.values() {
            if cell.value_type == CellValueType::SharedString {
                if let Some(raw) = &cell.raw_value {
                    if let Ok(idx) = raw.parse::<usize>() {
                        if idx >= model.shared_strings.len() {
                            issues.push(DocumentIssue {
                                severity: IssueSeverity::Error,
                                issue_type: "broken-shared-string".to_string(),
                                description: format!("Cell {} references shared string index {} but only {} strings exist", cell.ref_str, idx, model.shared_strings.len()),
                                path: Some(format!("/{}/{}", ws.name, cell.ref_str)),
                            });
                        }
                    }
                }
            }
        }
    }

    // localSheetId is a 0-based index into the workbook's sheet list. Excel
    // rejects out-of-range scopes, usually left behind by an incomplete sheet
    // remove/reorder operation.
    if let Ok(workbook_xml) = package.read_part_xml("xl/workbook.xml") {
        if let Ok(doc) = roxmltree::Document::parse(&workbook_xml) {
            for defined_name in doc
                .descendants()
                .filter(|node| node.is_element() && node.tag_name().name() == "definedName")
            {
                let Some(scope) = defined_name
                    .attribute("localSheetId")
                    .and_then(|value| value.parse::<usize>().ok())
                else {
                    continue;
                };
                if scope >= model.sheets.len() {
                    let name = defined_name.attribute("name").unwrap_or("(unnamed)");
                    issues.push(DocumentIssue {
                        severity: IssueSeverity::Error,
                        issue_type: "broken-defined-name-scope".to_string(),
                        description: format!(
                            "Defined name '{}' has out-of-range localSheetId={} but the workbook has {} sheet(s)",
                            name,
                            scope,
                            model.sheets.len()
                        ),
                        path: Some(format!("/namedrange[{}]", name)),
                    });
                }
            }
        }
    }

    // Filter by issue type
    if let Some(filter_type) = issue_type {
        issues.retain(|i| i.issue_type == filter_type);
    }

    // Apply limit
    if let Some(max) = limit {
        issues.truncate(max);
    }

    Ok(issues)
}

/// Validate the workbook structure.
pub fn validate(package: &OxmlPackage) -> Result<Vec<ValidationError>, HandlerError> {
    let mut errors = Vec::new();

    // Check for required workbook part
    if !package.has_part("xl/workbook.xml") {
        errors.push(ValidationError {
            error_type: "missing-part".to_string(),
            description: "required workbook part".to_string(),
            path: Some("xl/workbook.xml".to_string()),
            part: Some("xl/workbook.xml".to_string()),
        });
    }

    // Check workbook relationships
    if !package.has_part("xl/_rels/workbook.xml.rels") {
        errors.push(ValidationError {
            error_type: "missing-part".to_string(),
            description: "required workbook relationships part".to_string(),
            path: Some("xl/_rels/workbook.xml.rels".to_string()),
            part: Some("xl/_rels/workbook.xml.rels".to_string()),
        });
    }

    // Build model to check sheets
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    for ws in &model.sheets {
        if !package.has_part(&ws.part_path) {
            errors.push(ValidationError {
                error_type: "missing-part".to_string(),
                description: format!("sheet '{}' part is missing", ws.name),
                path: Some(format!("/{}", ws.name)),
                part: Some(ws.part_path.clone()),
            });
        }

        // Check each sheet's worksheet XML has <sheetData>
        if package.has_part(&ws.part_path) {
            let xml = package
                .read_part_xml(&ws.part_path)
                .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
            if !xml.contains("<sheetData") {
                errors.push(ValidationError {
                    error_type: "structure".to_string(),
                    description: format!("sheet '{}' has no <sheetData> element", ws.name),
                    path: Some(format!("/{}", ws.name)),
                    part: Some(ws.part_path.clone()),
                });
            }
        }
    }

    Ok(errors)
}

#[cfg(test)]
mod tests {
    use super::{truncate_cell_value, view_as_issues};
    use oxml::OxmlPackage;

    #[test]
    fn truncate_cell_value_handles_multibyte_text() {
        let truncated = truncate_cell_value("云村医—系统如何刷卡？", 10);

        assert_eq!(truncated, "云村医—系统如何刷…");
        assert_eq!(truncated.chars().count(), 10);
    }

    #[test]
    fn truncate_cell_value_preserves_short_text_and_small_widths() {
        assert_eq!(truncate_cell_value("中文", 2), "中文");
        assert_eq!(truncate_cell_value("中文", 1), "…");
        assert_eq!(truncate_cell_value("中文", 0), "");
    }

    #[test]
    fn issues_report_out_of_range_defined_name_scope() {
        let mut package = OxmlPackage::create("unused.xlsx");
        package.add_part(
            "xl/workbook.xml",
            br#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Only" sheetId="1" r:id="rId1"/></sheets><definedNames><definedName name="broken" localSheetId="3">Only!$A$1</definedName></definedNames></workbook>"#,
        );
        package.add_part(
            "xl/_rels/workbook.xml.rels",
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
        );
        package.add_part(
            "xl/worksheets/sheet1.xml",
            br#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>"#,
        );

        let issues = view_as_issues(&package, None, None).unwrap();

        assert!(issues.iter().any(|issue| {
            issue.issue_type == "broken-defined-name-scope"
                && issue.description.contains("localSheetId=3")
        }));
    }
}
