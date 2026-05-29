/// Parsing helpers for xlsx OOXML parts.
use crate::dom_types::*;
use oxml::OxmlPackage;
use quick_xml::events::Event;
use quick_xml::Reader;

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
                            .map(|a| String::from_utf8_lossy(a.value.as_ref()).to_string())
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
    let mut cell_value: Option<String> = None;
    let mut cell_formula: Option<String> = None;
    let mut in_v = false;
    let mut in_f = false;

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
                        }
                    }
                }
                b"v" if in_cell => {
                    in_v = true;
                }
                b"f" if in_cell => {
                    in_f = true;
                }
                _ => {}
            },
            Ok(Event::Text(e)) => {
                if in_v {
                    cell_value = Some(e.unescape().unwrap_or_default().to_string());
                }
                if in_f {
                    cell_formula = Some(e.unescape().unwrap_or_default().to_string());
                }
            }
            Ok(Event::End(e)) => match e.local_name().as_ref() {
                b"v" => in_v = false,
                b"f" => in_f = false,
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

/// Build the full workbook model from the package.
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

    Ok(WorkbookModel {
        sheets,
        shared_strings,
    })
}
