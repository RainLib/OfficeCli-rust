/// Mutation operations for xlsx documents: set cell values, formulas, remove, move, copy.
use crate::dom_types::*;
use crate::dynamic_array;
use crate::formula;
use crate::helpers;
use crate::navigation;
use crate::rich_value_image;
use handler_common::{
    self, extract_find_replace_props, replace_in_string, FindReplaceOptions, HandlerError,
    InsertPosition,
};
use oxml::OxmlPackage;
use std::collections::HashMap;
use std::ops::Range;

/// Remove an element from the workbook.
/// Supported paths:
///   /SheetName/A1 — remove a cell (clear its content from the worksheet XML)
///   /SheetName     — remove a sheet (remove part + update workbook.xml)
pub fn remove_element(
    package: &mut OxmlPackage,
    path: &str,
) -> Result<Option<String>, HandlerError> {
    if let Some((sheet_name, pivot_index)) = parse_pivot_path(path) {
        remove_pivot_table(package, &sheet_name, pivot_index)?;
        return Ok(Some(format!(
            "removed pivot table {} on {}",
            pivot_index, sheet_name
        )));
    }
    let pc = navigation::parse_path(path)?;

    match (pc.sheet_name, pc.cell_ref) {
        (Some(sheet_name), Some(cell_ref)) => {
            remove_cell(package, &sheet_name, &cell_ref)?;
            Ok(Some(format!(
                "removed cell {}{}",
                sheet_name,
                cell_ref.to_string_ref()
            )))
        }
        (Some(sheet_name), None) => {
            remove_sheet(package, &sheet_name)?;
            Ok(Some(format!("removed sheet {}", sheet_name)))
        }
        (None, None) => Err(HandlerError::InvalidPath(
            "remove requires a sheet or cell path".to_string(),
        )),
        (None, Some(_)) => Err(HandlerError::InvalidPath(
            "cell path requires a sheet name".to_string(),
        )),
    }
}

/// Parse the public C#-compatible `/Sheet/pivottable[N]` path.
fn parse_pivot_path(path: &str) -> Option<(String, usize)> {
    let normalized = path.trim_matches('/');
    let (sheet, tail) = normalized.rsplit_once('/')?;
    let tail = tail.to_ascii_lowercase();
    let index = tail
        .strip_prefix("pivottable[")
        .or_else(|| tail.strip_prefix("pivot["))?
        .strip_suffix(']')?
        .parse::<usize>()
        .ok()?;
    (index > 0 && !sheet.is_empty()).then(|| (sheet.to_string(), index))
}

