use clap::{Args, Subcommand};
use handler_common::{HandlerError, OutputFormat};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

mod plugin_lint_schemas {
    include!(concat!(env!("OUT_DIR"), "/plugin_lint_schemas.rs"));
}

/// Manage and inspect installed plugins.
///
/// Implements the discovery + manifest portions of the OfficeCli Plugin
/// Protocol v1 (see source/OfficeCLI/plugins/plugin-protocol.md). The
/// full proxy / dump-reader / exporter invokers live in later iterations;
/// this module covers:
///   - manifest schema (all required + optional fields)
///   - four-root discovery: env vars, user dir, bundled dir, PATH
///   - runtime `<exe> --info` manifest fetch (with on-disk fallback)
///   - protocol-version + required-field validation
#[derive(Args)]
pub struct PluginsCommand {
    #[command(subcommand)]
    pub action: PluginsAction,
}

#[derive(Subcommand)]
pub enum PluginsAction {
    /// List plugins discoverable in the standard search paths
    List,
    /// Show detailed info about a specific plugin
    Info {
        /// Plugin name
        name: String,
    },
    /// Validate a plugin manifest at the given path or executable
    Lint {
        /// Path to plugin directory, plugin.json, or executable
        path: String,
        /// Source file used to exercise a dump-reader. Falls back to
        /// OFFICECLI_LINT_FIXTURE.
        #[arg(long)]
        fixture: Option<String>,
    },
}

pub fn handle_plugins(cmd: PluginsCommand, format: OutputFormat) -> Result<String, HandlerError> {
    match cmd.action {
        PluginsAction::List => list_plugins(format),
        PluginsAction::Info { name } => info_plugin(&name, format),
        PluginsAction::Lint { path, fixture } => lint_target(&path, fixture.as_deref(), format),
    }
}

// ─── Plugin Manifest (protocol v1) ────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct PluginManifest {
    pub name: String,
    pub version: String,
    /// Protocol major version. v1 plugins MUST set `1`.
    #[serde(default)]
    pub protocol: u32,
    pub kinds: Vec<String>,
    pub extensions: Vec<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub supports: Vec<String>,
    #[serde(default)]
    pub limits: serde_json::Value,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub idle_timeout_seconds: Option<IdleTimeoutSpec>,
    #[serde(default)]
    pub vocabulary: Option<Vocabulary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct IdleTimeoutSpec {
    #[serde(default)]
    pub default: u64,
    #[serde(default)]
    pub verbs: std::collections::BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct Vocabulary {
    #[serde(default)]
    pub addable_types: Vec<String>,
    #[serde(default)]
    pub settable_props: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub path_segments: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedPlugin {
    pub manifest: PluginManifest,
    pub executable_path: String,
    /// How the manifest was obtained — "disk" (plugin.json) or "process"
    /// (`<exe> --info`). Useful for debugging stale on-disk manifests.
    pub manifest_source: &'static str,
    pub warnings: Vec<String>,
}

// ─── Discovery ────────────────────────────────────────────────

/// Enumerate all discoverable plugins from the four standard search roots
/// described in §3 of the protocol doc:
/// 1. `$OFFICECLI_PLUGIN_<KIND>_<EXT>` env vars (absolute path to exe)
/// 2. `~/.officecli/plugins/<kind>/<ext>/plugin(.exe)`
/// 3. `<exe-dir>/plugins/<kind>/<ext>/plugin(.exe)`
/// 4. `$PATH` lookup for `officecli-<kind>-<ext>` or `officecli-<ext>`
pub(crate) fn enumerate_all() -> Vec<ResolvedPlugin> {
    let mut plugins: Vec<ResolvedPlugin> = Vec::new();
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    // 1. Environment variables (highest priority).
    discover_from_env(&mut plugins, &mut seen_names);

    // 2. User plugins directory.
    if let Some(home) = dirs_home() {
        let user_plugins_dir = format!("{}/.officecli/plugins", home);
        discover_from_dir(&user_plugins_dir, &mut plugins, &mut seen_names);
    }

    // 3. Bundled plugins directory (next to current executable).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let bundled_dir = exe_dir.join("plugins");
            if bundled_dir.exists() {
                discover_from_dir(
                    &bundled_dir.to_string_lossy(),
                    &mut plugins,
                    &mut seen_names,
                );
            }
        }
    }

    // 4. PATH lookup (lowest priority).
    discover_from_path(&mut plugins, &mut seen_names);

    plugins
}

