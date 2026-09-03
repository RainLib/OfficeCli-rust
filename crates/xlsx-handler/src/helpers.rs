/// Parsing helpers for xlsx OOXML parts.
use crate::dom_types::*;
use crate::formula;
use handler_common::HandlerError;
use oxml::OxmlPackage;
use quick_xml::events::Event;
use quick_xml::Reader;

const DRAWING_RELATIONSHIP_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing";
const RELATIONSHIPS_NS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Parse the workbook.xml to extract the sheet list.
/// Returns (sheet_name, part_path, rel_id) for each sheet.
pub fn parse_workbook(package: &OxmlPackage) -> Result<Vec<(String, String, String)>, String> {
    let xml = package
        .read_part_xml("xl/workbook.xml")
        .map_err(|e| format!("failed to read xl/workbook.xml: {}", e))?;

    // Parse relationships for workbook to resolve sheet paths
    let rels = package
        .part_rels("xl/workbook.xml")
        .map_err(|e| format!("failed to read workbook rels: {}", e))?;

    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(true);

    let mut sheets = Vec::new();
    let mut in_sheets = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let local_name = e.local_name();
                let local_name_ref: &[u8] = local_name.as_ref();
                match local_name_ref {
                    b"sheets" => in_sheets = true,
                    b"sheet" if in_sheets => {
                        let name = e
                            .attributes()
                            .filter_map(|a| a.ok())
                            .find(|a| a.key.as_ref() == b"name")
                            .and_then(|a| a.decode_and_unescape_value(reader.decoder()).ok())
                            .map(|value| value.into_owned())
                            .unwrap_or_default();

                        let rel_id = e
                            .attributes()
                            .filter_map(|a| a.ok())
                            .find(|a| {
                                let key = a.key.as_ref();
                                key == b"r:id" || key.ends_with(b":id") || key == b"id"
                            })
                            .map(|a| String::from_utf8_lossy(a.value.as_ref()).to_string())
                            .unwrap_or_default();

                        // Resolve the relationship target to get the part path
                        let target = rels
                            .get(&rel_id)
                            .map(|r| r.target.clone())
                            .unwrap_or_default();
                        let part_path = package.resolve_rel_target("xl/workbook.xml", &target);

                        sheets.push((name, part_path, rel_id));
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                if e.local_name().as_ref() == b"sheets" {
                    in_sheets = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error: {}", e)),
            _ => {}
        }
    }

    Ok(sheets)
}

/// Resolve a C# OfficeCLI raw-layer semantic path to an OOXML part path.
///
/// Paths ending in `.xml` or `.rels` are handled literally, matching the C#
/// command's zip-URI convention.  The remaining aliases deliberately use the
/// workbook relationship table instead of assuming `sheetN.xml`: worksheet
/// filenames are not stable after a workbook has been edited by Excel.
pub fn resolve_raw_part_path(
    package: &OxmlPackage,
    part_path: &str,
) -> Result<String, HandlerError> {
    let literal_path = part_path.trim_start_matches('/');
    if literal_path.ends_with(".xml") || literal_path.ends_with(".rels") {
        return Ok(literal_path.to_string());
    }

    let alias = part_path.trim_matches('/');
    match alias.to_ascii_lowercase().as_str() {
        "" | "workbook" => return Ok("xl/workbook.xml".to_string()),
        "styles" => return Ok("xl/styles.xml".to_string()),
        "sharedstrings" => return Ok("xl/sharedStrings.xml".to_string()),
        "theme" => return Ok("xl/theme/theme1.xml".to_string()),
        _ => {}
    }

    if let Some((sheet_name, suffix)) = alias.rsplit_once('/') {
        let sheet_part = sheet_part_by_alias(package, sheet_name)?;
        if suffix == "drawing" {
            return drawing_part_for_sheet(package, &sheet_part);
        }
        if let Some(chart_index) = parse_chart_index(suffix) {
            return chart_part_for_sheet(package, &sheet_part, chart_index);
        }
        if suffix.starts_with(['r', 'R']) {
            return relationship_part_for_sheet(package, &sheet_part, suffix);
        }
    }
    if let Some(chart_index) = parse_chart_index(alias) {
        return chart_part_globally(package, chart_index);
    }

    sheet_part_by_alias(package, alias).map_err(|_| {
        HandlerError::PathNotFound(format!(
            "unknown XLSX raw part '{}'; use /workbook, /styles, /sharedStrings, /theme, /<SheetName>, /<SheetName>/drawing, /<SheetName>/chart[N], /chart[N], or a zip-internal .xml path",
            part_path
        ))
    })
}

fn sheet_part_by_alias(package: &OxmlPackage, alias: &str) -> Result<String, HandlerError> {
    if let Some(index) = alias
        .strip_prefix("sheet[")
        .and_then(|value| value.strip_suffix(']'))
        .and_then(|value| value.parse::<usize>().ok())
    {
        return sheet_part_by_index(package, index);
    }
    sheet_part_by_name(package, alias)
}

fn sheet_part_by_index(package: &OxmlPackage, index: usize) -> Result<String, HandlerError> {
    if index == 0 {
        return Err(HandlerError::PathNotFound("sheet[0]".to_string()));
    }
    let sheets = parse_workbook(package).map_err(HandlerError::OperationFailed)?;
    sheets
        .into_iter()
        .nth(index - 1)
        .map(|(_, path, _)| path)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet[{}]", index)))
}

fn sheet_part_by_name(package: &OxmlPackage, sheet_name: &str) -> Result<String, HandlerError> {
    let sheets = parse_workbook(package).map_err(HandlerError::OperationFailed)?;
    sheets
        .into_iter()
        .find(|(name, _, _)| name == sheet_name)
        .map(|(_, path, _)| path)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))
}

