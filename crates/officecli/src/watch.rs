//! Watch Mode — HTTP Live Preview server.
//!
//! When a user runs `officecli watch <file>`, an HTTP server starts on
//! localhost:26315 that provides a live preview of the document in a browser.
//!
//! If the server is already running on that port, the process registers the
//! document under a unique `id` and blocks in the foreground.

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
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
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
    ViewHtml {
        opts: ViewOptions,
    },
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

struct ActiveDocument {
    file_path: String,
    op_tx: mpsc::Sender<(HandlerOp, tokio::sync::oneshot::Sender<HandlerResult>)>,
    update_tx: tokio_watch::Sender<String>,
    watcher_abort: mpsc::Sender<()>,
}

impl Drop for ActiveDocument {
    fn drop(&mut self) {
        tracing::info!("ActiveDocument dropped: {}", self.file_path);
        // Aborting watcher task
        let abort_tx = self.watcher_abort.clone();
        tokio::spawn(async move {
            let _ = abort_tx.send(()).await;
        });
    }
}

struct AppState {
    port: u16,
    registry: Arc<Mutex<HashMap<String, Arc<ActiveDocument>>>>,
}

// ─── JSON request/response types ───────────────────────────────────────

#[derive(Debug, Deserialize)]
struct MarkRequest {
    path: String,
}

#[derive(Debug, Deserialize)]
struct SetRequest {
    path: String,
    properties: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct RegisterRequest {
    id: String,
    file_path: String,
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

pub async fn run_server(
    file_path: &str,
    abs_path: &str,
    port: Option<u16>,
    id: Option<String>,
) -> Result<(), anyhow::Error> {
    let actual_port = port.unwrap_or(DEFAULT_PORT);
    let doc_id = id.unwrap_or_else(|| get_default_id(file_path));

    match tokio::net::TcpListener::bind(("127.0.0.1", actual_port)).await {
        Ok(listener) => run_host_server(listener, actual_port, &doc_id, abs_path).await,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            register_client(actual_port, &doc_id, abs_path).await
        }
        Err(e) => Err(e.into()),
    }
}

fn get_default_id(file_path: &str) -> String {
    std::path::Path::new(file_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "default".to_string())
}

// ─── TCP client registration ──────────────────────────────────────────

async fn register_client(port: u16, id: &str, file_path: &str) -> Result<(), anyhow::Error> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;

    let body = serde_json::json!({
        "id": id,
        "file_path": file_path,
    })
    .to_string();

    let request = format!(
        "POST /register HTTP/1.1\r\n\
         Host: 127.0.0.1:{}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: keep-alive\r\n\r\n{}",
        port,
        body.len(),
        body
    );

    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    if !line.contains("200 OK") {
        return Err(anyhow::anyhow!("Registration failed: {}", line.trim()));
    }

    println!("Watch server is already running on port {}.", port);
    println!("Successfully registered document '{}' at:", id);
    println!("👉 http://127.0.0.1:{}/{}/", port, id);
    println!("Press Ctrl+C to unregister and stop");

    let mut buffer = [0u8; 1024];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => {
                println!("Connection to watch server closed.");
                break;
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("Socket error: {}", err);
                break;
            }
        }
    }
    Ok(())
}

// ─── Host server runner ────────────────────────────────────────────────

async fn run_host_server(
    listener: tokio::net::TcpListener,
    port: u16,
    initial_id: &str,
    initial_file: &str,
) -> Result<(), anyhow::Error> {
    let registry = Arc::new(Mutex::new(HashMap::<String, Arc<ActiveDocument>>::new()));
    let state = Arc::new(AppState {
        port,
        registry: registry.clone(),
    });

    register_document(&state, initial_id, initial_file)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    let app = Router::new()
        .route("/", get(handle_landing))
        .route("/ping", get(handle_ping))
        .route("/register", post(handle_register))
        .route("/{id}", get(handle_redirect_slash))
        .route("/{id}/", get(handle_index))
        .route("/{id}/sse", get(handle_sse))
        .route("/{id}/text", get(handle_text))
        .route("/{id}/mark", post(handle_mark))
        .route("/{id}/view", get(handle_view))
        .route("/{id}/get", get(handle_get))
        .route("/{id}/set", post(handle_set))
        .route("/{id}/page/{page_num}/html", get(handle_page_html))
        .layer(CorsLayer::permissive())
        .with_state(state);

    println!("Watch server started at http://127.0.0.1:{}/", port);
    println!(
        "Primary document '{}' registered at: http://127.0.0.1:{}/{}/",
        initial_id, port, initial_id
    );
    println!("Press Ctrl+C to stop the watch server");

    axum::serve(listener, app).await?;
    Ok(())
}