/// Discover plugins by walking `<root>/<kind>/<ext>/` directories and looking
/// for either an executable or a `plugin.json` manifest file.
fn discover_from_dir(
    root: &str,
    plugins: &mut Vec<ResolvedPlugin>,
    seen_names: &mut std::collections::HashSet<String>,
) {
    let root_path = Path::new(root);
    if !root_path.is_dir() {
        return;
    }
    let kind_entries = match std::fs::read_dir(root_path) {
        Ok(e) => e,
        Err(_) => return,
    };
    for kind_entry in kind_entries.flatten() {
        let ext_entries = match std::fs::read_dir(kind_entry.path()) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for ext_entry in ext_entries.flatten() {
            let dir = ext_entry.path();
            if let Some(resolved) = resolve_plugin_dir(&dir) {
                if seen_names.insert(resolved.manifest.name.clone()) {
                    plugins.push(resolved);
                }
            }
        }
    }
}

/// Resolve a single plugin directory to a manifest. Prefers the runtime
/// `<exe> --info` response; falls back to reading `plugin.json` from disk.
fn resolve_plugin_dir(dir: &Path) -> Option<ResolvedPlugin> {
    let exe_path = find_plugin_exe(dir);
    let exe_str = exe_path.to_string_lossy().to_string();

    // Try the executable's `--info` first — it's the authoritative source
    // (disk plugin.json may be stale relative to the binary's actual
    // capabilities).
    if !exe_str.is_empty() {
        if let Some(manifest) = run_info(&exe_str) {
            let warnings = validate_manifest(&manifest);
            return Some(ResolvedPlugin {
                manifest,
                executable_path: exe_str,
                manifest_source: "process",
                warnings,
            });
        }
    }

    // Fall back to on-disk manifest.
    let manifest_disk = dir.join("plugin.json");
    if manifest_disk.exists() {
        if let Ok(content) = std::fs::read_to_string(&manifest_disk) {
            if let Ok(manifest) = serde_json::from_str::<PluginManifest>(&content) {
                let warnings = validate_manifest(&manifest);
                return Some(ResolvedPlugin {
                    manifest,
                    executable_path: exe_str,
                    manifest_source: "disk",
                    warnings,
                });
            }
        }
    }
    None
}

/// Discover plugins from `OFFICECLI_PLUGIN_<KIND>_<EXT>` env vars. The value
/// may be either an executable path or a plugin directory containing a
/// `plugin.json` file.
fn discover_from_env(
    plugins: &mut Vec<ResolvedPlugin>,
    seen_names: &mut std::collections::HashSet<String>,
) {
    for (key, value) in std::env::vars() {
        if !key.starts_with("OFFICECLI_PLUGIN_") {
            continue;
        }
        let path = PathBuf::from(&value);
        let (dir, exe) = if path.is_dir() {
            (path.clone(), find_plugin_exe(&path))
        } else if path.is_file() {
            (
                path.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
                path.clone(),
            )
        } else {
            continue;
        };
        let exe_str = exe.to_string_lossy().to_string();
        // Try `<exe> --info` first.
        if !exe_str.is_empty() {
            if let Some(manifest) = run_info(&exe_str) {
                if seen_names.insert(manifest.name.clone()) {
                    let warnings = validate_manifest(&manifest);
                    plugins.push(ResolvedPlugin {
                        manifest,
                        executable_path: exe_str,
                        manifest_source: "process",
                        warnings,
                    });
                }
                continue;
            }
        }
        // Fall back to plugin.json in the directory.
        let manifest_disk = dir.join("plugin.json");
        if manifest_disk.exists() {
            if let Ok(content) = std::fs::read_to_string(&manifest_disk) {
                if let Ok(manifest) = serde_json::from_str::<PluginManifest>(&content) {
                    if seen_names.insert(manifest.name.clone()) {
                        let warnings = validate_manifest(&manifest);
                        plugins.push(ResolvedPlugin {
                            manifest,
                            executable_path: exe_str,
                            manifest_source: "disk",
                            warnings,
                        });
                    }
                }
            }
        }
    }
}

