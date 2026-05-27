//! Watch Mode — HTTP Live Preview server.
//!
//! When a user runs `officecli watch <file>`, an HTTP server starts on
//! localhost:26315 that provides a live preview of the document in a browser.
//!
//! Endpoints:
//!   GET /       — HTML preview page
//!   GET /sse    — Server-Sent Events stream for incremental updates
//!   GET /text   — Plain text view
//!   POST /mark  — Select/highlight elements
//!
//! Since DocumentHandler implementations use RefCell (non-Sync), we run
//! handler operations on a dedicated blocking thread via mpsc channels.

use axum::{
    extract::State,
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse,
    },
    routing::{get, post},
    Json, Router,
};
use handler_common::{InsertPosition, ViewOptions};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, watch as tokio_watch, Mutex};
use tower_http::cors::CorsLayer;

const DEFAULT_PORT: u16 = 26315;

// ─── Handler operation requests (sent to the blocking thread) ──────────

#[derive(Debug)]
enum HandlerOp {
    ViewText {
        opts: ViewOptions,
    },
    ViewAnnotated {
        opts: ViewOptions,
    },
    ViewOutline,
    ViewStats,
    ViewHtml,
    Reload,
    Get {
        path: String,
        depth: usize,
    },
    Query {
        selector: String,
    },
    Set {
        path: String,
        properties: HashMap<String, String>,
    },
    Add {
        parent: String,
        element_type: String,
        position: InsertPosition,
        properties: HashMap<String, String>,
    },
    Remove {
        path: String,
    },
    GetMark {
        path: String,
    },
}

#[derive(Debug, Serialize)]
struct HandlerResult {
    data: serde_json::Value,
}

// ─── Shared state (all Send+Sync safe) ──────────────────────────────────

struct AppState {
    file_path: String,
    op_tx: Mutex<mpsc::Sender<(HandlerOp, tokio::sync::oneshot::Sender<HandlerResult>)>>,
    update_tx: tokio_watch::Sender<String>,
}

// ─── JSON request/response types ───────────────────────────────────────

#[derive(Debug, Deserialize)]
struct MarkRequest {
    path: String,
}

#[derive(Debug, Serialize)]
struct ApiResponse {
    result: Option<serde_json::Value>,
    error: Option<String>,
}

impl ApiResponse {
    fn ok(value: serde_json::Value) -> Self {
        Self {
            result: Some(value),
            error: None,
        }
    }
    fn err(msg: impl Into<String>) -> Self {
        Self {
            result: None,
            error: Some(msg.into()),
        }
    }
}

// ─── Run the watch HTTP server ─────────────────────────────────────────

pub async fn run_server(file_path: &str, port: Option<u16>) -> Result<(), anyhow::Error> {
    let handler = crate::open_handler(file_path, true)?;

    // Channel for sending operations to the handler thread
    let (op_tx, op_rx) =
        mpsc::channel::<(HandlerOp, tokio::sync::oneshot::Sender<HandlerResult>)>(32);

    // SSE update notification channel
    let (update_tx, _) = tokio_watch::channel("init".to_string());

    // Spawn a dedicated thread for handler operations (non-Sync handler lives here)
    let file_path_str = file_path.to_string();
    let handler_thread = std::thread::spawn(move || {
        run_handler_thread(file_path_str, true, handler, op_rx);
    });

    let state = Arc::new(AppState {
        file_path: file_path.to_string(),
        op_tx: Mutex::new(op_tx),
        update_tx,
    });

    // Spawn a background task to watch the file for changes
    let file_path_clone = file_path.to_string();
    let state_clone = state.clone();
    tokio::spawn(async move {
        let mut last_modified = std::fs::metadata(&file_path_clone)
            .and_then(|m| m.modified())
            .ok();

        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            if let Ok(metadata) = std::fs::metadata(&file_path_clone) {
                if let Ok(modified) = metadata.modified() {
                    if Some(modified) != last_modified {
                        last_modified = Some(modified);
                        tracing::info!("File changed on disk, reloading handler...");
                        println!("File changed on disk, reloading...");
                        let reload_res = send_op(&state_clone, HandlerOp::Reload).await;
                        if reload_res.data.get("error").is_none() {
                            let _ = state_clone.update_tx.send("change".to_string());
                        } else {
                            eprintln!("Error reloading file: {:?}", reload_res.data.get("error"));
                        }
                    }
                }
            }
        }
    });

    let actual_port = port.unwrap_or(DEFAULT_PORT);

    let app = Router::new()
        .route("/", get(handle_index))
        .route("/sse", get(handle_sse))
        .route("/text", get(handle_text))
        .route("/mark", post(handle_mark))
        .route("/view", get(handle_view))
        .route("/get", get(handle_get))
        .route("/set", post(handle_set))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", actual_port)).await?;
    tracing::info!(
        "Watch server listening on http://127.0.0.1:{}/",
        actual_port
    );
    println!("Watch server started at http://127.0.0.1:{}/", actual_port);
    println!("Press Ctrl+C to stop");

    axum::serve(listener, app).await?;

    // The handler thread will exit when op_rx is closed
    drop(handler_thread);
    Ok(())
}

