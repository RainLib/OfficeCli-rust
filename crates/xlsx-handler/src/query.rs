/// Query operations for xlsx documents.
use crate::dom_types::*;
use crate::helpers;
use crate::rich_value_image;
use handler_common::DocumentNode;
use handler_common::HandlerError;
use oxml::OxmlPackage;
use std::collections::{HashMap, HashSet};

/// Query cells matching a selector pattern.
/// Supported selectors:
///   "sheet=SheetName" — all cells in a sheet
///   "formula" — all cells with formulas
///   "type=sharedString" — all cells of a specific type
///   "range=A1:C10" — cells in a range on the first sheet
///   "Sheet1!A1:C10" — cells in a range on a specific sheet
///   "pivot" — all pivot tables (read-only summary)
///   "table" / "tables" — real ListObjects plus high-confidence detected blocks
///   "listobject" — only real ListObjects (Excel Tables)
///   "Sheet!row[Col op Val]" / "row[Col op Val]" — table rows where every
///     column predicate holds. `op` ∈ {=, !=, >, >=, <, <=, contains,
///     startswith, endswith}. Column key may use header name (preferred) or
///     column letter; a `col.`/`column.` prefix forces column interpretation.
pub fn query_cells(
    package: &OxmlPackage,
    selector: &str,
) -> Result<Vec<DocumentNode>, HandlerError> {
    let model = helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    let mut results = Vec::new();

    // row[...] predicate — must be checked first because the leading part
    // may be a sheet name containing `!`.
    if selector.contains("row[") {
        return query_rows_by_predicate(&model, selector);
    }

    // Parse the selector
    if let Some(sheet_name) = selector.strip_prefix("sheet=") {
        // Sheet selector
        let ws = model
            .sheets
            .iter()
            .find(|s| s.name == sheet_name)
            .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", sheet_name)))?;

        for cell in ws.cells.values() {
            results.push(make_cell_node(package, ws, cell));
        }
    } else if selector == "formula" {
        // All formula cells
        for ws in &model.sheets {
            for cell in ws.cells.values() {
                if cell.formula.is_some() {
                    results.push(make_cell_node(package, ws, cell));
                }
            }
        }
    } else if selector == "pivot" {
        // All pivot tables
        for pt in &model.pivot_tables {
            results.push(make_pivot_node(pt));
        }
    } else if let Some((sheet_filter, include_detected)) = parse_table_selector(selector) {
        for tbl in &model.tables {
            if sheet_filter.is_none_or(|sheet| tbl.sheet_name.eq_ignore_ascii_case(sheet)) {
                results.push(make_table_node(tbl));
            }
        }
        if include_detected {
            for table in detect_tables(&model) {
                if sheet_filter.is_none_or(|sheet| table.sheet_name.eq_ignore_ascii_case(sheet)) {
                    results.push(make_detected_table_node(&table));
                }
            }
        }
    } else if let Some(type_name) = selector.strip_prefix("type=") {
        // Type selector
        if type_name.eq_ignore_ascii_case("image") {
            for ws in &model.sheets {
                for cell in ws.cells.values() {
                    if cell
                        .value_metadata_index
                        .and_then(|vm| rich_value_image::read_image_info(package, vm))
                        .is_some()
                    {
                        results.push(make_cell_node(package, ws, cell));
                    }
                }
            }
            return Ok(results);
        }
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
                    results.push(make_cell_node(package, ws, cell));
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
                    results.push(make_cell_node(package, ws, cell));
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

fn parse_table_selector(selector: &str) -> Option<(Option<&str>, bool)> {
    let (sheet, element) = selector
        .rsplit_once('!')
        .map_or((None, selector), |(sheet, element)| (Some(sheet), element));
    match element.to_ascii_lowercase().as_str() {
        "table" | "tables" => Some((sheet, true)),
        "listobject" | "listobjects" => Some((sheet, false)),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct DetectedTableDef {
    sheet_name: String,
    range: (usize, usize, usize, usize),
    columns: Vec<String>,
}

/// Detect high-confidence table-shaped blocks that are not real ListObjects.
/// Detection works from the sparse cell map, so declared but mostly-empty sheet
/// dimensions do not affect its cost.
fn detect_tables(model: &WorkbookModel) -> Vec<DetectedTableDef> {
    let mut results = Vec::new();

    for sheet in &model.sheets {
        let occupied: HashMap<(usize, usize), (&Cell, bool)> = sheet
            .cells
            .values()
            .filter(|cell| !cell.display_value.is_empty())
            .map(|cell| {
                let is_text = cell.formula.is_none() && cell.display_value.parse::<f64>().is_err();
                ((cell.col, cell.row), (cell, is_text))
            })
            .collect();
        if occupied.is_empty() {
            continue;
        }

        let mut anchors: Vec<(usize, usize)> = occupied
            .keys()
            .copied()
            .filter(|(col, row)| {
                !row.checked_sub(1)
                    .is_some_and(|above| occupied.contains_key(&(*col, above)))
                    && !col
                        .checked_sub(1)
                        .is_some_and(|left| occupied.contains_key(&(left, *row)))
            })
            .collect();
        anchors.sort_unstable_by_key(|(col, row)| (*row, *col));

        let real_ranges: Vec<(usize, usize, usize, usize)> = model
            .tables
            .iter()
            .filter(|table| table.sheet_name.eq_ignore_ascii_case(&sheet.name))
            .map(|table| table.range)
            .collect();
        let mut claimed = HashSet::new();

        for (anchor_col, anchor_row) in anchors {
            if claimed.contains(&(anchor_col, anchor_row)) {
                continue;
            }
            let Some((_, anchor_is_text)) = occupied.get(&(anchor_col, anchor_row)) else {
                continue;
            };
            if !anchor_is_text {
                continue;
            }

            let mut column = anchor_col;
            while occupied.contains_key(&(column, anchor_row)) {
                column += 1;
            }
            let end_col = column - 1;
            if end_col - anchor_col + 1 < 2 {
                continue;
            }

            let mut end_row = anchor_row;
            let mut row = anchor_row + 1;
            while (anchor_col..=end_col).any(|col| occupied.contains_key(&(col, row))) {
                end_row = row;
                row += 1;
            }
            if end_row == anchor_row {
                continue;
            }

            let block = (anchor_row, anchor_col, end_row, end_col);
            if real_ranges
                .iter()
                .any(|real_range| ranges_overlap(*real_range, block))
            {
                continue;
            }

            for claimed_row in anchor_row..=end_row {
                for claimed_col in anchor_col..=end_col {
                    claimed.insert((claimed_col, claimed_row));
                }
            }
            let columns = (anchor_col..=end_col)
                .map(|col| {
                    occupied
                        .get(&(col, anchor_row))
                        .map(|(cell, _)| cell.display_value.clone())
                        .unwrap_or_default()
                })
                .collect();
            results.push(DetectedTableDef {
                sheet_name: sheet.name.clone(),
                range: block,
                columns,
            });
        }
    }

    results
}

fn ranges_overlap(left: (usize, usize, usize, usize), right: (usize, usize, usize, usize)) -> bool {
    let (left_r1, left_c1, left_r2, left_c2) = left;
    let (right_r1, right_c1, right_r2, right_c2) = right;
    left_r1 <= right_r2 && right_r1 <= left_r2 && left_c1 <= right_c2 && right_c1 <= left_c2
}

/// Parse `[SheetName!]row[col op val and col2 op val2 ...]` and return the
/// matching data rows. AND-only; OR/parens are out of scope for v1.
fn query_rows_by_predicate(
    model: &WorkbookModel,
    selector: &str,
) -> Result<Vec<DocumentNode>, HandlerError> {
    let (sheet_filter, predicate_str) = split_row_predicate(selector).ok_or_else(|| {
        HandlerError::InvalidArgument(format!("malformed row predicate: {}", selector))
    })?;

    let predicates = parse_predicate_list(predicate_str)
        .map_err(|e| HandlerError::InvalidArgument(format!("row predicate: {}", e)))?;
    if predicates.is_empty() {
        return Err(HandlerError::InvalidArgument(
            "row predicate has no conditions".into(),
        ));
    }

    // Real ListObjects are authoritative. Only consult detected blocks when no
    // real table in scope owns every referenced column.
    let mut candidates = Vec::new();
    for tbl in &model.tables {
        if let Some(sheet_filter) = sheet_filter {
            if !tbl.sheet_name.eq_ignore_ascii_case(sheet_filter) {
                continue;
            }
        }
        if predicates
            .iter()
            .all(|p| resolve_column_index(tbl, &p.key).is_some())
        {
            candidates.push(RowTableCandidate {
                sheet_name: tbl.sheet_name.clone(),
                label: tbl.name.clone(),
                source: "table",
                range: tbl.range,
                columns: tbl.columns.clone(),
                data_r1: tbl.range.0 + usize::from(tbl.header_row),
                data_r2: tbl.range.2 - usize::from(tbl.totals_row),
            });
        }
    }

    if candidates.is_empty() {
        for table in detect_tables(model) {
            if sheet_filter.is_some_and(|sheet| !table.sheet_name.eq_ignore_ascii_case(sheet)) {
                continue;
            }
            if predicates.iter().all(|predicate| {
                resolve_column_in_range(&table.columns, table.range, &predicate.key).is_some()
            }) {
                let label = detected_range_ref(&table);
                candidates.push(RowTableCandidate {
                    sheet_name: table.sheet_name,
                    label,
                    source: "detected",
                    range: table.range,
                    columns: table.columns,
                    data_r1: table.range.0 + 1,
                    data_r2: table.range.2,
                });
            }
        }
    }

    if candidates.is_empty() {
        let cols: Vec<String> = predicates.iter().map(|p| format!("'{}'", p.key)).collect();
        let scope = sheet_filter.unwrap_or("any sheet");
        return Err(HandlerError::InvalidArgument(format!(
            "row predicate found no Excel Table or detected table-shaped block on {} with column(s) {}. \
             Column predicates resolve header names (or column letters) against a table.",
            scope,
            cols.join(", ")
        )));
    }
    if candidates.len() > 1 {
        let names: Vec<String> = candidates
            .iter()
            .map(|table| format!("{}!{}", table.sheet_name, table.label))
            .collect();
        return Err(HandlerError::InvalidArgument(format!(
            "row predicate is ambiguous — column(s) exist in {} tables ({}). \
             Scope by sheet, e.g. SheetName!row[...].",
            candidates.len(),
            names.join(", ")
        )));
    }

    let table = &candidates[0];
    let ws = model
        .sheets
        .iter()
        .find(|sheet| sheet.name.eq_ignore_ascii_case(&table.sheet_name))
        .ok_or_else(|| HandlerError::PathNotFound(format!("sheet '{}'", table.sheet_name)))?;

    let mut results = Vec::new();
    for row in table.data_r1..=table.data_r2 {
        let mut all_match = true;
        let mut probe_values: Vec<(String, String)> = Vec::new();
        for predicate in &predicates {
            let abs_col = resolve_column_in_range(&table.columns, table.range, &predicate.key)
                .expect("candidate was prefiltered by predicate columns");
            let cell = ws.cells.get(&(row, abs_col));
            let value = cell.map(|cell| cell.display_value.as_str()).unwrap_or("");
            if !eval_predicate(value, predicate) {
                all_match = false;
                break;
            }
            probe_values.push((predicate.key.clone(), value.to_string()));
        }
        if !all_match {
            continue;
        }

        let mut node = DocumentNode::new(&format!("/{}/row[{}]", table.sheet_name, row), "row")
            .with_preview(row.to_string());
        for (k, v) in probe_values {
            node = node.with_format(&k, serde_json::Value::String(v));
        }
        node = node.with_format(
            "matchedTable",
            serde_json::Value::String(table.label.clone()),
        );
        node = node.with_format(
            "tableSource",
            serde_json::Value::String(table.source.to_string()),
        );
        results.push(node);
    }

    Ok(results)
}

struct RowTableCandidate {
    sheet_name: String,
    label: String,
    source: &'static str,
    range: (usize, usize, usize, usize),
    columns: Vec<String>,
    data_r1: usize,
    data_r2: usize,
}

/// Split `[SheetName!]row[...]` into `(Option<SheetName>, "[...]")`.
fn split_row_predicate(selector: &str) -> Option<(Option<&str>, &str)> {
    let bang_idx = selector.find('!');
    let (sheet, rest) = match bang_idx {
        Some(i) => (Some(&selector[..i]), &selector[i + 1..]),
        None => (None, selector),
    };
    let pred_start = rest.find("row[")? + "row[".len();
    let pred_end = rest.rfind(']')?;
    if pred_end < pred_start {
        return None;
    }
    Some((sheet, &rest[pred_start..pred_end]))
}

/// A single leaf predicate: `col_key op value`.
struct Predicate {
    key: String,
    op: PredicateOp,
    value: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PredicateOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Contains,
    StartsWith,
    EndsWith,
}

/// Parse a list of predicates joined by `and`/`AND`/`&&`. Returns Err on
/// malformed input.
fn parse_predicate_list(s: &str) -> Result<Vec<Predicate>, String> {
    s.split("and")
        .flat_map(|chunk| {
            chunk
                .split("AND")
                .flat_map(|c| c.split("&&").collect::<Vec<_>>())
        })
        .map(parse_single_predicate)
        .collect()
}

fn parse_single_predicate(s: &str) -> Result<Predicate, String> {
    let s = s.trim();
    // Try longest-op-first to disambiguate `>=` from `>`.
    for (op_str, op) in [
        ("!=", PredicateOp::Ne),
        (">=", PredicateOp::Ge),
        ("<=", PredicateOp::Le),
        ("=", PredicateOp::Eq),
        (">", PredicateOp::Gt),
        ("<", PredicateOp::Lt),
    ] {
        if let Some(idx) = s.find(op_str) {
            let key = s[..idx].trim().to_string();
            let value = s[idx + op_str.len()..].trim().to_string();
            if key.is_empty() {
                return Err(format!("missing column name in '{}'", s));
            }
            return Ok(Predicate {
                key,
                op,
                value: unquote(&value),
            });
        }
    }
    // Word ops: contains X / contains(X) / contains "X"
    let lower = s.to_ascii_lowercase();
    for (op_word, op) in [
        ("contains", PredicateOp::Contains),
        ("startswith", PredicateOp::StartsWith),
        ("endswith", PredicateOp::EndsWith),
    ] {
        if let Some(rest) = lower.strip_prefix(&format!("{} ", op_word)) {
            // rest borrows the original `s` via lower → safe to slice back.
            let value = &s[rest.len() + op_word.len() + 1..];
            return Ok(Predicate {
                key: String::new(),
                op,
                value: unquote(value),
            });
        }
        if let Some(rest) = lower.strip_prefix(&format!("{}(", op_word)) {
            let value = &s[rest.len() + op_word.len() + 1..];
            let value = value.trim_end_matches(')').trim();
            return Ok(Predicate {
                key: String::new(),
                op,
                value: unquote(value),
            });
        }
    }
    Err(format!(
        "could not parse '{}' — expected `col op val` (ops: =, !=, >, >=, <, <=, contains, startswith, endswith)",
        s
    ))
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Resolve a predicate key (column name or letter, with optional `col.` /
/// `column.` prefix) to an absolute column index inside `tbl.range`.
fn resolve_column_index(tbl: &ListObjectDef, key: &str) -> Option<usize> {
    resolve_column_in_range(&tbl.columns, tbl.range, key)
}

fn resolve_column_in_range(
    columns: &[String],
    range: (usize, usize, usize, usize),
    key: &str,
) -> Option<usize> {
    let bare = strip_col_prefix(key);
    let (_, c1, _, c2) = range;
    // Header name wins over a column letter (case-insensitive).
    let name_idx = columns.iter().position(|c| c.eq_ignore_ascii_case(bare));
    if let Some(i) = name_idx {
        return Some(c1 + i);
    }
    // Try column-letter form (A–ZZ).
    if bare.chars().all(|c| c.is_ascii_alphabetic()) && bare.len() <= 3 {
        let upper = bare.to_ascii_uppercase();
        if let Some(n) = col_letters_to_num_pub(&upper) {
            if n >= c1 && n <= c2 {
                return Some(n);
            }
        }
    }
    None
}

fn strip_col_prefix(key: &str) -> &str {
    let lower = key.to_ascii_lowercase();
    if lower.starts_with("column.") {
        &key["column.".len()..]
    } else if lower.starts_with("col.") {
        &key["col.".len()..]
    } else {
        key
    }
}

/// Public re-export of the private `col_letters_to_num` to avoid a duplicate
/// implementation. Returns the 1-based column index for `A`..`ZZZ`.
fn col_letters_to_num_pub(letters: &str) -> Option<usize> {
    let mut n: usize = 0;
    for ch in letters.chars() {
        if !ch.is_ascii_uppercase() {
            return None;
        }
        n = n * 26 + (ch as usize - 'A' as usize + 1);
    }
    if n == 0 {
        return None;
    }
    Some(n)
}

fn eval_predicate(cell_value: &str, p: &Predicate) -> bool {
    match p.op {
        PredicateOp::Eq => cell_value == p.value,
        PredicateOp::Ne => cell_value != p.value,
        PredicateOp::Gt | PredicateOp::Ge | PredicateOp::Lt | PredicateOp::Le => {
            // Numeric-aware compare when both sides parse as f64, else lexicographic.
            let lhs: f64 = match cell_value.parse() {
                Ok(v) => v,
                Err(_) => return lexical_compare(cell_value, &p.value, p.op),
            };
            let rhs: f64 = match p.value.parse() {
                Ok(v) => v,
                Err(_) => return lexical_compare(cell_value, &p.value, p.op),
            };
            match p.op {
                PredicateOp::Gt => lhs > rhs,
                PredicateOp::Ge => lhs >= rhs,
                PredicateOp::Lt => lhs < rhs,
                PredicateOp::Le => lhs <= rhs,
                _ => unreachable!(),
            }
        }
        PredicateOp::Contains => cell_value.contains(&p.value),
        PredicateOp::StartsWith => cell_value.starts_with(&p.value),
        PredicateOp::EndsWith => cell_value.ends_with(&p.value),
    }
}

fn lexical_compare(lhs: &str, rhs: &str, op: PredicateOp) -> bool {
    match op {
        PredicateOp::Gt => lhs > rhs,
        PredicateOp::Ge => lhs >= rhs,
        PredicateOp::Lt => lhs < rhs,
        PredicateOp::Le => lhs <= rhs,
        _ => unreachable!(),
    }
}

fn detected_range_ref(table: &DetectedTableDef) -> String {
    let (r1, c1, r2, c2) = table.range;
    format!(
        "{}{}:{}{}",
        col_num_to_letters(c1),
        r1,
        col_num_to_letters(c2),
        r2
    )
}

fn make_detected_table_node(table: &DetectedTableDef) -> DocumentNode {
    let (r1, c1, r2, c2) = table.range;
    let range_ref = detected_range_ref(table);
    let data_range = format!(
        "{}{}:{}{}",
        col_num_to_letters(c1),
        r1 + 1,
        col_num_to_letters(c2),
        r2
    );
    let mut node = DocumentNode::new(
        &format!("/{}/{}", table.sheet_name, range_ref),
        "detectedtable",
    )
    .with_text(table.columns.first().cloned().unwrap_or_default())
    .with_preview(range_ref.clone())
    .with_format("source", serde_json::json!("header-sniff"))
    .with_format("stable", serde_json::json!(false))
    .with_format("ref", serde_json::json!(range_ref))
    .with_format("columns", serde_json::json!(table.columns.join(",")))
    .with_format("dataRange", serde_json::json!(data_range));
    node.child_count = table.columns.len();
    node
}

fn make_table_node(tbl: &ListObjectDef) -> DocumentNode {
    let (r1, c1, r2, c2) = tbl.range;
    let path = format!("/{}/table[{}]", tbl.sheet_name, tbl.name);
    let col_count = tbl.columns.len();
    let preview = format!(
        "\"{}\" — {} column(s), range {}{}:{}{}",
        tbl.name,
        col_count,
        col_num_to_letters(c1),
        r1,
        ':',
        col_num_to_letters(c2)
    );
    // Append end-row (kept separate to avoid nested format!).
    let preview = format!("{}{}", preview, r2);
    DocumentNode::new(&path, "table")
        .with_text(tbl.name.clone())
        .with_preview(preview)
}

/// Resolve a pivot-field index to its cacheField name, falling back to the
/// numeric index when the cache can't supply a name.
fn resolve_pivot_field_name(pt: &PivotTableDef, idx: i32) -> String {
    let i = idx as usize;
    if let Some(name) = pt.cache_fields.get(i) {
        if !name.is_empty() {
            return name.clone();
        }
    }
    idx.to_string()
}

fn make_pivot_node(pt: &PivotTableDef) -> DocumentNode {
    let path = format!("/pivot/\"{}\"", pt.name);
    let mut node = DocumentNode::new(&path, "pivot-table").with_text(pt.name.clone());
    node = node.with_format(
        "fieldCount",
        serde_json::Value::Number(pt.field_count.into()),
    );
    if let Some(loc) = &pt.location {
        node = node.with_format("location", serde_json::Value::String(loc.clone()));
    }
    if let Some(src) = &pt.source_range {
        node = node.with_format("source", serde_json::Value::String(src.clone()));
    }
    if let Some(cid) = &pt.cache_id {
        node = node.with_format("cacheId", serde_json::Value::String(cid.clone()));
    }
    if !pt.row_fields.is_empty() {
        let names: Vec<String> = pt
            .row_fields
            .iter()
            .map(|&i| resolve_pivot_field_name(pt, i))
            .collect();
        node = node.with_format("rows", serde_json::Value::String(names.join(",")));
    }
    if !pt.col_fields.is_empty() {
        let names: Vec<String> = pt
            .col_fields
            .iter()
            .map(|&i| resolve_pivot_field_name(pt, i))
            .collect();
        node = node.with_format("cols", serde_json::Value::String(names.join(",")));
    }
    if !pt.page_fields.is_empty() {
        let names: Vec<String> = pt
            .page_fields
            .iter()
            .map(|&i| resolve_pivot_field_name(pt, i))
            .collect();
        node = node.with_format("filters", serde_json::Value::String(names.join(",")));
    }
    node = node.with_format(
        "dataFieldCount",
        serde_json::Value::Number(pt.data_fields.len().into()),
    );
    for (i, (name, func, fld)) in pt.data_fields.iter().enumerate() {
        let field_name = resolve_pivot_field_name(pt, *fld);
        let composite = format!("{}:{}:{}", name, func, field_name);
        node = node.with_format(
            &format!("dataField{}", i + 1),
            serde_json::Value::String(composite),
        );
    }
    let range_info = pt.source_range.as_deref().unwrap_or("unknown");
    node = node.with_preview(format!(
        "\"{}\" — {} fields, source: {}",
        pt.name, pt.field_count, range_info
    ));
    node
}

fn make_cell_node(package: &OxmlPackage, ws: &Worksheet, cell: &Cell) -> DocumentNode {
    let path = format!("/{}/{}", ws.name, cell.ref_str);
    let mut node = DocumentNode::new(&path, "cell").with_text(cell.display_value.clone());

    if let Some(f) = &cell.formula {
        node = node.with_preview(f.clone());
    }

    if let Some(image) = cell
        .value_metadata_index
        .and_then(|vm| rich_value_image::read_image_info(package, vm))
    {
        node = node
            .with_text("[image]")
            .with_format("type", serde_json::Value::String("Image".to_string()))
            .with_format(
                "image.contentType",
                serde_json::Value::String(image.content_type),
            )
            .with_format(
                "image.fileSize",
                serde_json::Value::Number(image.byte_size.into()),
            );
        if let Some(alt) = image.alt {
            node = node.with_format("alt", serde_json::Value::String(alt));
        }
    }

    node
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(row: usize, col: usize, value: &str) -> Cell {
        Cell {
            ref_str: format!("{}{}", col_num_to_letters(col), row),
            col,
            row,
            value_type: if value.parse::<f64>().is_ok() {
                CellValueType::Number
            } else {
                CellValueType::InlineString
            },
            raw_value: Some(value.to_string()),
            formula: None,
            display_value: value.to_string(),
            style_index: None,
            value_metadata_index: None,
        }
    }

    fn model_with_cells(cells: Vec<Cell>) -> WorkbookModel {
        let max_row = cells.iter().map(|cell| cell.row).max().unwrap_or(0);
        let max_col = cells.iter().map(|cell| cell.col).max().unwrap_or(0);
        WorkbookModel {
            sheets: vec![Worksheet {
                name: "Data".to_string(),
                index: 1,
                part_path: "xl/worksheets/sheet1.xml".to_string(),
                rel_id: "rId1".to_string(),
                cells: cells
                    .into_iter()
                    .map(|cell| ((cell.row, cell.col), cell))
                    .collect(),
                max_col,
                max_row,
            }],
            shared_strings: Vec::new(),
            pivot_tables: Vec::new(),
            tables: Vec::new(),
        }
    }

    #[test]
    fn split_handles_sheet_prefix_and_bare() {
        let (s, p) = split_row_predicate("Sheet1!row[A>5]").unwrap();
        assert_eq!(s, Some("Sheet1"));
        assert_eq!(p, "A>5");

        let (s, p) = split_row_predicate("row[Name=Bob]").unwrap();
        assert_eq!(s, None);
        assert_eq!(p, "Name=Bob");
    }

    #[test]
    fn parse_predicate_eq_ne_gt() {
        let p = parse_single_predicate("Age > 30").unwrap();
        assert_eq!(p.key, "Age");
        assert_eq!(p.op, PredicateOp::Gt);
        assert_eq!(p.value, "30");

        let p = parse_single_predicate("Name != Bob").unwrap();
        assert_eq!(p.op, PredicateOp::Ne);

        let p = parse_single_predicate("col.B >= 5").unwrap();
        assert_eq!(p.key, "col.B");
        assert_eq!(p.op, PredicateOp::Ge);
    }

    #[test]
    fn parse_predicate_list_joins_with_and() {
        let list = parse_predicate_list("A>5 and B<10").unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn eval_numeric_and_lexical() {
        let p = Predicate {
            key: "X".into(),
            op: PredicateOp::Gt,
            value: "5".into(),
        };
        assert!(eval_predicate("10", &p));
        assert!(!eval_predicate("3", &p));
        // Non-numeric → lexical compare ("5" sorts before any ASCII letter).
        assert!(eval_predicate("z", &p));
        assert!(eval_predicate("a", &p));
    }

    #[test]
    fn detected_table_requires_header_span_and_data_row() {
        let model = model_with_cells(vec![
            cell(1, 1, "Region"),
            cell(1, 2, "2024"),
            cell(2, 1, "North"),
            cell(2, 2, "10"),
            cell(1, 4, "Single"),
            cell(2, 4, "value"),
        ]);

        let tables = detect_tables(&model);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].range, (1, 1, 2, 2));
        assert_eq!(tables[0].columns, vec!["Region", "2024"]);

        let node = make_detected_table_node(&tables[0]);
        assert_eq!(node.path, "/Data/A1:B2");
        assert_eq!(node.element_type, "detectedtable");
        assert_eq!(node.child_count, 2);
        assert_eq!(
            node.format["source"],
            Some(serde_json::json!("header-sniff"))
        );
        assert_eq!(node.format["stable"], Some(serde_json::json!(false)));
        assert_eq!(node.format["dataRange"], Some(serde_json::json!("A2:B2")));
    }

    #[test]
    fn detected_table_does_not_duplicate_real_listobject() {
        let mut model = model_with_cells(vec![
            cell(1, 1, "Name"),
            cell(1, 2, "Score"),
            cell(2, 1, "Ada"),
            cell(2, 2, "9"),
        ]);
        model.tables.push(ListObjectDef {
            name: "Scores".to_string(),
            display_name: "Scores".to_string(),
            sheet_name: "Data".to_string(),
            part_path: "xl/tables/table1.xml".to_string(),
            range: (1, 1, 2, 2),
            columns: vec!["Name".to_string(), "Score".to_string()],
            header_row: true,
            totals_row: false,
        });

        assert!(detect_tables(&model).is_empty());
    }

    #[test]
    fn row_predicate_falls_back_to_detected_table_and_preserves_comma_header() {
        let model = model_with_cells(vec![
            cell(1, 1, "Name"),
            cell(1, 2, "Amount, USD"),
            cell(2, 1, "Ada"),
            cell(2, 2, "8"),
            cell(3, 1, "Grace"),
            cell(3, 2, "12"),
        ]);

        let rows = query_rows_by_predicate(&model, "row[Amount, USD > 10]").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "/Data/row[3]");
        assert_eq!(
            rows[0].format["matchedTable"],
            Some(serde_json::json!("A1:B3"))
        );
        assert_eq!(
            rows[0].format["tableSource"],
            Some(serde_json::json!("detected"))
        );
    }

    #[test]
    fn table_selector_distinguishes_detected_from_listobject_only() {
        assert_eq!(parse_table_selector("table"), Some((None, true)));
        assert_eq!(parse_table_selector("tables"), Some((None, true)));
        assert_eq!(
            parse_table_selector("Data!listobject"),
            Some((Some("Data"), false))
        );
        assert_eq!(parse_table_selector("pivot"), None);
    }
}