// ─── Document Registration Helper ──────────────────────────────────────

async fn register_document(
    state: &Arc<AppState>,
    id: &str,
    file_path: &str,
) -> Result<Arc<ActiveDocument>, String> {
    let mut reg = state.registry.lock().await;
    if reg.contains_key(id) {
        return Err(format!("Document with ID '{}' is already registered", id));
    }

    let handler = match crate::open_handler(file_path, true) {
        Ok(h) => h,
        Err(e) => return Err(format!("Failed to open document: {}", e)),
    };

    let (op_tx, op_rx) =
        mpsc::channel::<(HandlerOp, tokio::sync::oneshot::Sender<HandlerResult>)>(32);
    let (update_tx, _) = tokio_watch::channel("init".to_string());
    let (watcher_abort_tx, mut watcher_abort_rx) = mpsc::channel::<()>(1);

    let file_path_str = file_path.to_string();
    std::thread::spawn(move || {
        run_handler_thread(file_path_str, true, handler, op_rx);
    });

    let file_path_clone = file_path.to_string();
    let update_tx_clone = update_tx.clone();
    let op_tx_clone = op_tx.clone();

    tokio::spawn(async move {
        let mut last_modified = std::fs::metadata(&file_path_clone)
            .and_then(|m| m.modified())
            .ok();

        loop {
            tokio::select! {
                _ = watcher_abort_rx.recv() => {
                    break;
                }
                _ = tokio::time::sleep(tokio::time::Duration::from_millis(500)) => {
                    if let Ok(metadata) = std::fs::metadata(&file_path_clone) {
                        if let Ok(modified) = metadata.modified() {
                            if Some(modified) != last_modified {
                                last_modified = Some(modified);
                                tracing::info!("File changed on disk: {}", file_path_clone);
                                println!("File changed on disk, reloading: {}", file_path_clone);
                                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                                if op_tx_clone.send((HandlerOp::Reload, reply_tx)).await.is_ok() {
                                    if let Ok(res) = reply_rx.await {
                                        if res.data.get("error").is_none() {
                                            let _ = update_tx_clone.send("change".to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    let doc = Arc::new(ActiveDocument {
        file_path: file_path.to_string(),
        op_tx,
        update_tx,
        watcher_abort: watcher_abort_tx,
    });

    reg.insert(id.to_string(), doc.clone());
    Ok(doc)
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
        HandlerOp::ViewHtml { opts } => match handler.view_as_html(opts) {
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

// ─── Helper: send an op to the doc handler thread and await result ─────

async fn send_op_for_doc(doc: &ActiveDocument, op: HandlerOp) -> HandlerResult {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if doc.op_tx.send((op, reply_tx)).await.is_err() {
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

// ─── GET /ping — Check if watch server is active ───────────────────────

async fn handle_ping() -> impl IntoResponse {
    Json(ApiResponse::ok(serde_json::json!({"status": "alive"})))
}

// ─── POST /register — Register a new preview client ────────────────────

struct RegisterStream {
    id: String,
    registry: Arc<Mutex<HashMap<String, Arc<ActiveDocument>>>>,
}

impl Drop for RegisterStream {
    fn drop(&mut self) {
        let id = self.id.clone();
        let registry = self.registry.clone();
        tokio::spawn(async move {
            let mut reg = registry.lock().await;
            if reg.remove(&id).is_some() {
                println!("Client disconnected, unregistered document ID: {}", id);
            }
        });
    }
}

async fn handle_register(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterRequest>,
) -> impl IntoResponse {
    let id = req.id.trim().to_string();
    let file_path = req.file_path.trim().to_string();

    if id.is_empty() || id.contains('/') || id.contains('\\') || id == "register" || id == "ping" {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ApiResponse::err("Invalid document ID")),
        )
            .into_response();
    }

    match register_document(&state, &id, &file_path).await {
        Ok(_) => {
            let registry_clone = state.registry.clone();

            let stream_holder = RegisterStream {
                id: id.clone(),
                registry: registry_clone,
            };

            let response_stream = async_stream::stream! {
                let _holder = stream_holder;
                yield Ok::<Event, std::convert::Infallible>(Event::default().data(serde_json::json!({"status": "registered", "id": id}).to_string()));

                let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
                loop {
                    interval.tick().await;
                    yield Ok::<Event, std::convert::Infallible>(Event::default().data(serde_json::json!({"status": "ping"}).to_string()));
                }
            };

            Sse::new(response_stream)
                .keep_alive(KeepAlive::default())
                .into_response()
        }
        Err(e) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ApiResponse::err(e)),
        )
            .into_response(),
    }
}

// ─── GET / — Server landing page ───────────────────────────────────────

async fn handle_landing(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let reg = state.registry.lock().await;
    let mut list_html = String::new();

    if reg.is_empty() {
        list_html.push_str("<p class=\"no-docs\">No documents are currently being watched.</p>");
    } else {
        list_html.push_str("<ul class=\"doc-list\">");
        for (id, doc) in reg.iter() {
            let file_name = std::path::Path::new(&doc.file_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("document");
            list_html.push_str(&format!(
                "<li><a href=\"/{}/\"><strong>{}</strong> <span class=\"file-path\">({})</span></a></li>",
                id, id, file_name
            ));
        }
        list_html.push_str("</ul>");
    }

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>OfficeCLI Watch Service</title>
<style>
body {{
    font-family: "Segoe UI", -apple-system, BlinkMacSystemFont, Roboto, Arial, sans-serif;
    margin: 0;
    background: #eef2f5;
    padding: 40px 20px;
    display: flex;
    flex-direction: column;
    align-items: center;
}}
.card {{
    background: white;
    padding: 30px;
    border-radius: 8px;
    box-shadow: 0 4px 12px rgba(0,0,0,0.1);
    max-width: 600px;
    width: 100%;
}}
h1 {{
    color: #2c3e50;
    margin-top: 0;
    margin-bottom: 20px;
    font-size: 24px;
}}
.doc-list {{
    list-style: none;
    padding: 0;
    margin: 0;
}}
.doc-list li {{
    padding: 12px 15px;
    border-bottom: 1px solid #eee;
}}
.doc-list li:last-child {{
    border-bottom: none;
}}
.doc-list a {{
    text-decoration: none;
    color: #3498db;
    display: flex;
    flex-direction: column;
    gap: 4px;
}}
.doc-list a:hover {{
    color: #2980b9;
}}
.file-path {{
    color: #7f8c8d;
    font-size: 12px;
}}
.no-docs {{
    color: #7f8c8d;
    font-style: italic;
}}
</style>
</head>
<body>
<div class="card">
    <h1>OfficeCLI Active Previews</h1>
    {}
</div>
</body>
</html>"#,
        list_html
    );

    Html(html)
}

// ─── GET /:id — Redirect to /:id/ ──────────────────────────────────────

async fn handle_redirect_slash(
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    axum::response::Redirect::to(&format!("/{}/", id))
}

// ─── Shared document route helpers ─────────────────────────────────────

async fn get_doc(
    state: &Arc<AppState>,
    id: &str,
) -> Result<Arc<ActiveDocument>, impl IntoResponse> {
    let reg = state.registry.lock().await;
    match reg.get(id).cloned() {
        Some(doc) => Ok(doc),
        None => Err((
            axum::http::StatusCode::NOT_FOUND,
            Json(ApiResponse::err(format!("Document '{}' not found", id))),
        )
            .into_response()),
    }
}

// ─── GET /:id/ — HTML preview page ─────────────────────────────────────

async fn handle_index(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let doc = match get_doc(&state, &id).await {
        Ok(d) => d,
        Err(r) => return r.into_response(),
    };

    let result = send_op_for_doc(
        &doc,
        HandlerOp::ViewHtml {
            opts: ViewOptions::default(),
        },
    )
    .await;

    if let Some(err) = result.data.get("error").and_then(|e| e.as_str()) {
        let text_res = send_op_for_doc(
            &doc,
            HandlerOp::ViewText {
                opts: ViewOptions::default(),
            },
        )
        .await;
        let text = text_res.data.as_str().unwrap_or("Error loading document");
        let file_name = std::path::Path::new(&doc.file_path)
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
        return Html(html).into_response();
    }

    let html = result
        .data
        .as_str()
        .unwrap_or("Error rendering HTML preview")
        .to_string();
    Html(inject_live_reload(&html)).into_response()
}

// ─── GET /:id/sse — Server-Sent Events stream ──────────────────────────

async fn handle_sse(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let doc = match get_doc(&state, &id).await {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };

    let mut rx = doc.update_tx.subscribe();

    let initial = send_op_for_doc(
        &doc,
        HandlerOp::ViewText {
            opts: ViewOptions::default(),
        },
    )
    .await;

    let doc_clone = doc.clone();
    let stream = async_stream::stream! {
        yield Ok::<Event, std::convert::Infallible>(Event::default().data(serde_json::json!({"text": initial.data}).to_string()));

        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let reason = rx.borrow().clone();
            if reason == "init" {
                continue;
            }

            let result = send_op_for_doc(&doc_clone, HandlerOp::ViewText { opts: ViewOptions::default() }).await;
            yield Ok::<Event, std::convert::Infallible>(Event::default().data(serde_json::json!({"text": result.data, "reason": reason}).to_string()));
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

// ─── GET /:id/text — Plain text view ───────────────────────────────────

async fn handle_text(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let doc = match get_doc(&state, &id).await {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let result = send_op_for_doc(
        &doc,
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

// ─── POST /:id/mark — Select/highlight elements ────────────────────────

async fn handle_mark(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<MarkRequest>,
) -> impl IntoResponse {
    let doc = match get_doc(&state, &id).await {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let result = send_op_for_doc(
        &doc,
        HandlerOp::GetMark {
            path: req.path.clone(),
        },
    )
    .await;

    if result.data.get("error").is_some() {
        Json(ApiResponse::err(
            result.data["error"].as_str().unwrap_or("unknown error"),
        ))
        .into_response()
    } else {
        let _ = doc.update_tx.send(format!("mark:{}", req.path));
        Json(ApiResponse::ok(result.data)).into_response()
    }
}

// ─── GET /:id/view — View with mode parameter ──────────────────────────

async fn handle_view(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let doc = match get_doc(&state, &id).await {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
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
        page: params.get("page").and_then(|v| v.parse::<usize>().ok()),
    };

    let op = match mode.as_str() {
        "text" => HandlerOp::ViewText { opts },
        "annotated" => HandlerOp::ViewAnnotated { opts },
        "outline" => HandlerOp::ViewOutline,
        "stats" => HandlerOp::ViewStats,
        "html" => HandlerOp::ViewHtml { opts },
        _ => {
            return Json(ApiResponse::err(format!("unsupported view mode: {}", mode)))
                .into_response()
        }
    };

    let result = send_op_for_doc(&doc, op).await;

    if result.data.get("error").is_some() {
        Json(ApiResponse::err(
            result.data["error"].as_str().unwrap_or("unknown error"),
        ))
        .into_response()
    } else {
        Json(ApiResponse::ok(result.data)).into_response()
    }
}

// ─── GET /:id/get — Get document node ──────────────────────────────────

async fn handle_get(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let doc = match get_doc(&state, &id).await {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let path = params
        .get("path")
        .cloned()
        .unwrap_or_else(|| "/".to_string());
    let depth = params
        .get("depth")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1);

    let result = send_op_for_doc(&doc, HandlerOp::Get { path, depth }).await;

    if result.data.get("error").is_some() {
        Json(ApiResponse::err(
            result.data["error"].as_str().unwrap_or("unknown error"),
        ))
        .into_response()
    } else {
        Json(ApiResponse::ok(result.data)).into_response()
    }
}

// ─── POST /:id/set — Set properties on an element ──────────────────────

async fn handle_set(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<SetRequest>,
) -> impl IntoResponse {
    let doc = match get_doc(&state, &id).await {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let result = send_op_for_doc(
        &doc,
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
        .into_response()
    } else {
        let _ = doc.update_tx.send("set".to_string());
        Json(ApiResponse::ok(result.data)).into_response()
    }
}

// ─── GET /:id/page/:page_num/html — Render single page html ──────────────

async fn handle_page_html(
    State(state): State<Arc<AppState>>,
    axum::extract::Path((id, page_num)): axum::extract::Path<(String, usize)>,
) -> impl IntoResponse {
    let doc = match get_doc(&state, &id).await {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let opts = ViewOptions {
        page: Some(page_num),
        ..Default::default()
    };
    let result = send_op_for_doc(&doc, HandlerOp::ViewHtml { opts }).await;

    if let Some(err) = result.data.get("error").and_then(|e| e.as_str()) {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            err.to_string(),
        )
            .into_response()
    } else {
        let html = result.data.as_str().unwrap_or("").to_string();
        Html(html).into_response()
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
  const ssePath = window.location.pathname.endsWith('/') ? 'sse' : window.location.pathname + '/sse';
  const es = new EventSource(ssePath);
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