// ─── Handler thread: processes operations on the non-Sync handler ──────

fn run_handler_thread(
    file_path: String,
    editable: bool,
    mut handler: Box<dyn handler_common::DocumentHandler>,
    mut op_rx: mpsc::Receiver<(HandlerOp, tokio::sync::oneshot::Sender<HandlerResult>)>,
) {
    while let Some((op, reply_tx)) = op_rx.blocking_recv() {
        if matches!(op, HandlerOp::Reload) {
            match crate::open_handler(&file_path, editable) {
                Ok(new_handler) => {
                    handler = new_handler;
                    let _ = reply_tx.send(HandlerResult {
                        data: serde_json::json!({"result": "OK"}),
                    });
                }
                Err(e) => {
                    let _ = reply_tx.send(HandlerResult {
                        data: serde_json::json!({"error": format!("Failed to reload handler: {}", e)}),
                    });
                }
            }
            continue;
        }
        let result = execute_handler_op(&*handler, op);
        // Send result back — ignore error if receiver was dropped
        let _ = reply_tx.send(result);
    }
}

fn execute_handler_op(
    handler: &dyn handler_common::DocumentHandler,
    op: HandlerOp,
) -> HandlerResult {
    match op {
        HandlerOp::ViewText { opts } => match handler.view_as_text(opts) {
            Ok(t) => HandlerResult {
                data: serde_json::Value::String(t),
            },
            Err(e) => HandlerResult {
                data: serde_json::json!({"error": e.to_string()}),
            },
        },
        HandlerOp::ViewAnnotated { opts } => match handler.view_as_annotated(opts) {
            Ok(t) => HandlerResult {
                data: serde_json::Value::String(t),
            },
            Err(e) => HandlerResult {
                data: serde_json::json!({"error": e.to_string()}),
            },
        },
        HandlerOp::ViewOutline => match handler.view_as_outline() {
            Ok(t) => HandlerResult {
                data: serde_json::Value::String(t),
            },
            Err(e) => HandlerResult {
                data: serde_json::json!({"error": e.to_string()}),
            },
        },
        HandlerOp::ViewStats => match handler.view_as_stats() {
            Ok(t) => HandlerResult {
                data: serde_json::Value::String(t),
            },
            Err(e) => HandlerResult {
                data: serde_json::json!({"error": e.to_string()}),
            },
        },
        HandlerOp::ViewHtml => match handler.view_as_html() {
            Ok(t) => HandlerResult {
                data: serde_json::Value::String(t),
            },
            Err(e) => HandlerResult {
                data: serde_json::json!({"error": e.to_string()}),
            },
        },
        HandlerOp::Get { path, depth } => match handler.get(&path, depth) {
            Ok(node) => HandlerResult {
                data: serde_json::to_value(node).unwrap_or_default(),
            },
            Err(e) => HandlerResult {
                data: serde_json::json!({"error": e.to_string()}),
            },
        },
        HandlerOp::Query { selector } => match handler.query(&selector) {
            Ok(nodes) => HandlerResult {
                data: serde_json::to_value(nodes).unwrap_or_default(),
            },
            Err(e) => HandlerResult {
                data: serde_json::json!({"error": e.to_string()}),
            },
        },
        HandlerOp::Set { path, properties } => match handler.set(&path, &properties) {
            Ok(unsupported) => HandlerResult {
                data: serde_json::json!({"result": "OK", "unsupported": unsupported}),
            },
            Err(e) => HandlerResult {
                data: serde_json::json!({"error": e.to_string()}),
            },
        },
        HandlerOp::Add {
            parent,
            element_type,
            position,
            properties,
        } => match handler.add(&parent, &element_type, position, &properties) {
            Ok(new_path) => HandlerResult {
                data: serde_json::json!({"path": new_path}),
            },
            Err(e) => HandlerResult {
                data: serde_json::json!({"error": e.to_string()}),
            },
        },
        HandlerOp::Remove { path } => match handler.remove(&path) {
            Ok(result) => HandlerResult {
                data: serde_json::json!({"removed": serde_json::to_value(result).unwrap_or_default()}),
            },
            Err(e) => HandlerResult {
                data: serde_json::json!({"error": e.to_string()}),
            },
        },
        HandlerOp::GetMark { path } => match handler.get(&path, 1) {
            Ok(node) => HandlerResult {
                data: serde_json::to_value(node).unwrap_or_default(),
            },
            Err(e) => HandlerResult {
                data: serde_json::json!({"error": e.to_string()}),
            },
        },
        HandlerOp::Reload => HandlerResult {
            data: serde_json::json!({"error": "reload should be handled by runner"}),
        },
    }
}

