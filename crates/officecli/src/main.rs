#![allow(
    clippy::match_result_ok,
    clippy::redundant_closure,
    clippy::let_and_return
)]

mod commands;
mod mcp;
#[cfg(unix)]
mod resident;
#[cfg(not(unix))]
mod resident {
    pub struct IpcResponse {
        pub error: Option<String>,
    }
    pub async fn run_server(_file_path: &str) -> Result<(), anyhow::Error> {
        Err(anyhow::anyhow!(
            "Resident mode is not supported on this platform"
        ))
    }
    pub fn spawn_server(_file_path: &str) -> Result<(), anyhow::Error> {
        Err(anyhow::anyhow!(
            "Resident mode is not supported on this platform"
        ))
    }
    pub async fn close_server(_file_path: &str) -> Result<IpcResponse, anyhow::Error> {
        Err(anyhow::anyhow!(
            "Resident mode is not supported on this platform"
        ))
    }
    pub fn socket_path_for_file(_file: &str) -> std::path::PathBuf {
        // Resident mode is not available on this platform; return a placeholder.
        std::path::PathBuf::from(format!(
            "{}\\officecli\\resident\\unsupported.sock",
            std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string())
        ))
    }
}
mod schema_crc;
mod screenshot;
mod watch;

use clap::Parser;
use handler_common::{DocumentHandler, HandlerError, OutputFormat};
use std::path::PathBuf;

/// OfficeCLI — CLI tool for Office documents (docx/xlsx/pptx) and PDF
#[derive(Parser)]
#[command(name = "officecli")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(disable_help_subcommand = true)]
#[command(about = "Create, view, query, and modify Office documents and PDFs")]
#[command(after_help = "\
EXAMPLES:
  officecli create demo.docx                  Create a blank Word document
  officecli convert old.doc                   Convert legacy .doc to .docx
  officecli convert old.xls -o new.xlsx       Convert with explicit output path
  officecli view demo.docx                    View document as plain text
  officecli view demo.docx -m outline         View outline with metadata
  officecli view demo.pdf -m annotated        View PDF with bbox coordinates
  officecli view demo.pdf -m html             Generate HTML layout preview for browser
  officecli get demo.docx '/body/p[1]'        Get a specific paragraph
  officecli set demo.docx '/body/p[1]' text='Hello'  Replace text
  officecli set demo.pdf '/page[1]/text[1]' text='Title' color='#FF0000' bgColor='#FFFF00'
  officecli set demo.pdf '/page[1]/text[1]' fontFile='assets/MyFont.ttf' size=14.5
  officecli query demo.docx paragraph         Find all paragraphs
  officecli extract-text demo.docx            Extract text with offset→path mapping
  officecli extract-text demo.pdf --with-offsets --json  Extract PDF text and offset mapping as JSON")]
struct Cli {
    /// Internal flag: run as resident IPC server (do not use directly)
    #[arg(long, hide = true)]
    resident_serve: Option<String>,

    /// Output as JSON instead of text
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<commands::Command>,
}