fn drawing_part_for_sheet(package: &OxmlPackage, sheet_part: &str) -> Result<String, HandlerError> {
    let rels = package
        .part_rels(sheet_part)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let drawing = rels
        .by_type(DRAWING_RELATIONSHIP_TYPE)
        .into_iter()
        .min_by(|left, right| left.id.cmp(&right.id))
        .ok_or_else(|| HandlerError::PathNotFound(format!("drawing for '{}'", sheet_part)))?;
    Ok(package.resolve_rel_target(sheet_part, &drawing.target))
}

fn relationship_part_for_sheet(
    package: &OxmlPackage,
    sheet_part: &str,
    relationship_id: &str,
) -> Result<String, HandlerError> {
    let rels = package
        .part_rels(sheet_part)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let relationship = rels.get(relationship_id).ok_or_else(|| {
        HandlerError::PathNotFound(format!(
            "relationship '{}' on '{}'",
            relationship_id, sheet_part
        ))
    })?;
    if relationship.target_mode.eq_ignore_ascii_case("External") {
        return Err(HandlerError::UnsupportedMode(format!(
            "raw access to external relationship '{}' is not supported",
            relationship_id
        )));
    }
    Ok(package.resolve_rel_target(sheet_part, &relationship.target))
}

fn chart_part_for_sheet(
    package: &OxmlPackage,
    sheet_part: &str,
    chart_index: usize,
) -> Result<String, HandlerError> {
    let drawing_part = drawing_part_for_sheet(package, sheet_part)?;
    let chart_ids = chart_relationship_ids(package, &drawing_part)?;
    let chart_id = chart_ids
        .get(chart_index.saturating_sub(1))
        .ok_or_else(|| {
            HandlerError::PathNotFound(format!("chart[{}] on '{}'", chart_index, sheet_part))
        })?;
    relationship_part(package, &drawing_part, chart_id)
}

fn chart_part_globally(package: &OxmlPackage, chart_index: usize) -> Result<String, HandlerError> {
    let sheets = parse_workbook(package).map_err(HandlerError::OperationFailed)?;
    let mut charts = Vec::new();
    for (_, sheet_part, _) in sheets {
        if let Ok(drawing_part) = drawing_part_for_sheet(package, &sheet_part) {
            for chart_id in chart_relationship_ids(package, &drawing_part)? {
                charts.push(relationship_part(package, &drawing_part, &chart_id)?);
            }
        }
    }
    charts
        .get(chart_index.saturating_sub(1))
        .cloned()
        .ok_or_else(|| HandlerError::PathNotFound(format!("chart[{}]", chart_index)))
}

fn relationship_part(
    package: &OxmlPackage,
    source_part: &str,
    relationship_id: &str,
) -> Result<String, HandlerError> {
    let rels = package
        .part_rels(source_part)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let relationship = rels.get(relationship_id).ok_or_else(|| {
        HandlerError::PathNotFound(format!(
            "relationship '{}' on '{}'",
            relationship_id, source_part
        ))
    })?;
    Ok(package.resolve_rel_target(source_part, &relationship.target))
}

fn chart_relationship_ids(
    package: &OxmlPackage,
    drawing_part: &str,
) -> Result<Vec<String>, HandlerError> {
    let xml = package
        .read_part_xml(drawing_part)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let document = roxmltree::Document::parse(&xml)
        .map_err(|e| HandlerError::OperationFailed(format!("invalid drawing XML: {}", e)))?;
    Ok(document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "chart")
        .filter_map(|node| {
            node.attribute((RELATIONSHIPS_NS, "id"))
                .or_else(|| node.attribute("r:id"))
                .map(ToOwned::to_owned)
        })
        .collect())
}

fn parse_chart_index(segment: &str) -> Option<usize> {
    segment
        .strip_prefix("chart[")?
        .strip_suffix(']')?
        .parse::<usize>()
        .ok()
        .filter(|index| *index > 0)
}

/// Parse the shared strings table from xl/sharedStrings.xml.
/// Returns a vector where index i corresponds to the shared string at that index.
pub fn parse_shared_strings(package: &OxmlPackage) -> Vec<String> {
    if !package.has_part("xl/sharedStrings.xml") {
        return Vec::new();
    }

    let xml = package
        .read_part_xml("xl/sharedStrings.xml")
        .unwrap_or_default();
    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(true);

    let mut strings = Vec::new();
    let mut current_text = String::new();
    let mut in_si = false;
    let mut in_t = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if e.local_name().as_ref() == b"si" {
                    in_si = true;
                    current_text.clear();
                }
                if e.local_name().as_ref() == b"t" && in_si {
                    in_t = true;
                }
            }
            Ok(Event::Text(e)) => {
                if in_t {
                    current_text.push_str(&e.unescape().unwrap_or_default());
                }
            }
            Ok(Event::End(e)) => {
                if e.local_name().as_ref() == b"t" {
                    in_t = false;
                }
                if e.local_name().as_ref() == b"si" {
                    in_si = false;
                    strings.push(current_text.clone());
                }
            }
            Ok(Event::Empty(e)) => {
                // Handle <t/> empty text elements
                if e.local_name().as_ref() == b"t" && in_si {
                    // Empty text — nothing to add
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
    }

    strings
}

