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

use handler_common::{DocumentHandler, HandlerError, InsertPosition, ViewOptions};
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
    socket_dir().join(format!("{:016x}.sock", hash))
}

// ─── Open handler (same as main.rs) ────────────────────────────────────

fn open_handler(file: &str, editable: bool) -> Result<Box<dyn DocumentHandler>, HandlerError> {
    crate::open_handler(file, editable)
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
        "view_issues" => match handler.view_as_issues(None, None) {
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
                Ok(unsupported) => IpcResponse::ok(serde_json::json!({
                    "result": "OK",
                    "unsupported": unsupported
                })),
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
            match handler.add(parent, element_type, position, &properties) {
                Ok(new_path) => IpcResponse::ok(serde_json::json!({"path": new_path})),
                Err(e) => IpcResponse::err(e.to_string()),
            }
        }
        "remove" => {
            let path = req
                .params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match handler.remove(path) {
                Ok(result) => IpcResponse::ok(serde_json::json!({"removed": result})),
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
                Ok(new_path) => IpcResponse::ok(serde_json::json!({"path": new_path})),
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
                Ok(new_path) => IpcResponse::ok(serde_json::json!({"path": new_path})),
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
    let sock_dir = socket_dir();
    std::fs::create_dir_all(&sock_dir)?;

    let sock_path = socket_path_for_file(file_path);

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
                if let Err(e) = handle_connection(handler.as_ref(), stream).await {
                    tracing::error!("Connection error: {}", e);
                }
            }
            Ok(Err(e)) => {
                tracing::error!("Accept error: {}", e);
            }
            Err(_) => {
                // Idle timeout expired — shut down
                tracing::info!("Idle timeout (60s), resident server exiting");
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
) -> Result<(), anyhow::Error> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (reader, mut writer) = stream.into_split();
    let reader = BufReader::new(reader);
    let mut lines = reader.lines();

    // Read a single request line, respond, then close.
    // This keeps the protocol simple: one request per connection.
    if let Some(line) = lines.next_line().await? {
        let req: IpcRequest = serde_json::from_str(&line)?;
        let resp = execute_request(handler, &req);

        // If the command is "close", signal shutdown after responding
        let is_close = req.command == "close";

        let resp_bytes = serde_json::to_vec(&resp)?;
        writer.write_all(&resp_bytes).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;

        if is_close {
            // The server should exit after responding to close
            // We return a special error to signal the server loop to break
            return Err(anyhow::anyhow!("client requested close"));
        }
    }

    Ok(())
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

// ─── Spawn: start the resident server as a background process ──────────

pub fn spawn_server(file_path: &str) -> Result<(), anyhow::Error> {
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
    // Keep stderr visible for debugging; child errors must be diagnosable
    cmd.stderr(std::process::Stdio::inherit());

    let child = cmd.spawn()?;
    // Don't wait — let it run in background
    // Just ensure the socket appears within a few seconds
    drop(child);

    // Wait for socket to appear (up to 5 seconds)
    let sock_path = socket_path_for_file(file_path);
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
            .and_then(|v| v.as_u64())
            .map(|v| v as usize),
    }
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
