use clap::Args;
use handler_common::{HandlerError, InsertPosition, OutputFormat};
use std::collections::HashMap;

/// Execute multiple commands from a batch file
#[derive(Args)]
pub struct BatchCommand {
    pub file: String,
    /// JSON string containing an array of operations
    pub batch_json: String,
}

pub fn handle_batch(cmd: BatchCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, true)?;

    let ops: Vec<BatchOp> = serde_json::from_str(&cmd.batch_json)
        .map_err(|e| HandlerError::InvalidArgument(format!("invalid batch JSON: {}", e)))?;

    let mut results = Vec::new();

    for op in ops {
        let result = execute_batch_op(&*handler, &op);
        results.push(BatchResult {
            op: op.command.clone(),
            result,
        });
    }

    // Auto-save after batch operations if any mutation was performed
    let has_mutations = results
        .iter()
        .any(|r| matches!(r.op.as_str(), "set" | "add" | "remove" | "move") && r.result.is_ok());
    if has_mutations {
        handler.save()?;
    }

    let output = if format == OutputFormat::Json {
        serde_json::to_string_pretty(&results).map_err(|e| HandlerError::JsonError(e))?
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

#[derive(Debug, serde::Deserialize)]
struct BatchOp {
    command: String,
    #[serde(default)]
    params: HashMap<String, serde_json::Value>,
}

#[derive(Debug, serde::Serialize)]
struct BatchResult {
    op: String,
    result: Result<String, String>,
}

fn execute_batch_op(
    handler: &dyn handler_common::DocumentHandler,
    op: &BatchOp,
) -> Result<String, String> {
    match op.command.as_str() {
        "set" => {
            let path = op.params.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let properties = string_map(&op.params, "properties");
            match handler.set(path, &properties) {
                Ok(unsupported) => {
                    if unsupported.is_empty() {
                        Ok("OK".to_string())
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
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let element_type = op.params.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let position = parse_position(&op.params);
            let properties = string_map(&op.params, "properties");
            match handler.add(parent, element_type, position, &properties) {
                Ok(path) => Ok(format!("created: {}", path)),
                Err(e) => Err(e.to_string()),
            }
        }
        "remove" => {
            let path = op.params.get("path").and_then(|v| v.as_str()).unwrap_or("");
            match handler.remove(path) {
                Ok(_) => Ok("removed".to_string()),
                Err(e) => Err(e.to_string()),
            }
        }
        "move" => {
            let source = op
                .params
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target = op.params.get("target").and_then(|v| v.as_str());
            let position = parse_position(&op.params);
            match handler.move_element(source, target, position) {
                Ok(path) => Ok(format!("moved to: {}", path)),
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
        "view" => {
            let mode = op
                .params
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("text");
            match mode {
                "text" => match handler
                    .view_as_text(handler_common::output_format::ViewOptions::default())
                {
                    Ok(t) => Ok(t),
                    Err(e) => Err(e.to_string()),
                },
                "outline" => match handler.view_as_outline() {
                    Ok(t) => Ok(t),
                    Err(e) => Err(e.to_string()),
                },
                other => Err(format!("unknown view mode: {}", other)),
            }
        }
        other => Err(format!("unknown command: {}", other)),
    }
}

fn parse_position(params: &HashMap<String, serde_json::Value>) -> InsertPosition {
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

fn string_map(params: &HashMap<String, serde_json::Value>, key: &str) -> HashMap<String, String> {
    params
        .get(key)
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}
