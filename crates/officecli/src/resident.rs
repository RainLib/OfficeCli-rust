//! Resident Mode — Unix Domain Socket IPC server/client.
//!
//! When a user runs `officecli open <file>`, a background server process is
//! spawned that keeps the document handler in memory. Subsequent commands
//! (view, get, set, etc.) are forwarded to this resident process via IPC,
//! avoiding repeated open/close overhead.
//!
//! The server listens on a Unix domain socket at:
//!   ~/.local/share/officecli/resident/<file-hash>.sock
//!
//! Client sends JSON commands, server responds with JSON results.
//! 60s idle timeout: if no command received for 60s, the server exits.

use handler_common::{
    BinaryInfo, DocumentHandler, DocumentIssue, DocumentNode, HandlerError, InsertPosition,
    MergeResult, RawOptions, TextOffsetMap, ValidationError, ViewOptions,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};
use tokio::time::timeout;

// ─── JSON protocol messages ────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct IpcRequest {
    pub command: String,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IpcResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl IpcResponse {
    fn ok(value: serde_json::Value) -> Self {
        Self {
            result: Some(value),
            error: None,
        }
    }
    fn err(msg: String) -> Self {
        Self {
            result: None,
            error: Some(msg),
        }
    }
}

// ─── Socket path helpers ───────────────────────────────────────────────

fn socket_dir() -> PathBuf {
    let base = dirs_base();
    let dir = base.join("officecli").join("resident");
    dir
}

fn dirs_base() -> PathBuf {
    // Use ~/.local/share on Unix, APPDATA on Windows (for future compat)
    if cfg!(target_os = "windows") {
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
    } else {
        std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                PathBuf::from(home).join(".local").join("share")
            })
    }
}

pub fn socket_path_for_file(file: &str) -> PathBuf {
    // Hash the absolute file path to get a stable socket name
    use std::hash::{Hash, Hasher};
    let abs = std::fs::canonicalize(file).unwrap_or_else(|_| PathBuf::from(file));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    abs.hash(&mut hasher);
    let hash = hasher.finish();
    let file_name = format!("{:016x}.sock", hash);
    let candidate = socket_dir().join(&file_name);
    // macOS has a particularly small sockaddr_un path budget (roughly 104
    // bytes). XDG_DATA_HOME is frequently a deep temporary path in tests and
    // can be deeply nested in managed environments, so retain the stable hash
    // but use a short shared fallback before bind/connect can fail.
    if candidate.as_os_str().as_encoded_bytes().len() < 100 {
        candidate
    } else {
        PathBuf::from("/tmp")
            .join("officecli-resident")
            .join(file_name)
    }
}

// ─── Open handler (same as main.rs) ────────────────────────────────────

fn open_handler(file: &str, editable: bool) -> Result<Box<dyn DocumentHandler>, HandlerError> {
    crate::open_handler_direct(file, editable)
}

/// IPC-backed handler returned to ordinary CLI commands when `officecli open`
/// owns the file. It intentionally implements the same trait as disk-backed
/// handlers so command implementations cannot accidentally bypass the live
/// in-memory document.
pub struct ResidentHandler {
    file_path: String,
    format: &'static str,
}

impl ResidentHandler {
    pub fn new(file_path: &str) -> Result<Self, HandlerError> {
        let format = match std::path::Path::new(file_path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "docx" | "docm" => "docx",
            "xlsx" | "xlsm" => "xlsx",
            "pptx" | "pptm" => "pptx",
            "pdf" => "pdf",
            _ => "unknown",
        };
        Ok(Self {
            file_path: file_path.to_string(),
            format,
        })
    }

