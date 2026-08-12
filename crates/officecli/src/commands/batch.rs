use clap::Args;
use handler_common::{
    DocumentHandler, HandlerError, InsertPosition, OffsetSpan, OutputFormat, TextOffsetMap,
};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// Execute multiple commands from inline JSON, a file, or stdin.
///
/// `--input` and `--commands` mirror the C# CLI.  The older positional JSON,
/// `--commands-file`, and `--stdin` forms remain supported for existing Rust
/// callers.
#[derive(Args)]
pub struct BatchCommand {
    pub file: String,
    /// JSON string containing an array of operations.
    #[arg(
        value_name = "BATCH_JSON",
        conflicts_with_all = ["commands_file", "commands", "stdin"]
    )]
    pub batch_json: Option<String>,

    /// Read the JSON array of operations from a file. Use `-` for stdin.
    #[arg(
        long = "input",
        visible_alias = "commands-file",
        value_name = "PATH",
        conflicts_with_all = ["batch_json", "commands", "stdin"]
    )]
    pub commands_file: Option<String>,

    /// JSON array of operations supplied inline.
    #[arg(
        long,
        value_name = "JSON",
        conflicts_with_all = ["batch_json", "commands_file", "stdin"]
    )]
    pub commands: Option<String>,

    /// Read the JSON array of operations from stdin.
    #[arg(long, conflicts_with_all = ["batch_json", "commands_file", "commands"])]
    pub stdin: bool,

    /// Emit the refreshed text+offset map after the batch (and per op) in JSON output.
    #[arg(long)]
    pub emit_map: bool,

    /// Persist successful operations even when another operation fails.
    /// Without this flag a batch is atomic: failed batches are not saved.
    #[arg(long)]
    pub best_effort: bool,

    /// Stop after the first failed operation. By default, all operations run
    /// so callers receive every error; atomic batches still roll back on any
    /// failure unless `--best-effort` is supplied.
    #[arg(long)]
    pub stop_on_error: bool,

    /// Compatibility flag for C# callers. Protection bypass is a no-op until
    /// document protection checks are implemented by the Rust handlers.
    #[arg(long)]
    pub force: bool,
}

/// Per-paragraph (or per-element) ledger of applied text-length edits.
///
/// Each entry is `(orig_start, orig_end, delta)` in the document's ORIGINAL
/// coordinate system (the map the caller fetched before the batch). Range
/// offsets of later ops — also expressed in original coordinates — are shifted
/// by the sum of deltas of earlier edits that lie before them, so a batch of
/// range edits stays correct without the caller re-fetching the map mid-batch.
type EditLedger = HashMap<String, Vec<(usize, usize, i64)>>;

/// Original-coordinate ranges `(element_path, start, end)` touched by an op,
/// used to record deltas after a text replacement.
type RangeOriginals = Vec<(String, usize, usize)>;