// ─── Helper: send an op to the handler thread and await result ─────────

async fn send_op(state: &Arc<AppState>, op: HandlerOp) -> HandlerResult {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let tx = state.op_tx.lock().await;
    if tx.send((op, reply_tx)).await.is_err() {
        return HandlerResult {
            data: serde_json::json!({"error": "handler thread closed"}),
        };
    }
    match reply_rx.await {
        Ok(result) => result,
        Err(_) => HandlerResult {
            data: serde_json::json!({"error": "handler thread dropped response"}),
        },
    }
}

// ─── GET / — HTML preview page ─────────────────────────────────────────

async fn handle_index(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let result = send_op(&state, HandlerOp::ViewHtml).await;

    if let Some(err) = result.data.get("error").and_then(|e| e.as_str()) {
        // Fallback to text view
        let text_res = send_op(
            &state,
            HandlerOp::ViewText {
                opts: ViewOptions::default(),
            },
        )
        .await;
        let text = text_res.data.as_str().unwrap_or("Error loading document");
        let file_name = std::path::Path::new(&state.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("document");

        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>OfficeCLI Watch — {file_name}</title>
<style>
body {{ font-family: monospace; margin: 20px; background: #1e1e1e; color: #d4d4d4; }}
#content {{ white-space: pre-wrap; line-height: 1.5; }}
#status {{ color: #ef5350; font-size: 0.8em; }}
</style>
</head>
<body>
<h2>{file_name} (Fallback Text View)</h2>
<div id="status">Error rendering HTML: {err}</div>
<pre id="content">{text_esc}</pre>
</body>
</html>"#,
            file_name = file_name,
            err = err,
            text_esc = html_escape(text),
        );
        return Html(html);
    }

    let html = result.data.as_str().unwrap_or("Error rendering HTML preview").to_string();
    Html(inject_live_reload(&html))
}

// ─── GET /sse — Server-Sent Events stream ──────────────────────────────

async fn handle_sse(
    State(state): State<Arc<AppState>>,
) -> Sse<impl futures::stream::Stream<Item = Result<Event, std::convert::Infallible>> + Send> {
    let mut rx = state.update_tx.subscribe();

    // Send initial content
    let initial = send_op(
        &state,
        HandlerOp::ViewText {
            opts: ViewOptions::default(),
        },
    )
    .await;

    let stream = async_stream::stream! {
        yield Ok(Event::default().data(serde_json::json!({"text": initial.data}).to_string()));

        loop {
            // Wait for update notification
            if rx.changed().await.is_err() {
                break;
            }
            let reason = rx.borrow().clone();
            if reason == "init" {
                continue;
            }

            // Read current text via handler thread
            let result = send_op(&state, HandlerOp::ViewText { opts: ViewOptions::default() }).await;
            yield Ok(Event::default().data(serde_json::json!({"text": result.data, "reason": reason}).to_string()));
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ─── GET /text — Plain text view ───────────────────────────────────────

async fn handle_text(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let result = send_op(
        &state,
        HandlerOp::ViewText {
            opts: ViewOptions::default(),
        },
    )
    .await;
    match result.data {
        serde_json::Value::String(s) => s.into_response(),
        other => other.to_string().into_response(),
    }
}

// ─── POST /mark — Select/highlight elements ────────────────────────────

async fn handle_mark(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MarkRequest>,
) -> Json<ApiResponse> {
    let result = send_op(
        &state,
        HandlerOp::GetMark {
            path: req.path.clone(),
        },
    )
    .await;

    if result.data.get("error").is_some() {
        Json(ApiResponse::err(
            result.data["error"].as_str().unwrap_or("unknown error"),
        ))
    } else {
        let _ = state.update_tx.send(format!("mark:{}", req.path));
        Json(ApiResponse::ok(result.data))
    }
}

// ─── GET /view — View with mode parameter ──────────────────────────────

async fn handle_view(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Json<ApiResponse> {
    let mode = params
        .get("mode")
        .cloned()
        .unwrap_or_else(|| "text".to_string());
    let opts = ViewOptions {
        start_line: params
            .get("start_line")
            .and_then(|v| v.parse::<usize>().ok()),
        end_line: params.get("end_line").and_then(|v| v.parse::<usize>().ok()),
        max_lines: params
            .get("max_lines")
            .and_then(|v| v.parse::<usize>().ok()),
        cols: params
            .get("cols")
            .map(|c| c.split(',').map(|s| s.to_string()).collect()),
    };

    let op = match mode.as_str() {
        "text" => HandlerOp::ViewText { opts },
        "annotated" => HandlerOp::ViewAnnotated { opts },
        "outline" => HandlerOp::ViewOutline,
        "stats" => HandlerOp::ViewStats,
        "html" => HandlerOp::ViewHtml,
        _ => return Json(ApiResponse::err(format!("unsupported view mode: {}", mode))),
    };

    let result = send_op(&state, op).await;

    if result.data.get("error").is_some() {
        Json(ApiResponse::err(
            result.data["error"].as_str().unwrap_or("unknown error"),
        ))
    } else {
        Json(ApiResponse::ok(result.data))
    }
}

// ─── GET /get — Get document node ──────────────────────────────────────

async fn handle_get(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Json<ApiResponse> {
    let path = params
        .get("path")
        .cloned()
        .unwrap_or_else(|| "/".to_string());
    let depth = params
        .get("depth")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1);

    let result = send_op(&state, HandlerOp::Get { path, depth }).await;

    if result.data.get("error").is_some() {
        Json(ApiResponse::err(
            result.data["error"].as_str().unwrap_or("unknown error"),
        ))
    } else {
        Json(ApiResponse::ok(result.data))
    }
}

// ─── POST /set — Set properties on an element ──────────────────────────

#[derive(Debug, Deserialize)]
struct SetRequest {
    path: String,
    properties: HashMap<String, String>,
}

async fn handle_set(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetRequest>,
) -> Json<ApiResponse> {
    let result = send_op(
        &state,
        HandlerOp::Set {
            path: req.path,
            properties: req.properties,
        },
    )
    .await;

    if result.data.get("error").is_some() {
        Json(ApiResponse::err(
            result.data["error"].as_str().unwrap_or("unknown error"),
        ))
    } else {
        // Notify SSE subscribers that document changed
        let _ = state.update_tx.send("set".to_string());
        Json(ApiResponse::ok(result.data))
    }
}

// ─── Utility ───────────────────────────────────────────────────────────

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn inject_live_reload(html: &str) -> String {
    let script = r#"
<script>
(function() {
  const es = new EventSource('/sse');
  es.onmessage = function(ev) {
    try {
      const obj = JSON.parse(ev.data);
      if (obj.reason && obj.reason !== 'init') {
        console.log('Document updated, reloading...');
        window.location.reload();
      }
    } catch(e) {
      if (ev.data !== 'init') {
        window.location.reload();
      }
    }
  };
  es.onerror = function() {
    console.log('SSE connection lost, reconnecting...');
  };
})();
</script>
"#;

    if let Some(pos) = html.rfind("</body>") {
        let (before, after) = html.split_at(pos);
        format!("{}{}{}", before, script, after)
    } else {
        format!("{}{}", html, script)
    }
}