/// Discover plugins by scanning `$PATH` for executables named
/// `officecli-<kind>-<ext>` or `officecli-<ext>`. Calls `<exe> --info` to
/// resolve the manifest; executables that don't respond are skipped.
fn discover_from_path(
    plugins: &mut Vec<ResolvedPlugin>,
    seen_names: &mut std::collections::HashSet<String>,
) {
    // Enumerate the candidate executable names derived from already-discovered
    // plugins is not possible here (chicken-and-egg), so we instead scan PATH
    // for binaries matching the convention.
    let path_var = match std::env::var("PATH") {
        Ok(v) => v,
        Err(_) => return,
    };
    for dir in std::env::split_paths(&path_var) {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let file_type = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            // Match `officecli-<kind>-<ext>` or `officecli-<ext>` (the latter
            // only when not also matched by the former — try the longer form
            // first).
            let is_kind_ext = name.starts_with("officecli-") && name.matches('-').count() >= 2;
            if !is_kind_ext {
                continue;
            }
            // Skip .exe suffix duplicates on Windows-style names encountered
            // on Unix (i.e. a `.exe` file sitting in PATH alongside the real
            // binary).
            let exe_path = entry.path();
            let exe_str = exe_path.to_string_lossy().to_string();
            if let Some(manifest) = run_info(&exe_str) {
                if seen_names.insert(manifest.name.clone()) {
                    let warnings = validate_manifest(&manifest);
                    plugins.push(ResolvedPlugin {
                        manifest,
                        executable_path: exe_str,
                        manifest_source: "process",
                        warnings,
                    });
                }
            }
        }
    }
}

/// Find the plugin executable inside a directory. Heuristic:
///   1. An executable named `plugin` (or `plugin.exe`)
///   2. The first executable starting with `officecli-`
fn find_plugin_exe(dir: &Path) -> PathBuf {
    if let Ok(entries) = std::fs::read_dir(dir) {
        // First pass: prefer bare `plugin` / `plugin.exe`.
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "plugin" || name == "plugin.exe" {
                let p = entry.path();
                if is_executable(&p) {
                    return p;
                }
            }
        }
        // Second pass: `officecli-*` executables.
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("officecli-") {
                let p = entry.path();
                if is_executable(&p) {
                    return p;
                }
            }
        }
    }
    PathBuf::new()
}

/// Check whether a path is executable on the current platform.
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(m) => m.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        // Best-effort on Windows: assume any file with the right name is
        // executable.
        path.exists()
    }
}

/// Spawn `<exe> --info`, capture stdout, parse the JSON manifest. Returns
/// None on any failure — discovery falls back to on-disk plugin.json.
fn run_info(exe: &str) -> Option<PluginManifest> {
    let output = Command::new(exe)
        .arg("--info")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Be tolerant of leading BOM / whitespace / trailing newline noise.
    let trimmed = stdout.trim_start_matches('\u{feff}').trim();
    // The manifest is one JSON object; if the plugin accidentally printed
    // more than one (e.g. a startup banner), try parsing only the first
    // line that begins with `{`.
    if let Ok(m) = serde_json::from_str::<PluginManifest>(trimmed) {
        return Some(m);
    }
    for line in trimmed.lines() {
        let line = line.trim();
        if line.starts_with('{') {
            if let Ok(m) = serde_json::from_str::<PluginManifest>(line) {
                return Some(m);
            }
        }
    }
    None
}

/// Validate a manifest against the protocol doc's required-field and
/// protocol-version rules. Returns human-readable warnings.
fn validate_manifest(manifest: &PluginManifest) -> Vec<String> {
    let mut warnings = Vec::new();
    if manifest.protocol != 1 {
        warnings.push(format!(
            "protocol version {} not supported by host (expected 1)",
            manifest.protocol
        ));
    }
    if manifest.kinds.is_empty() {
        warnings.push("no kinds specified".to_string());
    }
    for k in &manifest.kinds {
        if !matches!(k.as_str(), "dump-reader" | "exporter" | "format-handler") {
            warnings.push(format!("unknown kind '{}'", k));
        }
    }
    if manifest.extensions.is_empty() {
        warnings.push("no extensions specified".to_string());
    }
    // dump-reader requires `target`.
    if manifest.kinds.iter().any(|k| k == "dump-reader") {
        match &manifest.target {
            Some(t) if matches!(t.as_str(), "docx" | "xlsx" | "pptx") => {}
            Some(t) => warnings.push(format!("dump-reader target '{}' is invalid", t)),
            None => warnings.push("dump-reader requires `target` field".to_string()),
        }
    }
    // format-handler requires `vocabulary`.
    if manifest.kinds.iter().any(|k| k == "format-handler") && manifest.vocabulary.is_none() {
        warnings.push("format-handler requires `vocabulary` field".to_string());
    }
    // idle_timeout_seconds.default is mandatory per §4.2.
    if let Some(spec) = &manifest.idle_timeout_seconds {
        if spec.default == 0 {
            warnings.push("idle_timeout_seconds.default must be > 0".to_string());
        }
    } else {
        warnings.push("idle_timeout_seconds.default is missing".to_string());
    }
    // Runtime tag is purely diagnostic but spec-required.
    if manifest.runtime.is_none() {
        warnings.push("`runtime` tag is missing".to_string());
    }
    let _ = Duration::from_secs(0);
    warnings
}

