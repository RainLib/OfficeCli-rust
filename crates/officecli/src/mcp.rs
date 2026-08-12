//! MCP Stdio Server — JSON-RPC 2.0 for AI agents.
//!
//! When a user runs `officecli mcp`, a stdio-based JSON-RPC 2.0 server
//! starts that allows AI agents (like Claude) to interact with OfficeCLI.
//!
//! Methods:
//!   tools/list  — list all available tools (view, get, set, add, remove, extract-text, etc.)
//!   tools/call  — execute a tool with parameters
//!
//! Each tool maps to a DocumentHandler method.
//! The file path is a required parameter for each tool call.

use handler_common::{HandlerError, InsertPosition, ViewOptions};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

// ─── JSON-RPC 2.0 types ────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl JsonRpcResponse {
    fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }
    fn error(id: Option<Value>, code: i64, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
        }
    }
}

// ─── MCP Tool definitions ──────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct ToolDefinition {
    name: String,
    description: String,
    input_schema: Value,
}

fn get_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "view".to_string(),
            description:
                "View document content in various modes (text, annotated, outline, stats, html)"
                    .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the document file" },
                    "mode": { "type": "string", "enum": ["text", "annotated", "outline", "stats", "html"], "default": "text" },
                    "start_line": { "type": "integer", "description": "Start line number" },
                    "end_line": { "type": "integer", "description": "End line number" },
                },
                "required": ["file"]
            }),
        },
        ToolDefinition {
            name: "get".to_string(),
            description: "Get a document element by path with optional depth".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the document file" },
                    "path": { "type": "string", "description": "Path to the element (e.g. /body/p[1])" },
                    "depth": { "type": "integer", "default": 1, "description": "Depth of children to return" },
                },
                "required": ["file", "path"]
            }),
        },
        ToolDefinition {
            name: "query".to_string(),
            description: "Query document elements using a selector".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the document file" },
                    "selector": { "type": "string", "description": "Selector expression" },
                },
                "required": ["file", "selector"]
            }),
        },
        ToolDefinition {
            name: "set".to_string(),
            description: "Set properties on a document element".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the document file" },
                    "path": { "type": "string", "description": "Path to the element" },
                    "properties": { "type": "object", "description": "Properties to set (key=value)" },
                },
                "required": ["file", "path", "properties"]
            }),
        },
        ToolDefinition {
            name: "add".to_string(),
            description: "Add a new element to the document".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the document file" },
                    "parent": { "type": "string", "description": "Parent path where to add" },
                    "type": { "type": "string", "description": "Element type to add" },
                    "position": { "type": "string", "description": "Position: index, 'after:/path', 'before:/path'" },
                    "properties": { "type": "object", "description": "Properties for the new element" },
                },
                "required": ["file", "parent", "type"]
            }),
        },
        ToolDefinition {
            name: "remove".to_string(),
            description: "Remove an element from the document".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the document file" },
                    "path": { "type": "string", "description": "Path to the element to remove" },
                },
                "required": ["file", "path"]
            }),
        },
        ToolDefinition {
            name: "move".to_string(),
            description: "Move an element within the document".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the document file" },
                    "source": { "type": "string", "description": "Source path" },
                    "target": { "type": "string", "description": "Target parent path" },
                    "position": { "type": "string", "description": "Position: index, 'after:/path', 'before:/path'" },
                },
                "required": ["file", "source"]
            }),
        },
        ToolDefinition {
            name: "validate".to_string(),
            description: "Validate a document for errors".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the document file" },
                },
                "required": ["file"]
            }),
        },
        ToolDefinition {
            name: "extract_text".to_string(),
            description: "Extract text content with offset mapping from the document".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the document file" },
                },
                "required": ["file"]
            }),
        },
        ToolDefinition {
            name: "find_replace".to_string(),
            description: "Find and replace text across the document. Works on Word (.docx), \
                           Excel (.xlsx), and PowerPoint (.pptx). Re-routes the set handler with \
                           find/replace properties; honors regex, caseSensitive, and wholeWord."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the document file" },
                    "path": {
                        "type": "string",
                        "description": "Scope path. Use \"/\" or \"/body\" for the whole \
                                        document (Word), or \"/SheetName\" (Excel), or \
                                        \"/slide[N]\" (PowerPoint). For Word you can also target \
                                        a single paragraph like \"/body/p[2]\"."
                    },
                    "find": { "type": "string", "description": "Literal text or regex pattern to find" },
                    "replace": { "type": "string", "description": "Replacement text. Supports $1/${name} capture refs when regex=true." },
                    "regex": { "type": "boolean", "default": false, "description": "Treat find as a regex pattern" },
                    "caseSensitive": { "type": "boolean", "default": true, "description": "Case-sensitive match (default true)" },
                    "wholeWord": { "type": "boolean", "default": false, "description": "Match whole words only" }
                },
                "required": ["file", "find", "replace"]
            }),
        },
        ToolDefinition {
            name: "save".to_string(),
            description: "Save modifications to the document file".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the document file" },
                },
                "required": ["file"]
            }),
        },
        ToolDefinition {
            name: "raw".to_string(),
            description: "View raw XML/PDF content of a document part".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Path to the document file" },
                    "part": { "type": "string", "description": "Part path (e.g. word/document.xml)" },
                },
                "required": ["file", "part"]
            }),
        },
        ToolDefinition {
            name: "info".to_string(),
            description: "Get information about document types and commands".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "topic": { "type": "string", "description": "Topic: docx, xlsx, pptx, pdf, offset, convert" },
                },
            }),
        },
        ToolDefinition {
            name: "convert".to_string(),
            description: "Convert legacy Office formats and PDF to modern Office formats"
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "Input file path (.doc, .xls, .ppt, .docx, .xlsx, .pptx, .pdf)" },
                    "output": { "type": "string", "description": "Output file path (optional, defaults to same path with modern extension)" },
                    "force": { "type": "boolean", "description": "Overwrite output file if it exists" },
                    "engine": { "type": "string", "enum": ["libreoffice", "oxide", "pdf-text", "pdf2docx"], "default": "libreoffice", "description": "Conversion engine: libreoffice, oxide, pdf-text, or pdf2docx" },
                },
                "required": ["file"]
            }),
        },
    ]
}