/// Parse a single worksheet XML and extract cells.
pub fn parse_sheet(
    package: &OxmlPackage,
    part_path: &str,
    shared_strings: &[String],
) -> Result<Worksheet, String> {
    let xml = package
        .read_part_xml(part_path)
        .map_err(|e| format!("failed to read {}: {}", part_path, e))?;

    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(true);

    let mut cells: std::collections::HashMap<(usize, usize), Cell> =
        std::collections::HashMap::new();
    let mut max_col: usize = 0;
    let mut max_row: usize = 0;

    let mut in_cell = false;
    let mut cell_ref_str = String::new();
    let mut cell_value_type = CellValueType::Number;
    let mut cell_style_index: Option<usize> = None;
    let mut cell_value_metadata_index: Option<usize> = None;
    let mut cell_value: Option<String> = None;
    let mut cell_formula: Option<String> = None;
    let mut in_v = false;
    let mut in_f = false;
    // Inside an inline-string cell (`t="inlineStr"`), the value lives in
    // <is><t>...</t></is>. Track nesting so we capture the text.
    let mut in_is = false;
    let mut in_is_t = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"c" => {
                    in_cell = true;
                    cell_ref_str.clear();
                    cell_value = None;
                    cell_formula = None;
                    cell_value_type = CellValueType::Number;
                    cell_style_index = None;
                    cell_value_metadata_index = None;

                    for attr in e.attributes().filter_map(|a| a.ok()) {
                        let key = attr.key.as_ref();
                        if key == b"r" {
                            cell_ref_str = String::from_utf8_lossy(attr.value.as_ref()).to_string();
                        } else if key == b"t" {
                            cell_value_type = CellValueType::from_attr(Some(
                                &String::from_utf8_lossy(attr.value.as_ref()),
                            ));
                        } else if key == b"s" {
                            let s_val = String::from_utf8_lossy(attr.value.as_ref());
                            cell_style_index = s_val.parse::<usize>().ok();
                        } else if key == b"vm" {
                            let vm_val = String::from_utf8_lossy(attr.value.as_ref());
                            cell_value_metadata_index = vm_val.parse::<usize>().ok();
                        }
                    }
                }
                b"v" if in_cell => {
                    in_v = true;
                }
                b"f" if in_cell => {
                    in_f = true;
                }
                b"is" if in_cell => {
                    in_is = true;
                }
                b"t" if in_is => {
                    in_is_t = true;
                }
                _ => {}
            },
            Ok(Event::Text(e)) => {
                if in_v {
                    cell_value = Some(e.unescape().unwrap_or_default().to_string());
                }
                if in_f {
                    cell_formula = Some(formula::unqualify_for_readback(
                        &e.unescape().unwrap_or_default(),
                    ));
                }
                if in_is_t {
                    // Inline-string cells store text in <is><t>…</t></is>;
                    // collapse adjacent text runs into one display string.
                    let piece = e.unescape().unwrap_or_default().to_string();
                    match &mut cell_value {
                        Some(existing) => existing.push_str(&piece),
                        None => cell_value = Some(piece),
                    }
                }
            }
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"v" => in_v = false,
                b"f" => in_f = false,
                b"t" if in_is_t => in_is_t = false,
                b"is" => {
                    in_is = false;
                    in_is_t = false;
                }
                b"c" => {
                    in_cell = false;

                    let cref = CellRef::parse(&cell_ref_str);
                    if let Some(cr) = cref {
                        let display_value =
                            resolve_display_value(&cell_value_type, &cell_value, shared_strings);

                        if cr.col > max_col {
                            max_col = cr.col;
                        }
                        if cr.row > max_row {
                            max_row = cr.row;
                        }

                        cells.insert(
                            (cr.row, cr.col),
                            Cell {
                                ref_str: cell_ref_str.clone(),
                                col: cr.col,
                                row: cr.row,
                                value_type: cell_value_type.clone(),
                                raw_value: cell_value.clone(),
                                formula: cell_formula.clone(),
                                display_value,
                                style_index: cell_style_index,
                                value_metadata_index: cell_value_metadata_index,
                            },
                        );
                    }
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => {
                // Handle <c r="A1"/> cells without v or f children
                if e.local_name().as_ref() == b"c" {
                    cell_ref_str.clear();
                    cell_value_type = CellValueType::Number;
                    cell_style_index = None;
                    cell_value_metadata_index = None;

                    for attr in e.attributes().filter_map(|a| a.ok()) {
                        let key = attr.key.as_ref();
                        if key == b"r" {
                            cell_ref_str = String::from_utf8_lossy(attr.value.as_ref()).to_string();
                        } else if key == b"t" {
                            cell_value_type = CellValueType::from_attr(Some(
                                &String::from_utf8_lossy(attr.value.as_ref()),
                            ));
                        } else if key == b"s" {
                            let s_val = String::from_utf8_lossy(attr.value.as_ref());
                            cell_style_index = s_val.parse::<usize>().ok();
                        } else if key == b"vm" {
                            let vm_val = String::from_utf8_lossy(attr.value.as_ref());
                            cell_value_metadata_index = vm_val.parse::<usize>().ok();
                        }
                    }

                    let cref = CellRef::parse(&cell_ref_str);
                    if let Some(cr) = cref {
                        if cr.col > max_col {
                            max_col = cr.col;
                        }
                        if cr.row > max_row {
                            max_row = cr.row;
                        }

                        cells.insert(
                            (cr.row, cr.col),
                            Cell {
                                ref_str: cell_ref_str.clone(),
                                col: cr.col,
                                row: cr.row,
                                value_type: cell_value_type.clone(),
                                raw_value: None,
                                formula: None,
                                display_value: String::new(),
                                style_index: cell_style_index,
                                value_metadata_index: cell_value_metadata_index,
                            },
                        );
                    }
                }
                // Handle <v/> or <f/> empty elements
                if e.local_name().as_ref() == b"v" && in_cell {
                    cell_value = Some(String::new());
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error in {}: {}", part_path, e)),
            _ => {}
        }
    }

    Ok(Worksheet {
        name: String::new(),
        index: 0,
        part_path: part_path.to_string(),
        rel_id: String::new(),
        cells,
        max_col,
        max_row,
    })
}