/// Remove a PivotTable and only prune its cache tree if no sibling still uses
/// it.  Native Excel pivots may share a `pivotCacheDefinition`, so deleting a
/// pivot must not blindly remove its cache/records or the surviving pivot is
/// left with an unreadable relation tree.
fn remove_pivot_table(
    package: &mut OxmlPackage,
    sheet_name: &str,
    pivot_index: usize,
) -> Result<(), HandlerError> {
    const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;
    let sheet = model
        .sheets
        .iter()
        .find(|sheet| sheet.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;
    let worksheet_xml = package
        .read_part_xml(&sheet.part_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let worksheet_doc = roxmltree::Document::parse(&worksheet_xml).map_err(|error| {
        HandlerError::OperationFailed(format!("invalid worksheet XML: {}", error))
    })?;
    let parts_container = worksheet_doc
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "pivotTableParts")
        .ok_or_else(|| HandlerError::PathNotFound(format!("pivottable[{}]", pivot_index)))?;
    let pivot_nodes: Vec<_> = parts_container
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "pivotTablePart")
        .collect();
    let pivot_node = pivot_nodes
        .get(pivot_index - 1)
        .ok_or_else(|| HandlerError::PathNotFound(format!("pivottable[{}]", pivot_index)))?;
    let worksheet_rel_id = pivot_node
        .attribute((REL_NS, "id"))
        .or_else(|| pivot_node.attribute("r:id"))
        .ok_or_else(|| HandlerError::OperationFailed("pivotTablePart has no r:id".to_string()))?
        .to_string();
    let worksheet_rels_path = relationships_part_path(&sheet.part_path);
    let worksheet_rels = package
        .read_part_xml(&worksheet_rels_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let pivot_path =
        relationship_target_for_id(&worksheet_rels, &sheet.part_path, &worksheet_rel_id)?;
    let pivot_rels_path = relationships_part_path(&pivot_path);
    let pivot_rels = package
        .read_part_xml(&pivot_rels_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let cache_path = relationship_target_by_type(&pivot_rels, &pivot_path, "pivotCacheDefinition")?;

    // First unlink the definition from the worksheet and its owning relation.
    let updated_worksheet = if pivot_nodes.len() == 1 {
        remove_ranges(&worksheet_xml, vec![parts_container.range()])
    } else {
        let mut updated = remove_ranges(&worksheet_xml, vec![pivot_node.range()]);
        let count = pivot_nodes.len() - 1;
        if let Some(open_end) = updated[parts_container.range().start..]
            .find('>')
            .map(|offset| parts_container.range().start + offset + 1)
        {
            let opening = updated[parts_container.range().start..open_end].to_string();
            let rewritten = if opening.contains("count=\"") {
                let start = opening.find("count=\"").unwrap() + "count=\"".len();
                let end = opening[start..]
                    .find('"')
                    .map(|offset| start + offset)
                    .unwrap_or(start);
                format!("{}{}{}", &opening[..start], count, &opening[end..])
            } else {
                opening.replacen('>', &format!(" count=\"{}\">", count), 1)
            };
            updated.replace_range(parts_container.range().start..open_end, &rewritten);
        }
        updated
    };
    package
        .write_part_xml(&sheet.part_path, &updated_worksheet)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    package
        .write_part_xml(
            &worksheet_rels_path,
            &remove_relationship_by_id(&worksheet_rels, &worksheet_rel_id)?,
        )
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    remove_part_if_present(package, &pivot_path)?;
    remove_part_if_present(package, &pivot_rels_path)?;
    remove_content_type_part(package, &pivot_path)?;

    // The deleted definition is now absent, so scanning all remaining pivot
    // relationships gives an authoritative cache-sharing answer.
    let cache_still_used = package
        .list_parts()
        .iter()
        .filter(|part| part.starts_with("xl/pivotTables/") && part.ends_with(".xml"))
        .any(|part| {
            let rels_path = relationships_part_path(part);
            package
                .read_part_xml(&rels_path)
                .ok()
                .and_then(|rels| {
                    relationship_target_by_type(&rels, part, "pivotCacheDefinition").ok()
                })
                .is_some_and(|target| target == cache_path)
        });
    if cache_still_used {
        return Ok(());
    }

    let cache_rels_path = relationships_part_path(&cache_path);
    let records_path = package
        .read_part_xml(&cache_rels_path)
        .ok()
        .and_then(|rels| relationship_target_by_type(&rels, &cache_path, "pivotCacheRecords").ok());
    let workbook_rels_path = "xl/_rels/workbook.xml.rels";
    let workbook_rels = package
        .read_part_xml(workbook_rels_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let cache_workbook_rel_id =
        relationship_id_for_target(&workbook_rels, "xl/workbook.xml", &cache_path)?;
    let workbook_xml = package
        .read_part_xml("xl/workbook.xml")
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let updated_workbook = remove_pivot_cache_entry(&workbook_xml, &cache_workbook_rel_id)?;
    package
        .write_part_xml("xl/workbook.xml", &updated_workbook)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    package
        .write_part_xml(
            workbook_rels_path,
            &remove_relationship_by_id(&workbook_rels, &cache_workbook_rel_id)?,
        )
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    remove_part_if_present(package, &cache_path)?;
    remove_part_if_present(package, &cache_rels_path)?;
    remove_content_type_part(package, &cache_path)?;
    if let Some(records_path) = records_path {
        remove_part_if_present(package, &records_path)?;
        remove_content_type_part(package, &records_path)?;
    }
    Ok(())
}

fn relationship_target_for_id(
    rels_xml: &str,
    owner_part: &str,
    relationship_id: &str,
) -> Result<String, HandlerError> {
    let doc = roxmltree::Document::parse(rels_xml).map_err(|error| {
        HandlerError::OperationFailed(format!("invalid relationships XML: {}", error))
    })?;
    let relationship = doc
        .descendants()
        .find(|node| {
            node.is_element()
                && node.tag_name().name() == "Relationship"
                && node.attribute("Id") == Some(relationship_id)
        })
        .ok_or_else(|| {
            HandlerError::OperationFailed(format!("relationship {} not found", relationship_id))
        })?;
    let target = relationship
        .attribute("Target")
        .ok_or_else(|| HandlerError::OperationFailed("relationship has no Target".to_string()))?;
    Ok(resolve_relationship_target(owner_part, target))
}

fn relationship_target_by_type(
    rels_xml: &str,
    owner_part: &str,
    type_suffix: &str,
) -> Result<String, HandlerError> {
    let doc = roxmltree::Document::parse(rels_xml).map_err(|error| {
        HandlerError::OperationFailed(format!("invalid relationships XML: {}", error))
    })?;
    let relationship = doc
        .descendants()
        .find(|node| {
            node.is_element()
                && node.tag_name().name() == "Relationship"
                && node
                    .attribute("Type")
                    .is_some_and(|relationship_type| relationship_type.ends_with(type_suffix))
        })
        .ok_or_else(|| {
            HandlerError::OperationFailed(format!("{} relationship not found", type_suffix))
        })?;
    let target = relationship
        .attribute("Target")
        .ok_or_else(|| HandlerError::OperationFailed("relationship has no Target".to_string()))?;
    Ok(resolve_relationship_target(owner_part, target))
}

fn relationship_id_for_target(
    rels_xml: &str,
    owner_part: &str,
    wanted_target: &str,
) -> Result<String, HandlerError> {
    let doc = roxmltree::Document::parse(rels_xml).map_err(|error| {
        HandlerError::OperationFailed(format!("invalid relationships XML: {}", error))
    })?;
    doc.descendants()
        .find(|node| {
            node.is_element()
                && node.tag_name().name() == "Relationship"
                && node.attribute("Target").is_some_and(|target| {
                    resolve_relationship_target(owner_part, target) == wanted_target
                })
        })
        .and_then(|node| node.attribute("Id"))
        .map(str::to_string)
        .ok_or_else(|| {
            HandlerError::OperationFailed(format!("relationship to {} not found", wanted_target))
        })
}

fn resolve_relationship_target(owner_part: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
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
            segment => segments.push(segment),
        }
    }
    segments.join("/")
}

fn remove_pivot_cache_entry(xml: &str, relationship_id: &str) -> Result<String, HandlerError> {
    let doc = roxmltree::Document::parse(xml).map_err(|error| {
        HandlerError::OperationFailed(format!("invalid workbook.xml: {}", error))
    })?;
    let cache = doc
        .descendants()
        .find(|node| {
            node.is_element()
                && node.tag_name().name() == "pivotCache"
                && (node.attribute((
                    "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
                    "id",
                )) == Some(relationship_id)
                    || node.attribute("r:id") == Some(relationship_id))
        })
        .ok_or_else(|| HandlerError::OperationFailed("pivot cache entry not found".to_string()))?;
    Ok(remove_ranges(xml, vec![cache.range()]))
}

fn remove_part_if_present(package: &mut OxmlPackage, part_path: &str) -> Result<(), HandlerError> {
    if package.has_part(part_path) {
        package
            .remove_part(part_path)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    }
    Ok(())
}

fn remove_content_type_part(
    package: &mut OxmlPackage,
    part_path: &str,
) -> Result<(), HandlerError> {
    let content_types = package
        .read_part_xml("[Content_Types].xml")
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let updated = remove_content_type_override(&content_types, part_path)?;
    if updated != content_types {
        package
            .write_part_xml("[Content_Types].xml", &updated)
            .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    }
    Ok(())
}

/// Remove a cell from a worksheet by finding and deleting its <c> element.
fn remove_cell(
    package: &mut OxmlPackage,
    sheet_name: &str,
    cell_ref: &CellRef,
) -> Result<(), HandlerError> {
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    let ws = model
        .sheets
        .iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

    let part_path = ws.part_path.clone();
    let ref_str = cell_ref.to_string_ref();

    let xml = package
        .read_part_xml(&part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let p = detect_namespace_prefix(&xml);

    // Find the cell element
    let cell_pattern = format!("<{}c r=\"{}\"", p, ref_str);
    if let Some(cell_start) = xml.find(&cell_pattern) {
        let cell_end = find_cell_element_end(&xml, cell_start, &p)?;
        let mut result = xml[..cell_start].to_string();
        result.push_str(&xml[cell_end..]);
        package
            .write_part_xml(&part_path, &result)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    }

    Ok(())
}

/// Remove a sheet from the workbook.
fn remove_sheet(package: &mut OxmlPackage, sheet_name: &str) -> Result<(), HandlerError> {
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    if model.sheets.len() <= 1 {
        return Err(HandlerError::InvalidArgument(
            "cannot remove the workbook's only worksheet".to_string(),
        ));
    }

    let ws = model
        .sheets
        .iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

    let wb_xml = package
        .read_part_xml("xl/workbook.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let entries = workbook_sheet_entries(&wb_xml)?;
    let removed_index = entries
        .iter()
        .position(|entry| entry.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;
    let removed = &entries[removed_index];
    let workbook_without_sheet = remove_ranges(&wb_xml, vec![removed.range.clone()]);
    let updated_workbook = rewrite_defined_name_scopes(&workbook_without_sheet, |scope| {
        let scope = scope as usize;
        if scope == removed_index {
            None
        } else if scope > removed_index {
            Some((scope - 1) as u32)
        } else {
            Some(scope as u32)
        }
    })?;

    let workbook_rels_path = "xl/_rels/workbook.xml.rels";
    let workbook_rels = package
        .read_part_xml(workbook_rels_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let updated_rels = remove_relationship_by_id(&workbook_rels, &removed.relationship_id)?;

    let content_types_path = "[Content_Types].xml";
    let content_types = package
        .read_part_xml(content_types_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let updated_content_types = remove_content_type_override(&content_types, &ws.part_path)?;

    package
        .write_part_xml("xl/workbook.xml", &updated_workbook)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    package
        .write_part_xml(workbook_rels_path, &updated_rels)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    package
        .write_part_xml(content_types_path, &updated_content_types)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    package
        .remove_part(&ws.part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    package
        .remove_part(&relationships_part_path(&ws.part_path))
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    Ok(())
}

/// Move a worksheet while preserving sheet-scoped defined-name bindings.
pub fn move_sheet(
    package: &mut OxmlPackage,
    source: &str,
    target_parent: Option<&str>,
    position: InsertPosition,
) -> Result<String, HandlerError> {
    let source_path = navigation::parse_path(source)?;
    let source_name = source_path
        .sheet_name
        .ok_or_else(|| HandlerError::InvalidPath("move source requires a sheet".to_string()))?;
    if source_path.cell_ref.is_some() {
        return Err(HandlerError::InvalidPath(
            "worksheet move source must not include a cell".to_string(),
        ));
    }

    let workbook_xml = package
        .read_part_xml("xl/workbook.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let entries = workbook_sheet_entries(&workbook_xml)?;
    let old_order: Vec<String> = entries.iter().map(|entry| entry.name.clone()).collect();
    let source_index = old_order
        .iter()
        .position(|name| name == &source_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", source_name)))?;

    let mut new_entries = entries.clone();
    let moved = new_entries.remove(source_index);
    let anchor_name = |path: &str| path.trim().trim_start_matches('/').to_string();
    let insert_index = match position {
        InsertPosition::AtIndex(index) => index.min(new_entries.len()),
        InsertPosition::BeforeElement(anchor) => {
            let anchor = anchor_name(&anchor);
            new_entries
                .iter()
                .position(|entry| entry.name == anchor)
                .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", anchor)))?
        }
        InsertPosition::AfterElement(anchor) => {
            let anchor = anchor_name(&anchor);
            new_entries
                .iter()
                .position(|entry| entry.name == anchor)
                .map(|index| index + 1)
                .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", anchor)))?
        }
        InsertPosition::Append => {
            if let Some(target) = target_parent.filter(|target| !matches!(*target, "" | "/")) {
                let target = anchor_name(target);
                new_entries
                    .iter()
                    .position(|entry| entry.name == target)
                    .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", target)))?
            } else {
                new_entries.len()
            }
        }
    };
    new_entries.insert(insert_index, moved);

    let new_order: Vec<String> = new_entries.iter().map(|entry| entry.name.clone()).collect();
    if old_order == new_order {
        return Ok(format!("/{}", source_name));
    }

    let reordered = rewrite_sheet_order(&workbook_xml, &new_entries)?;
    let updated = rewrite_defined_name_scopes(&reordered, |scope| {
        let old_index = scope as usize;
        if old_index >= old_order.len() {
            return Some(scope);
        }
        new_order
            .iter()
            .position(|name| name == &old_order[old_index])
            .map(|index| index as u32)
    })?;
    package
        .write_part_xml("xl/workbook.xml", &updated)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(format!("/{}", source_name))
}

/// Move a cell's content from source to target.
/// Source: /SheetName/A1, Target: /SheetName/B1 (or different sheet)
pub fn move_cell(
    package: &mut OxmlPackage,
    source: &str,
    target_parent: Option<&str>,
) -> Result<String, HandlerError> {
    let source_pc = navigation::parse_path(source)?;

    let sheet_name = source_pc.sheet_name.ok_or_else(|| {
        HandlerError::InvalidPath("move source requires a sheet name".to_string())
    })?;
    let source_ref = source_pc.cell_ref.ok_or_else(|| {
        HandlerError::InvalidPath("move source requires a cell reference".to_string())
    })?;

    // Determine target
    let target_path = target_parent.unwrap_or("/");
    let target_pc = navigation::parse_path(target_path)?;

    let target_sheet = target_pc.sheet_name.unwrap_or(sheet_name.clone());
    let target_ref = target_pc.cell_ref.ok_or_else(|| {
        HandlerError::InvalidPath("move target requires a cell reference".to_string())
    })?;

    // 1. Copy cell content to target
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    let src_ws = model
        .sheets
        .iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

    let src_cell = src_ws
        .cells
        .get(&(source_ref.row, source_ref.col))
        .ok_or_else(|| {
            HandlerError::PathNotFound(format!("cell {}{}", sheet_name, source_ref.to_string_ref()))
        })?;

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

    let sheet_name = source_pc.sheet_name.ok_or_else(|| {
        HandlerError::InvalidPath("copy source requires a sheet name".to_string())
    })?;
    let source_ref = source_pc.cell_ref.ok_or_else(|| {
        HandlerError::InvalidPath("copy source requires a cell reference".to_string())
    })?;

    let target_pc = navigation::parse_path(target_parent)?;

    let target_sheet = target_pc.sheet_name.unwrap_or(sheet_name.clone());
    let target_ref = target_pc.cell_ref.ok_or_else(|| {
        HandlerError::InvalidPath("copy target requires a cell reference".to_string())
    })?;

    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    let src_ws = model
        .sheets
        .iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

    let src_cell = src_ws
        .cells
        .get(&(source_ref.row, source_ref.col))
        .ok_or_else(|| {
            HandlerError::PathNotFound(format!("cell {}{}", sheet_name, source_ref.to_string_ref()))
        })?;

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

/// Swap two cells' content (values, formulas, styles).
pub fn swap_cells(
    package: &mut OxmlPackage,
    path1: &str,
    path2: &str,
) -> Result<(String, String), HandlerError> {
    let pc1 = navigation::parse_path(path1)?;
    let pc2 = navigation::parse_path(path2)?;

    let sheet1 = pc1
        .sheet_name
        .ok_or_else(|| HandlerError::InvalidPath("swap path1 requires a sheet name".to_string()))?;
    let ref1 = pc1.cell_ref.ok_or_else(|| {
        HandlerError::InvalidPath("swap path1 requires a cell reference".to_string())
    })?;
    let sheet2 = pc2
        .sheet_name
        .ok_or_else(|| HandlerError::InvalidPath("swap path2 requires a sheet name".to_string()))?;
    let ref2 = pc2.cell_ref.ok_or_else(|| {
        HandlerError::InvalidPath("swap path2 requires a cell reference".to_string())
    })?;

    if sheet1 == sheet2 && ref1.row == ref2.row && ref1.col == ref2.col {
        return Err(HandlerError::InvalidArgument(
            "swap requires two different cells".to_string(),
        ));
    }

    // Read both cells' content
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    let get_cell_props =
        |sheet_name: &str, cell_ref: &CellRef| -> Result<HashMap<String, String>, HandlerError> {
            let ws = model
                .sheets
                .iter()
                .find(|s| s.name == sheet_name)
                .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;
            let cell = ws.cells.get(&(cell_ref.row, cell_ref.col));
            let mut props = HashMap::new();
            if let Some(c) = cell {
                if let Some(v) = &c.raw_value {
                    props.insert("value".to_string(), v.clone());
                }
                if let Some(f) = &c.formula {
                    props.insert("formula".to_string(), f.clone());
                }
                if let Some(si) = c.style_index {
                    props.insert("style".to_string(), si.to_string());
                }
            }
            Ok(props)
        };

    let props1 = get_cell_props(&sheet1, &ref1)?;
    let props2 = get_cell_props(&sheet2, &ref2)?;

    // Apply cell2's content to cell1 and vice versa
    let path1_str = format!("/{}/{}", sheet1, ref1.to_string_ref());
    let path2_str = format!("/{}/{}", sheet2, ref2.to_string_ref());

    set_cell_properties(package, &path1_str, &props2)?;
    set_cell_properties(package, &path2_str, &props1)?;

    Ok((path1_str, path2_str))
}

#[derive(Clone, Debug)]
pub(crate) struct WorkbookSheetEntry {
    pub name: String,
    pub relationship_id: String,
    pub xml: String,
    pub range: Range<usize>,
}

pub(crate) fn workbook_sheet_entries(
    workbook_xml: &str,
) -> Result<Vec<WorkbookSheetEntry>, HandlerError> {
    const RELATIONSHIPS_NS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let doc = roxmltree::Document::parse(workbook_xml)
        .map_err(|e| HandlerError::OperationFailed(format!("invalid workbook.xml: {}", e)))?;
    let sheets = doc
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "sheets")
        .ok_or_else(|| HandlerError::OperationFailed("workbook has no sheets list".to_string()))?;

    sheets
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "sheet")
        .map(|node| {
            let name = node.attribute("name").ok_or_else(|| {
                HandlerError::OperationFailed("worksheet entry has no name".to_string())
            })?;
            let relationship_id = node
                .attribute((RELATIONSHIPS_NS, "id"))
                .or_else(|| node.attribute("r:id"))
                .ok_or_else(|| {
                    HandlerError::OperationFailed(format!(
                        "worksheet '{}' has no relationship ID",
                        name
                    ))
                })?;
            let range = node.range();
            Ok(WorkbookSheetEntry {
                name: name.to_string(),
                relationship_id: relationship_id.to_string(),
                xml: workbook_xml[range.clone()].to_string(),
                range,
            })
        })
        .collect()
}

/// Rewrite localSheetId values. Returning `None` removes that defined name.
pub(crate) fn rewrite_defined_name_scopes(
    xml: &str,
    mapper: impl Fn(u32) -> Option<u32>,
) -> Result<String, HandlerError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| HandlerError::OperationFailed(format!("invalid workbook.xml: {}", e)))?;
    let mut replacements: Vec<(Range<usize>, String)> = Vec::new();

    for node in doc
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "definedName")
    {
        let Some(value) = node.attribute("localSheetId") else {
            continue;
        };
        let scope = value.parse::<u32>().map_err(|_| {
            HandlerError::OperationFailed(format!(
                "definedName has invalid localSheetId '{}'",
                value
            ))
        })?;
        match mapper(scope) {
            None => replacements.push((node.range(), String::new())),
            Some(new_scope) if new_scope != scope => {
                let value_range = attribute_value_range(xml, node.range(), "localSheetId")
                    .ok_or_else(|| {
                        HandlerError::OperationFailed(
                            "cannot locate localSheetId attribute in workbook XML".to_string(),
                        )
                    })?;
                replacements.push((value_range, new_scope.to_string()));
            }
            Some(_) => {}
        }
    }
    Ok(apply_replacements(xml, replacements))
}

fn attribute_value_range(
    xml: &str,
    node_range: Range<usize>,
    attribute_name: &str,
) -> Option<Range<usize>> {
    let node_xml = &xml[node_range.clone()];
    let opening_end = node_xml.find('>')?;
    let opening = &node_xml[..opening_end];
    let name_start = opening.find(attribute_name)?;
    let mut cursor = name_start + attribute_name.len();
    while opening.as_bytes().get(cursor)?.is_ascii_whitespace() {
        cursor += 1;
    }
    if opening.as_bytes().get(cursor) != Some(&b'=') {
        return None;
    }
    cursor += 1;
    while opening.as_bytes().get(cursor)?.is_ascii_whitespace() {
        cursor += 1;
    }
    let quote = *opening.as_bytes().get(cursor)?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    let value_start = cursor + 1;
    let value_end = opening.as_bytes()[value_start..]
        .iter()
        .position(|byte| *byte == quote)?
        + value_start;
    Some((node_range.start + value_start)..(node_range.start + value_end))
}

fn rewrite_sheet_order(
    workbook_xml: &str,
    entries: &[WorkbookSheetEntry],
) -> Result<String, HandlerError> {
    let doc = roxmltree::Document::parse(workbook_xml)
        .map_err(|e| HandlerError::OperationFailed(format!("invalid workbook.xml: {}", e)))?;
    let sheets = doc
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "sheets")
        .ok_or_else(|| HandlerError::OperationFailed("workbook has no sheets list".to_string()))?;
    let range = sheets.range();
    let container = &workbook_xml[range.clone()];
    let opening_end = container.find('>').ok_or_else(|| {
        HandlerError::OperationFailed("malformed sheets list opening tag".to_string())
    })? + 1;
    let closing_start = container.rfind("</").ok_or_else(|| {
        HandlerError::OperationFailed("malformed sheets list closing tag".to_string())
    })?;
    let content_range = (range.start + opening_end)..(range.start + closing_start);
    let content = if entries.is_empty() {
        String::new()
    } else {
        format!(
            "\n    {}\n  ",
            entries
                .iter()
                .map(|entry| entry.xml.as_str())
                .collect::<Vec<_>>()
                .join("\n    ")
        )
    };
    Ok(apply_replacements(
        workbook_xml,
        vec![(content_range, content)],
    ))
}

fn remove_relationship_by_id(xml: &str, relationship_id: &str) -> Result<String, HandlerError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| {
        HandlerError::OperationFailed(format!("invalid workbook relationships: {}", e))
    })?;
    let relationship = doc
        .descendants()
        .find(|node| {
            node.is_element()
                && node.tag_name().name() == "Relationship"
                && node.attribute("Id") == Some(relationship_id)
        })
        .ok_or_else(|| {
            HandlerError::OperationFailed(format!(
                "workbook relationship {} not found",
                relationship_id
            ))
        })?;
    Ok(remove_ranges(xml, vec![relationship.range()]))
}

fn remove_content_type_override(xml: &str, part_path: &str) -> Result<String, HandlerError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| HandlerError::OperationFailed(format!("invalid content types: {}", e)))?;
    let part_name = format!("/{}", part_path.trim_start_matches('/'));
    let ranges = doc
        .descendants()
        .filter(|node| {
            node.is_element()
                && node.tag_name().name() == "Override"
                && node.attribute("PartName") == Some(part_name.as_str())
        })
        .map(|node| node.range())
        .collect();
    Ok(remove_ranges(xml, ranges))
}

fn relationships_part_path(part_path: &str) -> String {
    match part_path.rsplit_once('/') {
        Some((directory, file_name)) => format!("{}/_rels/{}.rels", directory, file_name),
        None => format!("_rels/{}.rels", part_path),
    }
}

fn remove_ranges(xml: &str, ranges: Vec<Range<usize>>) -> String {
    apply_replacements(
        xml,
        ranges
            .into_iter()
            .map(|range| (range, String::new()))
            .collect(),
    )
}

fn apply_replacements(xml: &str, mut replacements: Vec<(Range<usize>, String)>) -> String {
    replacements.sort_by(|left, right| right.0.start.cmp(&left.0.start));
    let mut result = xml.to_string();
    for (range, replacement) in replacements {
        result.replace_range(range, &replacement);
    }
    result
}