fn main() {
    // Keep this as an exact one-argument early dispatch, matching the C# CLI.
    // It deliberately bypasses clap so downstream tooling can fingerprint the
    // embedded help surface without requiring a document subcommand.
    let mut raw_args = std::env::args_os();
    let _executable = raw_args.next();
    if raw_args.next().as_deref() == Some(std::ffi::OsStr::new("--output-schema-crc"))
        && raw_args.next().is_none()
    {
        println!("{}", schema_crc::compute());
        return;
    }

    // Parse CLI args — if invalid, print full help + error instead of terse usage
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            use clap::CommandFactory;
            if e.kind() == clap::error::ErrorKind::UnknownArgument
                || e.kind() == clap::error::ErrorKind::InvalidSubcommand
                || e.kind() == clap::error::ErrorKind::MissingSubcommand
            {
                // Print full help then the error message
                let _ = Cli::command().print_help();
                eprintln!("\n\n{}", e);
                std::process::exit(1);
            }
            // For other errors (wrong types, etc.), use default clap output
            e.exit();
        }
    };

    // Handle internal resident server mode
    if let Some(file_path) = cli.resident_serve {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async {
            if let Err(e) = resident::run_server(&file_path).await {
                eprintln!("Resident server error: {}", e);
                std::process::exit(1);
            }
        });
        return;
    }

    let format = if cli.json {
        OutputFormat::Json
    } else {
        OutputFormat::Text
    };

    let command = cli.command.unwrap_or_else(|| {
        // No subcommand → print full help and exit with error code
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        cmd.print_help().unwrap();
        eprintln!("\nError: a subcommand is required. See above for available commands.");
        std::process::exit(1);
    });

    let mut validate_succeeded = true;
    let result = match command {
        commands::Command::View(cmd) => commands::handle_view(cmd, format),
        commands::Command::Get(cmd) => commands::handle_get(cmd, format),
        commands::Command::Query(cmd) => commands::handle_query(cmd, format),
        commands::Command::Set(cmd) => commands::handle_set(cmd, format),
        commands::Command::Add(cmd) => commands::handle_add(cmd, format),
        commands::Command::AddPart(cmd) => commands::handle_add_part(cmd, format),
        commands::Command::Remove(cmd) => commands::handle_remove(cmd, format),
        commands::Command::Move(cmd) => commands::handle_move(cmd, format),
        commands::Command::Swap(cmd) => commands::handle_swap(cmd, format),
        commands::Command::Refresh(cmd) => commands::handle_refresh(cmd, format),
        commands::Command::Raw(cmd) => commands::handle_raw(cmd, format),
        commands::Command::RawSet(cmd) => commands::handle_raw_set(cmd, format),
        commands::Command::Validate(cmd) => {
            commands::handle_validate_with_status(cmd, format).map(|(output, succeeded)| {
                validate_succeeded = succeeded;
                output
            })
        }
        commands::Command::Save(cmd) => commands::handle_save(cmd, format),
        commands::Command::ExtractText(cmd) => commands::handle_extract_text(cmd, format),
        commands::Command::Create(cmd) => commands::handle_create(cmd, format),
        commands::Command::Dump(cmd) => commands::handle_dump(cmd, format),
        commands::Command::Convert(cmd) => commands::handle_convert(cmd, format),
        commands::Command::Config(cmd) => commands::handle_config(cmd),
        commands::Command::Batch(cmd) => commands::handle_batch(cmd, format),
        commands::Command::Info(cmd) => commands::handle_info(cmd, format),
        commands::Command::Merge(cmd) => commands::handle_merge(cmd, format),
        commands::Command::Help(cmd) => commands::handle_help(cmd, cli.json),
        commands::Command::Import(cmd) => commands::handle_import(cmd, format),
        commands::Command::Plugins(cmd) => commands::handle_plugins(cmd, format),
        commands::Command::Install(cmd) => commands::handle_install(cmd, format),
        commands::Command::LoadSkill(cmd) => commands::handle_load_skill(cmd),
        commands::Command::Skills(cmd) => commands::handle_skills(cmd, format),
        commands::Command::Open(cmd) => handle_open(cmd),
        commands::Command::Close(cmd) => handle_close(cmd),
        commands::Command::Watch(cmd) => handle_watch(cmd),
        commands::Command::Unwatch(cmd) => handle_unwatch(cmd),
        commands::Command::Mark(cmd) => commands::handle_mark(cmd, cli.json),
        commands::Command::Unmark(cmd) => commands::handle_unmark(cmd, cli.json),
        commands::Command::Marks(cmd) => commands::handle_marks(cmd, cli.json),
        commands::Command::Goto(cmd) => commands::handle_goto(cmd, cli.json),
        commands::Command::Mcp(cmd) => handle_mcp(cmd),
        commands::Command::_SocketPath(cmd) => handle_socket_path(cmd),
    };

    match result {
        Ok(text) => {
            if cli.json {
                let rendered = commands::ensure_json_success_envelope(&text);
                let succeeded = commands::json_envelope_succeeded(&rendered);
                println!("{}", rendered);
                if !succeeded {
                    std::process::exit(1);
                }
            } else if validate_succeeded {
                if !text.is_empty() {
                    println!("{}", text);
                }
            } else {
                eprintln!("{}", text);
                std::process::exit(1);
            }
        }
        Err(e) => {
            if cli.json {
                // C# writes structured JSON failures to stdout so agents can
                // consume success and error results from the same stream.
                println!("{}", commands::json_error_envelope(&e));
            } else {
                eprintln!("Error: {}", e);
            }
            std::process::exit(1);
        }
    }
}

// ─── Handler functions for resident, watch, and MCP commands ───────────

fn handle_open(cmd: commands::OpenCommand) -> Result<String, HandlerError> {
    resident::spawn_server(&cmd.file).map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    Ok(format!("Resident server started for: {}", cmd.file))
}

fn handle_close(cmd: commands::CloseCommand) -> Result<String, HandlerError> {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        resident::close_server(&cmd.file)
            .await
            .map(|resp| {
                if let Some(error) = resp.error {
                    format!("Error: {}", error)
                } else {
                    format!("Resident server closed for: {}", cmd.file)
                }
            })
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))
    })
}