pub fn handle_batch(cmd: BatchCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let batch_json = read_batch_json(&cmd)?;

    let batch = parse_batch_ops(&batch_json)?;
    let ops = batch.ops;
    let stop_on_error = cmd.stop_on_error || batch.stop_on_error.unwrap_or(false);

    if !cmd.emit_map && !crate::resident_available(&cmd.file) {
        if let Some(output) =
            try_handle_docx_range_set_batch(&cmd.file, &ops, format, cmd.best_effort)?
        {
            return Ok(output);
        }
        if let Some(output) =
            try_handle_docx_bookmark_add_batch(&cmd.file, &ops, format, cmd.best_effort)?
        {
            return Ok(output);
        }
    }

    let handler = crate::open_handler(&cmd.file, true)?;

    let mut results = Vec::new();
    let mut per_op_maps: Vec<Option<serde_json::Value>> = Vec::new();
    let mut ledger: EditLedger = HashMap::new();

    // Snapshot before any mutation — used to build old→new path migration records.
    let baseline_map = if cmd.emit_map {
        handler.extract_text_with_offsets().ok()
    } else {
        None
    };

    for op in ops {
        let result = execute_batch_op(&*handler, &op, &mut ledger, &cmd.file);
        if cmd.emit_map {
            // Re-extract after every op so callers can re-address against the
            // structure produced by this specific step.
            per_op_maps.push(super::offset_map_value(handler.as_ref()));
        }
        results.push(BatchResult {
            op: op.command.clone(),
            result,
        });
        if stop_on_error && results.last().is_some_and(|result| result.result.is_err()) {
            break;
        }
    }

    // By default a batch is transactional at the package boundary: handlers
    // mutate their in-memory package only, so omitting save leaves the source
    // file untouched after any error. --best-effort retains the historical
    // partial-save behaviour as an explicit opt-in.
    let has_error = results.iter().any(|r| r.result.is_err());
    let has_mutations = results.iter().any(|r| {
        matches!(
            r.op.as_str(),
            "set" | "add" | "remove" | "move" | "copy" | "swap" | "raw-set" | "add-part" | "import"
        ) && r.result.is_ok()
    });
    if has_mutations && (cmd.best_effort || !has_error) {
        handler.save()?;
    } else if has_error && !cmd.best_effort {
        mark_batch_results_rolled_back(&mut results);
    }

    let output = if format == OutputFormat::Json {
        if cmd.emit_map {
            let per_op: Vec<serde_json::Value> = results
                .iter()
                .zip(per_op_maps)
                .map(|(r, map)| {
                    serde_json::json!({
                        "op": r.op,
                        "result": match &r.result {
                            Ok(v) => serde_json::json!({ "ok": v }),
                            Err(e) => serde_json::json!({ "error": e }),
                        },
                        "offset_map": map,
                    })
                })
                .collect();
            let final_map = handler.extract_text_with_offsets().ok();
            let path_migrations = match (&baseline_map, &final_map) {
                (Some(before), Some(after)) => compute_path_migrations(before, after, &ledger),
                _ => Vec::new(),
            };
            let final_map_json = final_map.and_then(|m| serde_json::to_value(m).ok());
            serde_json::to_string_pretty(&serde_json::json!({
                "results": per_op,
                "final_map": final_map_json,
                "path_migrations": path_migrations,
            }))
            .map_err(HandlerError::JsonError)?
        } else {
            serde_json::to_string_pretty(&results).map_err(HandlerError::JsonError)?
        }
    } else {
        results
            .iter()
            .map(|r| match &r.result {
                Ok(val) => format!("{}: OK — {}", r.op, val),
                Err(e) => format!("{}: ERROR — {}", r.op, e),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    Ok(output)
}

struct ParsedBatch {
    ops: Vec<BatchOp>,
    stop_on_error: Option<bool>,
}

fn parse_batch_ops(batch_json: &str) -> Result<ParsedBatch, HandlerError> {
    let value: serde_json::Value = serde_json::from_str(batch_json)
        .map_err(|error| HandlerError::InvalidArgument(format!("invalid batch JSON: {error}")))?;
    let (operations, stop_on_error) = match &value {
        serde_json::Value::Array(_) => (value, None),
        serde_json::Value::Object(object) => (
            object
                .get("commands")
                .or_else(|| object.get("operations"))
                .or_else(|| object.get("items"))
                .cloned()
                .ok_or_else(|| {
                    HandlerError::InvalidArgument(
                        "batch JSON envelope must contain commands, operations, or items"
                            .to_string(),
                    )
                })?,
            object.get("stopOnError").and_then(|value| value.as_bool()),
        ),
        _ => {
            return Err(HandlerError::InvalidArgument(
                "batch JSON must be an operation array or object envelope".to_string(),
            ))
        }
    };
    let ops = serde_json::from_value(operations).map_err(|error| {
        HandlerError::InvalidArgument(format!("invalid batch operations: {error}"))
    })?;
    Ok(ParsedBatch { ops, stop_on_error })
}

fn read_batch_json(cmd: &BatchCommand) -> Result<String, HandlerError> {
    if let Some(batch_json) = &cmd.batch_json {
        return Ok(batch_json.clone());
    }
    if let Some(commands) = &cmd.commands {
        return Ok(commands.clone());
    }
    if let Some(path) = &cmd.commands_file {
        if path == "-" {
            return read_batch_stdin();
        }
        return std::fs::read_to_string(path).map_err(|e| {
            HandlerError::OperationFailed(format!(
                "failed to read batch commands file '{}': {}",
                path, e
            ))
        });
    }
    // C# batch reads stdin when no explicit source is supplied. `--stdin`
    // remains an accepted explicit spelling for the established Rust surface.
    read_batch_stdin()
}

fn read_batch_stdin() -> Result<String, HandlerError> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to read stdin: {}", e)))?;
    Ok(input.trim_start_matches('\u{feff}').to_string())
}

fn try_handle_docx_bookmark_add_batch(
    file: &str,
    ops: &[BatchOp],
    format: OutputFormat,
    best_effort: bool,
) -> Result<Option<String>, HandlerError> {
    if ops.is_empty() || !is_docx_path(file) || !ops.iter().all(is_bookmark_add_op) {
        return Ok(None);
    }

    let items = ops
        .iter()
        .map(bookmark_add_item)
        .collect::<Vec<docx_handler::AddBatchItem>>();
    let handler = docx_handler::WordHandler::open(file, true)?;
    let add_results = handler.add_batch(&items)?;
    let mut results = add_results
        .into_iter()
        .map(|result| BatchResult {
            op: "add".to_string(),
            result: result.map(|path| format!("created: {}", path)),
        })
        .collect::<Vec<_>>();

    let has_error = results.iter().any(|r| r.result.is_err());
    if results.iter().any(|r| r.result.is_ok()) && (best_effort || !has_error) {
        handler.save()?;
    } else if has_error && !best_effort {
        mark_batch_results_rolled_back(&mut results);
    }

    let output = if format == OutputFormat::Json {
        serde_json::to_string_pretty(&results).map_err(HandlerError::JsonError)?
    } else {
        results
            .iter()
            .map(|r| match &r.result {
                Ok(val) => format!("{}: OK — {}", r.op, val),
                Err(e) => format!("{}: ERROR — {}", r.op, e),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(Some(output))
}

fn try_handle_docx_range_set_batch(
    file: &str,
    ops: &[BatchOp],
    format: OutputFormat,
    best_effort: bool,
) -> Result<Option<String>, HandlerError> {
    if ops.is_empty() || !is_docx_path(file) || !ops.iter().all(is_range_set_op) {
        return Ok(None);
    }

    let items = ops
        .iter()
        .map(range_set_item)
        .collect::<Vec<docx_handler::SetRangeBatchItem>>();
    let handler = docx_handler::WordHandler::open(file, true)?;
    let set_results = handler.set_range_batch(&items)?;
    let mut results = set_results
        .into_iter()
        .map(|result| BatchResult {
            op: "set".to_string(),
            result: result.map(format_set_result),
        })
        .collect::<Vec<_>>();

    let has_error = results.iter().any(|r| r.result.is_err());
    if results.iter().any(|r| r.result.is_ok()) && (best_effort || !has_error) {
        handler.save()?;
    } else if has_error && !best_effort {
        mark_batch_results_rolled_back(&mut results);
    }

    let output = if format == OutputFormat::Json {
        serde_json::to_string_pretty(&results).map_err(HandlerError::JsonError)?
    } else {
        results
            .iter()
            .map(|r| match &r.result {
                Ok(val) => format!("{}: OK — {}", r.op, val),
                Err(e) => format!("{}: ERROR — {}", r.op, e),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(Some(output))
}

fn mark_batch_results_rolled_back(results: &mut [BatchResult]) {
    for result in results.iter_mut().filter(|result| result.result.is_ok()) {
        result.result = Err("rolled back because another batch operation failed".to_string());
    }
}

fn is_docx_path(file: &str) -> bool {
    Path::new(file)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("docx"))
}

fn is_range_set_op(op: &BatchOp) -> bool {
    if op.command != "set" {
        return false;
    }
    let properties = range_set_properties(op);
    properties.contains_key("range_paths")
}

fn is_bookmark_add_op(op: &BatchOp) -> bool {
    if op.command != "add" {
        return false;
    }
    let element_type = op
        .params
        .get("type")
        .or_else(|| op.params.get("typeName"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    matches!(element_type, "bookmark" | "bookmarkStart" | "bookmarkstart")
}

fn range_set_item(op: &BatchOp) -> docx_handler::SetRangeBatchItem {
    docx_handler::SetRangeBatchItem {
        properties: range_set_properties(op),
    }
}

fn range_set_properties(op: &BatchOp) -> HashMap<String, String> {
    let mut properties = string_map(&op.params, "properties")
        .or_else(|| string_map(&op.params, "props"))
        .unwrap_or_default();
    if let Some(rp) = op.params.get("range_paths").and_then(|v| v.as_str()) {
        properties.insert("range_paths".to_string(), rp.to_string());
    }
    properties
}

fn format_set_result(unsupported: Vec<String>) -> String {
    if unsupported.is_empty() {
        "OK".to_string()
    } else if let Some(hint) = handler_common::format_style_hint(&unsupported) {
        format!("OK ({})", hint)
    } else {
        format!("OK (unsupported: {})", unsupported.join(", "))
    }
}

fn bookmark_add_item(op: &BatchOp) -> docx_handler::AddBatchItem {
    let parent = op
        .params
        .get("parent")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let element_type = op
        .params
        .get("type")
        .or_else(|| op.params.get("typeName"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut properties = string_map(&op.params, "properties")
        .or_else(|| string_map(&op.params, "props"))
        .unwrap_or_default();
    if let Some(rp) = op.params.get("range_paths").and_then(|v| v.as_str()) {
        properties.insert("range_paths".to_string(), rp.to_string());
    }
    docx_handler::AddBatchItem {
        parent,
        element_type,
        position: parse_position(&op.params),
        properties,
        wrap: op
            .params
            .get("wrap")
            .and_then(|v| v.as_str())
            .map(String::from),
    }
}

/// Shift an original-coordinate offset by the deltas of edits that end at or
/// before it on the same element.
fn shift_offset(edits: Option<&Vec<(usize, usize, i64)>>, p: usize) -> usize {
    match edits {
        Some(es) => {
            let acc: i64 = es
                .iter()
                .filter(|(_s, e, _d)| *e <= p)
                .map(|(_s, _e, d)| *d)
                .sum();
            (p as i64 + acc).max(0) as usize
        }
        None => p,
    }
}

/// Rebuild a `range_paths` string with offsets remapped through the ledger.
/// Returns the remapped string plus the list of `(element_path, orig_start,
/// orig_end)` ranges (only those with both bounds) so the caller can record
/// new deltas once it knows the replacement text length.
fn remap_range_paths(rp: &str, ledger: &EditLedger) -> Result<(String, RangeOriginals), String> {
    let segments = handler_common::parse_range_paths(rp)?;
    let mut tokens = Vec::new();
    let mut originals = Vec::new();

    for seg in &segments {
        let edits = ledger.get(&seg.path);
        let new_start = seg.start.map(|s| shift_offset(edits, s));
        let new_end = seg.end.map(|e| shift_offset(edits, e));

        if let (Some(s), Some(e)) = (seg.start, seg.end) {
            originals.push((seg.path.clone(), s, e));
        }

        if new_start.is_some() || new_end.is_some() {
            tokens.push(format!(
                "{}[{}..{}]",
                seg.path,
                new_start.map(|v| v.to_string()).unwrap_or_default(),
                new_end.map(|v| v.to_string()).unwrap_or_default()
            ));
        } else {
            tokens.push(seg.path.clone());
        }
    }

    Ok((tokens.join(","), originals))
}

/// Record text-length deltas for a completed range text replacement so later
/// ops on the same element get their offsets shifted correctly.
fn record_text_deltas(ledger: &mut EditLedger, originals: &RangeOriginals, new_len: usize) {
    for (path, s, e) in originals {
        let delta = new_len as i64 - (*e as i64 - *s as i64);
        if delta != 0 {
            ledger
                .entry(path.clone())
                .or_default()
                .push((*s, *e, delta));
        }
    }
}

/// One side of a path migration (before or after a batch).
#[derive(Debug, serde::Serialize)]
struct MigrationSide {
    path: String,
    global_start: usize,
    global_end: usize,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    stable_id: Option<String>,
}

/// Old path → new path mapping for IR layer bookkeeping after range edits.
#[derive(Debug, serde::Serialize)]
struct PathMigration {
    /// `unchanged` | `path_changed` | `split` | `removed`
    kind: &'static str,
    before: MigrationSide,
    after: Vec<MigrationSide>,
}

/// Derive the range-owning element path from a run path, e.g.
/// `/body/p[1]/r[2]` → `/body/p[1]`.
fn parent_element_path(run_path: &str) -> Option<String> {
    run_path.rfind("/r[").map(|idx| run_path[..idx].to_string())
}

/// Global start offset of the first run span under an element path.
fn element_global_start(map: &TextOffsetMap, element_path: &str) -> Option<usize> {
    map.spans
        .iter()
        .filter(|s| s.element_type == "run" && s.path.starts_with(element_path))
        .map(|s| s.start)
        .min()
}

/// Find final-map run spans whose global range overlaps `[start, end)`.
fn overlapping_run_spans(map: &TextOffsetMap, start: usize, end: usize) -> Vec<&OffsetSpan> {
    map.spans
        .iter()
        .filter(|s| s.element_type == "run" && s.start < end && s.end > start)
        .collect()
}

fn to_migration_side(span: &OffsetSpan) -> MigrationSide {
    MigrationSide {
        path: span.path.clone(),
        global_start: span.start,
        global_end: span.end,
        text: span.text.clone(),
        stable_id: span.id.clone(),
    }
}

/// Compare pre-batch and post-batch maps and emit path migration records.
///
/// Coordinates in `ledger` are paragraph-local (as used in `range_paths`); global
/// offsets in baseline spans are converted through the ledger before lookup.
fn compute_path_migrations(
    baseline: &TextOffsetMap,
    final_map: &TextOffsetMap,
    ledger: &EditLedger,
) -> Vec<PathMigration> {
    let mut migrations = Vec::new();

    for span in baseline
        .spans
        .iter()
        .filter(|s| s.element_type == "run" && !s.text.is_empty())
    {
        let Some(element_path) = parent_element_path(&span.path) else {
            continue;
        };
        let Some(base_elem_start) = element_global_start(baseline, &element_path) else {
            continue;
        };

        let local_start = span.start.saturating_sub(base_elem_start);
        let local_end = span.end.saturating_sub(base_elem_start);
        let edits = ledger.get(&element_path);

        let new_local_start = shift_offset(edits, local_start);
        let new_local_end = shift_offset(edits, local_end);

        let final_elem_start =
            element_global_start(final_map, &element_path).unwrap_or(base_elem_start);
        let new_global_start = final_elem_start + new_local_start;
        let new_global_end = final_elem_start + new_local_end;

        let after_spans = overlapping_run_spans(final_map, new_global_start, new_global_end);

        let kind = if after_spans.is_empty() {
            "removed"
        } else if after_spans.len() > 1 {
            "split"
        } else if after_spans[0].path != span.path || after_spans[0].text != span.text {
            "path_changed"
        } else {
            "unchanged"
        };

        if kind == "unchanged" {
            continue;
        }

        migrations.push(PathMigration {
            kind,
            before: to_migration_side(span),
            after: after_spans.iter().map(|s| to_migration_side(s)).collect(),
        });
    }

    migrations
}

#[cfg(test)]
mod path_migration_tests {
    use super::*;
    use handler_common::{OffsetSpan, TextOffsetMap};

    #[test]
    fn shift_offset_applies_cumulative_delta() {
        let mut ledger: EditLedger = HashMap::new();
        ledger
            .entry("/body/p[1]".to_string())
            .or_default()
            .push((6, 11, -4)); // "brave"(5) -> "X"(1)
        assert_eq!(shift_offset(ledger.get("/body/p[1]"), 16), 12);
    }

    #[test]
    fn detect_split_migration() {
        let mut before = TextOffsetMap::empty("docx");
        before.spans.push(OffsetSpan {
            start: 0,
            end: 17,
            path: "/body/p[1]/r[1]".into(),
            text: "Hello brave new world".into(),
            element_type: "run".into(),
            id: Some("p1".into()),
            bbox: None,
            style: None,
        });
        let mut after = TextOffsetMap::empty("docx");
        after.spans.push(OffsetSpan {
            start: 0,
            end: 6,
            path: "/body/p[1]/r[1]".into(),
            text: "Hello ".into(),
            element_type: "run".into(),
            id: Some("p1".into()),
            bbox: None,
            style: None,
        });
        after.spans.push(OffsetSpan {
            start: 6,
            end: 7,
            path: "/body/p[1]/r[2]".into(),
            text: "X".into(),
            element_type: "run".into(),
            id: Some("p1".into()),
            bbox: None,
            style: None,
        });
        after.spans.push(OffsetSpan {
            start: 7,
            end: 17,
            path: "/body/p[1]/r[3]".into(),
            text: " new world".into(),
            element_type: "run".into(),
            id: Some("p1".into()),
            bbox: None,
            style: None,
        });

        let mut ledger: EditLedger = HashMap::new();
        ledger
            .entry("/body/p[1]".to_string())
            .or_default()
            .push((6, 11, -4));

        let m = compute_path_migrations(&before, &after, &ledger);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].kind, "split");
        assert_eq!(m[0].before.path, "/body/p[1]/r[1]");
        assert_eq!(m[0].after.len(), 3);
    }
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct BatchOp {
    #[serde(alias = "op")]
    pub command: String,
    #[serde(default, flatten)]
    pub params: HashMap<String, serde_json::Value>,
}

#[derive(Debug, serde::Serialize)]
struct BatchResult {
    op: String,
    result: Result<String, String>,
}

pub(crate) fn execute_batch_op(
    handler: &dyn handler_common::DocumentHandler,
    op: &BatchOp,
    ledger: &mut EditLedger,
    file: &str,
) -> Result<String, String> {
    match op.command.as_str() {
        // C# dump streams start with a metadata/version marker. It is advisory
        // for this compatible reader and deliberately does not mutate the file.
        "meta" => Ok("OK".to_string()),
        "set" => {
            let path = op.params.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let mut properties = string_map(&op.params, "properties")
                .or_else(|| string_map(&op.params, "props"))
                .unwrap_or_default();

            // Range offsets are expressed in original coordinates; remap them
            // through the ledger so earlier text edits in this batch are honored.
            let rp_opt = properties.get("range_paths").cloned().or_else(|| {
                op.params
                    .get("range_paths")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            });

            if let Some(rp) = rp_opt {
                let (remapped, originals) = remap_range_paths(&rp, ledger)?;
                properties.insert("range_paths".to_string(), remapped);
                let result = handler.set(path, &properties);
                if result.is_ok() {
                    if let Some(new_text) = properties.get("text") {
                        record_text_deltas(ledger, &originals, new_text.chars().count());
                    }
                }
                return match result {
                    Ok(unsupported) => {
                        if unsupported.is_empty() {
                            Ok("OK".to_string())
                        } else if let Some(hint) = handler_common::format_style_hint(&unsupported) {
                            Ok(format!("OK ({})", hint))
                        } else {
                            Ok(format!("OK (unsupported: {})", unsupported.join(", ")))
                        }
                    }
                    Err(e) => Err(e.to_string()),
                };
            }

            match handler.set(path, &properties) {
                Ok(unsupported) => {
                    if unsupported.is_empty() {
                        Ok("OK".to_string())
                    } else if let Some(hint) = handler_common::format_style_hint(&unsupported) {
                        Ok(format!("OK ({})", hint))
                    } else {
                        Ok(format!("OK (unsupported: {})", unsupported.join(", ")))
                    }
                }
                Err(e) => Err(e.to_string()),
            }
        }
        "add" => {
            let parent = op
                .params
                .get("parent")
                .or_else(|| op.params.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let element_type = op
                .params
                .get("type")
                .or_else(|| op.params.get("typeName"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let position = parse_position(&op.params);
            let mut properties = string_map(&op.params, "properties")
                .or_else(|| string_map(&op.params, "props"))
                .unwrap_or_default();
            if let Some(rp) = op.params.get("range_paths").and_then(|v| v.as_str()) {
                // Bookmarks do not change text length, but their offsets still
                // need remapping if earlier ops shifted the paragraph text.
                let (remapped, _originals) = remap_range_paths(rp, ledger)?;
                properties.insert("range_paths".to_string(), remapped);
            }
            let wrap = op.params.get("wrap").and_then(|v| v.as_str());
            if let Some(source) = op.params.get("from").and_then(|v| v.as_str()) {
                if parent.is_empty() {
                    return Err("'add' command with 'from' requires 'parent' or 'path'".to_string());
                }
                return handler
                    .copy_from(source, parent, position)
                    .map(|path| format!("copied to: {}", path))
                    .map_err(|e| e.to_string());
            }
            if parent.is_empty() || element_type.is_empty() {
                return Err(
                    "'add' command requires 'parent' (or 'path') and 'type' fields".to_string(),
                );
            }
            match handler.add(parent, element_type, position, &properties, wrap) {
                Ok(path) => Ok(format!("created: {}", path)),
                Err(e) => Err(e.to_string()),
            }
        }
        "remove" => {
            let path = op.params.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let properties = string_map(&op.params, "properties")
                .or_else(|| string_map(&op.params, "props"))
                .unwrap_or_default();
            match handler.remove_with_properties(path, &properties) {
                Ok(_) => Ok("removed".to_string()),
                Err(e) => Err(e.to_string()),
            }
        }
        "move" => {
            let source = op
                .params
                .get("source")
                .or_else(|| op.params.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target = op
                .params
                .get("target")
                .or_else(|| op.params.get("to"))
                .and_then(|v| v.as_str());
            if source.is_empty() {
                return Err("'move' command requires 'source' or 'path' field".to_string());
            }
            let position = parse_position(&op.params);
            match handler.move_element(source, target, position) {
                Ok(path) => Ok(format!("moved to: {}", path)),
                Err(e) => Err(e.to_string()),
            }
        }
        "copy" => {
            let source = op
                .params
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target = op
                .params
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let position = parse_position(&op.params);
            match handler.copy_from(source, target, position) {
                Ok(path) => Ok(format!("copied to: {}", path)),
                Err(e) => Err(e.to_string()),
            }
        }
        "get" => {
            let path = op
                .params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("/");
            let depth = op.params.get("depth").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
            match handler.get(path, depth) {
                Ok(node) => Ok(serde_json::to_string(&node).unwrap_or_default()),
                Err(e) => Err(e.to_string()),
            }
        }
        "query" => {
            let selector = op
                .params
                .get("selector")
                .or_else(|| op.params.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if selector.is_empty() {
                return Err("'query' command requires 'selector' field".to_string());
            }
            let mut nodes = handler.query(selector).map_err(|e| e.to_string())?;
            if let Some(find) = op
                .params
                .get("find")
                .or_else(|| op.params.get("text"))
                .and_then(|value| value.as_str())
            {
                nodes.retain(|node| match node.text.as_deref() {
                    Some(text) => handler_common::matches_text_filter(text, find).unwrap_or(false),
                    None => false,
                });
                if let Err(error) = handler_common::matches_text_filter("", find) {
                    return Err(format!("invalid regex pattern in '{}': {}", find, error));
                }
            }
            serde_json::to_string(&nodes)
                .map_err(HandlerError::JsonError)
                .map_err(|e| e.to_string())
        }
        "swap" => {
            let path1 = op
                .params
                .get("path")
                .or_else(|| op.params.get("path1"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let path2 = op
                .params
                .get("path2")
                .or_else(|| op.params.get("to"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if path1.is_empty() || path2.is_empty() {
                return Err(
                    "'swap' command requires 'path' and 'path2' (or 'to') fields".to_string(),
                );
            }
            handler
                .swap(path1, path2)
                .map(|(left, right)| format!("Swapped {} <-> {}", left, right))
                .map_err(|e| e.to_string())
        }
        "view" => {
            let mode = op
                .params
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("text");
            match mode.to_ascii_lowercase().as_str() {
                "text" | "t" => handler.view_as_text(batch_view_options(&op.params)),
                "annotated" | "a" => handler.view_as_annotated(batch_view_options(&op.params)),
                "outline" | "o" => handler.view_as_outline(),
                "stats" | "s" => handler.view_as_stats(),
                "html" | "h" => handler.view_as_html(batch_view_options(&op.params)),
                "svg" | "g" => handler.view_as_svg(),
                "forms" | "f" => handler.view_as_forms(),
                "issues" | "i" => handler
                    .view_as_issues(
                        op.params.get("type").and_then(|value| value.as_str()),
                        op.params
                            .get("limit")
                            .and_then(|value| value.as_u64())
                            .map(|value| value as usize),
                    )
                    .and_then(|issues| {
                        serde_json::to_string(&issues).map_err(HandlerError::JsonError)
                    }),
                other => return Err(format!("unknown view mode: {}", other)),
            }
            .map_err(|e| e.to_string())
        }
        "raw-set" => {
            let part = op.params.get("part").and_then(|v| v.as_str()).unwrap_or("");
            let xpath = op
                .params
                .get("xpath")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let action = op
                .params
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let xml = op.params.get("xml").and_then(|v| v.as_str());
            let part = super::raw::normalize_logical_part_path(file, part);
            handler
                .raw_set(&part, xpath, action, xml)
                .map(|_| "OK".to_string())
                .map_err(|e| e.to_string())
        }
        "raw" => {
            let part = op.params.get("part").and_then(|v| v.as_str()).unwrap_or("");
            if part.is_empty() {
                return Err("'raw' command requires 'part' field".to_string());
            }
            let part = super::raw::normalize_logical_part_path(file, part);
            let opts = handler_common::RawOptions {
                start_row: op
                    .params
                    .get("start_row")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize),
                end_row: op
                    .params
                    .get("end_row")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize),
                cols: op
                    .params
                    .get("cols")
                    .and_then(|v| v.as_str())
                    .map(|value| value.split(',').map(str::to_string).collect()),
            };
            handler.raw(&part, opts).map_err(|e| e.to_string())
        }
        "add-part" => {
            let parent = op
                .params
                .get("parent")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let part_type = op
                .params
                .get("type")
                .or_else(|| op.params.get("part_type"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if parent.is_empty() || part_type.is_empty() {
                return Err("'add-part' command requires 'parent' and 'type' fields".to_string());
            }
            let properties =
                string_map(&op.params, "properties").or_else(|| string_map(&op.params, "props"));
            handler
                .add_part(parent, part_type, properties.as_ref())
                .map(|(rel_id, path)| {
                    format!("Created {} part: relId={} path={}", part_type, rel_id, path)
                })
                .map_err(|e| e.to_string())
        }
        "validate" => {
            let errors = handler.validate().map_err(|e| e.to_string())?;
            if errors.is_empty() {
                Ok("Validation passed: no errors found.".to_string())
            } else {
                let mut lines = vec![format!("Found {} validation error(s):", errors.len())];
                for error in errors {
                    lines.push(format!("  [{}] {}", error.error_type, error.description));
                    if let Some(path) = error.path {
                        lines.push(format!("    Path: {}", path));
                    }
                    if let Some(part) = error.part {
                        lines.push(format!("    Part: {}", part));
                    }
                }
                Ok(lines.join("\n"))
            }
        }
        "import" => {
            let parent = op
                .params
                .get("parent")
                .or_else(|| op.params.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let content = op.params.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if parent.is_empty() || !op.params.contains_key("text") {
                return Err("'import' command requires 'parent' and 'text' fields".to_string());
            }
            let properties = string_map(&op.params, "properties")
                .or_else(|| string_map(&op.params, "props"))
                .unwrap_or_default();
            let delimiter = match properties
                .get("format")
                .map(String::as_str)
                .unwrap_or("csv")
            {
                "csv" => ',',
                "tsv" => '\t',
                other => return Err(format!("Unknown format: {}. Use 'csv' or 'tsv'", other)),
            };
            let header = properties
                .get("header")
                .is_some_and(|value| matches!(value.as_str(), "true" | "1" | "yes"));
            let start_cell = properties
                .get("start-cell")
                .or_else(|| properties.get("startcell"))
                .map(String::as_str)
                .unwrap_or("A1");
            handler
                .import_csv(parent, content, delimiter, header, start_cell)
                .map_err(|e| e.to_string())
        }
        other => Err(format!("unknown command: {}", other)),
    }
}

fn parse_position(params: &HashMap<String, serde_json::Value>) -> InsertPosition {
    if let Some(index) = params.get("index").and_then(|value| value.as_u64()) {
        return InsertPosition::AtIndex(index as usize);
    }
    if let Some(after) = params.get("after").and_then(|value| value.as_str()) {
        return InsertPosition::AfterElement(after.to_string());
    }
    if let Some(before) = params.get("before").and_then(|value| value.as_str()) {
        return InsertPosition::BeforeElement(before.to_string());
    }
    match params.get("position").and_then(|v| v.as_str()) {
        None => InsertPosition::Append,
        Some(s) => {
            if let Some(idx) = s.parse::<usize>().ok() {
                InsertPosition::AtIndex(idx)
            } else if let Some(rest) = s.strip_prefix("after:") {
                InsertPosition::AfterElement(rest.to_string())
            } else if let Some(rest) = s.strip_prefix("before:") {
                InsertPosition::BeforeElement(rest.to_string())
            } else {
                InsertPosition::Append
            }
        }
    }
}

fn batch_view_options(params: &HashMap<String, serde_json::Value>) -> handler_common::ViewOptions {
    handler_common::ViewOptions {
        range: params
            .get("range")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        start_line: params
            .get("start")
            .or_else(|| params.get("start_line"))
            .and_then(|value| value.as_u64())
            .map(|value| value as usize),
        end_line: params
            .get("end")
            .or_else(|| params.get("end_line"))
            .and_then(|value| value.as_u64())
            .map(|value| value as usize),
        max_lines: params
            .get("max_lines")
            .and_then(|value| value.as_u64())
            .map(|value| value as usize),
        cols: params
            .get("cols")
            .and_then(|value| value.as_str())
            .map(|value| value.split(',').map(str::to_string).collect()),
        page: params
            .get("page")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        lazy_load: false,
    }
}

fn string_map(
    params: &HashMap<String, serde_json::Value>,
    key: &str,
) -> Option<HashMap<String, String>> {
    params.get(key).and_then(|v| v.as_object()).map(|obj| {
        obj.iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect()
    })
}
