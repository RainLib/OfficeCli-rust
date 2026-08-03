//! Subprocess driver and invokers for OfficeCLI plugins
//! (plugins/plugin-protocol.md).
//!
//! Implements the §5.6 idle-timeout watchdog: any byte on stdout, or a
//! heartbeat line on stderr matching `{"heartbeat":true}`, resets the
//! activity timer; once the gap exceeds the budget, the process is killed
//! and the caller sees `plugin_idle_timeout`. Wall-clock time is not
//! bounded — a 4 GB .doc that takes 20 minutes to dump but is constantly
//! producing output is fine.

use handler_common::HandlerError;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::plugins::{enumerate_all, PluginManifest};

/// Result of running a short-lived plugin subprocess.
#[derive(Debug)]
pub struct RunResult {
    pub exit_code: i32,
    /// Truncated to 16 KB to bound memory on chatty plugins. Head kept
    /// because the first error line is usually the useful one.
    pub stderr: String,
    pub idle_timed_out: bool,
}

/// Options for spawning a plugin subprocess.
pub struct RunOptions {
    pub executable_path: String,
    pub arguments: Vec<String>,
    /// Idle timeout in seconds. 0 disables the watchdog entirely (per §4.2).
    pub idle_timeout_seconds: u64,
    /// When true, stdout lines are collected and returned in
    /// `stdout_lines` for the caller to inspect. Dump-reader callers set
    /// this; exporter / info callers leave it false to drain silently.
    pub capture_stdout: bool,
    /// Extra env vars. `OFFICECLI_BIN` is set automatically.
    pub extra_env: Vec<(String, String)>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            executable_path: String::new(),
            arguments: Vec::new(),
            idle_timeout_seconds: 60,
            capture_stdout: false,
            extra_env: Vec::new(),
        }
    }
}