fn handle_watch(cmd: commands::WatchCommand) -> Result<String, HandlerError> {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        let abs_path = std::fs::canonicalize(&cmd.file)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| {
                if let Ok(curr) = std::env::current_dir() {
                    curr.join(&cmd.file).to_string_lossy().to_string()
                } else {
                    cmd.file.clone()
                }
            });

        watch::run_server(&cmd.file, &abs_path, cmd.port, cmd.id)
            .await
            .map(|_| "Watch server stopped".to_string())
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))
    })
}

fn handle_unwatch(cmd: commands::UnwatchCommand) -> Result<String, HandlerError> {
    let port = cmd.port.unwrap_or(crate::watch::DEFAULT_PORT);
    let addr = format!("127.0.0.1:{}", port);
    let mut stream = std::net::TcpStream::connect_timeout(
        &addr
            .parse()
            .map_err(|e| HandlerError::OperationFailed(format!("invalid address: {}", e)))?,
        std::time::Duration::from_secs(3),
    )
    .map_err(|e| {
        HandlerError::OperationFailed(format!(
            "no watch server listening on port {}: {}. Start it with `officecli watch {}`.",
            port, e, cmd.file
        ))
    })?;

    use std::io::{Read, Write};
    let request = "POST /shutdown HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    stream
        .write_all(request.as_bytes())
        .map_err(|e| HandlerError::OperationFailed(format!("write: {}", e)))?;
    let mut buf = [0u8; 256];
    let n = stream
        .read(&mut buf)
        .map_err(|e| HandlerError::OperationFailed(format!("read: {}", e)))?;
    let head = String::from_utf8_lossy(&buf[..n]);
    let status_line = head.lines().next().unwrap_or("(no response)");
    if !status_line.contains("200") && !status_line.contains("204") {
        return Err(HandlerError::OperationFailed(format!(
            "watch server returned: {}",
            status_line
        )));
    }
    Ok(format!("Watch server on port {} shutting down", port))
}

fn handle_mcp(cmd: commands::McpCommand) -> Result<String, HandlerError> {
    match cmd.args.as_slice() {
        [] => mcp::run_server()
            .map(|_| "MCP server stopped".to_string())
            .map_err(|e| HandlerError::OperationFailed(e.to_string())),
        [action] if action.eq_ignore_ascii_case("list") => mcp_list(),
        [target] => mcp_install(target),
        [action, target] if action.eq_ignore_ascii_case("uninstall") => mcp_uninstall(target),
        _ => Err(HandlerError::InvalidArgument(
            "Usage: officecli mcp [list|<target>|uninstall <target>]".to_string(),
        )),
    }
}

fn mcp_home() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Resolve a command that remains usable after upgrades where possible.  This
/// follows the C# installer's ordering: user install, PATH wrapper, then the
/// currently running binary as a development-build fallback.
fn mcp_command_path() -> String {
    let executable = if cfg!(windows) {
        "officecli.exe"
    } else {
        "officecli"
    };
    let installed = mcp_home().join(".local/bin").join(executable);
    if installed.is_file() {
        return installed.to_string_lossy().into_owned();
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            let candidate = directory.join(executable);
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| executable.to_string())
}

fn mcp_target(target: &str) -> Result<(&'static str, std::path::PathBuf), HandlerError> {
    let home = mcp_home();
    match target.to_ascii_lowercase().as_str() {
        "lms" | "lmstudio" | "lm-studio" => Ok(("lms", home.join(".cache/lm-studio/extensions/plugins/mcp/officecli"))),
        "claude" | "claude-code" => Ok(("claude", home.join(".claude.json"))),
        "cursor" => Ok(("cursor", home.join(".cursor/mcp.json"))),
        "vscode" | "copilot" => Ok(("vscode", home.join(".vscode/mcp.json"))),
        _ => Err(HandlerError::InvalidArgument(format!(
            "Unknown target: {}. Supported: lms (LM Studio), claude (Claude Code), cursor, vscode (Copilot)",
            target
        ))),
    }
}

fn mcp_install(target: &str) -> Result<String, HandlerError> {
    let (target, path) = mcp_target(target)?;
    let command = mcp_command_path();
    if target == "lms" {
        std::fs::create_dir_all(&path).map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        std::fs::write(path.join("manifest.json"), "{\"type\":\"plugin\",\"runner\":\"mcpBridge\",\"owner\":\"mcp\",\"name\":\"officecli\"}\n").map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        let bridge =
            serde_json::to_string(&serde_json::json!({"command": command, "args": ["mcp"]}))
                .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        std::fs::write(path.join("mcp-bridge-config.json"), format!("{bridge}\n"))
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        let installed_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        std::fs::write(
            path.join("install-state.json"),
            format!("{{\"by\":\"mcp-bridge-v1\",\"at\":{installed_at}}}\n"),
        )
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        return Ok("Registered officecli MCP in LM Studio.".to_string());
    }
    let mut root = read_mcp_json(&path);
    let object = root.as_object_mut().ok_or_else(|| {
        HandlerError::OperationFailed("invalid MCP configuration root".to_string())
    })?;
    let servers = object
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| HandlerError::OperationFailed("mcpServers is not an object".to_string()))?;
    servers.insert(
        "officecli".to_string(),
        serde_json::json!({"command":command,"args":["mcp"]}),
    );
    write_mcp_json(&path, &root)?;
    Ok(format!("Registered officecli MCP in {}.", target))
}