fn dirs_home() -> Option<String> {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return Some(home);
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        if !home.is_empty() {
            return Some(home);
        }
    }
    None
}

// ─── Command Handlers ─────────────────────────────────────────

fn list_plugins(format: OutputFormat) -> Result<String, HandlerError> {
    let plugins = enumerate_all();

    match format {
        OutputFormat::Json => {
            let arr: Vec<serde_json::Value> = plugins.iter().map(plugin_to_json).collect();
            Ok(serde_json::to_string_pretty(&arr)
                .map_err(|e| HandlerError::OperationFailed(e.to_string()))?)
        }
        OutputFormat::Text => {
            if plugins.is_empty() {
                return Ok(
                    "No plugins installed.\n\nPlugins extend officecli to support additional \
                     formats (.doc, .hwpx, .pdf export, ...).\n\
                     See: plugins/plugin-protocol.md for installation paths."
                        .to_string(),
                );
            }

            let mut lines = Vec::new();
            lines.push(format!(
                "{:<22} {:<10} {:<9} {:<15} {:<8} {}",
                "NAME", "VERSION", "PROTOCOL", "KINDS", "SOURCE", "PATH"
            ));
            for p in &plugins {
                let kinds = p.manifest.kinds.join(",");
                lines.push(format!(
                    "{:<22} {:<10} {:<9} {:<15} {:<8} {}",
                    p.manifest.name,
                    p.manifest.version,
                    p.manifest.protocol,
                    kinds,
                    p.manifest_source,
                    p.executable_path
                ));
            }
            Ok(lines.join("\n"))
        }
    }
}

fn info_plugin(name: &str, format: OutputFormat) -> Result<String, HandlerError> {
    let plugins = enumerate_all();
    let plugin = plugins
        .iter()
        .find(|p| p.manifest.name == name)
        .ok_or_else(|| HandlerError::PathNotFound(format!("plugin '{}' not found", name)))?;

    match format {
        OutputFormat::Json => {
            let mut obj = plugin_to_json(plugin);
            if let serde_json::Value::Object(map) = &mut obj {
                if let Some(spec) = &plugin.manifest.idle_timeout_seconds {
                    let mut verbs = serde_json::Map::new();
                    for (k, v) in &spec.verbs {
                        verbs.insert(k.clone(), serde_json::Value::Number((*v).into()));
                    }
                    let mut idle = serde_json::Map::new();
                    idle.insert(
                        "default".into(),
                        serde_json::Value::Number(spec.default.into()),
                    );
                    idle.insert("verbs".into(), serde_json::Value::Object(verbs));
                    map.insert(
                        "idle_timeout_seconds".into(),
                        serde_json::Value::Object(idle),
                    );
                }
            }
            Ok(serde_json::to_string_pretty(&obj)
                .map_err(|e| HandlerError::OperationFailed(e.to_string()))?)
        }
        OutputFormat::Text => {
            let m = &plugin.manifest;
            let mut lines = Vec::new();
            lines.push(format!("Name:        {}", m.name));
            lines.push(format!("Version:     {}", m.version));
            lines.push(format!("Protocol:    {}", m.protocol));
            if let Some(rt) = &m.runtime {
                lines.push(format!("Runtime:     {}", rt));
            }
            lines.push(format!("Kinds:       {}", m.kinds.join(", ")));
            lines.push(format!("Extensions:  {}", m.extensions.join(", ")));
            if let Some(t) = &m.target {
                lines.push(format!("Target:      {}", t));
            }
            if let Some(d) = &m.description {
                lines.push(format!("Description: {}", d));
            }
            if let Some(t) = &m.tier {
                lines.push(format!("Tier:        {}", t));
            }
            if !m.supports.is_empty() {
                lines.push(format!("Supports:    {}", m.supports.join(", ")));
            }
            if let Some(h) = &m.homepage {
                lines.push(format!("Homepage:    {}", h));
            }
            if let Some(l) = &m.license {
                lines.push(format!("License:     {}", l));
            }
            if let Some(spec) = &m.idle_timeout_seconds {
                lines.push(format!("Idle timeout: default={}s", spec.default));
                for (k, v) in &spec.verbs {
                    lines.push(format!("              {}: {}s", k, v));
                }
            }
            lines.push(format!("Path:        {}", plugin.executable_path));
            lines.push(format!("Manifest:    via {}", plugin.manifest_source));
            if !plugin.warnings.is_empty() {
                lines.push("Warnings:".to_string());
                for w in &plugin.warnings {
                    lines.push(format!("  - {}", w));
                }
            }
            Ok(lines.join("\n"))
        }
    }
}