/// Set properties on a cell identified by path like /Sheet1/A1.
pub fn set_cell_properties(
    package: &mut OxmlPackage,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    // Find/replace short-circuit: when `find` is present, scan shared strings and
    // inline worksheet strings across the whole workbook (or a single sheet when
    // path is "/SheetName").
    if properties.contains_key("find") {
        return apply_xlsx_find_replace(package, path, properties);
    }

    if let Some((sheet_name, pivot_index)) = parse_pivot_path(path) {
        return set_pivot_table_properties(package, &sheet_name, pivot_index, properties);
    }

    let pc = navigation::parse_path(path)?;

    // Need both sheet name and cell reference for set operations
    let sheet_name = pc.sheet_name.ok_or_else(|| {
        HandlerError::InvalidPath("set requires a sheet name in the path".to_string())
    })?;
    let cell_ref = pc.cell_ref.ok_or_else(|| {
        HandlerError::InvalidPath("set requires a cell reference (e.g. /Sheet1/A1)".to_string())
    })?;

    // Parse the model to find the sheet part path
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    let ws = model
        .sheets
        .iter()
        .find(|s| s.name == sheet_name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

    let part_path = ws.part_path.clone();
    let cell_ref_str = cell_ref.to_string_ref();

    // Read the current worksheet XML
    let xml = package
        .read_part_xml(&part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let p = detect_namespace_prefix(&xml);

    let mut modified_xml = xml.clone();
    let mut unsupported = Vec::new();

    // Build a style string from all style-related properties so a single
    // style-id update carries every format key at once (matching C# which
    // merges font/fill/border/alignment into one cellXfs entry).
    let mut style_parts: Vec<String> = Vec::new();

    for (key, value) in properties {
        match key.as_str() {
            "value" => {
                modified_xml = set_cell_value(
                    &modified_xml,
                    &cell_ref_str,
                    value,
                    &model.shared_strings,
                    &p,
                )?;
            }
            "formula" => {
                let qualified =
                    formula::qualify_for_ooxml(value).map_err(HandlerError::InvalidArgument)?;
                let dynamic_cm = formula::is_dynamic_array_formula(&qualified)
                    .then(|| dynamic_array::ensure_metadata(package))
                    .transpose()?;
                modified_xml =
                    set_cell_formula(&modified_xml, &cell_ref_str, &qualified, dynamic_cm, &p)?;
                if formula::is_dynamic_array_formula(&qualified) {
                    if let Some(result) = formula::evaluate_spill(&qualified, &model) {
                        ensure_dynamic_spill_targets_clear(
                            &modified_xml,
                            &cell_ref_str,
                            &result,
                            &p,
                        )?;
                        modified_xml =
                            persist_dynamic_spill(&modified_xml, &cell_ref_str, &result, &p)?;
                    }
                }
            }
            "image" => {}
            "alt" | "altText" | "alttext" | "description" | "image.alt" => {
                if !properties.contains_key("image") {
                    unsupported.push(format!("{key} (only valid alongside image=)"));
                }
            }
            "style" => {
                style_parts.push(value.clone());
            }
            "numberformat" | "numberFormat" | "numFmt" => {
                style_parts.push(format!("numberformat={}", value));
            }
            "font" | "fontName" | "font.name" => {
                style_parts.push(format!("font={}", value));
            }
            "fontSize" | "size" | "font.size" => {
                style_parts.push(format!("fontSize={}", value));
            }
            "color" | "fontColor" | "font.color" => {
                style_parts.push(format!("fontColor={}", value));
            }
            "bold" | "b" | "font.bold" => {
                if value == "true" || value == "1" {
                    style_parts.push("bold=true".to_string());
                }
            }
            "italic" | "i" | "font.italic" => {
                if value == "true" || value == "1" {
                    style_parts.push("italic=true".to_string());
                }
            }
            "underline" | "u" | "font.underline" => {
                style_parts.push(format!("underline={}", value));
            }
            "fill" | "bgColor" | "bg" | "backgroundColor" => {
                style_parts.push(format!("fill={}", value));
            }
            "fontColor2" | "color2" => {
                style_parts.push(format!("fontColor={}", value));
            }
            "border" | "borderColor" => {
                style_parts.push(format!("border={}", value));
            }
            "alignment" | "align" => {
                style_parts.push(format!("alignment={}", value));
            }
            "valign" | "verticalAlignment" => {
                style_parts.push(format!("valign={}", value));
            }
            "wrap" | "wrapText" => {
                if value == "true" || value == "1" {
                    style_parts.push("wrap=true".to_string());
                }
            }
            "indent" => {
                style_parts.push(format!("indent={}", value));
            }
            "rotation" | "textRotation" => {
                style_parts.push(format!("rotation={}", value));
            }
            "type" => {
                // Cell type: n (number), s (string), b (boolean), str (formula string)
                style_parts.push(format!("cellType={}", value));
            }
            _ => {
                unsupported.push(key.clone());
            }
        }
    }

    // Apply combined style if any style-related keys were collected
    if !style_parts.is_empty() {
        let combined = style_parts.join(";");
        modified_xml = set_cell_style(&modified_xml, &cell_ref_str, &combined, &p)?;
    }

    if let Some(source) = properties.get("image") {
        modified_xml = if source.is_empty() || source.eq_ignore_ascii_case("none") {
            remove_in_cell_image(&modified_xml, &cell_ref_str, &p)?
        } else {
            let alt = properties
                .get("alt")
                .or_else(|| properties.get("altText"))
                .or_else(|| properties.get("alttext"))
                .or_else(|| properties.get("description"))
                .or_else(|| properties.get("image.alt"));
            let vm = rich_value_image::add_image(package, source, alt.map(String::as_str))?;
            set_in_cell_image(&modified_xml, &cell_ref_str, vm, &p)?
        };
    }

    // Write back the modified XML
    package
        .write_part_xml(&part_path, &modified_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    Ok(unsupported)
}

/// Apply definition-local PivotTable settings without changing its cache
/// schema.  C# has a wider field-area rebuild pipeline; keeping source/axis
/// mutation out of this path until cache rebuilding is implemented prevents a
/// structurally valid cache from being paired with incompatible pivot fields.
fn set_pivot_table_properties(
    package: &mut OxmlPackage,
    sheet_name: &str,
    pivot_index: usize,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;
    let pivot_path = helpers::pivot_part_path_for_sheet(package, sheet_name, pivot_index)?;
    let pivot = model
        .pivot_tables
        .iter()
        .find(|pivot| pivot.part_path == pivot_path)
        .ok_or_else(|| HandlerError::PathNotFound(format!("pivottable[{}]", pivot_index)))?;
    let mut xml = package
        .read_part_xml(&pivot_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let area_properties: HashMap<String, String> = properties
        .iter()
        .filter_map(|(key, value)| {
            let canonical = canonical_pivot_property(key);
            matches!(
                canonical.as_str(),
                "rows" | "cols" | "filters" | "values" | "aggregate"
            )
            .then_some((canonical, value.clone()))
        })
        .collect();
    let mut refreshed_source = false;
    if let Some(source) = properties.iter().find_map(|(key, value)| {
        (canonical_pivot_property(key) == "source").then_some(value.as_str())
    }) {
        xml = refresh_pivot_cache_from_source(
            package,
            &model,
            pivot,
            &pivot_path,
            source,
            &area_properties,
            &xml,
        )?;
        refreshed_source = true;
    }
    if !area_properties.is_empty() && !refreshed_source {
        xml = rebuild_pivot_field_areas(&xml, pivot, &area_properties)?;
    }
    let mut root_attrs: Vec<(&str, String)> = Vec::new();
    let mut style_attrs: Vec<(&str, String)> = Vec::new();
    let mut unsupported = Vec::new();

    for (key, value) in properties {
        if let Some(index) = pivot_data_field_show_as_index(key) {
            xml = set_pivot_data_field_show_as(&xml, index, value)?;
            continue;
        }
        match canonical_pivot_property(key).as_str() {
            "name" => {
                let name = validate_pivot_name(value)?;
                if model.pivot_tables.iter().any(|other| {
                    other.part_path != pivot.part_path && other.name.eq_ignore_ascii_case(&name)
                }) {
                    return Err(HandlerError::InvalidArgument(format!(
                        "pivot table name '{}' already exists",
                        name
                    )));
                }
                root_attrs.push(("name", name));
            }
            "style" => style_attrs.push(("name", value.clone())),
            "showrowstripes" => {
                style_attrs.push(("showRowStripes", pivot_bool(value)?.to_string()))
            }
            "showcolstripes" => {
                style_attrs.push(("showColStripes", pivot_bool(value)?.to_string()))
            }
            "showrowheaders" => {
                style_attrs.push(("showRowHeaders", pivot_bool(value)?.to_string()))
            }
            "showcolheaders" => {
                style_attrs.push(("showColHeaders", pivot_bool(value)?.to_string()))
            }
            "showlastcolumn" => {
                style_attrs.push(("showLastColumn", pivot_bool(value)?.to_string()))
            }
            "showdrill" => root_attrs.push(("showDrill", pivot_bool(value)?.to_string())),
            "mergelabels" => root_attrs.push(("mergeItem", pivot_bool(value)?.to_string())),
            "grandtotalcaption" => root_attrs.push(("grandTotalCaption", value.trim().to_string())),
            "rowgrandtotals" => root_attrs.push(("rowGrandTotals", pivot_bool(value)?.to_string())),
            "colgrandtotals" => root_attrs.push(("colGrandTotals", pivot_bool(value)?.to_string())),
            "grandtotals" => {
                let (row, col) = match value.trim().to_ascii_lowercase().as_str() {
                    "both" | "on" | "true" | "1" | "yes" => (true, true),
                    "none" | "off" | "false" | "0" | "no" => (false, false),
                    "rows" => (true, false),
                    "cols" | "columns" => (false, true),
                    _ => {
                        return Err(HandlerError::InvalidArgument(format!(
                            "invalid grandTotals value '{}'",
                            value
                        )))
                    }
                };
                root_attrs.push(("rowGrandTotals", row.to_string()));
                root_attrs.push(("colGrandTotals", col.to_string()));
            }
            "layout" => match value.trim().to_ascii_lowercase().as_str() {
                "compact" => {
                    root_attrs.push(("compact", "1".to_string()));
                    root_attrs.push(("compactData", "1".to_string()));
                    root_attrs.push(("outline", "1".to_string()));
                    root_attrs.push(("outlineData", "1".to_string()));
                }
                "outline" => {
                    root_attrs.push(("compact", "0".to_string()));
                    root_attrs.push(("compactData", "0".to_string()));
                    root_attrs.push(("outline", "1".to_string()));
                    root_attrs.push(("outlineData", "1".to_string()));
                }
                "tabular" => {
                    root_attrs.push(("compact", "0".to_string()));
                    root_attrs.push(("compactData", "0".to_string()));
                    root_attrs.push(("outline", "0".to_string()));
                    root_attrs.push(("outlineData", "0".to_string()));
                }
                _ => {
                    return Err(HandlerError::InvalidArgument(format!(
                        "invalid pivot layout '{}'; expected compact, outline, or tabular",
                        value
                    )))
                }
            },
            "subtotals" | "defaultsubtotal" => {
                let enabled = pivot_bool(value)?;
                xml =
                    set_all_pivot_field_attributes(&xml, "defaultSubtotal", &enabled.to_string())?;
            }
            "showdataas" => {
                xml = set_pivot_show_data_as(&xml, value)?;
            }
            "repeatlabels" => {
                xml = set_pivot_repeat_labels(&xml, pivot_bool(value)?)?;
            }
            "blankrows" => {
                xml = set_pivot_blank_rows(&xml, pivot_bool(value)?)?;
            }
            // Field areas are rebuilt atomically above. A source refresh uses
            // the new cache schema; definition-only edits use the existing
            // one.
            "rows" | "cols" | "filters" | "values" | "aggregate" => {}
            "source" => {}
            "sort" | "topn" | "labelfilter" | "calculatedfield" => unsupported.push(key.clone()),
            _ => unsupported.push(key.clone()),
        }
    }
    if !root_attrs.is_empty() {
        xml = set_first_element_attributes(&xml, "pivotTableDefinition", &root_attrs)?;
    }
    if !style_attrs.is_empty() {
        xml = set_or_insert_pivot_style(&xml, &style_attrs)?;
    }
    package
        .write_part_xml(&pivot_path, &xml)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    Ok(unsupported)
}

fn canonical_pivot_property(key: &str) -> String {
    match key.to_ascii_lowercase().as_str() {
        "src" => "source".to_string(),
        "row" | "rowfield" | "rowfields" => "rows".to_string(),
        "col" | "column" | "columns" | "colfield" | "colfields" | "columnfield"
        | "columnfields" => "cols".to_string(),
        "filter" | "filterfield" | "filterfields" => "filters".to_string(),
        "value" | "valuefield" | "valuefields" => "values".to_string(),
        "showcolumnstripes" | "bandedrows" => "showrowstripes".to_string(),
        "bandedcols" | "bandedcolumns" => "showcolstripes".to_string(),
        "showcolumnheaders" => "showcolheaders".to_string(),
        "columngrandtotals" => "colgrandtotals".to_string(),
        "repeatitemlabels" | "repeatalllabels" | "filldownlabels" => "repeatlabels".to_string(),
        "insertblankrow" | "insertblankrows" | "blankrow" | "blankline" | "blanklines" => {
            "blankrows".to_string()
        }
        key => key.to_string(),
    }
}

fn validate_pivot_name(value: &str) -> Result<String, HandlerError> {
    let name = value.trim();
    if name.is_empty() || name.len() > 255 || name.chars().any(char::is_control) {
        return Err(HandlerError::InvalidArgument(
            "pivot name must be non-empty, contain no control characters, and be at most 255 characters"
                .to_string(),
        ));
    }
    Ok(name.to_string())
}

fn pivot_bool(value: &str) -> Result<bool, HandlerError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(HandlerError::InvalidArgument(format!(
            "invalid pivot boolean '{}'",
            value
        ))),
    }
}

fn set_pivot_show_data_as(xml: &str, value: &str) -> Result<String, HandlerError> {
    let modes: Vec<Option<&str>> = value
        .split(',')
        .map(|mode| pivot_show_data_as_ooxml(mode.trim()))
        .collect::<Result<_, _>>()?;
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid pivot XML: {}", error)))?;
    let mut ranges: Vec<Range<usize>> = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "dataField")
        .map(|node| node.range())
        .collect();
    ranges.sort_by_key(|range| range.start);
    let mut out = xml.to_string();
    for (index, range) in ranges.into_iter().enumerate().rev() {
        let Some(mode) = modes.get(index) else {
            continue;
        };
        out = remove_opening_tag_attributes(&out, range.clone(), &["showDataAs"])?;
        if let Some(mode) = mode {
            out = rewrite_opening_tag_attributes(
                &out,
                range,
                &[("showDataAs", (*mode).to_string())],
            )?;
        }
    }
    Ok(out)
}

fn pivot_show_data_as_ooxml(value: &str) -> Result<Option<&'static str>, HandlerError> {
    match value.to_ascii_lowercase().as_str() {
        "" | "normal" => Ok(None),
        "percent_of_total" | "percentoftotal" | "percent" => Ok(Some("percent")),
        "percent_of_row" | "percentofrow" => Ok(Some("percentOfRow")),
        "percent_of_col" | "percent_of_column" | "percentofcol" | "percentofcolumn" => {
            Ok(Some("percentOfCol"))
        }
        "running_total" | "runningtotal" | "runtotal" => Ok(Some("runTotal")),
        "difference" | "diff" | "percent_diff" | "percentdiff" | "index" => {
            Err(HandlerError::InvalidArgument(format!(
                "showDataAs '{}' is not yet supported by the renderer",
                value
            )))
        }
        _ => Err(HandlerError::InvalidArgument(format!(
            "invalid showDataAs: '{}'. Valid: normal, percent_of_total, percent_of_row, percent_of_col, running_total",
            value
        ))),
    }
}

fn pivot_data_field_show_as_index(key: &str) -> Option<usize> {
    let lower = key.to_ascii_lowercase();
    let suffix = ".showas";
    let ordinal = lower
        .strip_prefix("datafield")?
        .strip_suffix(suffix)?
        .parse::<usize>()
        .ok()?;
    (ordinal > 0).then_some(ordinal - 1)
}