fn mcp_uninstall(target: &str) -> Result<String, HandlerError> {
    let (target, path) = mcp_target(target)?;
    if target == "lms" {
        if path.exists() {
            std::fs::remove_dir_all(&path)
                .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        }
        return Ok("Removed officecli MCP from LM Studio.".to_string());
    }
    let mut root = read_mcp_json(&path);
    let remove_servers_key = if let Some(servers) = root
        .get_mut("mcpServers")
        .and_then(serde_json::Value::as_object_mut)
    {
        servers.remove("officecli");
        servers.is_empty()
    } else {
        false
    };
    if remove_servers_key {
        root.as_object_mut()
            .expect("MCP configuration is always an object")
            .remove("mcpServers");
    }
    if path.exists() {
        write_mcp_json(&path, &root)?;
    }
    Ok(format!("Removed officecli MCP from {}.", target))
}

fn mcp_list() -> Result<String, HandlerError> {
    let mut lines = Vec::new();
    for target in ["lms", "claude", "cursor", "vscode"] {
        let (_, path) = mcp_target(target)?;
        let registered = if target == "lms" {
            path.join("mcp-bridge-config.json").exists()
        } else {
            read_mcp_json(&path)
                .get("mcpServers")
                .and_then(|v| v.get("officecli"))
                .is_some()
        };
        lines.push(format!(
            "{}: {}",
            target,
            if registered {
                "registered"
            } else {
                "not registered"
            }
        ));
    }
    Ok(lines.join("\n"))
}

fn read_mcp_json(path: &std::path::Path) -> serde_json::Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn write_mcp_json(path: &std::path::Path, value: &serde_json::Value) -> Result<(), HandlerError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    std::fs::write(path, text).map_err(|e| HandlerError::OperationFailed(e.to_string()))
}

fn handle_socket_path(cmd: commands::SocketPathCommand) -> Result<String, HandlerError> {
    let sock = resident::socket_path_for_file(&cmd.file);
    Ok(sock.to_string_lossy().to_string())
}

/// Open a document handler based on file extension.
pub(crate) fn open_handler(
    file: &str,
    editable: bool,
) -> Result<Box<dyn DocumentHandler>, HandlerError> {
    if resident_available(file) {
        #[cfg(unix)]
        return Ok(Box::new(resident::ResidentHandler::new(file)?));
    }
    open_handler_direct(file, editable)
}

pub(crate) fn resident_available(file: &str) -> bool {
    #[cfg(unix)]
    {
        resident::is_available(file)
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        false
    }
}

/// Open a document directly from disk, bypassing resident IPC. The resident
/// server uses this while it owns the in-memory session; every CLI command
/// should go through [`open_handler`] so an active session is reused.
pub(crate) fn open_handler_direct(
    file: &str,
    editable: bool,
) -> Result<Box<dyn DocumentHandler>, HandlerError> {
    let path = PathBuf::from(file);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        // Macro-enabled OOXML packages share the same WordprocessingML model.
        // OxmlPackage preserves unknown parts (including vbaProject.bin), so
        // routing .docm here enables safe structured edits without stripping
        // the macro payload.
        "docx" | "docm" => {
            let handler = docx_handler::WordHandler::open(file, editable)?;
            Ok(Box::new(handler))
        }
        // .xlsm uses the same SpreadsheetML handler; macro parts are retained
        // untouched by the package read/write cycle.
        "xlsx" | "xlsm" => {
            let handler = xlsx_handler::ExcelHandler::open(file, editable)?;
            Ok(Box::new(handler))
        }
        // .pptm likewise shares PresentationML with .pptx.
        "pptx" | "pptm" => {
            let handler = pptx_handler::PptxHandler::open(file, editable)?;
            Ok(Box::new(handler))
        }
        "pdf" => {
            let handler = pdf_handler::PdfHandler::open(file, editable)?;
            Ok(Box::new(handler))
        }
        other => {
            // Last-resort: any installed format-handler plugin that owns
            // this extension (e.g. .hwpx). See plugins/plugin-protocol.md §2.3.
            if commands::resolve_format_handler(other).is_some() {
                let proxy = commands::FormatHandlerProxy::open(file)?;
                return Ok(Box::new(proxy));
            }
            Err(HandlerError::OpenError(format!(
                "unsupported format: {}",
                other
            )))
        }
    }
}