fn lint_target(
    path: &str,
    fixture: Option<&str>,
    format: OutputFormat,
) -> Result<String, HandlerError> {
    let p = PathBuf::from(path);
    let resolved: Option<ResolvedPlugin> = if p.is_file() {
        // Could be plugin.json or an executable.
        let file_name = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if file_name == "plugin.json" {
            lint_manifest_file(&p).ok()
        } else if is_executable(&p) {
            let exe_str = p.to_string_lossy().to_string();
            run_info(&exe_str).map(|m| ResolvedPlugin {
                manifest: m,
                executable_path: exe_str,
                manifest_source: "process",
                warnings: Vec::new(),
            })
        } else {
            None
        }
    } else if p.is_dir() {
        resolve_plugin_dir(&p)
    } else {
        None
    };
    let resolved = resolved.ok_or_else(|| {
        HandlerError::OperationFailed(format!("could not resolve a plugin at '{}'", path))
    })?;
    let warnings = validate_manifest(&resolved.manifest);
    if !resolved
        .manifest
        .kinds
        .iter()
        .any(|kind| kind == "dump-reader")
    {
        return Err(HandlerError::UnsupportedMode(format!(
            "Plugin '{}' is not a dump-reader; lint only applies to dump-reader plugins",
            resolved.manifest.name
        )));
    }
    let (fixture, findings, batch_items) = {
        let fixture = fixture.map(str::to_string).or_else(|| std::env::var("OFFICECLI_LINT_FIXTURE").ok())
            .ok_or_else(|| HandlerError::InvalidArgument(format!(
                "No lint fixture available for plugin '{}'. Pass --fixture <path>, or set OFFICECLI_LINT_FIXTURE.",
                resolved.manifest.name
            )))?;
        if !Path::new(&fixture).is_file() {
            return Err(HandlerError::InvalidArgument(format!(
                "lint fixture '{}' does not exist",
                fixture
            )));
        }
        let executable = if resolved.executable_path.is_empty() {
            return Err(HandlerError::InvalidArgument(
                "dump-reader lint requires a plugin executable, not only plugin.json".to_string(),
            ));
        } else {
            &resolved.executable_path
        };
        let output = Command::new(executable)
            .arg("dump")
            .arg(&fixture)
            .output()
            .map_err(|error| {
                HandlerError::OperationFailed(format!(
                    "failed to run dump-reader '{}': {}",
                    resolved.manifest.name, error
                ))
            })?;
        if !output.status.success() {
            return Err(HandlerError::OperationFailed(format!(
                "dump-reader '{}' failed (exit {}): {}",
                resolved.manifest.name,
                output
                    .status
                    .code()
                    .map_or_else(|| "signal".to_string(), |code| code.to_string()),
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(500)
                    .collect::<String>()
            )));
        }
        let items = parse_lint_jsonl(&resolved.manifest.name, &output.stdout)?;
        let findings = lint_dump_items(&resolved.manifest, &items);
        let batch_items = items.len();
        (Some(fixture), findings, Some(batch_items))
    };
    if !findings.is_empty() {
        return Err(HandlerError::OperationFailed(format!(
            "dump-reader '{}' emitted {} unknown property/properties; run with --json after correcting its schema vocabulary",
            resolved.manifest.name,
            findings.len()
        )));
    }

    match format {
        OutputFormat::Json => {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "name".into(),
                serde_json::Value::String(resolved.manifest.name.clone()),
            );
            if let Some(fixture) = &fixture {
                obj.insert("fixture".into(), serde_json::json!(fixture));
            }
            if let Some(count) = batch_items {
                obj.insert("batch_items".into(), serde_json::json!(count));
            }
            obj.insert(
                "unknown_prop_count".into(),
                serde_json::json!(findings.len()),
            );
            obj.insert("unknown_props".into(), serde_json::json!(findings));
            obj.insert(
                "version".into(),
                serde_json::Value::String(resolved.manifest.version.clone()),
            );
            obj.insert(
                "protocol".into(),
                serde_json::Value::Number(resolved.manifest.protocol.into()),
            );
            obj.insert(
                "warnings".into(),
                serde_json::Value::Array(
                    warnings
                        .iter()
                        .map(|w| serde_json::Value::String(w.clone()))
                        .collect(),
                ),
            );
            obj.insert(
                "manifest_source".into(),
                serde_json::Value::String(resolved.manifest_source.into()),
            );
            Ok(
                serde_json::to_string_pretty(&serde_json::Value::Object(obj))
                    .map_err(|e| HandlerError::OperationFailed(e.to_string()))?,
            )
        }
        OutputFormat::Text => {
            let mut lines = Vec::new();
            lines.push(format!(
                "OK: {} v{} (protocol {}, via {})",
                resolved.manifest.name,
                resolved.manifest.version,
                resolved.manifest.protocol,
                resolved.manifest_source,
            ));
            if warnings.is_empty() {
                lines.push("No warnings.".to_string());
            } else {
                lines.push("Warnings:".to_string());
                for w in &warnings {
                    lines.push(format!("  - {}", w));
                }
            }
            if let Some(fixture) = fixture {
                lines.push(format!("Fixture: {}", fixture));
                lines.push(format!("Batch items: {}", batch_items.unwrap_or(0)));
                if findings.is_empty() {
                    lines.push(format!(
                        "Result: OK — every emitted prop is declared in the {} schema.",
                        resolved.manifest.target.as_deref().unwrap_or("docx")
                    ));
                } else {
                    lines.push(format!("Result: {} unknown prop(s):", findings.len()));
                    for finding in &findings {
                        lines.push(format!(
                            "  [#{}] type={} prop=\"{}\"",
                            finding.index, finding.element, finding.prop
                        ));
                    }
                }
            }
            Ok(lines.join("\n"))
        }
    }
}