/// Write-side counterpart to the `dataFieldN.showAs` values returned by Get.
/// Unlike the positional `showDataAs=a,b` form, this changes one field and
/// leaves every sibling data-field display mode untouched.
fn set_pivot_data_field_show_as(
    xml: &str,
    index: usize,
    value: &str,
) -> Result<String, HandlerError> {
    let mode = pivot_show_data_as_ooxml(value.trim())?;
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid pivot XML: {}", error)))?;
    let field = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "dataField")
        .nth(index)
        .ok_or_else(|| {
            HandlerError::InvalidArgument(format!(
                "dataField{}.showAs: index out of range",
                index + 1
            ))
        })?;
    let out = remove_opening_tag_attributes(xml, field.range(), &["showDataAs"])?;
    if let Some(mode) = mode {
        rewrite_opening_tag_attributes(&out, field.range(), &[("showDataAs", mode.to_string())])
    } else {
        Ok(out)
    }
}

const PIVOT_REPEAT_LABELS_EXT_URI: &str = "{962EF5D1-5CA2-4c93-8EF4-DBF5C05439D2}";
const PIVOT_X14_NS: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";

/// Update the Office 2010 pivot extension used by Excel's “Repeat All Item
/// Labels” option without replacing unrelated extLst payloads from a producer.
pub(crate) fn set_pivot_repeat_labels(xml: &str, enabled: bool) -> Result<String, HandlerError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid pivot XML: {}", error)))?;
    let old_extensions: Vec<Range<usize>> = document
        .descendants()
        .filter(|node| {
            node.is_element()
                && node.tag_name().name() == "ext"
                && node.attribute("uri") == Some(PIVOT_REPEAT_LABELS_EXT_URI)
        })
        .map(|node| node.range())
        .collect();
    let mut out = remove_ranges(xml, old_extensions);
    if !enabled {
        return Ok(out);
    }
    let extension = format!(
        "<ext uri=\"{}\"><x14:pivotTableDefinition xmlns:x14=\"{}\" fillDownLabelsDefault=\"1\"/></ext>",
        PIVOT_REPEAT_LABELS_EXT_URI, PIVOT_X14_NS
    );
    let document = roxmltree::Document::parse(&out)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid pivot XML: {}", error)))?;
    if let Some(ext_list) = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "extLst")
    {
        let end = out[ext_list.range()]
            .rfind("</")
            .map(|offset| ext_list.range().start + offset)
            .ok_or_else(|| HandlerError::OperationFailed("malformed pivot extLst".to_string()))?;
        out.insert_str(end, &extension);
        return Ok(out);
    }
    let root = document.root_element();
    let end = out[root.range()]
        .rfind("</")
        .map(|offset| root.range().start + offset)
        .ok_or_else(|| HandlerError::OperationFailed("malformed pivot definition".to_string()))?;
    out.insert_str(end, &format!("<extLst>{}</extLst>", extension));
    Ok(out)
}

/// Set `insertBlankRow` on the outermost row-axis field, the OOXML location
/// used by Excel for “Insert Blank Line After Each Item”.
pub(crate) fn set_pivot_blank_rows(xml: &str, enabled: bool) -> Result<String, HandlerError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid pivot XML: {}", error)))?;
    let row_field = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "rowFields")
        .and_then(|rows| {
            rows.children()
                .find(|node| node.is_element() && node.tag_name().name() == "field")
        });
    let Some(row_field) = row_field else {
        return Ok(xml.to_string());
    };
    let field_index = row_field
        .attribute("x")
        .or_else(|| row_field.attribute("idx"))
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or_else(|| HandlerError::OperationFailed("invalid pivot row field".to_string()))?;
    let pivot_fields = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "pivotFields")
        .ok_or_else(|| {
            HandlerError::OperationFailed("pivot definition has no pivotFields".to_string())
        })?;
    let pivot_field = pivot_fields
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "pivotField")
        .nth(field_index)
        .ok_or_else(|| {
            HandlerError::OperationFailed("pivot row field index is invalid".to_string())
        })?;
    if enabled {
        rewrite_opening_tag_attributes(
            xml,
            pivot_field.range(),
            &[("insertBlankRow", "1".to_string())],
        )
    } else {
        remove_opening_tag_attributes(xml, pivot_field.range(), &["insertBlankRow"])
    }
}

pub(crate) fn apply_pivot_display_options(
    xml: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let option = |name: &str| {
        properties
            .iter()
            .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value.as_str()))
    };
    let mut out = xml.to_string();
    if let Some(value) = option("repeatlabels")
        .or_else(|| option("repeatitemlabels"))
        .or_else(|| option("repeatalllabels"))
        .or_else(|| option("filldownlabels"))
    {
        out = set_pivot_repeat_labels(&out, pivot_bool(value)?)?;
    }
    if let Some(value) = option("blankrows")
        .or_else(|| option("insertblankrow"))
        .or_else(|| option("insertblankrows"))
        .or_else(|| option("blankrow"))
        .or_else(|| option("blankline"))
        .or_else(|| option("blanklines"))
    {
        out = set_pivot_blank_rows(&out, pivot_bool(value)?)?;
    }
    Ok(out)
}

/// Refresh a pivot cache from a replacement worksheet range.  A source change
/// on a shared cache uses copy-on-write: siblings keep their original source
/// and records while the edited PivotTable receives a fresh cache part and
/// workbook-level cache id.
///
/// Header changes are supported. Existing field assignments retain their
/// numeric field positions unless the same `set` provides a replacement
/// `rows`/`cols`/`filters`/`values` property; narrowing a source therefore
/// fails before any package write when an un-restated assignment would fall
/// outside the new field count, matching the C# safety policy.
fn refresh_pivot_cache_from_source(
    package: &mut OxmlPackage,
    model: &WorkbookModel,
    pivot: &PivotTableDef,
    pivot_path: &str,
    source_spec: &str,
    area_properties: &HashMap<String, String>,
    pivot_xml: &str,
) -> Result<String, HandlerError> {
    let (source_sheet, source_ref) = parse_pivot_source_for_refresh(source_spec, pivot)?;
    let (start, end) = parse_pivot_refresh_range(&source_ref)?;
    if start.row >= end.row {
        return Err(HandlerError::InvalidArgument(
            "pivot source must include at least one data row".to_string(),
        ));
    }
    let source = model
        .sheets
        .iter()
        .find(|sheet| sheet.name == source_sheet)
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", source_sheet)))?;
    let headers: Vec<String> = (start.col..=end.col)
        .map(|col| {
            source
                .cells
                .get(&(start.row, col))
                .map(|cell| cell.display_value.trim().to_string())
                .unwrap_or_default()
        })
        .collect();
    if headers.is_empty() || headers.iter().any(String::is_empty) {
        return Err(HandlerError::InvalidArgument(
            "pivot source header row must contain a name for every column".to_string(),
        ));
    }
    if headers.iter().enumerate().any(|(index, header)| {
        headers[..index]
            .iter()
            .any(|earlier| earlier.eq_ignore_ascii_case(header))
    }) {
        return Err(HandlerError::InvalidArgument(
            "pivot source headers must be unique".to_string(),
        ));
    }
    let rows: Vec<Vec<String>> = ((start.row + 1)..=end.row)
        .map(|row| {
            (start.col..=end.col)
                .map(|col| {
                    source
                        .cells
                        .get(&(row, col))
                        .map(|cell| cell.display_value.clone())
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect();
    let (row_fields, col_fields, page_fields, data_fields) =
        refreshed_pivot_field_areas(pivot, &headers, area_properties)?;
    let axis_fields: std::collections::HashSet<usize> = row_fields
        .iter()
        .chain(&col_fields)
        .chain(&page_fields)
        .copied()
        .collect();
    let numeric_fields: std::collections::HashSet<usize> = data_fields
        .iter()
        .map(|(field, _)| *field)
        .filter(|field| {
            !axis_fields.contains(field)
                && rows
                    .iter()
                    .all(|row| row[*field].is_empty() || row[*field].parse::<f64>().is_ok())
        })
        .collect();
    let (mut cache_xml, records_xml, field_items) = crate::add::build_pivot_cache_xml(
        &source_sheet,
        &source_ref,
        &headers,
        &rows,
        &numeric_fields,
    );

    let pivot_rels_path = relationships_part_path(pivot_path);
    let pivot_rels = package
        .read_part_xml(&pivot_rels_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let old_cache_path =
        relationship_target_by_type(&pivot_rels, pivot_path, "pivotCacheDefinition")?;
    let referrers = count_pivot_cache_referrers(package, &old_cache_path);
    let (cache_path, records_path, cache_id, cache_rel_id, is_copy_on_write) = if referrers > 1 {
        let cache_index =
            crate::add::next_part_index(package, "xl/pivotCache/pivotCacheDefinition");
        let records_index = crate::add::next_part_index(package, "xl/pivotCache/pivotCacheRecords");
        (
            format!("xl/pivotCache/pivotCacheDefinition{}.xml", cache_index),
            format!("xl/pivotCache/pivotCacheRecords{}.xml", records_index),
            crate::add::next_pivot_cache_id(package)?,
            "rId1".to_string(),
            true,
        )
    } else {
        let cache_rels_path = relationships_part_path(&old_cache_path);
        let cache_rels = package
            .read_part_xml(&cache_rels_path)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        let records_path =
            relationship_target_by_type(&cache_rels, &old_cache_path, "pivotCacheRecords")?;
        let records_rel_id = relationship_id_by_type(&cache_rels, "pivotCacheRecords")?;
        (
            old_cache_path,
            records_path,
            pivot
                .cache_id
                .as_deref()
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| {
                    HandlerError::OperationFailed("pivot cache ID is invalid".to_string())
                })?,
            records_rel_id,
            false,
        )
    };
    cache_xml = cache_xml.replace("r:id=\"rId1\"", &format!("r:id=\"{}\"", cache_rel_id));
    package
        .write_part_xml(&cache_path, &cache_xml)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    package
        .write_part_xml(&records_path, &records_xml)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;

    let mut updated_pivot_xml = rebuild_refreshed_pivot_definition(
        pivot_xml,
        pivot,
        &headers,
        &field_items,
        &row_fields,
        &col_fields,
        &page_fields,
        &data_fields,
    )?;
    if is_copy_on_write {
        let records_index = records_path
            .rsplit_once("pivotCacheRecords")
            .and_then(|(_, tail)| tail.strip_suffix(".xml"))
            .ok_or_else(|| HandlerError::OperationFailed("invalid records path".to_string()))?;
        crate::add::inject_relationship(
            package,
            &relationships_part_path(&cache_path),
            &format!(
                "<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotCacheRecords\" Target=\"pivotCacheRecords{}.xml\"/>",
                records_index
            ),
        )?;
        let cache_index = cache_path
            .rsplit_once("pivotCacheDefinition")
            .and_then(|(_, tail)| tail.strip_suffix(".xml"))
            .ok_or_else(|| HandlerError::OperationFailed("invalid cache path".to_string()))?;
        let workbook_rels_path = "xl/_rels/workbook.xml.rels";
        let workbook_rel_id = crate::add::next_rel_id_in_part(package, workbook_rels_path);
        crate::add::inject_relationship(
            package,
            workbook_rels_path,
            &format!(
                "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/pivotCacheDefinition\" Target=\"pivotCache/pivotCacheDefinition{}.xml\"/>",
                workbook_rel_id, cache_index
            ),
        )?;
        let workbook_xml = package
            .read_part_xml("xl/workbook.xml")
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        let workbook_xml =
            crate::add::insert_pivot_cache_entry(&workbook_xml, cache_id, &workbook_rel_id)?;
        package
            .write_part_xml("xl/workbook.xml", &workbook_xml)
            .map_err(|error| HandlerError::SaveError(error.to_string()))?;
        let target = crate::add::relative_path("xl/pivotTables", &cache_path);
        let pivot_rels =
            rewrite_relationship_target_by_type(&pivot_rels, "pivotCacheDefinition", &target)?;
        package
            .write_part_xml(&pivot_rels_path, &pivot_rels)
            .map_err(|error| HandlerError::SaveError(error.to_string()))?;
        updated_pivot_xml = set_first_element_attributes(
            &updated_pivot_xml,
            "pivotTableDefinition",
            &[("cacheId", cache_id.to_string())],
        )?;
        crate::add::update_content_types_for_pivot(
            package,
            pivot_path,
            &cache_path,
            &records_path,
        )?;
    }
    Ok(updated_pivot_xml)
}

type PivotFieldAreas = (Vec<usize>, Vec<usize>, Vec<usize>, Vec<(usize, String)>);

/// Resolve the field areas to apply after a source refresh. Existing areas
/// keep their positional field references, while properties supplied in this
/// same Set call are parsed against the replacement headers. This deliberately
/// validates before writing cache XML so a narrowed source cannot leave a
/// half-updated PivotTable behind.
fn refreshed_pivot_field_areas(
    pivot: &PivotTableDef,
    headers: &[String],
    properties: &HashMap<String, String>,
) -> Result<PivotFieldAreas, HandlerError> {
    let old_fields = |fields: &[i32], area: &str| -> Result<Vec<usize>, HandlerError> {
        fields
            .iter()
            .filter(|field| **field >= 0)
            .map(|field| {
                let field = *field as usize;
                (field < headers.len()).then_some(field).ok_or_else(|| {
                    HandlerError::InvalidArgument(format!(
                        "{} field index {} is out of range after source changed to {} column(s); restate {}= in the same set",
                        area,
                        field,
                        headers.len(),
                        area
                    ))
                })
            })
            .collect()
    };
    let rows = properties
        .get("rows")
        .map(|value| parse_cached_pivot_fields(value, headers, "rows"))
        .transpose()?
        .unwrap_or(old_fields(&pivot.row_fields, "rows")?);
    let cols = properties
        .get("cols")
        .map(|value| parse_cached_pivot_fields(value, headers, "cols"))
        .transpose()?
        .unwrap_or(old_fields(&pivot.col_fields, "cols")?);
    let filters = properties
        .get("filters")
        .map(|value| parse_cached_pivot_fields(value, headers, "filters"))
        .transpose()?
        .unwrap_or(old_fields(&pivot.page_fields, "filters")?);
    let mut values = properties
        .get("values")
        .map(|value| parse_cached_pivot_values(value, headers, None))
        .transpose()?
        .unwrap_or_else(|| {
            pivot
                .data_fields
                .iter()
                .filter_map(|(_, aggregate, field)| {
                    (*field >= 0).then_some((*field as usize, aggregate.clone()))
                })
                .collect()
        });
    if !properties.contains_key("values") {
        for (field, _) in &values {
            if *field >= headers.len() {
                return Err(HandlerError::InvalidArgument(format!(
                    "values field index {} is out of range after source changed to {} column(s); restate values= in the same set",
                    field,
                    headers.len()
                )));
            }
        }
    }
    if let Some(aggregate) = properties.get("aggregate") {
        let aggregate = normalize_pivot_aggregate(aggregate)?;
        for (_, value_aggregate) in &mut values {
            *value_aggregate = aggregate.clone();
        }
    }
    Ok((rows, cols, filters, values))
}

/// Replace only the schema-dependent definition children after the cache has
/// been rebuilt. Root attributes, location, style, extLst and unknown producer
/// extensions remain in place, which is materially safer than replacing the
/// complete pivotTableDefinition XML.
#[allow(clippy::too_many_arguments)]
fn rebuild_refreshed_pivot_definition(
    xml: &str,
    pivot: &PivotTableDef,
    headers: &[String],
    field_items: &[Vec<String>],
    rows: &[usize],
    cols: &[usize],
    filters: &[usize],
    values: &[(usize, String)],
) -> Result<String, HandlerError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid pivot XML: {}", error)))?;
    let pivot_fields = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "pivotFields")
        .ok_or_else(|| {
            HandlerError::OperationFailed("pivot definition has no pivotFields".to_string())
        })?;
    let entries = headers
        .iter()
        .enumerate()
        .map(|(field, _)| {
            let data_only = values.iter().any(|(index, _)| *index == field)
                && !rows.contains(&field)
                && !cols.contains(&field)
                && !filters.contains(&field);
            if data_only {
                return "<pivotField dataField=\"1\" showAll=\"0\"/>".to_string();
            }
            let axis = if rows.contains(&field) {
                " axis=\"axisRow\""
            } else if cols.contains(&field) {
                " axis=\"axisCol\""
            } else if filters.contains(&field) {
                " axis=\"axisPage\""
            } else if values.iter().any(|(index, _)| *index == field) {
                " dataField=\"1\""
            } else {
                ""
            };
            let items = field_items
                .get(field)
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, _)| format!("<item x=\"{}\"/>", index))
                .collect::<String>();
            let item_count = field_items.get(field).map_or(0, Vec::len);
            format!(
                "<pivotField{} showAll=\"0\"><items count=\"{}\">{}</items></pivotField>",
                axis, item_count, items
            )
        })
        .collect::<String>();
    let replacement = format!(
        "<pivotFields count=\"{}\">{}</pivotFields>",
        headers.len(),
        entries
    );
    let out = apply_replacements(xml, vec![(pivot_fields.range(), replacement)]);
    let out = replace_pivot_field_area_elements(&out, rows, cols, filters, values, headers)?;
    let out = restore_refreshed_data_field_show_as(&out, pivot, values)?;
    let out = set_pivot_repeat_labels(&out, pivot.repeat_labels)?;
    set_pivot_blank_rows(&out, pivot.blank_rows)
}