/// Spawn the plugin, drive stdout/stderr with the idle watchdog, and
/// return the captured result. Mirrors PluginProcess.Run from the C# tree.
/// When `capture_stdout` is set, stdout lines are returned as a vector; the
/// caller drains and processes them post-hoc (no callback lifetime
/// gymnastics needed).
pub fn run(opts: RunOptions) -> Result<(RunResult, Vec<String>), HandlerError> {
    let exe = std::env::current_exe().ok();
    let mut cmd = Command::new(&opts.executable_path);
    cmd.args(&opts.arguments);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    if let Some(p) = &exe {
        cmd.env("OFFICECLI_BIN", p);
    }
    for (k, v) in &opts.extra_env {
        cmd.env(k, v);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| HandlerError::OperationFailed(format!("failed to spawn plugin: {}", e)))?;

    // Wall-clock timestamp shared by both reader tasks and the watchdog.
    // We use wall-clock (not Instant) deliberately: on systems where the
    // machine sleeps mid-run, an Instant-based watchdog would under-count
    // elapsed wall-clock and let a hung plugin survive the wake-up. Wall
    // clock advances through suspend.
    let last_activity = Arc::new(AtomicI64::new(now_millis()));

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let stdout_la = last_activity.clone();
    let (stdout_tx, stdout_rx): (Sender<String>, Receiver<String>) = mpsc::channel();
    let capture = opts.capture_stdout;
    let stdout_handle = std::thread::spawn(move || {
        use std::io::BufRead;
        let reader = match stdout {
            Some(s) => std::io::BufReader::new(s),
            None => return,
        };
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            stdout_la.store(now_millis(), Ordering::SeqCst);
            if capture && stdout_tx.send(line).is_err() {
                break;
            }
        }
    });

    let stderr_la = last_activity.clone();
    let stderr_handle = std::thread::spawn(move || -> String {
        use std::io::BufRead;
        let mut collector = String::new();
        let reader = match stderr {
            Some(s) => std::io::BufReader::new(s),
            None => return collector,
        };
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            // Heartbeat resets activity without surfacing to the caller.
            if is_heartbeat(&line) {
                stderr_la.store(now_millis(), Ordering::SeqCst);
                continue;
            }
            stderr_la.store(now_millis(), Ordering::SeqCst);
            if collector.len() < 16 * 1024 {
                collector.push_str(&line);
                collector.push('\n');
            }
        }
        collector
    });

    let mut idle_timed_out = false;
    if opts.idle_timeout_seconds > 0 {
        let budget_ms = opts.idle_timeout_seconds * 1000;
        let poll_ms = (250).max(budget_ms / 4);
        loop {
            // poll exit with timeout
            match child.wait_timeout(Duration::from_millis(poll_ms)) {
                Ok(Some(_)) => break,
                Ok(None) => {
                    let last = last_activity.load(Ordering::SeqCst);
                    if now_millis().saturating_sub(last) > budget_ms as i64 {
                        idle_timed_out = true;
                        let _ = child.kill();
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }

    // Drain the process; ignore errors here, we still want to return
    // what we collected. The stdout reader thread owns `stdout_tx` and
    // drops it on exit, which unblocks `stdout_rx.iter().collect()`.
    let _ = child.wait();
    let stdout_lines: Vec<String> = stdout_rx.iter().collect();
    let stderr_collected = stderr_handle.join().unwrap_or_default();
    let _ = stdout_handle.join();

    let exit_code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);

    Ok((
        RunResult {
            exit_code,
            stderr: stderr_collected,
            idle_timed_out,
        },
        stdout_lines,
    ))
}

/// Wall-clock millis since UNIX_EPOCH. Stored in the activity atomic.
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// True if `line` is a §5.6 heartbeat envelope
/// (`{"heartbeat":true,...}`). Tolerant — leading whitespace and
/// additional keys are fine; we just require the leading `{` and the
/// substring `heartbeat` as a cheap pre-filter before parsing.
pub(crate) fn is_heartbeat(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.len() < 14 || !trimmed.starts_with('{') {
        return false;
    }
    if !trimmed.contains("heartbeat") {
        return false;
    }
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(trimmed);
    let Ok(serde_json::Value::Object(map)) = parsed else {
        return false;
    };
    matches!(map.get("heartbeat"), Some(serde_json::Value::Bool(true)))
}

// ─── Exporter Invoker (§5.2) ──────────────────────────────────

/// Outcome of a successful exporter invocation.
pub struct ExportResult {
    pub output_path: String,
    pub plugin_name: String,
}

// ─── Dump-Reader Invoker (§5.1) ──────────────────────────────

/// Outcome of a successful dump-reader invocation.
pub struct DumpResult {
    /// Path to the native file (`<stem>.<target>` next to source).
    pub converted_path: String,
    /// Name of the plugin that performed the conversion.
    pub plugin_name: String,
    /// Number of batch items replayed. Surfaced so callers can warn when 0.
    pub items_replayed: usize,
    /// Manifest target extension (e.g. "docx"). Used to chain into the
    /// next plugin stage when doing foreign→foreign conversions.
    pub target_family: String,
}

impl DumpResult {
    /// Return the manifest target extension.
    pub fn target_family(&self) -> &str {
        &self.target_family
    }
}

/// Resolve a format-handler plugin for `source_ext` (e.g. "hwpx"). Returns
/// the plugin's executable path + manifest when one is installed. The
/// session/proxy machinery in `format_handler_session` does the actual
/// long-lived IPC.
pub fn resolve_format_handler(source_ext: &str) -> Option<(String, PluginManifest)> {
    let source_dotted = ensure_dotted(source_ext);
    for plugin in enumerate_all() {
        if !plugin.manifest.kinds.iter().any(|k| k == "format-handler") {
            continue;
        }
        if !plugin
            .manifest
            .extensions
            .iter()
            .any(|e| e.eq_ignore_ascii_case(&source_dotted) || e.eq_ignore_ascii_case(source_ext))
        {
            continue;
        }
        return Some((plugin.executable_path, plugin.manifest));
    }
    None
}

/// Resolve a dump-reader plugin for `source_ext` and run it. Returns the
/// manifest's `target` extension (always docx / xlsx / pptx).
pub fn resolve_dump_reader(source_ext: &str) -> Option<(String, PluginManifest)> {
    let source_dotted = ensure_dotted(source_ext);
    for plugin in enumerate_all() {
        if !plugin.manifest.kinds.iter().any(|k| k == "dump-reader") {
            continue;
        }
        if !plugin
            .manifest
            .extensions
            .iter()
            .any(|e| e.eq_ignore_ascii_case(&source_dotted) || e.eq_ignore_ascii_case(source_ext))
        {
            continue;
        }
        return Some((plugin.executable_path, plugin.manifest));
    }
    None
}

/// Run a dump-reader plugin against `source_path`, replaying its JSONL
/// stream into a fresh native file at `out_path` (created via the in-tree
/// `create` skeleton). The plugin's manifest `target` field selects the
/// output family (docx / xlsx / pptx).
pub fn run_dump_reader(source_path: &str, out_path: &str) -> Result<DumpResult, HandlerError> {
    let source_ext = Path::new(source_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    let (exe, manifest) = resolve_dump_reader(&source_ext).ok_or_else(|| {
        HandlerError::UnsupportedMode(format!(
            "no dump-reader plugin found for .{} — install one or see plugins/plugin-protocol.md",
            source_ext
        ))
    })?;

    // Create a blank native skeleton of the right family.
    let target_ext = manifest.target.as_deref().unwrap_or("docx");
    match target_ext {
        "docx" => {
            let _ = super::create::create_blank_docx(out_path)?;
        }
        "xlsx" => {
            let _ = super::create::create_blank_xlsx(out_path)?;
        }
        "pptx" => {
            let _ = super::create::create_blank_pptx(out_path)?;
        }
        other => {
            return Err(HandlerError::OperationFailed(format!(
                "dump-reader plugin '{}' declared invalid target '{}'",
                manifest.name, other
            )));
        }
    }

    let idle = resolve_idle_timeout(&manifest, "dump");
    let opts = RunOptions {
        executable_path: exe,
        arguments: vec!["dump".to_string(), source_path.to_string()],
        idle_timeout_seconds: idle,
        capture_stdout: true,
        extra_env: Vec::new(),
    };

    let (result, stdout_lines) = run(opts)?;

    if result.idle_timed_out {
        let _ = std::fs::remove_file(out_path);
        return Err(HandlerError::OperationFailed(format!(
            "dump-reader plugin '{}' produced no output for {}s — likely hung.",
            manifest.name, idle
        )));
    }
    if result.exit_code != 0 {
        let _ = std::fs::remove_file(out_path);
        return Err(HandlerError::OperationFailed(format!(
            "dump-reader plugin '{}' failed (exit {} / {}): {}",
            manifest.name,
            result.exit_code,
            exit_code_to_error(result.exit_code),
            truncate_str(&result.stderr, 500)
        )));
    }

    // Reject legacy top-level JSON arrays explicitly.
    for line in &stdout_lines {
        let trimmed = line.trim_start_matches('\u{feff}').trim();
        if !trimmed.is_empty() && trimmed.starts_with('[') {
            let _ = std::fs::remove_file(out_path);
            return Err(HandlerError::OperationFailed(format!(
                "dump-reader plugin '{}' emitted a JSON array; \
                 protocol v1 requires JSONL (one BatchItem per line).",
                manifest.name
            )));
        }
    }

    // Open the handler and replay buffered lines synchronously on this
    // thread. (C# v6.4: the OOXML package's internal state is not
    // thread-safe under heavy multi-part Update-mode mutation, so we
    // buffer first and replay after the plugin exits.)
    let handler = crate::open_handler(out_path, true)?;
    let mut items_replayed = 0;
    for (idx, line) in stdout_lines.iter().enumerate() {
        let trimmed = line.trim_start_matches('\u{feff}').trim();
        if trimmed.is_empty() {
            continue;
        }
        let op: super::batch::BatchOp = serde_json::from_str(trimmed).map_err(|e| {
            HandlerError::OperationFailed(format!(
                "dump-reader plugin '{}' emitted invalid JSON at item #{}: {}",
                manifest.name, idx, e
            ))
        })?;
        super::batch::execute_batch_op(
            &*handler,
            &op,
            &mut std::collections::HashMap::new(),
            out_path,
        )
        .map_err(|e| {
            HandlerError::OperationFailed(format!(
                "dump-reader plugin '{}' command #{} ({}) failed while replaying: {}",
                manifest.name, idx, op.command, e
            ))
        })?;
        items_replayed += 1;
    }
    handler.save().map_err(|e| {
        HandlerError::OperationFailed(format!("failed to save replayed file: {}", e))
    })?;

    Ok(DumpResult {
        converted_path: out_path.to_string(),
        plugin_name: manifest.name,
        items_replayed,
        target_family: target_ext.to_string(),
    })
}

/// Resolve an exporter for `(source_ext, target_ext)`. The plugin's
/// declared `extensions` field nominates the target; its `supports` list
/// must accept the source (with a `from:<ext>` tag, a bare extension, or a
/// dotted extension). A missing/empty `supports` is treated as "accepts
/// all native sources" — conservative for older manifests.
pub fn resolve_exporter(source_ext: &str, target_ext: &str) -> Option<(String, PluginManifest)> {
    let target_dotted = ensure_dotted(target_ext);
    let source_bare = source_ext.trim_start_matches('.').to_ascii_lowercase();
    let source_dotted = format!(".{}", source_bare);

    for plugin in enumerate_all() {
        if !plugin.manifest.kinds.iter().any(|k| k == "exporter") {
            continue;
        }
        // Match by target extension.
        let handles_target =
            plugin.manifest.extensions.iter().any(|e| {
                e.eq_ignore_ascii_case(&target_dotted) || e.eq_ignore_ascii_case(target_ext)
            });
        if !handles_target {
            continue;
        }
        // Filter by source via `supports`.
        if plugin.manifest.supports.is_empty() {
            return Some((plugin.executable_path, plugin.manifest));
        }
        let accepts_source = plugin.manifest.supports.iter().any(|s| {
            let s_lc = s.to_ascii_lowercase();
            s_lc == format!("from:{}", source_bare) || s_lc == source_bare || s_lc == source_dotted
        });
        if accepts_source {
            return Some((plugin.executable_path, plugin.manifest));
        }
    }
    None
}

fn ensure_dotted(ext: &str) -> String {
    if ext.starts_with('.') {
        ext.to_string()
    } else {
        format!(".{}", ext)
    }
}

/// Map a plugin exit code to an error code string per §6.5.
pub fn exit_code_to_error(code: i32) -> &'static str {
    match code {
        2 => "corrupt_input",
        3 => "unsupported_feature",
        4 => "license_expired",
        5 => "protocol_mismatch",
        6 => "plugin_idle_timeout",
        _ => "plugin_failed",
    }
}

/// Resolve and run an exporter plugin for `(source_ext, target_ext)`.
/// On success, the target file exists at `out_path`.
pub fn run_exporter(
    source_path: &str,
    target_ext: &str,
    out_path: &str,
) -> Result<ExportResult, HandlerError> {
    let source_ext = Path::new(source_path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    let (exe, manifest) = resolve_exporter(&source_ext, target_ext).ok_or_else(|| {
        HandlerError::UnsupportedMode(format!(
            "no exporter plugin found for .{} → .{} — install one or see plugins/plugin-protocol.md",
            source_ext, target_ext
        ))
    })?;

    let idle = resolve_idle_timeout(&manifest, "export");
    let opts = RunOptions {
        executable_path: exe.clone(),
        arguments: vec![
            "export".to_string(),
            source_path.to_string(),
            "--out".to_string(),
            out_path.to_string(),
        ],
        idle_timeout_seconds: idle,
        capture_stdout: false,
        extra_env: Vec::new(),
    };

    let (result, _stdout_lines) = run(opts)?;

    if result.idle_timed_out {
        return Err(HandlerError::OperationFailed(format!(
            "exporter plugin '{}' produced no output for {}s — likely hung. \
             Long-running exporters should emit {{\"heartbeat\":true}} on stderr periodically.",
            manifest.name, idle
        )));
    }
    if result.exit_code != 0 {
        return Err(HandlerError::OperationFailed(format!(
            "exporter plugin '{}' failed (exit {} / {}): {}",
            manifest.name,
            result.exit_code,
            exit_code_to_error(result.exit_code),
            truncate_str(&result.stderr, 500)
        )));
    }
    if !Path::new(out_path).exists() {
        return Err(HandlerError::OperationFailed(format!(
            "exporter plugin '{}' reported success but wrote no file at '{}'",
            manifest.name, out_path
        )));
    }
    Ok(ExportResult {
        output_path: out_path.to_string(),
        plugin_name: manifest.name,
    })
}

/// Resolve the per-verb idle timeout from a plugin manifest. Prefers the
/// verb-specific entry, falls back to the manifest default, falls back to
/// 60s when neither is set. The user-side `OFFICECLI_PLUGIN_IDLE_TIMEOUT_SECONDS`
/// env var overrides both, and `0` disables the watchdog. Shared by the
/// short-lived exporter/dump-reader driver and the long-lived format-handler
/// session.
pub(crate) fn resolve_idle_timeout(manifest: &PluginManifest, verb: &str) -> u64 {
    if let Ok(v) = std::env::var("OFFICECLI_PLUGIN_IDLE_TIMEOUT_SECONDS") {
        if let Ok(n) = v.parse::<u64>() {
            return n;
        }
    }
    if let Some(spec) = &manifest.idle_timeout_seconds {
        if let Some(v) = spec.verbs.get(verb).copied() {
            return v;
        }
        return spec.default;
    }
    60
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

/// Extension trait for `std::process::Child::wait` with a timeout. The
/// std API doesn't expose this directly, so we wrap a poll loop.
trait ChildWaitTimeoutExt {
    fn wait_timeout(&mut self, dur: Duration) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl ChildWaitTimeoutExt for std::process::Child {
    fn wait_timeout(&mut self, dur: Duration) -> std::io::Result<Option<std::process::ExitStatus>> {
        // Try a non-blocking try_wait first; if not ready, sleep briefly and
        // retry. For sub-second polling intervals this is fine.
        match self.try_wait()? {
            Some(status) => Ok(Some(status)),
            None => {
                let start = Instant::now();
                while start.elapsed() < dur {
                    std::thread::sleep(Duration::from_millis(50));
                    if let Some(status) = self.try_wait()? {
                        return Ok(Some(status));
                    }
                }
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_detects_truthy_envelope() {
        assert!(is_heartbeat(r#"{"heartbeat":true}"#));
        assert!(is_heartbeat(r#"  {"heartbeat":true, "msg":"hi"}"#));
        assert!(!is_heartbeat(r#"{"heartbeat":false}"#));
        assert!(!is_heartbeat(r#"{"heartbeat":"yes"}"#));
        assert!(!is_heartbeat("plain diagnostic"));
        assert!(!is_heartbeat(r#"{"msg":"no hb"}"#));
    }

    #[test]
    fn exit_code_maps_to_protocol_strings() {
        assert_eq!(exit_code_to_error(2), "corrupt_input");
        assert_eq!(exit_code_to_error(3), "unsupported_feature");
        assert_eq!(exit_code_to_error(4), "license_expired");
        assert_eq!(exit_code_to_error(5), "protocol_mismatch");
        assert_eq!(exit_code_to_error(6), "plugin_idle_timeout");
        assert_eq!(exit_code_to_error(99), "plugin_failed");
    }

    #[test]
    fn truncate_keeps_head_and_appends_ellipsis() {
        assert_eq!(truncate_str("hello", 10), "hello");
        let s = "abcdefghij".repeat(100);
        let t = truncate_str(&s, 50);
        assert!(t.ends_with("..."));
        assert_eq!(t.len(), 53);
    }

    #[test]
    fn ensure_dotted_handles_bare_and_dotted() {
        assert_eq!(ensure_dotted("pdf"), ".pdf");
        assert_eq!(ensure_dotted(".pdf"), ".pdf");
    }
}