#[derive(Serialize)]
struct LintFinding {
    index: usize,
    element: String,
    prop: String,
}

fn parse_lint_jsonl(name: &str, stdout: &[u8]) -> Result<Vec<super::batch::BatchOp>, HandlerError> {
    let text = String::from_utf8_lossy(stdout);
    let mut items = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            return Err(HandlerError::OperationFailed(format!(
                "dump-reader plugin '{}' emitted a JSON array; protocol v1 requires JSONL",
                name
            )));
        }
        let item = serde_json::from_str(line).map_err(|error| {
            HandlerError::OperationFailed(format!(
                "dump-reader plugin '{}' emitted invalid JSON at line #{}: {}",
                name,
                index + 1,
                error
            ))
        })?;
        items.push(item);
    }
    Ok(items)
}

fn lint_dump_items(manifest: &PluginManifest, items: &[super::batch::BatchOp]) -> Vec<LintFinding> {
    let target = manifest.target.as_deref().unwrap_or("docx");
    let mut findings = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let verb = item.command.to_ascii_lowercase();
        if verb != "add" && verb != "set" {
            continue;
        }
        let Some(props) = item
            .params
            .get("props")
            .or_else(|| item.params.get("properties"))
            .and_then(|value| value.as_object())
        else {
            continue;
        };
        let element = if verb == "add" {
            item.params
                .get("type")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string()
        } else {
            item.params
                .get("path")
                .and_then(|value| value.as_str())
                .and_then(infer_element_from_path)
                .unwrap_or("")
                .to_string()
        };
        if element.is_empty() {
            continue;
        }
        let Some(contents) = plugin_lint_schemas::schema_json(target, &element) else {
            if verb == "add" {
                findings.push(LintFinding {
                    index,
                    element,
                    prop: "<unknown_type>".to_string(),
                });
            }
            continue;
        };
        let known = serde_json::from_str::<serde_json::Value>(contents)
            .ok()
            .and_then(|value| {
                value
                    .get("properties")
                    .and_then(|value| value.as_object())
                    .cloned()
            })
            .unwrap_or_default();
        for key in props.keys() {
            if !known.contains_key(key)
                && !known.values().any(|property| {
                    property
                        .get("aliases")
                        .and_then(|value| value.as_array())
                        .is_some_and(|aliases| {
                            aliases.iter().any(|alias| alias.as_str() == Some(key))
                        })
                })
            {
                findings.push(LintFinding {
                    index,
                    element: element.clone(),
                    prop: key.clone(),
                });
            }
        }
    }
    findings
}