fn restore_refreshed_data_field_show_as(
    xml: &str,
    pivot: &PivotTableDef,
    values: &[(usize, String)],
) -> Result<String, HandlerError> {
    let mut modes = Vec::with_capacity(values.len());
    for (field, aggregate) in values {
        let mode = pivot
            .data_fields
            .iter()
            .enumerate()
            .find(|(_, (_, old_aggregate, old_field))| {
                *old_field == *field as i32 && old_aggregate.eq_ignore_ascii_case(aggregate)
            })
            .and_then(|(index, _)| pivot.data_field_show_as.get(index))
            .cloned()
            .flatten();
        modes.push(mode);
    }
    if modes.iter().all(Option::is_none) {
        return Ok(xml.to_string());
    }
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid pivot XML: {}", error)))?;
    let mut ranges: Vec<Range<usize>> = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "dataField")
        .map(|node| node.range())
        .collect();
    ranges.sort_by_key(|range| range.start);
    let mut out = xml.to_string();
    for (index, range) in ranges.into_iter().enumerate().rev() {
        let Some(Some(mode)) = modes.get(index) else {
            continue;
        };
        out = rewrite_opening_tag_attributes(&out, range, &[("showDataAs", mode.clone())])?;
    }
    Ok(out)
}

fn parse_pivot_source_for_refresh(
    source: &str,
    pivot: &PivotTableDef,
) -> Result<(String, String), HandlerError> {
    let source = source.trim();
    if source.is_empty() || source.starts_with('[') {
        return Err(HandlerError::InvalidArgument(
            "pivot source must be a non-empty local worksheet range".to_string(),
        ));
    }
    let (sheet, reference) = match source.rsplit_once('!') {
        Some((sheet, reference)) => (sheet.trim().trim_matches('\''), reference.trim()),
        None => (
            pivot
                .source_range
                .as_deref()
                .and_then(|source| source.rsplit_once('!').map(|(sheet, _)| sheet))
                .ok_or_else(|| {
                    HandlerError::OperationFailed("pivot cache has no worksheet source".to_string())
                })?,
            source,
        ),
    };
    if sheet.is_empty() || reference.is_empty() {
        return Err(HandlerError::InvalidArgument(
            "invalid pivot source".to_string(),
        ));
    }
    Ok((sheet.to_string(), reference.replace('$', "")))
}

fn parse_pivot_refresh_range(reference: &str) -> Result<(CellRef, CellRef), HandlerError> {
    let mut cells = reference.split(':');
    let parse = |value: Option<&str>| {
        value
            .map(str::to_ascii_uppercase)
            .as_deref()
            .and_then(CellRef::parse)
    };
    let start = parse(cells.next()).ok_or_else(|| {
        HandlerError::InvalidArgument(format!("invalid pivot source range '{}'", reference))
    })?;
    let end = parse(cells.next()).ok_or_else(|| {
        HandlerError::InvalidArgument(format!("invalid pivot source range '{}'", reference))
    })?;
    if cells.next().is_some() || start.row > end.row || start.col > end.col {
        return Err(HandlerError::InvalidArgument(format!(
            "invalid pivot source range '{}'",
            reference
        )));
    }
    Ok((start, end))
}

fn count_pivot_cache_referrers(package: &OxmlPackage, cache_path: &str) -> usize {
    package
        .list_parts()
        .iter()
        .filter(|part| part.starts_with("xl/pivotTables/") && part.ends_with(".xml"))
        .filter(|part| {
            package
                .read_part_xml(&relationships_part_path(part))
                .ok()
                .and_then(|rels| {
                    relationship_target_by_type(&rels, part, "pivotCacheDefinition").ok()
                })
                .is_some_and(|target| target == cache_path)
        })
        .count()
}

fn relationship_id_by_type(rels_xml: &str, type_suffix: &str) -> Result<String, HandlerError> {
    let document = roxmltree::Document::parse(rels_xml).map_err(|error| {
        HandlerError::OperationFailed(format!("invalid relationships XML: {}", error))
    })?;
    document
        .descendants()
        .find(|node| {
            node.is_element()
                && node.tag_name().name() == "Relationship"
                && node
                    .attribute("Type")
                    .is_some_and(|relationship_type| relationship_type.ends_with(type_suffix))
        })
        .and_then(|node| node.attribute("Id"))
        .map(str::to_string)
        .ok_or_else(|| {
            HandlerError::OperationFailed(format!("{} relationship not found", type_suffix))
        })
}

fn rewrite_relationship_target_by_type(
    rels_xml: &str,
    type_suffix: &str,
    target: &str,
) -> Result<String, HandlerError> {
    let document = roxmltree::Document::parse(rels_xml).map_err(|error| {
        HandlerError::OperationFailed(format!("invalid relationships XML: {}", error))
    })?;
    let relationship = document
        .descendants()
        .find(|node| {
            node.is_element()
                && node.tag_name().name() == "Relationship"
                && node
                    .attribute("Type")
                    .is_some_and(|relationship_type| relationship_type.ends_with(type_suffix))
        })
        .ok_or_else(|| {
            HandlerError::OperationFailed(format!("{} relationship not found", type_suffix))
        })?;
    rewrite_opening_tag_attributes(
        rels_xml,
        relationship.range(),
        &[("Target", target.to_string())],
    )
}

/// Reassign existing cache fields to the pivot row/column/page/data areas.
/// This changes only the definition; cacheFields and cacheRecords remain
/// intact, so field names are validated against the cache's authoritative
/// order and no stale records can be introduced.
fn rebuild_pivot_field_areas(
    xml: &str,
    pivot: &crate::dom_types::PivotTableDef,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    if pivot.cache_fields.is_empty() {
        return Err(HandlerError::OperationFailed(
            "pivot cache fields are unavailable; cannot rebuild pivot areas".to_string(),
        ));
    }
    let rows = properties
        .get("rows")
        .map(|value| parse_cached_pivot_fields(value, &pivot.cache_fields, "rows"))
        .transpose()?
        .unwrap_or_else(|| {
            pivot
                .row_fields
                .iter()
                .map(|field| *field as usize)
                .collect()
        });
    let cols = properties
        .get("cols")
        .map(|value| parse_cached_pivot_fields(value, &pivot.cache_fields, "cols"))
        .transpose()?
        .unwrap_or_else(|| {
            pivot
                .col_fields
                .iter()
                .map(|field| *field as usize)
                .collect()
        });
    let filters = properties
        .get("filters")
        .map(|value| parse_cached_pivot_fields(value, &pivot.cache_fields, "filters"))
        .transpose()?
        .unwrap_or_else(|| {
            pivot
                .page_fields
                .iter()
                .map(|field| *field as usize)
                .collect()
        });
    let values = properties
        .get("values")
        .map(|value| parse_cached_pivot_values(value, &pivot.cache_fields, None))
        .transpose()?
        .unwrap_or_else(|| {
            pivot
                .data_fields
                .iter()
                .filter(|(_, _, field)| *field >= 0)
                .map(|(_, aggregate, field)| (*field as usize, aggregate.clone()))
                .collect()
        });
    let values = if let Some(aggregate) = properties.get("aggregate") {
        let aggregate = normalize_pivot_aggregate(aggregate)?;
        values
            .into_iter()
            .map(|(field, _)| (field, aggregate.clone()))
            .collect()
    } else {
        values
    };
    let field_count = pivot.cache_fields.len();
    for field in rows
        .iter()
        .chain(&cols)
        .chain(&filters)
        .chain(values.iter().map(|(field, _)| field))
    {
        if *field >= field_count {
            return Err(HandlerError::InvalidArgument(
                "pivot field index out of range".to_string(),
            ));
        }
    }
    let mut out = rewrite_pivot_field_axis_attributes(xml, &rows, &cols, &filters, &values)?;
    out = replace_pivot_field_area_elements(
        &out,
        &rows,
        &cols,
        &filters,
        &values,
        &pivot.cache_fields,
    )?;
    Ok(out)
}

fn parse_cached_pivot_fields(
    value: &str,
    headers: &[String],
    property: &str,
) -> Result<Vec<usize>, HandlerError> {
    let mut fields = Vec::new();
    for field in value.split(',').filter(|field| !field.trim().is_empty()) {
        let name = field.trim();
        let index = headers
            .iter()
            .position(|header| header.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                HandlerError::InvalidArgument(format!(
                    "pivot {} field '{}' is not present in cache headers",
                    property, name
                ))
            })?;
        if !fields.contains(&index) {
            fields.push(index);
        }
    }
    Ok(fields)
}

fn parse_cached_pivot_values(
    value: &str,
    headers: &[String],
    default_aggregate: Option<&str>,
) -> Result<Vec<(usize, String)>, HandlerError> {
    let mut fields = Vec::new();
    for field in value.split(',').filter(|field| !field.trim().is_empty()) {
        let (name, aggregate) = field.trim().split_once(':').unwrap_or((field.trim(), ""));
        let index = headers
            .iter()
            .position(|header| header.eq_ignore_ascii_case(name.trim()))
            .ok_or_else(|| {
                HandlerError::InvalidArgument(format!(
                    "pivot values field '{}' is not present in cache headers",
                    name.trim()
                ))
            })?;
        let aggregate = if aggregate.trim().is_empty() {
            default_aggregate.unwrap_or("sum")
        } else {
            aggregate.trim()
        };
        fields.push((index, normalize_pivot_aggregate(aggregate)?));
    }
    Ok(fields)
}

fn normalize_pivot_aggregate(value: &str) -> Result<String, HandlerError> {
    let aggregate = value.trim().to_ascii_lowercase();
    if matches!(
        aggregate.as_str(),
        "sum"
            | "count"
            | "avg"
            | "max"
            | "min"
            | "product"
            | "stdev"
            | "stdevp"
            | "var"
            | "varp"
            | "countnums"
    ) {
        Ok(aggregate)
    } else {
        Err(HandlerError::InvalidArgument(format!(
            "unsupported pivot aggregate '{}'",
            value
        )))
    }
}

fn rewrite_pivot_field_axis_attributes(
    xml: &str,
    rows: &[usize],
    cols: &[usize],
    filters: &[usize],
    values: &[(usize, String)],
) -> Result<String, HandlerError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid pivot XML: {}", error)))?;
    let mut ranges: Vec<Range<usize>> = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "pivotField")
        .map(|node| node.range())
        .collect();
    ranges.sort_by_key(|range| range.start);
    let mut out = xml.to_string();
    for (index, range) in ranges.into_iter().enumerate().rev() {
        out = remove_opening_tag_attributes(&out, range.clone(), &["axis", "dataField"])?;
        let axis = if rows.contains(&index) {
            Some("axisRow")
        } else if cols.contains(&index) {
            Some("axisCol")
        } else if filters.contains(&index) {
            Some("axisPage")
        } else {
            None
        };
        let mut attributes = Vec::new();
        if let Some(axis) = axis {
            attributes.push(("axis", axis.to_string()));
        }
        if values.iter().any(|(field, _)| *field == index) {
            attributes.push(("dataField", "1".to_string()));
        }
        if !attributes.is_empty() {
            out = rewrite_opening_tag_attributes(&out, range, &attributes)?;
        }
    }
    Ok(out)
}

fn replace_pivot_field_area_elements(
    xml: &str,
    rows: &[usize],
    cols: &[usize],
    filters: &[usize],
    values: &[(usize, String)],
    headers: &[String],
) -> Result<String, HandlerError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid pivot XML: {}", error)))?;
    let ranges: Vec<Range<usize>> = document
        .descendants()
        .filter(|node| {
            node.is_element()
                && matches!(
                    node.tag_name().name(),
                    "rowFields" | "colFields" | "pageFields" | "dataFields"
                )
        })
        .map(|node| node.range())
        .collect();
    let mut out = remove_ranges(xml, ranges);
    let fields = |tag: &str, values: &[usize]| {
        (!values.is_empty()).then(|| {
            let entries = values
                .iter()
                .map(|field| format!("<field x=\"{}\"/>", field))
                .collect::<String>();
            format!("<{} count=\"{}\">{}</{}>", tag, values.len(), entries, tag)
        })
    };
    let pages = (!filters.is_empty()).then(|| {
        let entries = filters
            .iter()
            .map(|field| format!("<pageField fld=\"{}\" hier=\"-1\"/>", field))
            .collect::<String>();
        format!(
            "<pageFields count=\"{}\">{}</pageFields>",
            filters.len(),
            entries
        )
    });
    let data_entries = values
        .iter()
        .map(|(field, aggregate)| {
            let display = if aggregate == "avg" {
                "Average"
            } else {
                aggregate
            };
            format!(
                "<dataField name=\"{} of {}\" fld=\"{}\" subtotal=\"{}\"/>",
                escape_pivot_xml_attribute(display),
                escape_pivot_xml_attribute(&headers[*field]),
                field,
                pivot_aggregate_ooxml(aggregate)
            )
        })
        .collect::<String>();
    let area_xml = format!(
        "{}{}{}<dataFields count=\"{}\">{}</dataFields>",
        fields("rowFields", rows).unwrap_or_default(),
        fields("colFields", cols).unwrap_or_default(),
        pages.unwrap_or_default(),
        values.len(),
        data_entries
    );
    let anchor = out.find("</pivotFields>").ok_or_else(|| {
        HandlerError::OperationFailed("pivot definition has no pivotFields".to_string())
    })? + "</pivotFields>".len();
    out.insert_str(anchor, &area_xml);
    Ok(out)
}