/// Resolve the display value for a cell, considering its type and shared strings.
fn resolve_display_value(
    value_type: &CellValueType,
    raw_value: &Option<String>,
    shared_strings: &[String],
) -> String {
    match value_type {
        CellValueType::SharedString => {
            if let Some(val) = raw_value {
                let idx = val.parse::<usize>().unwrap_or(0);
                if idx < shared_strings.len() {
                    shared_strings[idx].clone()
                } else {
                    format!("[ss:{}]", idx)
                }
            } else {
                String::new()
            }
        }
        CellValueType::Boolean => {
            if let Some(val) = raw_value {
                if val == "1" {
                    "TRUE".to_string()
                } else if val == "0" {
                    "FALSE".to_string()
                } else {
                    val.clone()
                }
            } else {
                "".to_string()
            }
        }
        CellValueType::Error => raw_value.clone().unwrap_or_default(),
        CellValueType::InlineString | CellValueType::Number => {
            raw_value.clone().unwrap_or_default()
        }
    }
}

/// Build the full workbook model from the package, including formula evaluation.
pub fn build_workbook_model(package: &OxmlPackage) -> Result<WorkbookModel, String> {
    let shared_strings = parse_shared_strings(package);
    let sheet_info = parse_workbook(package)?;

    let mut sheets = Vec::new();
    for (idx, (name, part_path, rel_id)) in sheet_info.iter().enumerate() {
        let ws = parse_sheet(package, part_path, &shared_strings)?;
        sheets.push(Worksheet {
            name: name.clone(),
            index: idx + 1,
            part_path: part_path.clone(),
            rel_id: rel_id.clone(),
            ..ws
        });
    }

    let mut model = WorkbookModel {
        sheets,
        shared_strings,
        pivot_tables: Vec::new(),
        tables: Vec::new(),
    };

    // Evaluate formulas and populate display_value for formula cells
    evaluate_formulas(&mut model);

    // Discover pivot table definitions
    model.pivot_tables = parse_pivot_tables(package);

    // Discover ListObject (Excel Table) definitions
    model.tables = parse_list_objects(package, &model.sheets);

    Ok(model)
}

/// Evaluate all formula cells in the workbook and update their display_value.
fn evaluate_formulas(model: &mut WorkbookModel) {
    use crate::formula;

    // Collect all formula cells (sheet_idx, (row, col), formula_text)
    let formula_cells: Vec<(usize, (usize, usize), String)> = model
        .sheets
        .iter()
        .flat_map(|ws| {
            ws.cells
                .iter()
                .filter(|(_, c)| c.formula.is_some())
                .map(|(key, c)| (ws.index - 1, *key, c.formula.clone().unwrap_or_default()))
        })
        .collect();

    for (sheet_idx, key, formula_text) in formula_cells {
        if sheet_idx >= model.sheets.len() {
            continue;
        }
        // Strip the leading '=' if present
        let expr = formula_text.strip_prefix('=').unwrap_or(&formula_text);
        if let Some(result) = formula::evaluate(expr, model) {
            if let Some(cell) = model.sheets[sheet_idx].cells.get_mut(&key) {
                cell.display_value = result.as_string();
            }
        }
    }
}