    fn call(
        &self,
        command: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value, HandlerError> {
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        let response = runtime
            .block_on(send_request(
                &self.file_path,
                &IpcRequest {
                    command: command.to_string(),
                    params,
                },
            ))
            .map_err(|error| {
                HandlerError::OperationFailed(format!("resident request failed: {error}"))
            })?;
        response.result.ok_or_else(|| {
            HandlerError::OperationFailed(
                response
                    .error
                    .unwrap_or_else(|| "resident returned no result".to_string()),
            )
        })
    }

    fn value<T: serde::de::DeserializeOwned>(
        &self,
        command: &str,
        params: HashMap<String, serde_json::Value>,
    ) -> Result<T, HandlerError> {
        serde_json::from_value(self.call(command, params)?).map_err(|error| {
            HandlerError::OperationFailed(format!("invalid resident response: {error}"))
        })
    }
}

fn params(
    entries: impl IntoIterator<Item = (&'static str, serde_json::Value)>,
) -> HashMap<String, serde_json::Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn position_value(position: InsertPosition) -> String {
    match position {
        InsertPosition::Append => "append".to_string(),
        InsertPosition::AtIndex(index) => index.to_string(),
        InsertPosition::AfterElement(path) => format!("after:{path}"),
        InsertPosition::BeforeElement(path) => format!("before:{path}"),
    }
}

impl DocumentHandler for ResidentHandler {
    fn format_name(&self) -> &str {
        self.format
    }
    fn view_as_text(&self, opts: ViewOptions) -> Result<String, HandlerError> {
        self.value("view_text", view_params(opts))
    }
    fn view_as_annotated(&self, opts: ViewOptions) -> Result<String, HandlerError> {
        self.value("view_annotated", view_params(opts))
    }
    fn view_as_outline(&self) -> Result<String, HandlerError> {
        self.value("view_outline", HashMap::new())
    }
    fn view_as_stats(&self) -> Result<String, HandlerError> {
        self.value("view_stats", HashMap::new())
    }
    fn view_as_issues(
        &self,
        issue_type: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<DocumentIssue>, HandlerError> {
        self.value(
            "view_issues",
            params([
                (
                    "type",
                    issue_type.map_or(serde_json::Value::Null, |value| serde_json::json!(value)),
                ),
                (
                    "limit",
                    limit.map_or(serde_json::Value::Null, |value| serde_json::json!(value)),
                ),
            ]),
        )
    }
    fn view_as_text_json(&self, opts: ViewOptions) -> Result<serde_json::Value, HandlerError> {
        self.call("view_text_json", view_params(opts))
    }
    fn view_as_outline_json(&self) -> Result<serde_json::Value, HandlerError> {
        self.call("view_outline_json", HashMap::new())
    }
    fn view_as_stats_json(&self) -> Result<serde_json::Value, HandlerError> {
        self.call("view_stats_json", HashMap::new())
    }
    fn view_as_html(&self, opts: ViewOptions) -> Result<String, HandlerError> {
        self.value("view_html", view_params(opts))
    }
    fn view_as_svg(&self) -> Result<String, HandlerError> {
        self.value("view_svg", HashMap::new())
    }
    fn view_as_forms(&self) -> Result<String, HandlerError> {
        self.value("view_forms", HashMap::new())
    }
    fn get(&self, path: &str, depth: usize) -> Result<DocumentNode, HandlerError> {
        self.value(
            "get",
            params([
                ("path", serde_json::json!(path)),
                ("depth", serde_json::json!(depth)),
            ]),
        )
    }
    fn query(&self, selector: &str) -> Result<Vec<DocumentNode>, HandlerError> {
        self.value("query", params([("selector", serde_json::json!(selector))]))
    }
    fn set(
        &self,
        path: &str,
        properties: &HashMap<String, String>,
    ) -> Result<Vec<String>, HandlerError> {
        self.value(
            "set",
            params([
                ("path", serde_json::json!(path)),
                ("properties", serde_json::json!(properties)),
            ]),
        )
    }
    fn add(
        &self,
        parent: &str,
        element_type: &str,
        position: InsertPosition,
        properties: &HashMap<String, String>,
        wrap: Option<&str>,
    ) -> Result<String, HandlerError> {
        self.value(
            "add",
            params([
                ("parent", serde_json::json!(parent)),
                ("type", serde_json::json!(element_type)),
                ("position", serde_json::json!(position_value(position))),
                ("properties", serde_json::json!(properties)),
                (
                    "wrap",
                    wrap.map_or(serde_json::Value::Null, |value| serde_json::json!(value)),
                ),
            ]),
        )
    }
    fn remove(&self, path: &str) -> Result<Option<String>, HandlerError> {
        self.value("remove", params([("path", serde_json::json!(path))]))
    }
    fn remove_with_properties(
        &self,
        path: &str,
        properties: &HashMap<String, String>,
    ) -> Result<Option<String>, HandlerError> {
        self.value(
            "remove",
            params([
                ("path", serde_json::json!(path)),
                ("properties", serde_json::json!(properties)),
            ]),
        )
    }
    fn move_element(
        &self,
        source: &str,
        target: Option<&str>,
        position: InsertPosition,
    ) -> Result<String, HandlerError> {
        self.value(
            "move",
            params([
                ("source", serde_json::json!(source)),
                (
                    "target",
                    target.map_or(serde_json::Value::Null, |value| serde_json::json!(value)),
                ),
                ("position", serde_json::json!(position_value(position))),
            ]),
        )
    }
    fn copy_from(
        &self,
        source: &str,
        target: &str,
        position: InsertPosition,
    ) -> Result<String, HandlerError> {
        self.value(
            "copy",
            params([
                ("source", serde_json::json!(source)),
                ("target", serde_json::json!(target)),
                ("position", serde_json::json!(position_value(position))),
            ]),
        )
    }
    fn swap(&self, path1: &str, path2: &str) -> Result<(String, String), HandlerError> {
        self.value(
            "swap",
            params([
                ("path1", serde_json::json!(path1)),
                ("path2", serde_json::json!(path2)),
            ]),
        )
    }
    fn merge(&self, data: &HashMap<String, String>) -> Result<MergeResult, HandlerError> {
        self.value("merge", params([("data", serde_json::json!(data))]))
    }
    fn raw(&self, part_path: &str, opts: RawOptions) -> Result<String, HandlerError> {
        self.value("raw", raw_params(part_path, opts))
    }
    fn raw_set(
        &self,
        part_path: &str,
        xpath: &str,
        action: &str,
        xml: Option<&str>,
    ) -> Result<(), HandlerError> {
        self.call(
            "raw_set",
            params([
                ("part", serde_json::json!(part_path)),
                ("xpath", serde_json::json!(xpath)),
                ("action", serde_json::json!(action)),
                (
                    "xml",
                    xml.map_or(serde_json::Value::Null, |value| serde_json::json!(value)),
                ),
            ]),
        )
        .map(|_| ())
    }
    fn add_part(
        &self,
        parent: &str,
        part_type: &str,
        properties: Option<&HashMap<String, String>>,
    ) -> Result<(String, String), HandlerError> {
        self.value(
            "add_part",
            params([
                ("parent", serde_json::json!(parent)),
                ("part_type", serde_json::json!(part_type)),
                (
                    "properties",
                    properties.map_or(serde_json::Value::Null, |value| serde_json::json!(value)),
                ),
            ]),
        )
    }
    fn validate(&self) -> Result<Vec<ValidationError>, HandlerError> {
        self.value("validate", HashMap::new())
    }
    fn try_extract_binary(
        &self,
        path: &str,
        dest: &str,
    ) -> Result<Option<BinaryInfo>, HandlerError> {
        self.value(
            "extract_binary",
            params([
                ("path", serde_json::json!(path)),
                ("dest", serde_json::json!(dest)),
            ]),
        )
    }
    fn save(&self) -> Result<(), HandlerError> {
        self.call("save", HashMap::new()).map(|_| ())
    }
    fn extract_text_with_offsets(&self) -> Result<TextOffsetMap, HandlerError> {
        self.value("extract_text", HashMap::new())
    }
}

// ─── Server: execute an IPC request against the in-memory handler ──────

fn execute_request(handler: &dyn DocumentHandler, req: &IpcRequest) -> IpcResponse {
    match req.command.as_str() {
        // View commands
        "view_text" => {
            let opts = view_opts_from_params(&req.params);
            match handler.view_as_text(opts) {
                Ok(text) => IpcResponse::ok(serde_json::Value::String(text)),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "view_annotated" => {
            let opts = view_opts_from_params(&req.params);
            match handler.view_as_annotated(opts) {
                Ok(text) => IpcResponse::ok(serde_json::Value::String(text)),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "view_outline" => match handler.view_as_outline() {
            Ok(text) => IpcResponse::ok(serde_json::Value::String(text)),
            Err(e) => IpcResponse::err(e.to_string()),
        },
        "view_stats" => match handler.view_as_stats() {
            Ok(text) => IpcResponse::ok(serde_json::Value::String(text)),
            Err(e) => IpcResponse::err(e.to_string()),
        },
        "view_text_json" => {
            let opts = view_opts_from_params(&req.params);
            match handler.view_as_text_json(opts) {
                Ok(value) => IpcResponse::ok(value),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "view_outline_json" => match handler.view_as_outline_json() {
            Ok(value) => IpcResponse::ok(value),
            Err(e) => IpcResponse::err(e.to_string()),
        },
        "view_stats_json" => match handler.view_as_stats_json() {
            Ok(value) => IpcResponse::ok(value),
            Err(e) => IpcResponse::err(e.to_string()),
        },
        "view_svg" => match handler.view_as_svg() {
            Ok(text) => IpcResponse::ok(serde_json::Value::String(text)),
            Err(e) => IpcResponse::err(e.to_string()),
        },
        "view_forms" => match handler.view_as_forms() {
            Ok(text) => IpcResponse::ok(serde_json::Value::String(text)),
            Err(e) => IpcResponse::err(e.to_string()),
        },
        "view_issues" => match handler.view_as_issues(
            req.params.get("type").and_then(|value| value.as_str()),
            req.params
                .get("limit")
                .and_then(|value| value.as_u64())
                .map(|value| value as usize),
        ) {
            Ok(issues) => IpcResponse::ok(serde_json::to_value(issues).unwrap_or_default()),
            Err(e) => IpcResponse::err(e.to_string()),
        },
        "view_html" => {
            let opts = view_opts_from_params(&req.params);
            match handler.view_as_html(opts) {
                Ok(text) => IpcResponse::ok(serde_json::Value::String(text)),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }

        // Query commands
        "get" => {
            let path = req
                .params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("/");
            let depth = req
                .params
                .get("depth")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as usize;
            match handler.get(path, depth) {
                Ok(node) => IpcResponse::ok(serde_json::to_value(node).unwrap_or_default()),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "query" => {
            let selector = req
                .params
                .get("selector")
                .and_then(|v| v.as_str())
                .unwrap_or("*");
            match handler.query(selector) {
                Ok(nodes) => IpcResponse::ok(serde_json::to_value(nodes).unwrap_or_default()),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "set" => {
            let path = req
                .params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let properties = string_map_from_params(&req.params, "properties");
            match handler.set(path, &properties) {
                Ok(unsupported) => {
                    IpcResponse::ok(serde_json::to_value(unsupported).unwrap_or_default())
                }
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "add" => {
            let parent = req
                .params
                .get("parent")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let element_type = req
                .params
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let position = parse_insert_position(&req.params);
            let properties = string_map_from_params(&req.params, "properties");
            let wrap = req.params.get("wrap").and_then(|value| value.as_str());
            match handler.add(parent, element_type, position, &properties, wrap) {
                Ok(new_path) => IpcResponse::ok(serde_json::Value::String(new_path)),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "remove" => {
            let path = req
                .params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let properties = string_map_from_params(&req.params, "properties");
            match handler.remove_with_properties(path, &properties) {
                Ok(result) => IpcResponse::ok(serde_json::to_value(result).unwrap_or_default()),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "move" => {
            let source = req
                .params
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target = req.params.get("target").and_then(|v| v.as_str());
            let position = parse_insert_position(&req.params);
            match handler.move_element(source, target, position) {
                Ok(new_path) => IpcResponse::ok(serde_json::Value::String(new_path)),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "copy" => {
            let source = req
                .params
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let target = req
                .params
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let position = parse_insert_position(&req.params);
            match handler.copy_from(source, target, position) {
                Ok(new_path) => IpcResponse::ok(serde_json::Value::String(new_path)),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }

        // Raw commands
        "raw" => {
            let part = req
                .params
                .get("part")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let opts = raw_opts_from_params(&req.params);
            match handler.raw(part, opts) {
                Ok(content) => IpcResponse::ok(serde_json::Value::String(content)),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "raw_set" => {
            let part = req
                .params
                .get("part")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let xpath = req
                .params
                .get("xpath")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let action = req
                .params
                .get("action")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let xml = req.params.get("xml").and_then(|value| value.as_str());
            match handler.raw_set(part, xpath, action, xml) {
                Ok(()) => IpcResponse::ok(serde_json::json!({"result": "ok"})),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "add_part" => {
            let parent = req
                .params
                .get("parent")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let part_type = req
                .params
                .get("part_type")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let properties = string_map_from_params(&req.params, "properties");
            match handler.add_part(
                parent,
                part_type,
                (!properties.is_empty()).then_some(&properties),
            ) {
                Ok(value) => IpcResponse::ok(serde_json::to_value(value).unwrap_or_default()),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "swap" => {
            let path1 = req
                .params
                .get("path1")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let path2 = req
                .params
                .get("path2")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            match handler.swap(path1, path2) {
                Ok(value) => IpcResponse::ok(serde_json::to_value(value).unwrap_or_default()),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "merge" => {
            let data = string_map_from_params(&req.params, "data");
            match handler.merge(&data) {
                Ok(result) => IpcResponse::ok(serde_json::json!({
                    "replaced_count": result.replaced_count,
                    "unresolved_count": result.unresolved_count,
                })),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "extract_binary" => {
            let path = req
                .params
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let dest = req
                .params
                .get("dest")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            match handler.try_extract_binary(path, dest) {
                Ok(value) => IpcResponse::ok(serde_json::to_value(value).unwrap_or_default()),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "validate" => match handler.validate() {
            Ok(errors) => IpcResponse::ok(serde_json::to_value(errors).unwrap_or_default()),
            Err(e) => IpcResponse::err(e.to_string()),
        },
        "save" => match handler.save() {
            Ok(()) => IpcResponse::ok(serde_json::json!({"result": "saved"})),
            Err(e) => IpcResponse::err(e.to_string()),
        },
        "extract_text" => match handler.extract_text_with_offsets() {
            Ok(map) => IpcResponse::ok(serde_json::to_value(map).unwrap_or_default()),
            Err(e) => IpcResponse::err(e.to_string()),
        },
        "ping" => IpcResponse::ok(serde_json::json!({"status": "alive"})),
        "close" => IpcResponse::ok(serde_json::json!({"status": "closing"})),

        other => IpcResponse::err(format!("unknown command: {}", other)),
    }
}

// ─── Server: background process that holds the document open ───────────

pub async fn run_server(file_path: &str) -> Result<(), anyhow::Error> {
    // Ensure socket directory exists
    let sock_path = socket_path_for_file(file_path);
    let sock_dir = sock_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("resident socket has no parent directory"))?;
    std::fs::create_dir_all(sock_dir)?;

    // Remove stale socket if it exists
    if sock_path.exists() {
        std::fs::remove_file(&sock_path)?;
    }

    // Open the document in editable mode
    let handler = open_handler(file_path, true)?;

    let listener = UnixListener::bind(&sock_path)?;
    tracing::info!("Resident server listening on {}", sock_path.display());

    // Idle timeout: 60 seconds
    let idle_duration = Duration::from_secs(60);

    loop {
        // Accept with idle timeout — if no connection arrives in 60s, exit
        let accept_result = timeout(idle_duration, listener.accept()).await;
        match accept_result {
            Ok(Ok((stream, _addr))) => {
                // Reset idle timer: we got a connection
                match handle_connection(handler.as_ref(), stream).await {
                    Ok(true) => {
                        handler.save()?;
                        let _ = std::fs::remove_file(&sock_path);
                        return Ok(());
                    }
                    Ok(false) => {}
                    Err(e) => {
                        tracing::error!("Connection error: {}", e);
                    }
                }
            }
            Ok(Err(e)) => {
                tracing::error!("Accept error: {}", e);
            }
            Err(_) => {
                // Idle timeout expired — shut down
                tracing::info!("Idle timeout (60s), resident server exiting");
                handler.save()?;
                // Clean up socket file
                let _ = std::fs::remove_file(&sock_path);
                return Ok(());
            }
        }
    }
}

async fn handle_connection(
    handler: &dyn DocumentHandler,
    stream: UnixStream,
) -> Result<bool, anyhow::Error> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (reader, mut writer) = stream.into_split();
    let reader = BufReader::new(reader);
    let mut lines = reader.lines();

    // Read a single request line, respond, then close.
    // This keeps the protocol simple: one request per connection.
    if let Some(line) = lines.next_line().await? {
        let req: IpcRequest = serde_json::from_str(&line)?;
        // If the command is "close", signal shutdown after responding
        let is_close = req.command == "close";
        let resp = execute_request(handler, &req);

        let resp_bytes = serde_json::to_vec(&resp)?;
        writer.write_all(&resp_bytes).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;

        if is_close {
            return Ok(true);
        }
    }

    Ok(false)
}

// ─── Client: connect to resident server and send a command ─────────────

pub async fn send_request(file_path: &str, req: &IpcRequest) -> Result<IpcResponse, anyhow::Error> {
    let sock_path = socket_path_for_file(file_path);

    if !sock_path.exists() {
        return Err(anyhow::anyhow!(
            "No resident server for this file. Run 'officecli open {}' first.",
            file_path
        ));
    }

    let stream = UnixStream::connect(&sock_path).await?;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (reader, mut writer) = stream.into_split();
    let req_bytes = serde_json::to_vec(req)?;
    writer.write_all(&req_bytes).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    // Read response
    let mut reader = BufReader::new(reader);
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).await?;

    let resp: IpcResponse = serde_json::from_str(&resp_line)?;
    Ok(resp)
}

/// Whether the file currently has a resident session. The actual request is
/// still authoritative; this fast check only selects the handler proxy.
pub fn is_available(file_path: &str) -> bool {
    let socket_path = socket_path_for_file(file_path);
    if !socket_path.exists() {
        return false;
    }
    match std::os::unix::net::UnixStream::connect(&socket_path) {
        Ok(stream) => {
            drop(stream);
            true
        }
        Err(_) => {
            // A stale socket must not turn ordinary document commands into
            // permanent IPC failures after a resident crash.
            let _ = std::fs::remove_file(socket_path);
            false
        }
    }
}

// ─── Spawn: start the resident server as a background process ──────────

pub fn spawn_server(file_path: &str) -> Result<(), anyhow::Error> {
    if is_available(file_path) {
        return Ok(());
    }

    // Resolve to absolute path so parent and child compute the same socket hash
    let abs_path = std::fs::canonicalize(file_path)
        .map_err(|e| anyhow::anyhow!("cannot resolve file path '{}': {}", file_path, e))?;

    // Spawn ourselves as a child process with --resident-serve flag
    let current_exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(current_exe);
    cmd.arg("--resident-serve")
        .arg(abs_path.to_str().unwrap_or(file_path));

    // Detach from parent: on Unix, we can use double-fork-like approach
    // by just spawning and not waiting
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // new process group so parent exit doesn't kill child
    }

    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    // Do not inherit the caller's stderr: in a pipe or test harness the
    // resident would retain that descriptor after `open` exits, preventing
    // the caller from observing EOF. IPC responses remain the command-level
    // diagnostic surface.
    cmd.stderr(std::process::Stdio::null());

    let child = cmd.spawn()?;
    // Don't wait — let it run in background
    // Just ensure the socket appears within a few seconds
    drop(child);

    // Wait for socket to appear (up to 5 seconds)
    let sock_path = socket_path_for_file(file_path);
    if sock_path.exists() {
        let _ = std::fs::remove_file(&sock_path);
    }
    for _ in 0..50 {
        if sock_path.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    Err(anyhow::anyhow!(
        "resident server did not start within 5 seconds"
    ))
}

// ─── Close: send close command to resident server ──────────────────────

pub async fn close_server(file_path: &str) -> Result<IpcResponse, anyhow::Error> {
    let req = IpcRequest {
        command: "close".to_string(),
        params: HashMap::new(),
    };
    send_request(file_path, &req).await
}

// ─── Parameter parsing helpers ─────────────────────────────────────────

fn view_opts_from_params(params: &HashMap<String, serde_json::Value>) -> ViewOptions {
    ViewOptions {
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
        page: params
            .get("page")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        lazy_load: false,
    }
}

fn view_params(opts: ViewOptions) -> HashMap<String, serde_json::Value> {
    params([
        (
            "start_line",
            opts.start_line
                .map_or(serde_json::Value::Null, |value| serde_json::json!(value)),
        ),
        (
            "end_line",
            opts.end_line
                .map_or(serde_json::Value::Null, |value| serde_json::json!(value)),
        ),
        (
            "max_lines",
            opts.max_lines
                .map_or(serde_json::Value::Null, |value| serde_json::json!(value)),
        ),
        (
            "cols",
            opts.cols.map_or(serde_json::Value::Null, |value| {
                serde_json::json!(value.join(","))
            }),
        ),
        (
            "page",
            opts.page
                .map_or(serde_json::Value::Null, |value| serde_json::json!(value)),
        ),
    ])
}

fn raw_opts_from_params(params: &HashMap<String, serde_json::Value>) -> handler_common::RawOptions {
    handler_common::RawOptions {
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
    }
}

fn raw_params(part_path: &str, opts: RawOptions) -> HashMap<String, serde_json::Value> {
    params([
        ("part", serde_json::json!(part_path)),
        (
            "start_row",
            opts.start_row
                .map_or(serde_json::Value::Null, |value| serde_json::json!(value)),
        ),
        (
            "end_row",
            opts.end_row
                .map_or(serde_json::Value::Null, |value| serde_json::json!(value)),
        ),
        (
            "cols",
            opts.cols.map_or(serde_json::Value::Null, |value| {
                serde_json::json!(value.join(","))
            }),
        ),
    ])
}

fn parse_insert_position(params: &HashMap<String, serde_json::Value>) -> InsertPosition {
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

fn string_map_from_params(
    params: &HashMap<String, serde_json::Value>,
    key: &str,
) -> HashMap<String, String> {
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