fn pivot_aggregate_ooxml(aggregate: &str) -> &str {
    match aggregate {
        "avg" => "average",
        "stdev" => "stdDev",
        "stdevp" => "stdDevp",
        "countnums" => "countNums",
        other => other,
    }
}

fn remove_opening_tag_attributes(
    xml: &str,
    node_range: Range<usize>,
    names: &[&str],
) -> Result<String, HandlerError> {
    let opening_end = xml[node_range.clone()]
        .find('>')
        .map(|offset| node_range.start + offset + 1)
        .ok_or_else(|| HandlerError::OperationFailed("malformed pivot XML tag".to_string()))?;
    let opening = &xml[node_range.start..opening_end];
    let mut rewritten = opening.to_string();
    for name in names {
        let needle = format!(" {}=\"", name);
        while let Some(start) = rewritten.find(&needle) {
            let value_start = start + needle.len();
            let value_end = rewritten[value_start..]
                .find('"')
                .map(|offset| value_start + offset + 1)
                .ok_or_else(|| {
                    HandlerError::OperationFailed("malformed pivot attribute".to_string())
                })?;
            rewritten.replace_range(start..value_end, "");
        }
    }
    let mut out = xml.to_string();
    out.replace_range(node_range.start..opening_end, &rewritten);
    Ok(out)
}

fn set_first_element_attributes(
    xml: &str,
    element_name: &str,
    attributes: &[(&str, String)],
) -> Result<String, HandlerError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid pivot XML: {}", error)))?;
    let node = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == element_name)
        .ok_or_else(|| HandlerError::OperationFailed(format!("missing {}", element_name)))?;
    rewrite_opening_tag_attributes(xml, node.range(), attributes)
}

fn set_all_pivot_field_attributes(
    xml: &str,
    attribute: &str,
    value: &str,
) -> Result<String, HandlerError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid pivot XML: {}", error)))?;
    let mut ranges: Vec<Range<usize>> = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "pivotField")
        .map(|node| node.range())
        .collect();
    ranges.sort_by(|left, right| right.start.cmp(&left.start));
    let mut out = xml.to_string();
    for range in ranges {
        out = rewrite_opening_tag_attributes(&out, range, &[(attribute, value.to_string())])?;
    }
    Ok(out)
}

fn set_or_insert_pivot_style(
    xml: &str,
    attributes: &[(&str, String)],
) -> Result<String, HandlerError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid pivot XML: {}", error)))?;
    if let Some(node) = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "pivotTableStyleInfo")
    {
        return rewrite_opening_tag_attributes(xml, node.range(), attributes);
    }
    let root = document.root_element();
    let root_xml = &xml[root.range()];
    let closing = root_xml.rfind("</").ok_or_else(|| {
        HandlerError::OperationFailed("malformed pivotTableDefinition XML".to_string())
    })? + root.range().start;
    let mut attrs = vec![
        ("name", "PivotStyleLight16".to_string()),
        ("showRowHeaders", "1".to_string()),
        ("showColHeaders", "1".to_string()),
        ("showRowStripes", "0".to_string()),
        ("showColStripes", "0".to_string()),
    ];
    for (key, value) in attributes {
        if let Some(existing) = attrs.iter_mut().find(|(existing, _)| *existing == *key) {
            existing.1 = value.clone();
        } else {
            attrs.push((key, value.clone()));
        }
    }
    let rendered = attrs
        .iter()
        .map(|(key, value)| format!(" {}=\"{}\"", key, escape_pivot_xml_attribute(value)))
        .collect::<String>();
    let mut out = xml.to_string();
    out.insert_str(closing, &format!("<pivotTableStyleInfo{}/>", rendered));
    Ok(out)
}

fn rewrite_opening_tag_attributes(
    xml: &str,
    node_range: Range<usize>,
    attributes: &[(&str, String)],
) -> Result<String, HandlerError> {
    let opening_end = xml[node_range.clone()]
        .find('>')
        .map(|offset| node_range.start + offset + 1)
        .ok_or_else(|| HandlerError::OperationFailed("malformed pivot XML tag".to_string()))?;
    let opening = &xml[node_range.start..opening_end];
    let self_closing = opening.ends_with("/>");
    let suffix = if self_closing { "/>" } else { ">" };
    let mut rewritten = opening.trim_end_matches(suffix).to_string();
    for (name, value) in attributes {
        let needle = format!("{}=\"", name);
        if let Some(value_start) = rewritten.find(&needle).map(|index| index + needle.len()) {
            let value_end = rewritten[value_start..]
                .find('"')
                .map(|offset| value_start + offset)
                .ok_or_else(|| {
                    HandlerError::OperationFailed("malformed pivot attribute".to_string())
                })?;
            rewritten.replace_range(value_start..value_end, &escape_pivot_xml_attribute(value));
        } else {
            rewritten.push_str(&format!(
                " {}=\"{}\"",
                name,
                escape_pivot_xml_attribute(value)
            ));
        }
    }
    rewritten.push_str(suffix);
    let mut out = xml.to_string();
    out.replace_range(node_range.start..opening_end, &rewritten);
    Ok(out)
}