fn infer_element_from_path(path: &str) -> Option<&str> {
    let segment = path.rsplit('/').next()?;
    let element = segment.split('[').next().unwrap_or(segment);
    (!element.is_empty()).then_some(element)
}

/// Read a `plugin.json` file and parse it. Does not consult the runtime
/// executable.
fn lint_manifest_file(path: &Path) -> Result<ResolvedPlugin, HandlerError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        HandlerError::OperationFailed(format!("failed to read '{}': {}", path.display(), e))
    })?;
    let manifest: PluginManifest = serde_json::from_str(&content)
        .map_err(|e| HandlerError::OperationFailed(format!("invalid plugin manifest: {}", e)))?;
    Ok(ResolvedPlugin {
        manifest,
        executable_path: String::new(),
        manifest_source: "disk",
        warnings: Vec::new(),
    })
}

/// Serialize a ResolvedPlugin's manifest + provenance into a JSON object.
fn plugin_to_json(p: &ResolvedPlugin) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "name".into(),
        serde_json::Value::String(p.manifest.name.clone()),
    );
    obj.insert(
        "version".into(),
        serde_json::Value::String(p.manifest.version.clone()),
    );
    obj.insert(
        "protocol".into(),
        serde_json::Value::Number(p.manifest.protocol.into()),
    );
    obj.insert(
        "kinds".into(),
        serde_json::Value::Array(
            p.manifest
                .kinds
                .iter()
                .cloned()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    obj.insert(
        "extensions".into(),
        serde_json::Value::Array(
            p.manifest
                .extensions
                .iter()
                .cloned()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    if let Some(rt) = &p.manifest.runtime {
        obj.insert("runtime".into(), serde_json::Value::String(rt.clone()));
    }
    if let Some(t) = &p.manifest.target {
        obj.insert("target".into(), serde_json::Value::String(t.clone()));
    }
    if let Some(d) = &p.manifest.description {
        obj.insert("description".into(), serde_json::Value::String(d.clone()));
    }
    if let Some(t) = &p.manifest.tier {
        obj.insert("tier".into(), serde_json::Value::String(t.clone()));
    }
    if !p.manifest.supports.is_empty() {
        obj.insert(
            "supports".into(),
            serde_json::Value::Array(
                p.manifest
                    .supports
                    .iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
    }
    obj.insert(
        "path".into(),
        serde_json::Value::String(p.executable_path.clone()),
    );
    obj.insert(
        "manifest_source".into(),
        serde_json::Value::String(p.manifest_source.into()),
    );
    if !p.warnings.is_empty() {
        obj.insert(
            "warnings".into(),
            serde_json::Value::Array(
                p.warnings
                    .iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
    }
    serde_json::Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_dump_reader_example() {
        let json = r#"{
            "name": "officecli-doc",
            "version": "1.0.0",
            "protocol": 1,
            "kinds": ["dump-reader"],
            "extensions": [".doc"],
            "target": "docx",
            "runtime": "dotnet",
            "idle_timeout_seconds": { "default": 60, "verbs": { "dump": 30 } },
            "tier": "basic",
            "supports": ["paragraphs"]
        }"#;
        let m: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.name, "officecli-doc");
        assert_eq!(m.protocol, 1);
        assert_eq!(m.target.as_deref(), Some("docx"));
        assert_eq!(m.idle_timeout_seconds.unwrap().verbs.get("dump"), Some(&30));
        assert_eq!(m.tier.as_deref(), Some("basic"));
    }

    #[test]
    fn manifest_parses_format_handler_example() {
        let json = r#"{
            "name": "officecli-hwpx",
            "version": "0.9.0",
            "protocol": 1,
            "kinds": ["format-handler"],
            "extensions": [".hwpx"],
            "runtime": "dotnet",
            "idle_timeout_seconds": { "default": 30, "verbs": { "save": 60 } },
            "vocabulary": {
                "addable_types": ["paragraph"],
                "settable_props": {},
                "path_segments": []
            }
        }"#;
        let m: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.kinds, vec!["format-handler".to_string()]);
        let voc = m.vocabulary.unwrap();
        assert_eq!(voc.addable_types, vec!["paragraph".to_string()]);
    }

    #[test]
    fn validate_flags_protocol_mismatch() {
        let mut m = PluginManifest {
            name: "x".into(),
            version: "0.0.1".into(),
            protocol: 2,
            kinds: vec!["dump-reader".into()],
            extensions: vec![".doc".into()],
            runtime: Some("rust".into()),
            target: Some("docx".into()),
            idle_timeout_seconds: Some(IdleTimeoutSpec {
                default: 30,
                verbs: Default::default(),
            }),
            ..Default::default()
        };
        let w = validate_manifest(&m);
        assert!(w.iter().any(|s| s.contains("protocol version 2")));

        m.protocol = 1;
        let w = validate_manifest(&m);
        assert!(!w.iter().any(|s| s.contains("protocol version")));
    }

    #[test]
    fn validate_requires_target_for_dump_reader() {
        let m = PluginManifest {
            name: "x".into(),
            version: "0.0.1".into(),
            protocol: 1,
            kinds: vec!["dump-reader".into()],
            extensions: vec![".doc".into()],
            runtime: Some("rust".into()),
            target: None,
            idle_timeout_seconds: Some(IdleTimeoutSpec {
                default: 30,
                verbs: Default::default(),
            }),
            ..Default::default()
        };
        let w = validate_manifest(&m);
        assert!(w
            .iter()
            .any(|s| s.contains("dump-reader requires `target`")));
    }

    #[test]
    fn validate_requires_vocabulary_for_format_handler() {
        let m = PluginManifest {
            name: "x".into(),
            version: "0.0.1".into(),
            protocol: 1,
            kinds: vec!["format-handler".into()],
            extensions: vec![".hwpx".into()],
            runtime: Some("rust".into()),
            idle_timeout_seconds: Some(IdleTimeoutSpec {
                default: 30,
                verbs: Default::default(),
            }),
            ..Default::default()
        };
        let w = validate_manifest(&m);
        assert!(w
            .iter()
            .any(|s| s.contains("format-handler requires `vocabulary`")));
    }

    #[test]
    fn validate_rejects_zero_idle_default() {
        let m = PluginManifest {
            name: "x".into(),
            version: "0.0.1".into(),
            protocol: 1,
            kinds: vec!["exporter".into()],
            extensions: vec![".pdf".into()],
            runtime: Some("rust".into()),
            idle_timeout_seconds: Some(IdleTimeoutSpec {
                default: 0,
                verbs: Default::default(),
            }),
            ..Default::default()
        };
        let w = validate_manifest(&m);
        assert!(w.iter().any(|s| s.contains("default must be > 0")));
    }

    #[test]
    fn validate_flags_unknown_kind() {
        let m = PluginManifest {
            name: "x".into(),
            version: "0.0.1".into(),
            protocol: 1,
            kinds: vec!["magic-reader".into()],
            extensions: vec![".foo".into()],
            runtime: Some("rust".into()),
            idle_timeout_seconds: Some(IdleTimeoutSpec {
                default: 30,
                verbs: Default::default(),
            }),
            ..Default::default()
        };
        let w = validate_manifest(&m);
        assert!(w.iter().any(|s| s.contains("unknown kind 'magic-reader'")));
    }

    #[test]
    fn resolve_plugin_dir_returns_none_when_no_manifest_or_exe() {
        let tmpdir = std::env::temp_dir().join(format!(
            "oc-plugin-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmpdir).unwrap();
        assert!(resolve_plugin_dir(&tmpdir).is_none());
        std::fs::remove_dir_all(&tmpdir).ok();
    }

    #[test]
    fn dump_lint_reports_unknown_property() {
        let manifest = PluginManifest {
            target: Some("xlsx".into()),
            ..Default::default()
        };
        let items = vec![super::super::batch::BatchOp {
            command: "add".into(),
            params: serde_json::from_value(serde_json::json!({
                "type": "cell",
                "props": { "definitely-not-a-cell-property": "x" }
            }))
            .unwrap(),
        }];
        let findings = lint_dump_items(&manifest, &items);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].prop, "definitely-not-a-cell-property");
    }
}