// ─── Tool execution ────────────────────────────────────────────────────

fn execute_tool(name: &str, params: &HashMap<String, Value>) -> Result<Value, String> {
    // convert tool does not use open_handler — it operates on legacy formats directly
    if name == "convert" {
        let file = params
            .get("file")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing required parameter: file".to_string())?;
        let output = params.get("output").and_then(|v| v.as_str());
        let force = params
            .get("force")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let engine_str = params
            .get("engine")
            .and_then(|v| v.as_str())
            .unwrap_or("libreoffice");
        let engine = crate::commands::parse_engine(engine_str).map_err(|e| e.to_string())?;

        let cmd = crate::commands::ConvertCommand {
            file: file.to_string(),
            output: output.map(|s| s.to_string()),
            force,
            engine,
        };

        return crate::commands::handle_convert(cmd, handler_common::OutputFormat::Json)
            .map(|s| serde_json::Value::String(s))
            .map_err(|e| e.to_string());
    }

    // info tool does not require file parameter
    if name == "info" {
        let topic = params.get("topic").and_then(|v| v.as_str());
        let info = match topic {
            Some("docx") => "Word document (.docx): Elements: p, r, tbl, tr, tc. Paths: /body/p[N], /body/tbl[N]/tr[N]/tc[N]",
            Some("xlsx") => "Excel spreadsheet (.xlsx): Elements: sheet, cell, chart, table, pivot. Paths: /SheetName/A1",
            Some("pptx") => "PowerPoint (.pptx): Elements: slide, shape, picture, textbox, table. Paths: /slide[N]/shape[N]",
            Some("pdf") => "PDF: Elements: page, text, image, annotation. Paths: /page[N]",
            Some("offset") => "Text Offset Mapping: Use extract_text tool to get text+offset->path mapping",
            Some("convert") => "Convert: .doc->.docx, .xls->.xlsx, .ppt->.pptx, .pdf->.docx. Engines: libreoffice, oxide, pdf-text, pdf2docx.",
            None => "OfficeCLI Tools: view, get, query, set, add, remove, move, validate, extract_text, save, raw, convert, info",
            Some(other) => return Err(format!("unknown info topic: {}", other)),
        };
        return Ok(serde_json::Value::String(info.to_string()));
    }

    // All other document tools require "file" parameter
    let file = params
        .get("file")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter: file".to_string())?;

    let editable = matches!(
        name,
        "set" | "add" | "remove" | "move" | "save" | "find_replace"
    );

    let handler = crate::open_handler(file, editable)
        .map_err(|e| format!("failed to open document: {}", e))?;

    match name {
        "view" => {
            let mode = params
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("text");
            let opts = ViewOptions {
                range: params
                    .get("range")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                start_line: params
                    .get("start_line")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize),
                end_line: params
                    .get("end_line")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize),
                max_lines: params
                    .get("max_lines")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize),
                cols: params
                    .get("cols")
                    .and_then(|v| v.as_str())
                    .map(|c| c.split(',').map(|s| s.to_string()).collect()),
                page: None,
                lazy_load: false,
            };

            let result = match mode {
                "text" => handler.view_as_text(opts),
                "annotated" => handler.view_as_annotated(opts),
                "outline" => handler.view_as_outline(),
                "stats" => handler.view_as_stats(),
                "html" => handler.view_as_html(opts),
                other => Err(HandlerError::UnsupportedMode(format!(
                    "view mode '{}' not supported",
                    other
                ))),
            };

            result
                .map(|t| serde_json::Value::String(t))
                .map_err(|e| e.to_string())
        }
        "get" => {
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing required parameter: path".to_string())?;
            let depth = params.get("depth").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
            handler
                .get(path, depth)
                .map(|node| serde_json::to_value(node).unwrap_or_default())
                .map_err(|e| e.to_string())
        }
        "query" => {
            let selector = params
                .get("selector")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing required parameter: selector".to_string())?;
            handler
                .query(selector)
                .map(|nodes| serde_json::to_value(nodes).unwrap_or_default())
                .map_err(|e| e.to_string())
        }
        "set" => {
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing required parameter: path".to_string())?;
            let properties: HashMap<String, String> = params
                .get("properties")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();

            handler
                .set(path, &properties)
                .map(|unsupported| serde_json::json!({"result": "OK", "unsupported": unsupported}))
                .map_err(|e| e.to_string())
        }
        "add" => {
            let parent = params
                .get("parent")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing required parameter: parent".to_string())?;
            let element_type = params
                .get("type")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing required parameter: type".to_string())?;
            let position = mcp_parse_position(params.get("position").and_then(|v| v.as_str()));
            let properties: HashMap<String, String> = params
                .get("properties")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();

            handler
                .add(parent, element_type, position, &properties, None)
                .map(|new_path| serde_json::json!({"path": new_path}))
                .map_err(|e| e.to_string())
        }
        "remove" => {
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing required parameter: path".to_string())?;
            handler
                .remove(path)
                .map(|result| serde_json::json!({"removed": result}))
                .map_err(|e| e.to_string())
        }
        "move" => {
            let source = params
                .get("source")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing required parameter: source".to_string())?;
            let target = params.get("target").and_then(|v| v.as_str());
            let position = mcp_parse_position(params.get("position").and_then(|v| v.as_str()));
            handler
                .move_element(source, target, position)
                .map(|new_path| serde_json::json!({"path": new_path}))
                .map_err(|e| e.to_string())
        }
        "validate" => handler
            .validate()
            .map(|errors| serde_json::to_value(errors).unwrap_or_default())
            .map_err(|e| e.to_string()),
        "extract_text" => handler
            .extract_text_with_offsets()
            .map(|map| serde_json::to_value(map).unwrap_or_default())
            .map_err(|e| e.to_string()),
        "find_replace" => {
            let find = params
                .get("find")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing required parameter: find".to_string())?
                .to_string();
            let replace = params
                .get("replace")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing required parameter: replace".to_string())?
                .to_string();
            // Default scope is the whole document.
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("/")
                .to_string();
            let mut props = HashMap::new();
            props.insert("find".to_string(), find);
            props.insert("replace".to_string(), replace);
            if let Some(true) = params.get("regex").and_then(|v| v.as_bool()) {
                props.insert("regex".to_string(), "true".to_string());
            }
            if let Some(b) = params.get("caseSensitive").and_then(|v| v.as_bool()) {
                props.insert(
                    "caseSensitive".to_string(),
                    if b { "true" } else { "false" }.to_string(),
                );
            }
            if let Some(true) = params.get("wholeWord").and_then(|v| v.as_bool()) {
                props.insert("wholeWord".to_string(), "true".to_string());
            }
            handler
                .set(&path, &props)
                .and_then(|out| {
                    handler.save()?;
                    // Handler returns "replaced=N" in the unsupported Vec.
                    let replaced = out
                        .iter()
                        .find_map(|s| s.strip_prefix("replaced=").and_then(|n| n.parse::<usize>().ok()))
                        .unwrap_or(0);
                    Ok(serde_json::json!({
                        "result": "OK",
                        "replaced": replaced,
                        "unsupported": out.iter().filter(|s| !s.starts_with("replaced=")).cloned().collect::<Vec<_>>()
                    }))
                })
                .map_err(|e| e.to_string())
        }
        "save" => handler
            .save()
            .map(|_| serde_json::json!({"result": "saved"}))
            .map_err(|e| e.to_string()),
        "raw" => {
            let part = params
                .get("part")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "missing required parameter: part".to_string())?;
            let opts = handler_common::RawOptions {
                start_row: params
                    .get("start_row")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize),
                end_row: params
                    .get("end_row")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize),
                cols: params
                    .get("cols")
                    .and_then(|v| v.as_str())
                    .map(|c| c.split(',').map(|s| s.to_string()).collect()),
            };
            handler
                .raw(part, opts)
                .map(|content| serde_json::Value::String(content))
                .map_err(|e| e.to_string())
        }
        other => Err(format!("unknown tool: {}", other)),
    }
}