fn escape_pivot_xml_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
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
fn set_cell_value(
    xml: &str,
    cell_ref: &str,
    value: &str,
    shared_strings: &[String],
    p: &str,
) -> Result<String, HandlerError> {
    // Check if the value matches an existing shared string
    let ss_idx = shared_strings.iter().position(|s| s == value);

    let (t_attr, v_content) = if let Some(idx) = ss_idx {
        // Use shared string reference
        ("t=\"s\"".to_string(), idx.to_string())
    } else if value == "TRUE" || value == "FALSE" {
        // Boolean
        (
            "t=\"b\"".to_string(),
            if value == "TRUE" {
                "1".to_string()
            } else {
                "0".to_string()
            },
        )
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
        let new_cell = build_cell_xml(
            cell_ref,
            &t_attr,
            &v_content,
            None,
            &extract_existing_style(cell_xml),
            p,
        );

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
fn set_cell_formula(
    xml: &str,
    cell_ref: &str,
    formula: &str,
    dynamic_cm: Option<usize>,
    p: &str,
) -> Result<String, HandlerError> {
    let cell_pattern = format!("<{}c r=\"{}\"", p, cell_ref);
    if let Some(cell_start) = xml.find(&cell_pattern) {
        let cell_end = find_cell_element_end(xml, cell_start, p)?;

        let cell_xml = &xml[cell_start..cell_end];
        let existing_style = extract_existing_style(cell_xml);
        let cell_attributes = with_dynamic_cm(&existing_style, dynamic_cm);
        let existing_type = extract_existing_type(cell_xml);
        let existing_value = extract_existing_value(cell_xml, p);

        // Formula cells: type should be empty (calculated) or "str" if result is inline string
        let new_cell = build_cell_xml(
            cell_ref,
            &existing_type,
            &existing_value,
            Some(formula),
            &cell_attributes,
            p,
        );

        let mut result = xml[..cell_start].to_string();
        result.push_str(&new_cell);
        result.push_str(&xml[cell_end..]);
        Ok(result)
    } else {
        // Insert new cell with formula (type defaults to calculated)
        let cell_attributes = with_dynamic_cm("", dynamic_cm);
        insert_new_cell(xml, cell_ref, "", "", Some(formula), &cell_attributes, p)
    }
}

/// Materialize cached values for a calculated dynamic spill. The anchor keeps
/// its formula; descendants are ordinary cache cells so non-Excel consumers
/// can read the complete result before Excel recalculates the workbook.
pub(crate) fn persist_dynamic_spill(
    xml: &str,
    anchor: &str,
    result: &formula::FormulaResult,
    p: &str,
) -> Result<String, HandlerError> {
    let formula::FormulaResult::Matrix(rows) = result else {
        return Ok(xml.to_string());
    };
    let anchor = CellRef::parse(anchor)
        .ok_or_else(|| HandlerError::InvalidPath("invalid dynamic spill anchor".to_string()))?;
    let mut output = xml.to_string();
    for (row_offset, values) in rows.iter().enumerate() {
        for (column_offset, value) in values.iter().enumerate() {
            if row_offset == 0 && column_offset == 0 {
                continue;
            }
            let reference = CellRef {
                row: anchor.row + row_offset,
                col: anchor.col + column_offset,
            }
            .to_string_ref();
            let cache = value.to_cell_value_text();
            output = set_cell_value(&output, &reference, &cache, &[], p)?;
        }
    }
    Ok(output)
}

pub(crate) fn ensure_dynamic_spill_targets_clear(
    xml: &str,
    anchor: &str,
    result: &formula::FormulaResult,
    p: &str,
) -> Result<(), HandlerError> {
    let formula::FormulaResult::Matrix(rows) = result else {
        return Ok(());
    };
    let anchor = CellRef::parse(anchor)
        .ok_or_else(|| HandlerError::InvalidPath("invalid dynamic spill anchor".to_string()))?;
    for (row_offset, values) in rows.iter().enumerate() {
        for (column_offset, _) in values.iter().enumerate() {
            if row_offset == 0 && column_offset == 0 {
                continue;
            }
            let reference = CellRef {
                row: anchor.row + row_offset,
                col: anchor.col + column_offset,
            }
            .to_string_ref();
            let pattern = format!("<{}c r=\"{}\"", p, reference);
            if let Some(start) = xml.find(&pattern) {
                let end = find_cell_element_end(xml, start, p)?;
                if !xml[start..end].trim_end().ends_with("/>") {
                    return Err(HandlerError::OperationFailed(format!(
                        "#SPILL! dynamic array at {} is blocked by {}",
                        anchor.to_string_ref(),
                        reference
                    )));
                }
            }
        }
    }
    Ok(())
}

fn set_in_cell_image(
    xml: &str,
    cell_ref: &str,
    value_metadata_index: usize,
    p: &str,
) -> Result<String, HandlerError> {
    let cell_pattern = format!("<{}c r=\"{}\"", p, cell_ref);
    let style = if let Some(start) = xml.find(&cell_pattern) {
        let end = find_cell_element_end(xml, start, p)?;
        extract_existing_style(&xml[start..end])
    } else {
        String::new()
    };
    let style = if style.is_empty() {
        String::new()
    } else {
        format!(" {style}")
    };
    let replacement = format!(
        "<{}c r=\"{}\"{} t=\"e\" vm=\"{}\"><{}v>#VALUE!</{}v></{}c>",
        p, cell_ref, style, value_metadata_index, p, p, p
    );
    replace_or_insert_cell(xml, cell_ref, &replacement, p)
}

fn remove_in_cell_image(xml: &str, cell_ref: &str, p: &str) -> Result<String, HandlerError> {
    let cell_pattern = format!("<{}c r=\"{}\"", p, cell_ref);
    let Some(start) = xml.find(&cell_pattern) else {
        return Ok(xml.to_string());
    };
    let end = find_cell_element_end(xml, start, p)?;
    let existing = &xml[start..end];
    if !existing.contains(" vm=") || !existing.contains(">#VALUE!<") {
        return Ok(xml.to_string());
    }
    let style = extract_existing_style(existing);
    let style = if style.is_empty() {
        String::new()
    } else {
        format!(" {style}")
    };
    let replacement = format!("<{}c r=\"{}\"{}/>", p, cell_ref, style);
    let mut result = xml[..start].to_string();
    result.push_str(&replacement);
    result.push_str(&xml[end..]);
    Ok(result)
}

/// Set the style index of a cell.
fn set_cell_style(
    xml: &str,
    cell_ref: &str,
    style_index: &str,
    p: &str,
) -> Result<String, HandlerError> {
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
        let spill = if formula::is_dynamic_array_formula(f) {
            format!(" t=\"array\" ref=\"{}\"", ref_str)
        } else {
            String::new()
        };
        cell.push_str(&format!(
            "<{}f{spill}>{}</{}f>",
            p,
            escape_xml_formula(f),
            p
        ));
    }

    if !v_content.is_empty() {
        cell.push_str(&format!("<{}v>{}</{}v>", p, v_content, p));
    }

    cell.push_str(&format!("</{}c>", p));
    cell
}

fn with_dynamic_cm(style_attr: &str, dynamic_cm: Option<usize>) -> String {
    match dynamic_cm {
        Some(cm) if style_attr.is_empty() => format!("cm=\"{cm}\""),
        Some(cm) => format!("{style_attr} cm=\"{cm}\""),
        None => style_attr.to_string(),
    }
}

fn escape_xml_formula(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Find the end position of a <c> element in XML text.
/// Handles both self-closing <c .../> and regular <c ...>...</c>.
fn find_cell_element_end(xml: &str, start: usize, p: &str) -> Result<usize, HandlerError> {
    // Check for regular closing tag — need to find matching </c>
    // Look for the next '>' after start to see if self-closing or not
    let first_gt = xml[start..]
        .find('>')
        .map(|pos| start + pos)
        .ok_or_else(|| {
            HandlerError::OperationFailed("malformed XML: no '>' in cell tag".to_string())
        })?;

    // Check if the character before '>' is '/' (self-closing)
    if xml.as_bytes().get(first_gt - 1) == Some(&b'/') {
        // Self-closing: end is at first_gt + 1
        Ok(first_gt + 1)
    } else {
        // Regular element: find </c>
        let close_tag = format!("</{}c>", p);
        let close_tag_pos = xml[first_gt..]
            .find(&close_tag)
            .map(|pos| first_gt + pos + close_tag.len())
            .ok_or_else(|| {
                HandlerError::OperationFailed(format!(
                    "malformed XML: no '{}' closing tag",
                    close_tag
                ))
            })?;
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
    // If the style is just a number, treat it as a style index (legacy behavior).
    // Otherwise (contains '=' or ';'), treat as a property-style key-value spec
    // and synthesize a stable style key for now.
    let resolved_style = if new_style.chars().all(|c| c.is_ascii_digit()) || new_style.is_empty() {
        new_style.to_string()
    } else {
        // Style properties string — for now, use a hash placeholder.
        // A future PR will register the style in styles.xml and return its id.
        let hash = new_style
            .bytes()
            .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
        // We can't actually register here without access to styles.xml,
        // so we emit a comment marker that downstream raw-set can interpret.
        // For now, return the parsed numeric portion if any.
        extract_style_id_from_spec(new_style).unwrap_or_else(|| (hash % 100).to_string())
    };

    let s_pattern = "s=\"";
    if let Some(s_start) = cell_xml.find(s_pattern) {
        let val_start = s_start + s_pattern.len();
        if let Some(val_end) = cell_xml[val_start..].find('"') {
            let full_val_end = val_start + val_end;
            let mut result = cell_xml[..s_start].to_string();
            result.push_str(&format!("s=\"{}\"", resolved_style));
            result.push_str(&cell_xml[full_val_end + 1..]);
            return result;
        }
    }
    // No existing style — insert s= attribute into the opening tag
    let insert_pos = cell_xml
        .find("/>")
        .or_else(|| cell_xml.find('>'))
        .unwrap_or(cell_xml.len());
    let mut result = cell_xml[..insert_pos].to_string();
    result.push_str(&format!(" s=\"{}\"", resolved_style));
    result.push_str(&cell_xml[insert_pos..]);
    result
}

/// Parse a style spec string ("bold=true;fontColor=FF0000;...") and return
/// a deterministic style index. This is a simplified mapping that converts
/// well-known property combinations into numeric style IDs.
/// Full implementation would register styles in xl/styles.xml.
fn extract_style_id_from_spec(spec: &str) -> Option<String> {
    // Map common style specs to predefined style indices.
    // In a full implementation this would parse styles.xml and add new entries.
    let parts: Vec<&str> = spec.split(';').collect();

    let mut has_bold = false;
    let mut has_italic = false;
    let mut has_fill = false;
    let mut has_color = false;
    let mut has_border = false;

    for part in &parts {
        let lower = part.to_lowercase();
        if lower.starts_with("bold=true") {
            has_bold = true;
        } else if lower.starts_with("italic=true") {
            has_italic = true;
        } else if lower.starts_with("fill=") {
            has_fill = true;
        } else if lower.starts_with("fontcolor=") || lower.starts_with("color=") {
            has_color = true;
        } else if lower.starts_with("border=") {
            has_border = true;
        }
    }

    // Simple deterministic mapping
    let mut id = 0;
    if has_bold {
        id += 1;
    }
    if has_italic {
        id += 2;
    }
    if has_fill {
        id += 4;
    }
    if has_color {
        id += 8;
    }
    if has_border {
        id += 16;
    }

    if id == 0 {
        None
    } else {
        Some(id.to_string())
    }
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
    replace_or_insert_cell(xml, ref_str, &new_cell, p)
}

fn replace_or_insert_cell(
    xml: &str,
    ref_str: &str,
    new_cell: &str,
    p: &str,
) -> Result<String, HandlerError> {
    let cell_pattern = format!("<{}c r=\"{}\"", p, ref_str);
    if let Some(start) = xml.find(&cell_pattern) {
        let end = find_cell_element_end(xml, start, p)?;
        let mut result = xml[..start].to_string();
        result.push_str(new_cell);
        result.push_str(&xml[end..]);
        return Ok(result);
    }

    // Find <sheetData> opening tag
    let sd_start = xml.find(&format!("<{}sheetData", p)).ok_or_else(|| {
        HandlerError::OperationFailed(format!("no <{}sheetData> element found", p))
    })?;

    // Find the first <row> inside sheetData, or the closing </sheetData>
    let after_sd = &xml[sd_start..];
    let sd_gt = after_sd
        .find('>')
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
        let row_gt = xml[abs_row_start..]
            .find('>')
            .map(|pos| abs_row_start + pos + 1)
            .ok_or_else(|| HandlerError::OperationFailed(format!("malformed <{}row>", p)))?;

        // Insert cell after the row opening tag
        let mut result = xml[..row_gt].to_string();
        result.push_str(new_cell);
        result.push_str(&xml[row_gt..]);
        Ok(result)
    } else {
        // No existing row — insert a new <row> with the cell
        let new_row = format!("<{}row r=\"{}\">{}  </{}row>", p, row_num, new_cell, p);

        // Insert before </sheetData>
        let sd_end_pattern = format!("</{}sheetData>", p);
        let sd_end = xml.find(&sd_end_pattern).ok_or_else(|| {
            HandlerError::OperationFailed(format!("no {} closing tag", sd_end_pattern))
        })?;

        let mut result = xml[..sd_end].to_string();
        result.push_str(&new_row);
        result.push('\n');
        result.push_str(&xml[sd_end..]);
        Ok(result)
    }
}

/// Apply font color and background highlights on cells.
pub fn apply_xlsx_range_highlights(
    package: &mut OxmlPackage,
    properties: &HashMap<String, String>,
    segments: &[handler_common::PathRangeSegment],
) -> Result<Vec<String>, HandlerError> {
    let color = properties
        .get("color")
        .or_else(|| properties.get("fontColor"));
    let bg_color = properties
        .get("bgColor")
        .or_else(|| properties.get("highlight"))
        .or_else(|| properties.get("bg"));

    if color.is_none() && bg_color.is_none() {
        return Ok(Vec::new());
    }

    // 1. Read and parse styles.xml
    let mut styles_xml = package.read_part_xml("xl/styles.xml").map_err(|e| {
        HandlerError::OperationFailed(format!("failed to read xl/styles.xml: {}", e))
    })?;

    let p = detect_stylesheet_namespace_prefix(&styles_xml);

    // 2. Build the new font XML if color specified
    let mut final_font_id = 0;
    if let Some(color_val) = color {
        let hex = format_excel_color(color_val);
        let mut new_font_xml = format!("<{}font>", p);

        let doc = roxmltree::Document::parse(&styles_xml)
            .map_err(|e| HandlerError::OperationFailed(format!("failed to parse styles: {}", e)))?;
        let fonts_node = doc.descendants().find(|n| n.has_tag_name("fonts"));
        let mut font_copied = false;
        if let Some(fn_node) = fonts_node {
            if let Some(first_font) = fn_node.children().find(|n| n.has_tag_name("font")) {
                for child in first_font.children().filter(|n| n.is_element()) {
                    if child.tag_name().name() != "color" {
                        let child_slice = &styles_xml[child.range()];
                        new_font_xml.push_str(child_slice);
                    }
                }
                font_copied = true;
            }
        }
        if !font_copied {
            new_font_xml.push_str(&format!(
                "<{}sz val=\"11\"/><{}name val=\"Calibri\"/>",
                p, p
            ));
        }
        new_font_xml.push_str(&format!("<{}color rgb=\"{}\"/>", p, hex));
        new_font_xml.push_str(&format!("</{}font>", p));

        final_font_id = append_element_to_tag(&mut styles_xml, "fonts", &new_font_xml)?;
    }

    // 3. Build the new fill XML if bg_color specified
    let mut final_fill_id = 0;
    if let Some(bg_val) = bg_color {
        let hex = format_excel_color(bg_val);
        let new_fill_xml = format!(
            "<{}fill><{}patternFill patternType=\"solid\"><{}fgColor rgb=\"{}\"/></{}patternFill></{}fill>",
            p, p, p, hex, p, p
        );
        final_fill_id = append_element_to_tag(&mut styles_xml, "fills", &new_fill_xml)?;
    }

    // 4. Parse original cellXfs list from original styles_xml to allow style inheritance without borrowing styles_xml
    struct XfInfo {
        font_id: usize,
        fill_id: usize,
        xml: String,
    }

    let xf_infos: Vec<XfInfo> = {
        let doc = roxmltree::Document::parse(&styles_xml)
            .map_err(|e| HandlerError::OperationFailed(format!("failed to parse styles: {}", e)))?;
        let cell_xfs_node = doc
            .descendants()
            .find(|n| n.has_tag_name("cellXfs"))
            .ok_or_else(|| HandlerError::OperationFailed("cellXfs not found".to_string()))?;
        cell_xfs_node
            .children()
            .filter(|n| n.has_tag_name("xf"))
            .map(|n| {
                let font_id = n
                    .attribute("fontId")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(0);
                let fill_id = n
                    .attribute("fillId")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(0);
                let xml_slice = &styles_xml[n.range()];
                XfInfo {
                    font_id,
                    fill_id,
                    xml: xml_slice.to_string(),
                }
            })
            .collect()
    };

    // 5. Group target cells by worksheet
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;
    let mut sheets_cells: HashMap<String, Vec<CellRef>> = HashMap::new();

    for seg in segments {
        let pc = navigation::parse_path(&seg.path)?;
        if let Some(sheet_name) = pc.sheet_name {
            if let Some(cell_ref) = pc.cell_ref {
                sheets_cells.entry(sheet_name).or_default().push(cell_ref);
            }
        }
    }

    // Cache to reuse new style indices: (orig_xf_id, font_id, fill_id) -> new_xf_id
    let mut style_cache: HashMap<(usize, usize, usize), usize> = HashMap::new();

    // 6. Process each worksheet
    for (sheet_name, cell_refs) in sheets_cells {
        let ws = model
            .sheets
            .iter()
            .find(|s| s.name == sheet_name)
            .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

        let part_path = ws.part_path.clone();
        let mut sheet_xml = package.read_part_xml(&part_path).map_err(|e| {
            HandlerError::OperationFailed(format!("failed to read sheet XML: {}", e))
        })?;

        let sp = detect_namespace_prefix(&sheet_xml);

        for cell_ref in cell_refs {
            let cell_ref_str = cell_ref.to_string_ref();
            let cell_pattern = format!("<{}c r=\"{}\"", sp, cell_ref_str);

            if let Some(cell_start) = sheet_xml.find(&cell_pattern) {
                let cell_end = find_cell_element_end(&sheet_xml, cell_start, &sp)?;
                let cell_xml = &sheet_xml[cell_start..cell_end];

                // Extract original style index
                let orig_style_index = if let Some(s_pos) = cell_xml.find("s=\"") {
                    let val_start = s_pos + "s=\"".len();
                    if let Some(val_len) = cell_xml[val_start..].find('"') {
                        cell_xml[val_start..val_start + val_len]
                            .parse::<usize>()
                            .unwrap_or(0)
                    } else {
                        0
                    }
                } else {
                    0
                };

                let (orig_font_id, orig_fill_id) = if orig_style_index < xf_infos.len() {
                    let xf = &xf_infos[orig_style_index];
                    (xf.font_id, xf.fill_id)
                } else {
                    (0, 0)
                };

                let target_font_id = if color.is_some() {
                    final_font_id
                } else {
                    orig_font_id
                };
                let target_fill_id = if bg_color.is_some() {
                    final_fill_id
                } else {
                    orig_fill_id
                };

                let cache_key = (orig_style_index, target_font_id, target_fill_id);
                let new_style_index = if let Some(&xf_id) = style_cache.get(&cache_key) {
                    xf_id
                } else {
                    let xf_xml = if orig_style_index < xf_infos.len() {
                        &xf_infos[orig_style_index].xml
                    } else {
                        &xf_infos[0].xml
                    };
                    let new_xf_xml = clone_xf_with_changes(
                        xf_xml,
                        target_font_id,
                        target_fill_id,
                        color.is_some(),
                        bg_color.is_some(),
                        &p,
                    )?;
                    let xf_id = append_element_to_tag(&mut styles_xml, "cellXfs", &new_xf_xml)?;
                    style_cache.insert(cache_key, xf_id);
                    xf_id
                };

                let updated_cell_xml = modify_style_in_cell(cell_xml, &new_style_index.to_string());
                sheet_xml = format!(
                    "{}{}{}",
                    &sheet_xml[..cell_start],
                    updated_cell_xml,
                    &sheet_xml[cell_end..]
                );
            } else {
                // Cell doesn't exist. We use default style 0 as original style
                let target_font_id = if color.is_some() { final_font_id } else { 0 };
                let target_fill_id = if bg_color.is_some() { final_fill_id } else { 0 };

                let cache_key = (0, target_font_id, target_fill_id);
                let new_style_index = if let Some(&xf_id) = style_cache.get(&cache_key) {
                    xf_id
                } else {
                    let xf_xml = &xf_infos[0].xml;
                    let new_xf_xml = clone_xf_with_changes(
                        xf_xml,
                        target_font_id,
                        target_fill_id,
                        color.is_some(),
                        bg_color.is_some(),
                        &p,
                    )?;
                    let xf_id = append_element_to_tag(&mut styles_xml, "cellXfs", &new_xf_xml)?;
                    style_cache.insert(cache_key, xf_id);
                    xf_id
                };

                sheet_xml = insert_new_cell(
                    &sheet_xml,
                    &cell_ref_str,
                    "",
                    "",
                    None,
                    &format!("s=\"{}\"", new_style_index),
                    &sp,
                )?;
            }
        }

        package
            .write_part_xml(&part_path, &sheet_xml)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    }

    // Write styles.xml back
    package
        .write_part_xml("xl/styles.xml", &styles_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    Ok(Vec::new())
}

fn detect_stylesheet_namespace_prefix(xml: &str) -> String {
    if let Some(pos) = xml.find("styleSheet") {
        if let Some(lt_pos) = xml[..pos].rfind('<') {
            let prefix = &xml[lt_pos + 1..pos];
            if !prefix.is_empty() && prefix.ends_with(':') {
                return prefix.to_string();
            }
        }
    }
    "".to_string()
}

fn format_excel_color(color_str: &str) -> String {
    let clean = color_str.trim_start_matches('#');
    let hex_lower = clean.to_lowercase();
    let resolved_hex = match hex_lower.as_str() {
        "yellow" => "FFFF00",
        "green" => "00FF00",
        "blue" => "0000FF",
        "magenta" => "FF00FF",
        "cyan" => "00FFFF",
        "red" => "FF0000",
        "white" => "FFFFFF",
        "black" => "000000",
        other => other,
    };
    if resolved_hex.len() == 6 {
        format!("FF{}", resolved_hex.to_uppercase())
    } else if resolved_hex.len() == 8 {
        resolved_hex.to_uppercase()
    } else {
        "FF000000".to_string()
    }
}

fn clone_xf_with_changes(
    xf_xml: &str,
    font_id: usize,
    fill_id: usize,
    apply_font: bool,
    apply_fill: bool,
    p: &str,
) -> Result<String, HandlerError> {
    let wrapped = format!(
        "<x:dummy xmlns:x=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">{}</x:dummy>",
        xf_xml
    );
    let doc = roxmltree::Document::parse(&wrapped)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to parse single xf: {}", e)))?;
    let xf_node = doc.root_element().first_element_child().ok_or_else(|| {
        HandlerError::OperationFailed("xf element not found inside dummy root".to_string())
    })?;

    let mut attrs = Vec::new();
    for attr in xf_node.attributes() {
        let name = attr.name();
        if name != "fontId" && name != "fillId" && name != "applyFont" && name != "applyFill" {
            attrs.push(format!("{}=\"{}\"", attr.name(), attr.value()));
        }
    }

    attrs.push(format!("fontId=\"{}\"", font_id));
    attrs.push(format!("fillId=\"{}\"", fill_id));
    if apply_font {
        attrs.push("applyFont=\"1\"".to_string());
    } else if let Some(val) = xf_node.attribute("applyFont") {
        attrs.push(format!("applyFont=\"{}\"", val));
    }
    if apply_fill {
        attrs.push("applyFill=\"1\"".to_string());
    } else if let Some(val) = xf_node.attribute("applyFill") {
        attrs.push(format!("applyFill=\"{}\"", val));
    }

    let mut children_xml = String::new();
    for child in xf_node.children().filter(|n| n.is_element()) {
        children_xml.push_str(&wrapped[child.range()]);
    }

    if children_xml.is_empty() {
        Ok(format!("<{}xf {}/>", p, attrs.join(" ")))
    } else {
        Ok(format!(
            "<{}xf {}>{}</{}xf>",
            p,
            attrs.join(" "),
            children_xml,
            p
        ))
    }
}

fn append_element_to_tag(
    xml: &mut String,
    tag_name: &str,
    new_element_xml: &str,
) -> Result<usize, HandlerError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to parse XML: {}", e)))?;
    let node = doc
        .descendants()
        .find(|n| n.has_tag_name(tag_name))
        .ok_or_else(|| HandlerError::OperationFailed(format!("tag <{}> not found", tag_name)))?;

    let node_start = node.range().start;

    let open_tag_end = xml[node_start..]
        .find('>')
        .map(|pos| node_start + pos)
        .ok_or_else(|| HandlerError::OperationFailed("malformed tag: no '>'".to_string()))?;

    let open_tag_text = &xml[node_start..=open_tag_end];

    let mut current_count = 0;
    let mut count_attr_range = None;

    if let Some(pos) = open_tag_text.find("count=\"") {
        let val_start = node_start + pos + "count=\"".len();
        if let Some(val_len) = xml[val_start..].find('"') {
            let val_end = val_start + val_len;
            if let Ok(c) = xml[val_start..val_end].parse::<usize>() {
                current_count = c;
                count_attr_range = Some(val_start..val_end);
            }
        }
    }

    let last_child = node.children().filter(|n| n.is_element()).next_back();

    let new_count = current_count + 1;

    let mut result = String::new();

    let full_name = xml[node_start + 1..]
        .split([' ', '>', '/', '\n', '\r', '\t'])
        .next()
        .unwrap_or("");
    let prefix = if let Some(colon_pos) = full_name.find(':') {
        full_name[..colon_pos + 1].to_string()
    } else {
        "".to_string()
    };

    if let Some(r) = count_attr_range {
        result.push_str(&xml[..r.start]);
        result.push_str(&new_count.to_string());

        if let Some(lc) = last_child {
            let lc_end = lc.range().end;
            result.push_str(&xml[r.end..lc_end]);
            result.push_str(new_element_xml);
            result.push_str(&xml[lc_end..]);
        } else if open_tag_text.trim_end().ends_with("/>") {
            let tag_open_without_slash = open_tag_text.replace("/>", ">");
            result.push_str(&xml[r.end..node_start]);
            result.push_str(&tag_open_without_slash);
            result.push_str(new_element_xml);
            result.push_str(&format!("</{}{}>", prefix, node.tag_name().name()));
            result.push_str(&xml[open_tag_end + 1..]);
        } else {
            result.push_str(&xml[r.end..open_tag_end + 1]);
            result.push_str(new_element_xml);
            result.push_str(&xml[open_tag_end + 1..]);
        }
    } else if let Some(lc) = last_child {
        let lc_end = lc.range().end;
        result.push_str(&xml[..lc_end]);
        result.push_str(new_element_xml);
        result.push_str(&xml[lc_end..]);
    } else if open_tag_text.trim_end().ends_with("/>") {
        let tag_open_without_slash = open_tag_text.replace("/>", ">");
        result.push_str(&xml[..node_start]);
        result.push_str(&tag_open_without_slash);
        result.push_str(new_element_xml);
        result.push_str(&format!("</{}{}>", prefix, node.tag_name().name()));
        result.push_str(&xml[open_tag_end + 1..]);
    } else {
        result.push_str(&xml[..open_tag_end + 1]);
        result.push_str(new_element_xml);
        result.push_str(&xml[open_tag_end + 1..]);
    }

    *xml = result;
    Ok(current_count)
}

// ─── Find & Replace ──────────────────────────────────────────────────

/// Apply find/replace to xlsx shared strings and worksheet inline strings.
///
/// Scope rules:
///   - Path "/" or "" → all shared strings + all worksheets
///   - Path "/SheetName" → only that sheet's inline strings
///   - Path "/SheetName/A1" → just that cell's <is><t>...</t></is> inline string
pub fn apply_xlsx_find_replace(
    package: &mut OxmlPackage,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let (find, replace, opts) = extract_find_replace_props(properties).ok_or_else(|| {
        HandlerError::InvalidArgument(
            "find/replace requires at least a 'find=<text>' property".to_string(),
        )
    })?;

    let mut total = 0usize;

    // 1. Shared strings table — always scan unless scoped to a specific cell.
    let pc = navigation::parse_path(path).ok();
    let scoped_to_cell = pc.as_ref().and_then(|p| p.cell_ref.clone()).is_some();

    if !scoped_to_cell {
        if let Some(ss_xml) = read_part_xml_optional(package, "xl/sharedStrings.xml")? {
            let xml = ss_xml;
            let (new_xml, n) = replace_in_xml_text_nodes(&xml, &find, &replace, &opts, "t", "</t>");
            if n > 0 {
                total += n;
                package
                    .write_part_xml("xl/sharedStrings.xml", &new_xml)
                    .map_err(|e| HandlerError::SaveError(e.to_string()))?;
            }
        }
    }

    // 2. Worksheet inline (only if path is root, /SheetName, or /SheetName/cell)
    let sheet_filter = pc.as_ref().and_then(|p| p.sheet_name.as_deref());

    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;
    for ws in &model.sheets {
        if let Some(s) = sheet_filter {
            if ws.name != s {
                continue;
            }
        }
        let part = match ws.part_path.strip_prefix('/') {
            Some(p) => p.to_string(),
            None => ws.part_path.clone(),
        };
        let Some(xml) = read_part_xml_optional(package, &part)? else {
            continue;
        };
        // Worksheet text lives in two shapes: shared-string references become
        // `<c t="s"><v>idx</v></c>` (numeric index, not actual text — skip), while
        // inline/typed strings become `<c t="str"><v>text</v></c>` or
        // `<c t="inlineStr"><is><t>text</t></is></c>`. We scan both `<v>` and `<t>`
        // here; for shared-string-index `<v>` nodes the value is a number and
        // won't match user-supplied find text, so it's safe.
        let mut xml = xml;
        let (next, n_t) = replace_in_xml_text_nodes(&xml, &find, &replace, &opts, "t", "</t>");
        xml = next;
        let (newer, n_v) = replace_in_xml_text_nodes(&xml, &find, &replace, &opts, "v", "</v>");
        let n = n_t + n_v;
        if n > 0 {
            xml = newer;
            total += n;
            package
                .write_part_xml(&part, &xml)
                .map_err(|e| HandlerError::SaveError(e.to_string()))?;
        }
    }

    Ok(vec![format!("replaced={}", total)])
}

/// Read a part as XML if it exists, returning None if not present.
fn read_part_xml_optional(
    package: &OxmlPackage,
    part_path: &str,
) -> Result<Option<String>, HandlerError> {
    match package.read_part_xml(part_path) {
        Ok(xml) => Ok(Some(xml)),
        Err(_) => Ok(None),
    }
}

/// Find every `<open_prefix...>...</close_tag>` block in `xml` and run
/// replace_in_string on its inner text. Returns (new_xml, count).
/// `open_prefix` is the leading tag name without `<`, e.g. "t" matches `<t>`,
/// `<t ...>`, `<t/>`. `close_tag` includes the angle brackets, e.g. "</t>".
fn replace_in_xml_text_nodes(
    xml: &str,
    find: &str,
    replace: &str,
    opts: &FindReplaceOptions,
    open_prefix: &str,
    close_tag: &str,
) -> (String, usize) {
    let needle = format!("<{}", open_prefix);
    let mut out = String::with_capacity(xml.len());
    let mut cursor = 0;
    let mut total = 0usize;

    while let Some(close_start) = xml[cursor..].find(close_tag) {
        let close_abs = cursor + close_start;
        // Walk forward through every `<tag` occurrence in the prefix and keep
        // the last one (the innermost open for this close tag).
        let prefix = &xml[..close_abs];
        let mut open_idx = None;
        let mut search_from = 0;
        while let Some(o) = prefix[search_from..].find(&needle) {
            let abs = search_from + o;
            // Must be followed by `>`, ` `, `/`, or attribute whitespace
            let after = &prefix[abs + needle.len()..];
            let c = after.as_bytes().first().copied();
            match c {
                Some(b'>') | Some(b' ') | Some(b'/') | Some(b'\t') | Some(b'\n') => {
                    open_idx = Some(abs);
                }
                _ => {}
            }
            search_from = abs + needle.len();
        }

        let Some(open_abs) = open_idx else {
            out.push_str(&xml[cursor..close_abs + close_tag.len()]);
            cursor = close_abs + close_tag.len();
            continue;
        };

        // Find the close of the opening tag (`>`)
        let Some(gt_rel) = xml[open_abs..close_abs].find('>') else {
            out.push_str(&xml[cursor..close_abs + close_tag.len()]);
            cursor = close_abs + close_tag.len();
            continue;
        };
        let open_close = open_abs + gt_rel + 1;
        let inner = &xml[open_close..close_abs];
        let (new_inner, n) = replace_in_string(inner, find, replace, opts);
        total += n;

        out.push_str(&xml[cursor..open_close]);
        out.push_str(&new_inner);
        cursor = close_abs;
        out.push_str(&xml[cursor..cursor + close_tag.len()]);
        cursor += close_tag.len();
    }
    out.push_str(&xml[cursor..]);
    (out, total)
}

// Re-export the find/replace property key list so the handler surface
// matches the C# command registration.
pub use handler_common::find_replace_property_keys;

#[cfg(test)]
mod sheet_order_tests {
    use super::*;

    const WORKBOOK_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="A" sheetId="1" r:id="rId1"/>
    <sheet name="B" sheetId="3" r:id="rId4"/>
    <sheet name="C" sheetId="9" r:id="rId7"/>
  </sheets>
  <definedNames>
    <definedName name="scopeA" localSheetId="0">A!$A$1</definedName>
    <definedName name="scopeB" localSheetId="1">B!$A$1</definedName>
    <definedName name="scopeC" localSheetId="2">C!$A$1</definedName>
    <definedName name="global">A!$B$1</definedName>
  </definedNames>
</workbook>"#;

    const WORKBOOK_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId4" Type="worksheet" Target="worksheets/sheet2.xml"/>
  <Relationship Id="rId7" Type="worksheet" Target="worksheets/sheet5.xml"/>
</Relationships>"#;

    const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Override PartName="/xl/workbook.xml" ContentType="workbook"/>
  <Override PartName="/xl/worksheets/sheet1.xml" ContentType="worksheet"/>
  <Override PartName="/xl/worksheets/sheet2.xml" ContentType="worksheet"/>
  <Override PartName="/xl/worksheets/sheet5.xml" ContentType="worksheet"/>
</Types>"#;

    fn package_fixture() -> OxmlPackage {
        let mut package = OxmlPackage::create("unused.xlsx");
        package.add_part("xl/workbook.xml", WORKBOOK_XML.as_bytes());
        package.add_part("xl/_rels/workbook.xml.rels", WORKBOOK_RELS.as_bytes());
        package.add_part("[Content_Types].xml", CONTENT_TYPES.as_bytes());
        for part in ["sheet1.xml", "sheet2.xml", "sheet5.xml"] {
            package.add_part(
                &format!("xl/worksheets/{}", part),
                br#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData/></worksheet>"#,
            );
        }
        package.add_part("xl/worksheets/_rels/sheet2.xml.rels", b"<Relationships/>");
        package
    }

    fn sheet_names(xml: &str) -> Vec<String> {
        workbook_sheet_entries(xml)
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect()
    }

    fn scope(xml: &str, name: &str) -> Option<u32> {
        let doc = roxmltree::Document::parse(xml).unwrap();
        doc.descendants()
            .find(|node| {
                node.is_element()
                    && node.tag_name().name() == "definedName"
                    && node.attribute("name") == Some(name)
            })
            .and_then(|node| node.attribute("localSheetId"))
            .and_then(|value| value.parse().ok())
    }

    #[test]
    fn insert_sheet_shifts_scopes_and_allocates_unique_package_ids() {
        let mut package = package_fixture();
        let properties = HashMap::from([("name".to_string(), "Inserted".to_string())]);

        let path = crate::add::add_element(
            &mut package,
            "/",
            "sheet",
            InsertPosition::AtIndex(1),
            &properties,
        )
        .unwrap();

        assert_eq!(path, "/Inserted");
        let workbook = package.read_part_xml("xl/workbook.xml").unwrap();
        assert_eq!(sheet_names(&workbook), ["A", "Inserted", "B", "C"]);
        assert_eq!(scope(&workbook, "scopeA"), Some(0));
        assert_eq!(scope(&workbook, "scopeB"), Some(2));
        assert_eq!(scope(&workbook, "scopeC"), Some(3));
        assert!(workbook.contains(r#"sheetId="10""#));
        assert!(workbook.contains(r#"r:id="rId8""#));
        assert!(package.has_part("xl/worksheets/sheet6.xml"));
        assert!(package
            .read_part_xml("[Content_Types].xml")
            .unwrap()
            .contains("/xl/worksheets/sheet6.xml"));
    }

    #[test]
    fn move_sheet_remaps_scopes_by_sheet_identity() {
        let mut package = package_fixture();

        move_sheet(
            &mut package,
            "/A",
            None,
            InsertPosition::AfterElement("/C".to_string()),
        )
        .unwrap();

        let workbook = package.read_part_xml("xl/workbook.xml").unwrap();
        assert_eq!(sheet_names(&workbook), ["B", "C", "A"]);
        assert_eq!(scope(&workbook, "scopeA"), Some(2));
        assert_eq!(scope(&workbook, "scopeB"), Some(0));
        assert_eq!(scope(&workbook, "scopeC"), Some(1));
        assert_eq!(scope(&workbook, "global"), None);
    }

    #[test]
    fn remove_sheet_drops_own_scopes_and_cleans_package_references() {
        let mut package = package_fixture();

        remove_sheet(&mut package, "B").unwrap();

        let workbook = package.read_part_xml("xl/workbook.xml").unwrap();
        assert_eq!(sheet_names(&workbook), ["A", "C"]);
        assert_eq!(scope(&workbook, "scopeA"), Some(0));
        assert_eq!(scope(&workbook, "scopeB"), None);
        assert_eq!(scope(&workbook, "scopeC"), Some(1));
        assert!(!package.has_part("xl/worksheets/sheet2.xml"));
        assert!(!package.has_part("xl/worksheets/_rels/sheet2.xml.rels"));
        assert!(!package
            .read_part_xml("xl/_rels/workbook.xml.rels")
            .unwrap()
            .contains("rId4"));
        assert!(!package
            .read_part_xml("[Content_Types].xml")
            .unwrap()
            .contains("/xl/worksheets/sheet2.xml"));
    }
}