/// Parse all pivot table definitions from xl/pivotTables/*.xml parts.
fn parse_pivot_tables(package: &OxmlPackage) -> Vec<PivotTableDef> {
    let mut pivot_tables = Vec::new();

    // Find all pivot table parts
    let all_parts = package.list_parts();
    let pivot_parts: Vec<String> = all_parts
        .iter()
        .filter(|p| p.starts_with("xl/pivotTables/") && p.ends_with(".xml"))
        .map(|s| (*s).clone())
        .collect();

    for part_path in &pivot_parts {
        let xml = match package.read_part_xml(part_path) {
            Ok(x) => x,
            Err(_) => continue,
        };

        let mut name = String::new();
        let mut cache_id: Option<String> = None;
        let mut field_count: usize = 0;
        let mut location: Option<String> = None;
        let mut row_fields: Vec<i32> = Vec::new();
        let mut col_fields: Vec<i32> = Vec::new();
        let mut page_fields: Vec<i32> = Vec::new();
        let mut data_fields: Vec<(String, String, i32)> = Vec::new();
        let mut data_field_show_as: Vec<Option<String>> = Vec::new();
        let repeat_labels = xml.contains("fillDownLabelsDefault=\"1\"")
            || xml.contains("fillDownLabelsDefault=\"true\"");
        let blank_rows =
            xml.contains("insertBlankRow=\"1\"") || xml.contains("insertBlankRow=\"true\"");
        // Track which "fields container" we're currently inside so that
        // nested <field idx="N"/> / <pageField fld="N"/> events route correctly.
        let mut container_scope: PivotFieldsContainer = PivotFieldsContainer::None;

        let mut reader = Reader::from_str(&xml);
        reader.config_mut().trim_text(true);

        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    let local_name = e.local_name();
                    let local_name_ref: &[u8] = local_name.as_ref();
                    match local_name_ref {
                        b"pivotTableDefinition" => {
                            for attr in e.attributes().filter_map(|a| a.ok()) {
                                let key = attr.key.as_ref();
                                if key == b"name" {
                                    name = String::from_utf8_lossy(attr.value.as_ref()).to_string();
                                } else if key == b"cacheId" {
                                    cache_id = Some(
                                        String::from_utf8_lossy(attr.value.as_ref()).to_string(),
                                    );
                                }
                            }
                        }
                        b"pivotFields" => {
                            for attr in e.attributes().filter_map(|a| a.ok()) {
                                if attr.key.as_ref() == b"count" {
                                    field_count = String::from_utf8_lossy(attr.value.as_ref())
                                        .parse()
                                        .unwrap_or(0);
                                }
                            }
                        }
                        b"location" => {
                            for attr in e.attributes().filter_map(|a| a.ok()) {
                                if attr.key.as_ref() == b"ref" {
                                    location = Some(
                                        String::from_utf8_lossy(attr.value.as_ref()).to_string(),
                                    );
                                }
                            }
                        }
                        b"rowFields" => container_scope = PivotFieldsContainer::Row,
                        b"colFields" => container_scope = PivotFieldsContainer::Col,
                        b"pageFields" => container_scope = PivotFieldsContainer::Page,
                        b"field" => match container_scope {
                            PivotFieldsContainer::Row => {
                                if let Some(idx) = read_field_idx(&e) {
                                    if idx >= 0 {
                                        row_fields.push(idx);
                                    }
                                }
                            }
                            PivotFieldsContainer::Col => {
                                if let Some(idx) = read_field_idx(&e) {
                                    if idx >= 0 {
                                        col_fields.push(idx);
                                    }
                                }
                            }
                            _ => {}
                        },
                        b"pageField" if matches!(container_scope, PivotFieldsContainer::Page) => {
                            if let Some(fld) = read_page_field_fld(&e) {
                                if fld >= 0 {
                                    page_fields.push(fld);
                                }
                            }
                        }
                        b"dataField" => {
                            let mut df_name = String::new();
                            let mut df_func = "sum".to_string();
                            let mut df_field: i32 = 0;
                            let mut df_show_as: Option<String> = None;
                            for attr in e.attributes().filter_map(|a| a.ok()) {
                                let key = attr.key.as_ref();
                                let val = String::from_utf8_lossy(attr.value.as_ref());
                                match key {
                                    b"name" => df_name = val.to_string(),
                                    b"subtotal" => df_func = val.to_string(),
                                    b"fld" => df_field = val.parse().unwrap_or(0),
                                    b"showDataAs" => df_show_as = Some(val.to_string()),
                                    _ => {}
                                }
                            }
                            data_fields.push((df_name, df_func, df_field));
                            data_field_show_as.push(df_show_as);
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(e)) => {
                    let local_name = e.local_name();
                    let local_name_ref: &[u8] = local_name.as_ref();
                    if matches!(local_name_ref, b"rowFields" | b"colFields" | b"pageFields") {
                        container_scope = PivotFieldsContainer::None;
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
        }

        // Try to resolve the source range from the pivot cache definition
        let source_range = cache_id
            .as_ref()
            .and_then(|cid| resolve_pivot_cache_range_for_part(package, part_path, cid));
        // Try to load cacheField names so we can resolve field indices to names.
        let cache_fields = cache_id
            .as_ref()
            .and_then(|cid| resolve_pivot_cache_fields_for_part(package, part_path, cid))
            .unwrap_or_default();

        pivot_tables.push(PivotTableDef {
            name,
            cache_id,
            source_range,
            field_count,
            part_path: part_path.clone(),
            location,
            cache_fields,
            row_fields,
            col_fields,
            page_fields,
            data_fields,
            data_field_show_as,
            repeat_labels,
            blank_rows,
        });
    }

    pivot_tables
}

/// Resolve the Nth PivotTable referenced by one worksheet.  The public L2
/// path is sheet-scoped (`/Sheet1/pivottable[1]`), while pivot definitions are
/// stored in a separate package folder, so the worksheet's r:id relation is
/// the authoritative bridge between those two views.
pub(crate) fn pivot_part_path_for_sheet(
    package: &OxmlPackage,
    sheet_name: &str,
    index: usize,
) -> Result<String, HandlerError> {
    if index == 0 {
        return Err(HandlerError::InvalidArgument(
            "pivottable indexes are 1-based".to_string(),
        ));
    }
    let sheet = build_workbook_model(package)
        .map_err(HandlerError::OperationFailed)?
        .sheets
        .into_iter()
        .find(|sheet| sheet.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;
    let xml = package
        .read_part_xml(&sheet.part_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let document = roxmltree::Document::parse(&xml).map_err(|error| {
        HandlerError::OperationFailed(format!("invalid worksheet XML: {}", error))
    })?;
    const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let relationship_id = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "pivotTablePart")
        .nth(index - 1)
        .and_then(|node| {
            node.attribute((REL_NS, "id"))
                .or_else(|| node.attribute("r:id"))
        })
        .ok_or_else(|| HandlerError::PathNotFound(format!("pivottable[{}]", index)))?;
    let relationships = package
        .part_rels(&sheet.part_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let relationship = relationships.get(relationship_id).ok_or_else(|| {
        HandlerError::OperationFailed(format!("relationship {} not found", relationship_id))
    })?;
    Ok(package.resolve_rel_target(&sheet.part_path, &relationship.target))
}

/// Which pivotFields container the parser is currently inside.
#[derive(Clone, Copy, PartialEq)]
enum PivotFieldsContainer {
    None,
    Row,
    Col,
    Page,
}

/// Extract the pivot-field index from a `<field x="N"/>` event.
///
/// OOXML's rowFields/colFields use `x`; a few producer variants use `idx`,
/// so accept both when reading an externally-authored package.
fn read_field_idx(e: &quick_xml::events::BytesStart<'_>) -> Option<i32> {
    for attr in e.attributes().filter_map(|a| a.ok()) {
        if matches!(attr.key.as_ref(), b"x" | b"idx") {
            return String::from_utf8_lossy(attr.value.as_ref()).parse().ok();
        }
    }
    None
}

/// Extract the `fld` attribute from a `<pageField fld="N"/>` event.
fn read_page_field_fld(e: &quick_xml::events::BytesStart<'_>) -> Option<i32> {
    for attr in e.attributes().filter_map(|a| a.ok()) {
        if attr.key.as_ref() == b"fld" {
            return String::from_utf8_lossy(attr.value.as_ref()).parse().ok();
        }
    }
    None
}

/// Resolve `cacheFields` names from the pivotCacheDefinition referenced by
/// `cache_id`. Returns ordered names (index in vec == pivotField index).
fn resolve_pivot_cache_fields_for_part(
    package: &OxmlPackage,
    pivot_part_path: &str,
    cache_id: &str,
) -> Option<Vec<String>> {
    let cache_xml = find_pivot_cache_definition_xml_for_part(package, pivot_part_path, cache_id)?;
    let mut names = Vec::new();
    let mut reader = Reader::from_str(&cache_xml);
    reader.config_mut().trim_text(true);
    let mut in_cache_fields = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let local_name = e.local_name();
                let local_name_ref: &[u8] = local_name.as_ref();
                if local_name_ref == b"cacheFields" {
                    in_cache_fields = true;
                    continue;
                }
                if in_cache_fields && local_name_ref == b"cacheField" {
                    let mut name = String::new();
                    for attr in e.attributes().filter_map(|a| a.ok()) {
                        if attr.key.as_ref() == b"name" {
                            name = String::from_utf8_lossy(attr.value.as_ref()).to_string();
                        }
                    }
                    names.push(name);
                }
            }
            Ok(Event::End(e)) => {
                let local_name = e.local_name();
                let local_name_ref: &[u8] = local_name.as_ref();
                if local_name_ref == b"cacheFields" {
                    in_cache_fields = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    Some(names)
}

/// Find the pivotCacheDefinition XML associated with the given pivot table
/// part. Resolution order:
///   1. Walk the pivot table's `_rels/pivotTableN.xml.rels` file for a
///      `pivotCacheDefinition` relationship target — that's the authoritative
///      mapping (cache_id is an index into the workbook's cache list, not
///      directly into the cache definition XML).
///   2. Fall back to scanning all cache parts and returning the one whose
///      `cacheId` attribute matches (rare, but some files do store it).
///   3. Fall back to the only cache when the workbook has exactly one.
fn find_pivot_cache_definition_xml_for_part(
    package: &OxmlPackage,
    pivot_part_path: &str,
    cache_id: &str,
) -> Option<String> {
    // Try the rels file first.
    if let Some(cache_path) = resolve_pivot_cache_via_rels(package, pivot_part_path) {
        if let Ok(xml) = package.read_part_xml(&cache_path) {
            return Some(xml);
        }
    }
    // Fallbacks.
    let all_parts = package.list_parts();
    let cache_parts: Vec<String> = all_parts
        .iter()
        .filter(|p| {
            (p.starts_with("xl/pivotCache/") || p.starts_with("pivotCache/"))
                && p.contains("pivotCacheDefinition")
        })
        .map(|s| (*s).clone())
        .collect();
    if cache_parts.is_empty() {
        return None;
    }
    for cache_part in &cache_parts {
        if let Ok(xml) = package.read_part_xml(cache_part) {
            if xml.contains(&format!("cacheId=\"{}\"", cache_id)) {
                return Some(xml);
            }
        }
    }
    if cache_parts.len() == 1 {
        return package.read_part_xml(&cache_parts[0]).ok();
    }
    None
}

/// Walk `xl/pivotTables/_rels/pivotTableN.xml.rels` (or the equivalent for
/// any pivot part) and return the absolute path of the related cache
/// definition. Returns the path as it appears in the package (relative
/// targets starting with `/` or `../` are normalized).
fn resolve_pivot_cache_via_rels(package: &OxmlPackage, pivot_part_path: &str) -> Option<String> {
    let rels_path = pivot_part_rels_path(pivot_part_path)?;
    let rels_xml = package.read_part_xml(&rels_path).ok()?;
    let mut reader = Reader::from_str(&rels_xml);
    reader.config_mut().trim_text(true);
    let mut target: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let local_name = e.local_name();
                let local_name_ref: &[u8] = local_name.as_ref();
                if local_name_ref == b"Relationship" {
                    let mut t = None;
                    let mut typ = None;
                    for attr in e.attributes().filter_map(|a| a.ok()) {
                        match attr.key.as_ref() {
                            b"Target" => {
                                t = Some(String::from_utf8_lossy(attr.value.as_ref()).to_string());
                            }
                            b"Type" => {
                                typ =
                                    Some(String::from_utf8_lossy(attr.value.as_ref()).to_string());
                            }
                            _ => {}
                        }
                    }
                    if let (Some(t), Some(typ)) = (t, typ) {
                        if typ.ends_with("/pivotCacheDefinition") {
                            target = Some(t);
                            break;
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    let raw = target?;
    Some(resolve_part_target_from(pivot_part_path, &raw))
}

/// Resolve an OPC relationship target against its owning part.  Pivot cache
/// definitions are normally referenced from `xl/pivotTables/*` with a target
/// such as `../pivotCache/pivotCacheDefinition1.xml`; returning that raw
/// target made the parser silently miss otherwise valid native pivots.
fn resolve_part_target_from(owner_part: &str, target: &str) -> String {
    if let Some(stripped) = target.strip_prefix('/') {
        return stripped.to_string();
    }
    let mut segments: Vec<&str> = owner_part
        .rsplit_once('/')
        .map(|(directory, _)| directory.split('/').collect())
        .unwrap_or_default();
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

/// Construct the path to the `_rels/pivotTableN.xml.rels` part for a given
/// pivot table part path. E.g. `xl/pivotTables/pivotTable1.xml` →
/// `xl/pivotTables/_rels/pivotTable1.xml.rels`.
fn pivot_part_rels_path(pivot_part_path: &str) -> Option<String> {
    let slash = pivot_part_path.rfind('/')?;
    let dir = &pivot_part_path[..slash];
    let file = &pivot_part_path[slash + 1..];
    Some(format!("{}/_rels/{}.rels", dir, file))
}

/// Parse every ListObject (Excel "Table") from `xl/tables/tableN.xml` parts.
/// Each table's `ref` attribute (e.g. `A1:D10`) is parsed into an absolute
/// range and bound to the worksheet whose cells include that range.
fn parse_list_objects(package: &OxmlPackage, sheets: &[Worksheet]) -> Vec<ListObjectDef> {
    let mut out = Vec::new();
    let parts = package.list_parts();
    let table_parts: Vec<String> = parts
        .iter()
        .filter(|p| p.starts_with("xl/tables/") && p.ends_with(".xml"))
        .map(|s| (*s).clone())
        .collect();

    for part_path in &table_parts {
        let xml = match package.read_part_xml(part_path) {
            Ok(x) => x,
            Err(_) => continue,
        };

        let parsed = match parse_table_xml(&xml) {
            Some(t) => t,
            None => continue,
        };
        let (ref_str, name, display_name, header_row, totals_row, columns) = parsed;

        let (r1, c1, r2, c2) = match parse_a1_range(&ref_str) {
            Some(r) => r,
            None => continue,
        };

        // Bind to the sheet that owns the table's top-left cell. The
        // relationship is encoded via worksheet rels (table/*), but cell
        // presence on the sheet is a good proxy and avoids walking rels.
        let sheet_name = sheets
            .iter()
            .find(|s| s.cells.contains_key(&(r1, c1)))
            .or_else(|| sheets.iter().find(|s| s.cells.contains_key(&(r2, c2))))
            .map(|s| s.name.clone())
            .unwrap_or_else(|| sheets.first().map(|s| s.name.clone()).unwrap_or_default());

        out.push(ListObjectDef {
            name,
            display_name,
            sheet_name,
            part_path: part_path.clone(),
            range: (r1, c1, r2, c2),
            columns,
            header_row,
            totals_row,
        });
    }

    out
}

/// Extract (ref, name, displayName, headerRowCount?, totalsRowCount?, columns)
/// from a single `<table>` definition. Returns None if the table has no `ref`.
fn parse_table_xml(xml: &str) -> Option<(String, String, String, bool, bool, Vec<String>)> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut ref_attr: Option<String> = None;
    let mut name_attr = String::new();
    let mut display_attr = String::new();
    let mut header_row_count: Option<i64> = None;
    let mut totals_row_count: Option<i64> = None;
    let mut columns = Vec::new();
    let mut in_columns = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let local = e.local_name();
                let local_ref: &[u8] = local.as_ref();
                match local_ref {
                    b"table" => {
                        for attr in e.attributes().flatten() {
                            let k = attr.key.as_ref();
                            let v = std::str::from_utf8(attr.value.as_ref()).unwrap_or("");
                            match k {
                                b"ref" => ref_attr = Some(v.to_string()),
                                b"name" => name_attr = v.to_string(),
                                b"displayName" => display_attr = v.to_string(),
                                b"headerRowCount" => {
                                    header_row_count = v.parse().ok();
                                }
                                b"totalsRowCount" => {
                                    totals_row_count = v.parse().ok();
                                }
                                _ => {}
                            }
                        }
                    }
                    b"tableColumns" => {
                        in_columns = true;
                    }
                    b"tableColumn" if in_columns => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"name" {
                                let v = std::str::from_utf8(attr.value.as_ref()).unwrap_or("");
                                columns.push(v.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                if e.local_name().as_ref() == b"tableColumns" {
                    in_columns = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    let r = ref_attr?;
    let header = header_row_count.unwrap_or(1) != 0;
    let totals = totals_row_count.unwrap_or(0) > 0;
    Some((r, name_attr, display_attr, header, totals, columns))
}

/// Parse `A1:D10` into `(start_row, start_col, end_row, end_col)`, 1-based.
fn parse_a1_range(s: &str) -> Option<(usize, usize, usize, usize)> {
    let (left, right) = s.split_once(':')?;
    let start = CellRef::parse(left)?;
    if right.eq_ignore_ascii_case(left) {
        return Some((start.row, start.col, start.row, start.col));
    }
    let end = CellRef::parse(right)?;
    let (r1, r2) = (start.row.min(end.row), start.row.max(end.row));
    let (c1, c2) = (start.col.min(end.col), start.col.max(end.col));
    Some((r1, c1, r2, c2))
}

/// Resolve the source range for a pivot table from its cache definition,
/// preferring the cache referenced from the pivot table's own rels file.
fn resolve_pivot_cache_range_for_part(
    package: &OxmlPackage,
    pivot_part_path: &str,
    cache_id: &str,
) -> Option<String> {
    if let Some(cache_xml) =
        find_pivot_cache_definition_xml_for_part(package, pivot_part_path, cache_id)
    {
        return extract_source_range_from_cache_xml(&cache_xml);
    }
    None
}

/// Back-compat shim used by older callers (none currently in-tree, but kept
/// for any future callers that only have a cache_id).
#[allow(dead_code)]
fn resolve_pivot_cache_range(package: &OxmlPackage, cache_id: &str) -> Option<String> {
    // The cache ID is referenced from the pivot table definition.
    // We need to find the pivotCacheDefinition that corresponds to this cache ID.
    // The mapping is through workbook.xml relationships.

    // Try to find the cache definition by looking at pivotCache parts
    let all_parts = package.list_parts();
    let cache_parts: Vec<String> = all_parts
        .iter()
        .filter(|p| {
            (p.starts_with("xl/pivotCache/") || p.starts_with("pivotCache/"))
                && p.contains("pivotCacheDefinition")
        })
        .map(|s| (*s).clone())
        .collect();

    for cache_part in &cache_parts {
        let xml = package.read_part_xml(cache_part).ok()?;

        // Check if this cache definition references the same cache ID
        if !xml.contains(&format!("cacheId=\"{}\"", cache_id))
            && !xml.contains(&format!("cacheId=\"{}\" ", cache_id))
            && !xml.contains(&format!("cacheId=\"{}\">", cache_id))
        {
            // If we can't match by cacheId, try the first cache definition
            // (many workbooks have only one)
            if cache_parts.len() > 1 {
                continue;
            }
        }

        if let Some(range) = extract_source_range_from_cache_xml(&xml) {
            return Some(range);
        }
    }

    None
}

/// Parse a pivotCacheDefinition XML and return the worksheet source range in
/// "Sheet!Ref" form (or just "Ref" when no sheet attribute is present).
/// Returns None for non-worksheet cache sources.
fn extract_source_range_from_cache_xml(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut sheet = String::new();
    let mut ref_val: Option<String> = None;
    let mut saw_source = false;
    let mut source_is_worksheet = true;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let local_name = e.local_name();
                let local_name_ref: &[u8] = local_name.as_ref();
                if local_name_ref == b"cacheSource" {
                    saw_source = true;
                    for attr in e.attributes().filter_map(|a| a.ok()) {
                        if attr.key.as_ref() == b"type" {
                            let val = String::from_utf8_lossy(attr.value.as_ref());
                            if val != "worksheet" {
                                source_is_worksheet = false;
                            }
                        }
                    }
                }
                if local_name_ref == b"worksheetSource" {
                    for attr in e.attributes().filter_map(|a| a.ok()) {
                        let key = attr.key.as_ref();
                        let val = String::from_utf8_lossy(attr.value.as_ref()).to_string();
                        if key == b"ref" {
                            ref_val = Some(val);
                        } else if key == b"sheet" {
                            sheet = val;
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    if !saw_source || !source_is_worksheet {
        return None;
    }
    let r = ref_val?;
    if sheet.is_empty() {
        Some(r)
    } else {
        Some(format!("{}!{}", sheet, r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pivot_part_rels_path_for_xl_prefix() {
        assert_eq!(
            pivot_part_rels_path("xl/pivotTables/pivotTable1.xml"),
            Some("xl/pivotTables/_rels/pivotTable1.xml.rels".to_string())
        );
        assert_eq!(
            pivot_part_rels_path("xl/pivotTables/pivotTable2.xml"),
            Some("xl/pivotTables/_rels/pivotTable2.xml.rels".to_string())
        );
        assert!(pivot_part_rels_path("nopath").is_none());
    }

    #[test]
    fn resolve_target_handles_absolute_and_relative() {
        assert_eq!(
            resolve_part_target_from("xl/pivotTables/pivotTable1.xml", "/pivotCache/x.xml"),
            "pivotCache/x.xml"
        );
        assert_eq!(
            resolve_part_target_from("xl/pivotTables/pivotTable1.xml", "pivotCache/x.xml"),
            "xl/pivotTables/pivotCache/x.xml"
        );
        assert_eq!(
            resolve_part_target_from("xl/pivotTables/pivotTable1.xml", "../pivotCache/x.xml"),
            "xl/pivotCache/x.xml"
        );
    }

    #[test]
    fn extract_source_range_returns_sheet_qualified_form() {
        let xml = r#"<pivotCacheDefinition xmlns:x="x">
            <x:cacheSource type="worksheet">
              <x:worksheetSource ref="A1:J51" sheet="Sheet1" />
            </x:cacheSource>
          </pilotCacheDefinition>"#;
        assert_eq!(
            extract_source_range_from_cache_xml(xml),
            Some("Sheet1!A1:J51".to_string())
        );
    }

    #[test]
    fn extract_source_range_bare_when_no_sheet() {
        let xml = r#"<pivotCacheDefinition xmlns:x="x">
            <x:cacheSource type="worksheet">
              <x:worksheetSource ref="A1:C13" />
            </x:cacheSource>
          </pilotCacheDefinition>"#;
        assert_eq!(
            extract_source_range_from_cache_xml(xml),
            Some("A1:C13".to_string())
        );
    }

    #[test]
    fn extract_source_range_none_for_non_worksheet() {
        let xml = r#"<pivotCacheDefinition xmlns:x="x">
            <x:cacheSource type="external" />
          </pilotCacheDefinition>"#;
        assert_eq!(extract_source_range_from_cache_xml(xml), None);
    }

    #[test]
    fn extract_source_range_none_when_no_source() {
        let xml = r#"<pivotCacheDefinition xmlns:x="x">
            <x:cacheFields count="0" />
          </pilotCacheDefinition>"#;
        assert_eq!(extract_source_range_from_cache_xml(xml), None);
    }
}