fn mcp_parse_position(input: Option<&str>) -> InsertPosition {
    match input {
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

// ─── Main MCP server loop ──────────────────────────────────────────────

pub fn run_server() -> Result<(), anyhow::Error> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }

        let req: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse::error(None, -32700, format!("Parse error: {}", e));
                let resp_str = serde_json::to_string(&resp)?;
                stdout.write_all(resp_str.as_bytes())?;
                stdout.write_all(b"\n")?;
                stdout.flush()?;
                continue;
            }
        };

        let resp = handle_request(req);
        let resp_str = serde_json::to_string(&resp)?;
        stdout.write_all(resp_str.as_bytes())?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }

    Ok(())
}

fn handle_request(req: JsonRpcRequest) -> JsonRpcResponse {
    match req.method.as_str() {
        "initialize" => JsonRpcResponse::success(
            req.id,
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "officecli",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),

        "tools/list" => {
            let tools = get_tool_definitions();
            JsonRpcResponse::success(req.id, serde_json::json!({ "tools": tools }))
        }

        "tools/call" => {
            let params_obj = req
                .params
                .and_then(|p| serde_json::from_value::<HashMap<String, Value>>(p).ok())
                .unwrap_or_default();

            let tool_name = params_obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let call_params = params_obj
                .get("arguments")
                .and_then(|v| serde_json::from_value::<HashMap<String, Value>>(v.clone()).ok())
                .unwrap_or_default();

            let result = execute_tool(tool_name, &call_params);

            match result {
                Ok(val) => JsonRpcResponse::success(
                    req.id,
                    serde_json::json!({
                        "content": [
                            { "type": "text", "text": val.to_string() }
                        ]
                    }),
                ),
                Err(msg) => JsonRpcResponse::success(
                    req.id,
                    serde_json::json!({
                        "isError": true,
                        "content": [
                            { "type": "text", "text": msg }
                        ]
                    }),
                ),
            }
        }

        "notifications/initialized" => {
            // No response needed for notifications (no id)
            if req.id.is_none() {
                return JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    result: None,
                    error: None,
                };
            }
            JsonRpcResponse::success(req.id, serde_json::json!({}))
        }

        other => JsonRpcResponse::error(req.id, -32601, format!("Method not found: {}", other)),
    }
}
