//! CLI smoke tests — mirrors the C# CI smoke test pattern.
//!
//! These tests run the `officecli` binary and verify it produces correct
//! output for the core command pipeline: create → add → get → view → close.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;

/// Helper: get the officecli binary under test.
fn officecli() -> Command {
    Command::cargo_bin("officecli").unwrap()
}

/// Helper: create a temp dir for test files.
fn temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// Helper: workspace root for sample files.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// ═══════════════════════════════════════════════════════════════════════
// Basic CLI
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_version() {
    officecli().arg("--version").assert().success();
}

#[test]
fn test_output_schema_crc_is_stable_lowercase_hex() {
    let first = officecli()
        .arg("--output-schema-crc")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let second = officecli()
        .arg("--output-schema-crc")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    assert_eq!(first, second);
    let output = String::from_utf8(first).unwrap();
    let fingerprint = output.trim_end();
    assert_eq!(fingerprint.len(), 8);
    assert!(fingerprint
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
}

#[test]
fn test_load_skill_catalog_and_content_are_read_only() {
    let home = temp_dir();

    officecli()
        .arg("load_skill")
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("# officecli skills"))
        .stdout(predicate::str::contains("## pptx"))
        .stdout(predicate::str::contains("## word-form"));

    officecli()
        .args(["load_skill", "pptx"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("# OfficeCLI PPTX Skill"))
        .stdout(predicate::str::contains("## Setup").not());

    assert_eq!(
        std::fs::read_dir(home.path()).unwrap().count(),
        0,
        "load_skill must not install files under HOME"
    );
}

#[test]
fn test_load_skill_reads_reference_and_rejects_unsafe_paths() {
    officecli()
        .args([
            "load_skill",
            "morph-ppt",
            "--path",
            "reference/decision-rules.md",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("# PPT Planner"));

    officecli()
        .args(["load_skill", "morph-ppt", "--path", "../SKILL.md"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid skill file path"));

    officecli()
        .args([
            "load_skill",
            "morph-ppt",
            "--path",
            "reference/styles/dark--premium-navy/template.pptx",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("binary asset"));
}

#[test]
fn test_help() {
    officecli()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Commands:"))
        .stdout(predicate::str::contains("view"))
        .stdout(predicate::str::contains("get"))
        .stdout(predicate::str::contains("set"))
        .stdout(predicate::str::contains("add"))
        .stdout(predicate::str::contains("create"))
        .stdout(predicate::str::contains("help"));
}

#[test]
fn test_help_schema() {
    officecli()
        .args(["help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("docx"))
        .stdout(predicate::str::contains("xlsx"))
        .stdout(predicate::str::contains("pptx"));
}

#[test]
fn test_help_format_detail() {
    officecli()
        .args(["help", "xlsx"])
        .assert()
        .success()
        .stdout(predicate::str::contains("cell"))
        .stdout(predicate::str::contains("sheet"))
        .stdout(predicate::str::contains("formula"));
}

#[test]
fn test_pptx_note_help_alias() {
    officecli()
        .args(["help", "pptx", "note"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pptx:notes"))
        .stdout(predicate::str::contains("Speaker notes"));
}

#[test]
fn test_info() {
    officecli()
        .args(["info"])
        .assert()
        .success()
        .stdout(predicate::str::contains("OfficeCLI"));
}

#[test]
fn test_config_csharp_compatibility_surface() {
    let home = temp_dir();
    officecli()
        .args(["config", "autoUpdate"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout("true\n");
    officecli()
        .args(["config", "autoUpdate", "false"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout("autoUpdate = false\n");
    officecli()
        .args(["config", "autoUpdate"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout("false\n");
    officecli()
        .args(["config", "log", "clear"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout("Log cleared.\n");
    officecli()
        .args(["config", "other"])
        .env("HOME", home.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Available: autoUpdate, log, log clear",
        ));
}

#[test]
fn test_mcp_registration_lifecycle() {
    let home = temp_dir();
    officecli()
        .args(["mcp", "cursor"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Registered officecli MCP in cursor",
        ));
    officecli()
        .args(["mcp", "list"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("cursor: registered"));
    officecli()
        .args(["mcp", "uninstall", "cursor"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Removed officecli MCP from cursor",
        ));
    let cursor_config = std::fs::read_to_string(home.path().join(".cursor/mcp.json")).unwrap();
    assert!(
        !cursor_config.contains("mcpServers"),
        "uninstall must not leave an empty mcpServers object"
    );
    officecli()
        .args(["mcp", "unknown"])
        .env("HOME", home.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("Supported: lms"));
}

// ═══════════════════════════════════════════════════════════════════════
// Create — all three formats
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_create_docx() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_create.docx");
    let path_str = path.to_string_lossy().to_string();

    officecli().args(["create", &path_str]).assert().success();
    assert!(path.exists(), "created file should exist");
}

#[test]
fn test_create_minimal_docx_omits_style_baseline() {
    let tmp = temp_dir();
    let path = tmp.path().join("minimal.docx");
    let p = path.to_string_lossy().to_string();

    officecli()
        .args(["create", &p, "--minimal"])
        .assert()
        .success();
    let package = oxml::OxmlPackage::open(&p, false).unwrap();
    assert!(package.read_part_xml("word/styles.xml").is_err());
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_create_docx_accepts_csharp_locale_fonts_and_rtl_defaults() {
    let tmp = temp_dir();
    let path = tmp.path().join("localized.docx");
    let p = path.to_string_lossy().to_string();
    officecli()
        .args(["create", &p, "--locale", "ar-SA"])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "/styles"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Arabic Typesetting"))
        .stdout(predicate::str::contains("w:bidi"));
    officecli()
        .args(["raw", &p, "/document"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<w:bidi"));
}

#[test]
fn test_create_locale_applies_theme_fonts_to_xlsx_and_pptx() {
    let tmp = temp_dir();
    for extension in ["xlsx", "pptx"] {
        let path = tmp.path().join(format!("localized.{extension}"));
        let p = path.to_string_lossy().to_string();
        officecli()
            .args(["create", &p, "--locale", "zh-CN"])
            .assert()
            .success();
        officecli()
            .args(["raw", &p, "/theme"])
            .assert()
            .success()
            .stdout(predicate::str::contains("等线"));
    }
}

#[test]
fn test_create_infers_non_western_locale_from_unix_environment() {
    let tmp = temp_dir();
    let path = tmp.path().join("inferred.docx");
    let p = path.to_string_lossy().to_string();
    officecli()
        .args(["create", &p])
        .env("LC_ALL", "ja_JP.UTF-8")
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "/styles"])
        .assert()
        .success()
        .stdout(predicate::str::contains("游明朝"));
}

#[test]
fn test_create_xlsx() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_create.xlsx");
    let path_str = path.to_string_lossy().to_string();

    officecli().args(["create", &path_str]).assert().success();
    assert!(path.exists(), "created file should exist");
}

#[test]
fn test_import_accepts_csharp_positional_source_file() {
    let tmp = temp_dir();
    let workbook = tmp.path().join("import_target.xlsx");
    let source = tmp.path().join("input.csv");
    let workbook_path = workbook.to_string_lossy().to_string();
    let source_path = source.to_string_lossy().to_string();
    std::fs::write(&source, "Name,Score\nAda,100\n").unwrap();

    officecli()
        .args(["create", &workbook_path])
        .assert()
        .success();
    officecli()
        .args([
            "import",
            &workbook_path,
            "/Sheet1",
            &source_path,
            "--header",
        ])
        .assert()
        .success();
    officecli()
        .args(["get", &workbook_path, "/Sheet1/A2"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Ada"));
}

#[test]
fn test_merge_accepts_csharp_template_output_syntax() {
    let tmp = temp_dir();
    let template = tmp.path().join("merge_template.docx");
    let output = tmp.path().join("merge_output.docx");
    let template_path = template.to_string_lossy().to_string();
    let output_path = output.to_string_lossy().to_string();

    officecli()
        .args(["create", &template_path])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &template_path,
            "/body",
            "--type",
            "paragraph",
            "--prop",
            "text=Hello {{name}}",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "merge",
            &template_path,
            &output_path,
            "--data",
            r#"{"name":"Ada"}"#,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Merged:"));
    officecli()
        .args(["view", &template_path, "-m", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello {{name}}"));
    officecli()
        .args(["view", &output_path, "-m", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello Ada"));
}

#[test]
fn test_create_pptx() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_create.pptx");
    let path_str = path.to_string_lossy().to_string();

    officecli().args(["create", &path_str]).assert().success();
    assert!(path.exists(), "created file should exist");
}

// ═══════════════════════════════════════════════════════════════════════
// View — various modes (docx)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_view_docx_text() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_view.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["view", &p, "-m", "text"])
        .assert()
        .success();
}

#[test]
fn test_view_docx_outline() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_outline.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["view", &p, "-m", "outline"])
        .assert()
        .success();
}

#[test]
fn test_view_docx_stats() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_stats.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["view", &p, "-m", "stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Paragraphs"));
}

#[test]
fn test_view_docx_annotated() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_annotated.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["view", &p, "-m", "annotated"])
        .assert()
        .success();
}

#[test]
fn test_view_docx_issues() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_issues.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["view", &p, "-m", "issues"])
        .assert()
        .success();
}

#[test]
fn test_view_docx_forms() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_forms.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    // Blank docx has no form fields — should report "No form fields"
    officecli()
        .args(["view", &p, "-m", "forms"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No form fields"));
}

// ═══════════════════════════════════════════════════════════════════════
// View — stats for xlsx and pptx
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_view_xlsx_stats() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_stats.xlsx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["view", &p, "-m", "stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Sheets"));
}

#[test]
fn test_view_pptx_stats() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_stats.pptx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["view", &p, "-m", "stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Slides"));
}

// ═══════════════════════════════════════════════════════════════════════
// Add + Get — mirrors the C# CI smoke test exactly
//   C# CI: create → add /body --type paragraph --prop text="Hello from CI"
//          → get /body/p[1] → close
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_add_and_get_paragraph() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_add.docx");
    let p = path.to_string_lossy().to_string();

    // Create blank docx (contains a default empty p[1])
    officecli().args(["create", &p]).assert().success();

    // Add a paragraph — new paragraph becomes p[2] since blank docx has p[1]
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=Hello from test",
        ])
        .assert()
        .success();
    // Get the newly added paragraph at p[2]
    officecli()
        .args(["get", &p, "/body/p[2]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello from test"));
}

#[test]
fn test_add_accepts_csharp_parent_type_and_prop_syntax() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_add_csharp_syntax.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "/body",
            "--type",
            "paragraph",
            "--prop",
            "text=C# command spelling",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Created: /body/p[2]"));
    officecli()
        .args(["get", &p, "/body/p[2]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("C# command spelling"));

    officecli()
        .args([
            "add",
            &p,
            "/body",
            "--parent",
            "/body",
            "--type",
            "paragraph",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "either positional parent or --parent",
        ));
}

#[test]
fn test_add_accepts_csharp_position_flags() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_add_csharp_positions.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=tail",
        ])
        .assert()
        .success();

    // `--index` is zero-based, as in the C# CLI. It inserts before the
    // document's initial blank paragraph at index 0.
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--index",
            "0",
            "--properties",
            "text=head",
        ])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/body/p[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("head"));

    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--before",
            "/body/p[3]",
            "--properties",
            "text=before-tail",
        ])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/body/p[3]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("before-tail"));

    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--index",
            "0",
            "--before",
            "/body/p[1]",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("mutually exclusive"));
}

#[test]
fn test_docx_table_before_anchor_inserts_at_requested_position() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_table_before_anchor.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "/body",
            "--type",
            "paragraph",
            "--prop",
            "text=anchor",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "/body",
            "--type",
            "table",
            "--before",
            "/body/p[2]",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Created: /body/tbl[1]"));
    officecli()
        .args(["get", &p, "/body/tbl[1]"])
        .assert()
        .success();

    officecli()
        .args(["add", &p, "/body/tbl[1]", "--type", "row"])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "/body/tbl[1]",
            "--type",
            "row",
            "--before",
            "/body/tbl[1]/tr[2]",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Created: /body/tbl[1]/tr[2]"));

    officecli()
        .args([
            "add",
            &p,
            "/body/tbl[1]/tr[2]",
            "--type",
            "cell",
            "--prop",
            "text=tail",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "/body/tbl[1]/tr[2]",
            "--type",
            "cell",
            "--before",
            "/body/tbl[1]/tr[2]/tc[2]",
            "--prop",
            "text=head",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Created: /body/tbl[1]/tr[2]/tc[2]",
        ));
    officecli()
        .args(["get", &p, "/body/tbl[1]/tr[2]/tc[2]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("head"));
}

#[test]
fn test_docx_tabstop_uses_flat_csharp_compatible_get_path() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_tabstop.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[1]",
            "--type-name",
            "tabstop",
            "--properties",
            "pos=6cm",
            "--properties",
            "val=right",
            "--properties",
            "leader=dot",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("/body/p[1]/tab[1]"));

    officecli()
        .args(["get", &p, "/body/p[1]/tab[1]", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"pos\": \"3402\""))
        .stdout(predicate::str::contains("\"val\": \"right\""))
        .stdout(predicate::str::contains("\"leader\": \"dot\""));

    officecli()
        .args(["set", &p, "/body/p[1]/tab[1]", "leader=underscore"])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/body/p[1]/tab[1]", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"leader\": \"underscore\""));
    officecli()
        .args(["remove", &p, "/body/p[1]/tab[1]"])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/body/p[1]/tab[1]", "--json"])
        .assert()
        .failure();
}

#[test]
fn test_docx_permission_range_add_get_remove() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_permission_range.docx");
    let p = path.to_string_lossy().to_string();
    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[1]",
            "--type-name",
            "permStart",
            "--properties",
            "id=7",
            "--properties",
            "ed=user@example.com",
            "--properties",
            "colFirst=0",
            "--properties",
            "colLast=2",
        ])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/body/p[1]/permStart[1]", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("user@example.com"))
        .stdout(predicate::str::contains("\"colLast\": \"2\""));
    officecli()
        .args(["remove", &p, "/body/p[1]/permStart[1]"])
        .assert()
        .success();
    officecli()
        .args(["validate", &p, "--json"])
        .assert()
        .success();
}

#[test]
fn test_docx_numbering_definition_package_and_stable_get_paths() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_numbering.docx");
    let p = path.to_string_lossy().to_string();
    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/numbering",
            "--type-name",
            "abstractNum",
            "--properties",
            "id=3",
            "--properties",
            "format=decimal",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/numbering",
            "--type-name",
            "num",
            "--properties",
            "id=9",
            "--properties",
            "abstractNumId=3",
        ])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/numbering", "--depth", "1", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"levelCount\": 9"))
        .stdout(predicate::str::contains("\"abstractNumId\": \"3\""));
    officecli()
        .args(["get", &p, "/numbering/num[@id=9]", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"numId\": \"9\""));
    officecli()
        .args(["validate", &p, "--json"])
        .assert()
        .success();
}

#[test]
fn test_docx_num_reference_update_rejects_dangling_template() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_numbering_set.docx");
    let p = path.to_string_lossy().to_string();
    officecli().args(["create", &p]).assert().success();
    for id in ["3", "4"] {
        officecli()
            .args([
                "add",
                &p,
                "--parent",
                "/numbering",
                "--type-name",
                "abstractNum",
                "--properties",
                &format!("id={id}"),
            ])
            .assert()
            .success();
    }
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/numbering",
            "--type-name",
            "num",
            "--properties",
            "id=9",
            "--properties",
            "abstractNumId=3",
        ])
        .assert()
        .success();
    officecli()
        .args(["set", &p, "/numbering/num[@id=9]", "abstractNumId=4"])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/numbering/num[@id=9]", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"abstractNumId\": \"4\""));
    officecli()
        .args(["set", &p, "/numbering/num[@id=9]", "abstractNumId=99"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("abstractNumId=99 not found"));
    officecli()
        .args(["validate", &p, "--json"])
        .assert()
        .success();
}

#[test]
fn test_docx_num_format_auto_creates_numbering_template() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_num_auto_template.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/numbering",
            "--type-name",
            "num",
            "--properties",
            "id=9",
            "--properties",
            "format=bullet",
            "--properties",
            "text=•",
            "--properties",
            "type=single",
        ])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/numbering", "--depth", "1", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"abstractNumId\": \"0\""))
        .stdout(predicate::str::contains("\"numId\": \"9\""));
    officecli()
        .args([
            "get",
            &p,
            "/numbering/abstractNum[@id=0]/level[0]",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"format\": \"bullet\""))
        .stdout(predicate::str::contains("\"text\": \"•\""));
    officecli()
        .args(["validate", &p, "--json"])
        .assert()
        .success();
}

#[test]
fn test_docx_abstract_num_level_properties_seed_all_template_levels() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_abstract_num_level_properties.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/numbering",
            "--type-name",
            "abstractNum",
            "--properties",
            "id=6",
            "format=decimal",
            "start=3",
            "indent=900",
            "level1.numFmt=upperRoman",
            "level1.lvlText=%1.%2)",
            "level1.start=4",
            "level1.suff=space",
            "level1.jc=center",
            "level1.indent=1800",
            "level1.hanging=480",
            "level1.direction=rtl",
            "level1.font=Symbol",
            "level1.size=12pt",
            "level1.color=#336699",
            "level1.bold=true",
            "level1.italic=true",
        ])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "word/numbering.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "<w:lvl w:ilvl=\"0\"><w:start w:val=\"3\"/><w:numFmt w:val=\"decimal\"/>",
        ))
        .stdout(predicate::str::contains("<w:ind w:left=\"900\" w:hanging=\"360\"/>"))
        .stdout(predicate::str::contains(
            "<w:lvl w:ilvl=\"1\"><w:start w:val=\"4\"/><w:numFmt w:val=\"upperRoman\"/><w:suff w:val=\"space\"/><w:lvlText w:val=\"%1.%2)\"/><w:lvlJc w:val=\"center\"/>",
        ))
        .stdout(predicate::str::contains("<w:bidi/><w:ind w:left=\"1800\" w:hanging=\"480\"/>"))
        .stdout(predicate::str::contains("w:ascii=\"Symbol\""))
        .stdout(predicate::str::contains("<w:sz w:val=\"24\"/>"))
        .stdout(predicate::str::contains("<w:color w:val=\"336699\"/>"));
    officecli()
        .args(["validate", &p, "--json"])
        .assert()
        .success();
}

#[test]
fn test_docx_numbering_level_set_targets_the_selected_abstract_num() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_numbering_level_set.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    for id in ["3", "4"] {
        officecli()
            .args([
                "add",
                &p,
                "--parent",
                "/numbering",
                "--type-name",
                "abstractNum",
                "--properties",
                &format!("id={id}"),
                "format=decimal",
                "text=%1.",
            ])
            .assert()
            .success();
    }

    officecli()
        .args([
            "set",
            &p,
            "/numbering/abstractNum[@id=4]/level[0]",
            "format=upperRoman",
            "text=%1)",
            "start=7",
            "lvlRestart=0",
            "suff=space",
            "jc=center",
            "isLgl=true",
            "indent=1440",
            "hanging=360",
            "direction=rtl",
            "font=Symbol",
            "size=14pt",
            "color=#FF0000",
            "bold=true",
            "italic=true",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "get",
            &p,
            "/numbering/abstractNum[@id=3]/level[0]",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"format\": \"decimal\""));
    officecli()
        .args([
            "get",
            &p,
            "/numbering/abstractNum[@id=4]/level[0]",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"format\": \"upperRoman\""))
        .stdout(predicate::str::contains("\"text\": \"%1)\""))
        .stdout(predicate::str::contains("\"start\": \"7\""))
        .stdout(predicate::str::contains("\"lvlRestart\": \"0\""))
        .stdout(predicate::str::contains("\"suff\": \"space\""))
        .stdout(predicate::str::contains("\"justification\": \"center\""))
        .stdout(predicate::str::contains("\"isLgl\": true"));
    officecli()
        .args(["raw", &p, "word/numbering.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "<w:ind w:left=\"1440\" w:hanging=\"360\"/>",
        ))
        .stdout(predicate::str::contains("<w:bidi/>"))
        .stdout(predicate::str::contains("w:ascii=\"Symbol\""))
        .stdout(predicate::str::contains("<w:sz w:val=\"28\"/>"))
        .stdout(predicate::str::contains("<w:color w:val=\"FF0000\"/>"))
        .stdout(predicate::str::contains("<w:b/>"))
        .stdout(predicate::str::contains("<w:i/>"));
}

#[test]
fn test_docx_numbering_level_add_replaces_matching_ilvl() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_numbering_level_add.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/numbering",
            "--type-name",
            "abstractNum",
            "--properties",
            "id=3",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/numbering/abstractNum[@id=3]",
            "--type-name",
            "lvl",
            "--properties",
            "ilvl=2",
            "--properties",
            "format=upperRoman",
            "--properties",
            "lvlText=%1.%2.%3",
            "--properties",
            "start=5",
            "--properties",
            "indent=1440",
            "--properties",
            "hanging=360",
            "--properties",
            "font=Symbol",
            "--properties",
            "bold=true",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "get",
            &p,
            "/numbering/abstractNum[@id=3]/level[2]",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"format\": \"upperRoman\""))
        .stdout(predicate::str::contains("\"text\": \"%1.%2.%3\""));
    officecli()
        .args(["validate", &p, "--json"])
        .assert()
        .success();
}

#[test]
fn test_docx_numbering_level_remove_only_drops_target_level() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_numbering_level_remove.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/numbering",
            "--type-name",
            "abstractNum",
            "--properties",
            "id=3",
        ])
        .assert()
        .success();
    officecli()
        .args(["remove", &p, "/numbering/abstractNum[@id=3]/level[2]"])
        .assert()
        .success();
    officecli()
        .args([
            "get",
            &p,
            "/numbering/abstractNum[@id=3]/level[2]",
            "--json",
        ])
        .assert()
        .failure();
    officecli()
        .args([
            "get",
            &p,
            "/numbering/abstractNum[@id=3]/level[1]",
            "--json",
        ])
        .assert()
        .success();
    officecli()
        .args(["validate", &p, "--json"])
        .assert()
        .success();
}

#[test]
fn test_docx_abstract_num_top_level_properties_round_trip() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_abstract_num_properties.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/numbering",
            "--type-name",
            "abstractNum",
            "--properties",
            "id=3",
            "--properties",
            "type=single",
            "--properties",
            "name=Initial outline",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/numbering/abstractNum[@id=3]",
            "type=multi",
            "name=Chapter outline",
            "styleLink=MyListStyle",
            "numStyleLink=OutlineList",
        ])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/numbering/abstractNum[@id=3]", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\": \"multilevel\""))
        .stdout(predicate::str::contains("\"name\": \"Chapter outline\""))
        .stdout(predicate::str::contains("\"styleLink\": \"MyListStyle\""))
        .stdout(predicate::str::contains(
            "\"numStyleLink\": \"OutlineList\"",
        ));
    officecli()
        .args(["validate", &p, "--json"])
        .assert()
        .success();
}

#[test]
fn test_docx_num_start_overrides_round_trip() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_num_start_overrides.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/numbering",
            "--type-name",
            "abstractNum",
            "--properties",
            "id=3",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/numbering",
            "--type-name",
            "num",
            "--properties",
            "id=9",
            "abstractNumId=3",
            "start=5",
            "startOverride.2=7",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/numbering/num[@id=9]",
            "startOverride.0=8",
            "startOverride.2=11",
        ])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/numbering/num[@id=9]", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"startOverride.0\": \"8\""))
        .stdout(predicate::str::contains("\"startOverride.2\": \"11\""));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_removing_num_clears_direct_paragraph_bindings() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_num_remove_cleanup.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/numbering",
            "--type-name",
            "abstractNum",
            "--properties",
            "id=3",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/numbering",
            "--type-name",
            "num",
            "--properties",
            "id=9",
            "--properties",
            "abstractNumId=3",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=numbered item",
            "--properties",
            "numId=9",
            "--properties",
            "numLevel=0",
        ])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "word/document.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<w:numId w:val=\"9\""));
    officecli()
        .args(["get", &p, "/body/p[2]", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"numId\": \"9\""))
        .stdout(predicate::str::contains("\"numLevel\": \"0\""));
    officecli()
        .args(["remove", &p, "/numbering/num[@id=9]"])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/numbering/num[@id=9]", "--json"])
        .assert()
        .failure();
    officecli()
        .args(["raw", &p, "word/document.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<w:numPr").not());
    officecli()
        .args(["get", &p, "/body/p[2]", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"numId\"").not())
        .stdout(predicate::str::contains("\"numLevel\"").not());
    officecli()
        .args(["validate", &p, "--json"])
        .assert()
        .success();
}

#[test]
fn test_docx_revision_run_lifecycle() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_revision_lifecycle.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=base ",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[2]",
            "--type-name",
            "run",
            "--properties",
            "text=added ",
            "revision.type=ins",
            "revision.author=Ada",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[2]",
            "--type-name",
            "run",
            "--properties",
            "text=removed",
            "revision.type=del",
            "revision.author=Bea",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "query", &p, "revision"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\": \"revision\""))
        .stdout(predicate::str::contains("\"author\": \"Ada\""))
        .stdout(predicate::str::contains("\"author\": \"Bea\""));
    officecli()
        .args(["--json", "get", &p, "/revision[@id=1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"text\": \"added \""));
    officecli()
        .args(["set", &p, "/revision[@id=1]", "revision.action=accept"])
        .assert()
        .success();
    officecli()
        .args(["set", &p, "/body/p[2]/del[1]", "revision.action=reject"])
        .assert()
        .success();
    officecli()
        .args(["view", &p, "-m", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("base added removed"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_move_revision_creates_and_resolves_range_markers() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_move_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=base ",
        ])
        .assert()
        .success();
    for (revision_type, text) in [("moveFrom", "from "), ("moveTo", "to")] {
        officecli()
            .args([
                "add",
                &p,
                "--parent",
                "/body/p[2]",
                "--type-name",
                "run",
                "--properties",
                &format!("text={text}"),
                &format!("revision.type={revision_type}"),
                "revision.id=44",
                "revision.author=Ada",
            ])
            .assert()
            .success();
    }
    officecli()
        .args(["--json", "query", &p, "revision[@id=44]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\": \"moveFrom\""))
        .stdout(predicate::str::contains("\"type\": \"moveTo\""));
    officecli()
        .args([
            "set",
            &p,
            "/body/p[2]/moveFrom[1]",
            "revision.action=accept",
        ])
        .assert()
        .success();
    officecli()
        .args(["view", &p, "-m", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("base to"));
    officecli()
        .args(["--json", "query", &p, "revision"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"matches\": 0"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_format_revision_reject_restores_snapshot() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_format_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=base",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[2]",
            "--type-name",
            "run",
            "--properties",
            "text=changed",
            "bold=true",
            "revision.type=format",
            "revision.id=7",
            "revision.author=Ada",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "query", &p, "revision[@type=format]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"id\": \"7\""));
    officecli()
        .args(["set", &p, "/revision[@id=7]", "revision.action=reject"])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/body/p[2]/r[2]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"bold\": true").not());
    officecli()
        .args(["--json", "query", &p, "revision"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"matches\": 0"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_set_existing_run_as_revision() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_set_existing_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=existing",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/body/p[2]/r[1]",
            "revision.type=del",
            "revision.author=Ada",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/revision[@id=1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"text\": \"existing\""));
    officecli()
        .args(["set", &p, "/body/p[2]/del[1]", "revision.action=reject"])
        .assert()
        .success();
    officecli()
        .args(["view", &p, "-m", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("existing"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_remove_accepts_csharp_revision_props() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_remove_revision_props.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "/body",
            "--type",
            "paragraph",
            "--prop",
            "text=tracked",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "remove",
            &p,
            "/body/p[2]/r[1]",
            "--prop",
            "revision.author=Ada",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/revision[@id=1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"text\": \"tracked\""));
}

#[test]
fn test_docx_set_existing_run_format_revision_restores_prior_properties() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_set_format_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=existing",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/body/p[2]/r[1]",
            "bold=true",
            "revision.type=format",
            "revision.id=3",
            "revision.author=Ada",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "query", &p, "revision[@type=format]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"id\": \"3\""));
    officecli()
        .args(["set", &p, "/revision[@id=3]", "revision.action=reject"])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/body/p[2]/r[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"bold\": true").not());
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_set_existing_paragraph_format_revision_restores_prior_properties() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_set_paragraph_format_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=existing",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/body/p[2]",
            "alignment=center",
            "revision.type=format",
            "revision.id=4",
            "revision.author=Ada",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "query", &p, "revision[@type=format]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"id\": \"4\""));
    officecli()
        .args(["set", &p, "/revision[@id=4]", "revision.action=reject"])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/body/p[2]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"alignment\": \"center\"").not());
    officecli()
        .args(["--json", "query", &p, "revision"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"matches\": 0"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_add_paragraph_format_revision_rejects_to_empty_snapshot() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_add_paragraph_format_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=existing",
            "alignment=center",
            "revision.type=format",
            "revision.id=5",
            "revision.author=Ada",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/revision[@id=5]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\": \"format\""));
    officecli()
        .args(["set", &p, "/revision[@id=5]", "revision.action=reject"])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/body/p[2]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"alignment\": \"center\"").not());
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_row_insertion_revision_reject_removes_whole_row() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_row_insertion_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "table",
            "--properties",
            "rows=1",
            "cols=1",
            "r1c1=base",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/tbl[1]",
            "--type-name",
            "row",
            "--properties",
            "revision.type=ins",
            "revision.id=12",
            "revision.author=Ada",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/revision[@id=12]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\": \"ins\""));
    officecli()
        .args(["set", &p, "/revision[@id=12]", "revision.action=reject"])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/body/tbl[1]/tr[2]"])
        .assert()
        .failure();
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_existing_row_deletion_revision_accept_removes_whole_row() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_row_deletion_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "table",
            "--properties",
            "rows=1",
            "cols=1",
            "r1c1=base",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/body/tbl[1]/tr[1]",
            "revision.type=del",
            "revision.id=13",
            "revision.author=Ada",
        ])
        .assert()
        .success();
    officecli()
        .args(["set", &p, "/revision[@id=13]", "revision.action=accept"])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/body/tbl[1]/tr[1]"])
        .assert()
        .failure();
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_cell_insertion_revision_reject_removes_whole_cell() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_cell_insertion_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "table",
            "--properties",
            "rows=1",
            "cols=1",
            "r1c1=base",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/tbl[1]/tr[1]",
            "--type-name",
            "cell",
            "--properties",
            "text=added",
            "revision.type=ins",
            "revision.id=14",
            "revision.author=Ada",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/revision[@id=14]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\": \"cellIns\""));
    officecli()
        .args(["set", &p, "/revision[@id=14]", "revision.action=reject"])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/body/tbl[1]/tr[1]/tc[2]"])
        .assert()
        .failure();
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_existing_cell_deletion_revision_accept_removes_whole_cell() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_cell_deletion_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "table",
            "--properties",
            "rows=1",
            "cols=1",
            "r1c1=base",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/body/tbl[1]/tr[1]/tc[1]",
            "revision.type=del",
            "revision.id=15",
            "revision.author=Ada",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/revision[@id=15]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\": \"cellDel\""));
    officecli()
        .args(["set", &p, "/revision[@id=15]", "revision.action=accept"])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/body/tbl[1]/tr[1]/tc[1]"])
        .assert()
        .failure();
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_paragraph_mark_insertion_reject_merges_into_previous_paragraph() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_paragraph_mark_insertion.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    for (text, revision) in [("base", None), ("added", Some("20"))] {
        let mut args = vec![
            "add".to_string(),
            p.clone(),
            "--parent".to_string(),
            "/body".to_string(),
            "--type-name".to_string(),
            "paragraph".to_string(),
            "--properties".to_string(),
            format!("text={text}"),
        ];
        if let Some(id) = revision {
            args.extend([
                "revision.type=ins".to_string(),
                format!("revision.id={id}"),
                "revision.author=Ada".to_string(),
            ]);
        }
        officecli().args(&args).assert().success();
    }
    officecli()
        .args(["set", &p, "/revision[@id=20]", "revision.action=reject"])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/body/p[3]"])
        .assert()
        .failure();
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_paragraph_mark_deletion_accept_merges_into_next_paragraph() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_paragraph_mark_deletion.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    for (text, revision) in [("first", None), ("second", Some("21")), ("third", None)] {
        let mut args = vec![
            "add".to_string(),
            p.clone(),
            "--parent".to_string(),
            "/body".to_string(),
            "--type-name".to_string(),
            "paragraph".to_string(),
            "--properties".to_string(),
            format!("text={text}"),
        ];
        if let Some(id) = revision {
            args.extend([
                "revision.type=del".to_string(),
                format!("revision.id={id}"),
                "revision.author=Ada".to_string(),
            ]);
        }
        officecli().args(&args).assert().success();
    }
    officecli()
        .args(["set", &p, "/revision[@id=21]", "revision.action=accept"])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/body/p[4]"])
        .assert()
        .failure();
    officecli()
        .args(["view", &p, "-m", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("secondthird"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_find_replace_with_revision_tracks_precise_fragment() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_find_replace_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=prefix old suffix",
            "bold=true",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/body/p[2]",
            "find=old",
            "replace=new",
            "revision.author=Ada",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("replaced=1"));
    officecli()
        .args(["--json", "query", &p, "revision"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\": \"del\""))
        .stdout(predicate::str::contains("\"type\": \"ins\""));
    officecli()
        .args(["set", &p, "/revision[@id=1]", "revision.action=accept"])
        .assert()
        .success();
    officecli()
        .args(["set", &p, "/revision[@id=2]", "revision.action=accept"])
        .assert()
        .success();
    officecli()
        .args(["view", &p, "-m", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prefix new suffix"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_find_format_with_revision_reject_restores_run_properties() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_find_format_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=match me",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/body/p[2]",
            "find=match",
            "bold=true",
            "revision.type=format",
            "revision.author=Ada",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("replaced=1"));
    officecli()
        .args(["--json", "query", &p, "revision[@type=format]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\": \"format\""));
    officecli()
        .args(["set", &p, "/revision[@id=1]", "revision.action=reject"])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/body/p[2]/r[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"bold\": true").not());
    officecli()
        .args(["view", &p, "-m", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("match me"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_regex_find_format_with_revision_reject_restores_run_properties() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_regex_find_format_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=invoice 2026",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/body/p[2]",
            "find=[0-9]+",
            "regex=true",
            "bold=true",
            "revision.type=format",
            "revision.author=Ada",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("replaced=1"));
    officecli()
        .args(["--json", "query", &p, "revision[@type=format]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\": \"format\""));
    officecli()
        .args(["set", &p, "/revision[@id=1]", "revision.action=reject"])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/body/p[2]/r[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"bold\": true").not());
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_cross_run_find_replace_with_revision_tracks_each_fragment() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_cross_run_find_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=hel",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[2]",
            "--type-name",
            "run",
            "--properties",
            "text=lo world",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/body/p[2]",
            "find=hello",
            "replace=hi",
            "revision.author=Ada",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("replaced=1"));
    officecli()
        .args(["--json", "query", &p, "revision"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\": \"del\"").count(2))
        .stdout(predicate::str::contains("\"type\": \"ins\""));
    for id in ["1", "2", "3"] {
        officecli()
            .args([
                "set",
                &p,
                &format!("/revision[@id={id}]"),
                "revision.action=accept",
            ])
            .assert()
            .success();
    }
    officecli()
        .args(["view", &p, "-m", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hi world"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_cross_run_regex_find_replace_with_revision_expands_captures() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_cross_run_regex_find_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=item-",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[2]",
            "--type-name",
            "run",
            "--properties",
            "text=42",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/body/p[2]",
            "find=(\\w+)-(\\d+)",
            "regex=true",
            "replace=$2:$1",
            "revision.author=Ada",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("replaced=1"));
    for id in ["1", "2", "3"] {
        officecli()
            .args([
                "set",
                &p,
                &format!("/revision[@id={id}]"),
                "revision.action=accept",
            ])
            .assert()
            .success();
    }
    officecli()
        .args(["view", &p, "-m", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("42:item"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_tracked_find_rejects_hyperlink_boundary() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_tracked_find_hyperlink_boundary.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=prefix ",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[2]",
            "--type-name",
            "hyperlink",
            "--properties",
            "text=link",
            "url=https://example.com",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[2]",
            "--type-name",
            "run",
            "--properties",
            "text=tail",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/body/p[2]",
            "find=linktail",
            "replace=x",
            "revision.author=Ada",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("inline structure boundary"));
    officecli()
        .args(["view", &p, "-m", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prefix linktail"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_hyperlink_add_and_set_write_external_relationships() {
    let tmp = temp_dir();
    let path = tmp.path().join("hyperlink_relationship.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[1]",
            "--type-name",
            "hyperlink",
            "--properties",
            "text=first",
            "url=https://example.com/first",
        ])
        .assert()
        .success();

    let package = oxml::OxmlPackage::open(&p, false).unwrap();
    let document = package.read_part_xml("word/document.xml").unwrap();
    let rels = package
        .read_part_xml("word/_rels/document.xml.rels")
        .unwrap();
    assert!(document.contains(r#"w:hyperlink r:id="rId2""#));
    assert!(rels.contains(r#"Id="rId2""#));
    assert!(rels.contains(r#"Target="https://example.com/first" TargetMode="External""#));

    officecli()
        .args([
            "set",
            &p,
            "/body/p[1]/hyperlink[1]",
            "url=https://example.com/second",
        ])
        .assert()
        .success();
    let package = oxml::OxmlPackage::open(&p, false).unwrap();
    let document = package.read_part_xml("word/document.xml").unwrap();
    let rels = package
        .read_part_xml("word/_rels/document.xml.rels")
        .unwrap();
    assert!(document.contains(r#"w:hyperlink r:id="rId3""#));
    assert!(rels.contains(r#"Id="rId3""#));
    assert!(rels.contains(r#"Target="https://example.com/second" TargetMode="External""#));
    officecli()
        .args(["--json", "validate", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"data\": []"));
}

#[test]
fn test_docx_comment_set_formats_existing_body_without_flattening() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_comment_set_format.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[1]",
            "--type-name",
            "comment",
            "--properties",
            "text=review me",
            "author=Ada",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/comments/comment[@commentId=0]",
            "bold=true",
            "italic=true",
            "alignment=center",
        ])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "word/comments.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<w:b />"))
        .stdout(predicate::str::contains("<w:i />"))
        .stdout(predicate::str::contains("<w:jc w:val=\"center\" />"))
        .stdout(predicate::str::contains("review me"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_comment_body_run_get_and_set_are_path_scoped() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_comment_body_run_path.docx");
    let p = path.to_string_lossy().to_string();
    let run_path = "/comments/comment[@commentId=0]/p[1]/r[1]";

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[1]",
            "--type-name",
            "comment",
            "--properties",
            "text=first body",
            "author=Ada",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, run_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\": \"r\""))
        .stdout(predicate::str::contains("first body"));
    officecli()
        .args(["set", &p, run_path, "text=edited body", "bold=true"])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "word/comments.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("edited body"))
        .stdout(predicate::str::contains("<w:b />"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_comment_body_add_and_remove_paragraph_run() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_comment_body_add_remove.docx");
    let p = path.to_string_lossy().to_string();
    let comment_path = "/comments/comment[@commentId=0]";
    let paragraph_path = "/comments/comment[@commentId=0]/p[1]";

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[1]",
            "--type-name",
            "comment",
            "--properties",
            "text=first",
            "author=Ada",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            paragraph_path,
            "--type-name",
            "run",
            "--properties",
            "text= second",
            "bold=true",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("/r[2]"));
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            comment_path,
            "--type-name",
            "paragraph",
            "--properties",
            "text=third",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("/p[2]"));
    officecli()
        .args(["--json", "get", &p, "/comments/comment[@commentId=0]/p[2]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("third"));
    officecli()
        .args(["remove", &p, "/comments/comment[@commentId=0]/p[1]/r[2]"])
        .assert()
        .success();
    officecli()
        .args(["remove", &p, "/comments/comment[@commentId=0]/p[2]"])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "word/comments.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains(" second").not())
        .stdout(predicate::str::contains("third").not())
        .stdout(predicate::str::contains("first"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_query_paragraph_and_run_includes_comment_body() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_comment_query_subtree.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[1]",
            "--type-name",
            "comment",
            "--properties",
            "text=comment text",
            "author=Ada",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/comments/comment[@commentId=0]/p[1]/r[1]",
            "bold=true",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "query", &p, "p"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "/comments/comment[@commentId=0]/p[1]",
        ));
    officecli()
        .args(["--json", "query", &p, "r[bold]"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "/comments/comment[@commentId=0]/p[1]/r[1]",
        ));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_raw_comments_extended_logical_alias_preserves_thread_metadata() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_comments_extended_alias.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[1]",
            "--type-name",
            "comment",
            "--properties",
            "text=parent",
            "author=Ada",
            "done=true",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[1]",
            "--type-name",
            "comment",
            "--properties",
            "text=reply",
            "author=Bea",
            "parentId=0",
        ])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "/commentsExtended"])
        .assert()
        .success()
        .stdout(predicate::str::contains("w15:commentEx"))
        .stdout(predicate::str::contains("w15:paraIdParent"))
        .stdout(predicate::str::contains("w15:done=\"1\""));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_add_markdown_expands_editable_blocks() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_markdown_blocks.docx");
    let p = path.to_string_lossy().to_string();
    let markdown = "# Title\n\nA paragraph\ncontinued\n\n- first\n2. second\n\n> quote\n\n```txt\nlet x = 1;\n```";

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "markdown",
            "--properties",
            &format!("markdown={markdown}"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("/body/p[2]"));
    officecli()
        .args(["--json", "get", &p, "/body/p[2]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Title"))
        .stdout(predicate::str::contains("Heading1"));
    officecli()
        .args(["view", &p, "-m", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("A paragraph continued"))
        .stdout(predicate::str::contains("• first"))
        .stdout(predicate::str::contains("2. second"))
        .stdout(predicate::str::contains("let x = 1;"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_new_docx_declares_markdown_heading_styles() {
    let tmp = temp_dir();
    let path = tmp.path().join("markdown_styles.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "markdown",
            "--properties",
            "markdown=# Title\n## Subtitle",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "validate", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"data\": []"));

    let package = oxml::OxmlPackage::open(&p, false).unwrap();
    let styles = package.read_part_xml("word/styles.xml").unwrap();
    assert!(styles.contains(r#"w:styleId="Heading1""#));
    assert!(styles.contains(r#"w:styleId="Heading9""#));
}

#[test]
fn test_docx_add_markdown_emits_inline_format_runs() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_markdown_inline.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "markdown",
            "--properties",
            "markdown=plain **bold** *italic* ~~gone~~ `code`",
        ])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "word/document.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<w:b />"))
        .stdout(predicate::str::contains("<w:i />"))
        .stdout(predicate::str::contains("<w:strike />"))
        .stdout(predicate::str::contains("Consolas"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_add_markdown_emits_editable_gfm_table() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_markdown_table.docx");
    let p = path.to_string_lossy().to_string();
    let markdown = "| Name | Value |\n| --- | :---: |\n| Ada | **42** |\n| Bea | 7 |";

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "markdown",
            "--properties",
            &format!("markdown={markdown}"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("/body/tbl[1]"));
    officecli()
        .args(["--json", "get", &p, "/body/tbl[1]/tr[2]/tc[2]/p[1]/r[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("42"));
    officecli()
        .args(["raw", &p, "word/document.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<w:tbl"))
        .stdout(predicate::str::contains("<w:b />"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_drawingml_shape_and_textbox_lifecycle() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_drawingml_shapes.docx");
    let p = path.to_string_lossy();
    officecli().args(["create", &p]).assert().success();

    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "shape",
            "--properties",
            "geometry=ellipse,width=4cm,height=2cm,fill=FF0000",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("/body/shape[1]"));
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "textbox",
            "--properties",
            "text=Sidebar note,width=6cm,geometry=roundRect",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("/body/textbox[1]"));
    officecli()
        .args(["--json", "get", &p, "/body/textbox[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Sidebar note"))
        .stdout(predicate::str::contains("roundRect"));
    officecli()
        .args(["set", &p, "/body/shape[1]", "geometry=diamond", "width=4cm"])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/body/shape[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1440000"));
    officecli()
        .args(["raw", &p, "word/document.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prst=\"diamond\""))
        .stdout(predicate::str::contains("wps:wsp"));
    officecli()
        .args(["remove", &p, "/body/textbox[1]"])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "word/document.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Sidebar note").not());
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_add_native_mermaid_flowchart_group() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_docx_diagram.docx");
    let p = path.to_string_lossy();
    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "diagram",
            "--properties",
            "mermaid=flowchart TD; A[Start] -->|next| B{Ready?}; B --> C[Done],width=10cm",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("/body/group[1]"));
    officecli()
        .args(["raw", &p, "word/document.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("wpg:wgp"))
        .stdout(predicate::str::contains("Diagram node"))
        .stdout(predicate::str::contains("diamond"))
        .stdout(predicate::str::contains("Start"))
        .stdout(predicate::str::contains("next"));
    officecli()
        .args(["--json", "get", &p, "/body/group[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Start"));
    officecli()
        .args(["set", &p, "/body/group[1]", "width=8cm"])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/body/group[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2880000"));
    officecli().args(["validate", &p]).assert().success();
    officecli()
        .args(["remove", &p, "/body/group[1]"])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "word/document.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("wpg:wgp").not());
}

#[test]
fn test_pptx_add_native_mermaid_flowchart_group() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_pptx_diagram.pptx");
    let p = path.to_string_lossy();
    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/slide[1]",
            "--type-name",
            "diagram",
            "--properties",
            "mermaid=flowchart LR; A[Start] --> B{Ready?} --> C[Done]",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("/slide[1]/group[1]"));
    officecli()
        .args(["raw", &p, "ppt/slides/slide1.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("p:grpSp"))
        .stdout(predicate::str::contains("Diagram node"))
        .stdout(predicate::str::contains("diamond"))
        .stdout(predicate::str::contains("Start"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_add_native_mermaid_sequence_group() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_docx_sequence_diagram.docx");
    let p = path.to_string_lossy();
    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "diagram",
            "--properties",
            "mermaid=sequenceDiagram; participant A as Alice; participant B as Bob; A->>B: Hello; B-->>A: Reply",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("/body/group[1]"));
    officecli()
        .args(["raw", &p, "word/document.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Alice"))
        .stdout(predicate::str::contains("Hello"))
        .stdout(predicate::str::contains("prstDash"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_pptx_add_native_mermaid_sequence_group() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_pptx_sequence_diagram.pptx");
    let p = path.to_string_lossy();
    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/slide[1]",
            "--type-name",
            "diagram",
            "--properties",
            "mermaid=sequenceDiagram; participant A as Alice; participant B as Bob; A->>B: Hello; B-->>A: Reply",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("/slide[1]/group[1]"));
    officecli()
        .args(["raw", &p, "ppt/slides/slide1.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Alice"))
        .stdout(predicate::str::contains("Hello"))
        .stdout(predicate::str::contains("prstDash"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_docx_cross_run_find_format_with_revision_tracks_exact_fragments() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_cross_run_find_format_revision.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=hel",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[2]",
            "--type-name",
            "run",
            "--properties",
            "text=lo world",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/body/p[2]",
            "find=hello",
            "bold=true",
            "revision.type=format",
            "revision.author=Ada",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("replaced=1"));
    officecli()
        .args(["--json", "query", &p, "revision[@type=format]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\": \"format\"").count(2));
    for id in ["1", "2"] {
        officecli()
            .args([
                "set",
                &p,
                &format!("/revision[@id={id}]"),
                "revision.action=reject",
            ])
            .assert()
            .success();
    }
    officecli()
        .args(["view", &p, "-m", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hello world"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_get_save_extracts_docx_drawing_payload_to_nested_destination() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_get_save_image.docx");
    let output = tmp.path().join("nested/exported-image.bin");
    let p = path.to_string_lossy().to_string();
    let output_string = output.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[1]",
            "--type-name",
            "image",
            "--properties",
            "payloadBase64=AAEC",
            "--properties",
            "format=png",
        ])
        .assert()
        .success();

    officecli()
        .args([
            "get",
            &p,
            "/body/p[1]/drawing[1]",
            "--depth",
            "0",
            "--save",
            &output_string,
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("savedTo"))
        .stdout(predicate::str::contains("savedBytes"))
        .stdout(predicate::str::contains("image/png"));

    assert_eq!(std::fs::read(output).unwrap(), vec![0, 1, 2]);
}

#[test]
fn test_add_and_view_paragraph() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_add_view.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();

    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=View test",
        ])
        .assert()
        .success();

    officecli()
        .args(["view", &p, "-m", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("View test"));
}

// ═══════════════════════════════════════════════════════════════════════
// Set — modify an existing element's property
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_set_text() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_set.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();

    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=Original",
        ])
        .assert()
        .success();

    // Set the text on the added paragraph (p[2])
    officecli()
        .args(["set", &p, "/body/p[2]", "text=Modified"])
        .assert()
        .success();

    // Verify the change
    officecli()
        .args(["get", &p, "/body/p[2]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Modified"));
}

#[test]
fn test_set_accepts_csharp_prop_syntax() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_set_csharp_prop.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["set", &p, "/body/p[1]", "--prop", "text=C# property flag"])
        .assert()
        .success()
        .stdout(predicate::str::contains("OK"));
    officecli()
        .args(["get", &p, "/body/p[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("C# property flag"));

    officecli()
        .args([
            "set",
            &p,
            "/body/p[1]",
            "--find",
            "property",
            "--replace",
            "selector",
        ])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/body/p[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("C# selector flag"));
}

// ═══════════════════════════════════════════════════════════════════════
// Remove — delete an element
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_remove_paragraph() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_remove.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();

    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=Remove me",
        ])
        .assert()
        .success();

    // Remove the added paragraph
    officecli()
        .args(["remove", &p, "/body/p[2]"])
        .assert()
        .success();
}

// ═══════════════════════════════════════════════════════════════════════
// Query — find elements by CSS-like selector
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_query_paragraphs() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_query.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();

    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=First",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=Second",
        ])
        .assert()
        .success();

    // Query using "p" selector (the actual CLI uses CSS-like selectors)
    officecli()
        .args(["query", &p, "p"])
        .assert()
        .success()
        .stdout(predicate::str::contains("/body/p"));
}

// ═══════════════════════════════════════════════════════════════════════
// Validate — check document structure
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_validate_docx() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_validate.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli().args(["validate", &p]).assert().success();
}

// ═══════════════════════════════════════════════════════════════════════
// Dump — show full XML
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_dump_docx() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_dump.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli().args(["dump", &p]).assert().success();
}

#[test]
fn test_dump_docx_batch_replays_into_fresh_document() {
    let tmp = temp_dir();
    let source = tmp.path().join("dump_source.docx");
    let target = tmp.path().join("dump_target.docx");
    let source_path = source.to_string_lossy().to_string();
    let target_path = target.to_string_lossy().to_string();
    officecli()
        .args(["create", &source_path])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &source_path,
            "/body",
            "--type",
            "paragraph",
            "--prop",
            "text=Round trip",
        ])
        .assert()
        .success();
    let dump = officecli()
        .args(["dump", &source_path])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    officecli()
        .args(["create", &target_path])
        .assert()
        .success();
    officecli()
        .args(["batch", &target_path, std::str::from_utf8(&dump).unwrap()])
        .assert()
        .success();
    officecli()
        .args(["view", &target_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("Round trip"));
}

#[test]
fn test_dump_accepts_csharp_positional_path_and_legacy_path_flag() {
    let tmp = temp_dir();
    let path = tmp.path().join("dump_path.docx");
    let p = path.to_string_lossy().to_string();
    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["dump", &p, "/body", "--dom"])
        .assert()
        .success()
        .stdout(predicate::str::contains("/body"));
    officecli()
        .args(["dump", &p, "--path", "/body", "--dom"])
        .assert()
        .success()
        .stdout(predicate::str::contains("/body"));
}

#[test]
fn test_dump_docx_styles_replays_semantic_part() {
    let tmp = temp_dir();
    let source = tmp.path().join("dump_styles_source.docx");
    let target = tmp.path().join("dump_styles_target.docx");
    let source_path = source.to_string_lossy().to_string();
    let target_path = target.to_string_lossy().to_string();
    officecli()
        .args(["create", &source_path])
        .assert()
        .success();
    let dump = officecli()
        .args(["dump", &source_path, "/styles"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    officecli()
        .args(["create", &target_path])
        .assert()
        .success();
    officecli()
        .args(["batch", &target_path, std::str::from_utf8(&dump).unwrap()])
        .assert()
        .success();
    officecli()
        .args(["raw", &target_path, "/styles"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Heading1"));
}

#[test]
fn test_dump_docx_body_subtree_replays_into_fresh_document() {
    let tmp = temp_dir();
    let source = tmp.path().join("dump_body_source.docx");
    let target = tmp.path().join("dump_body_target.docx");
    let source_path = source.to_string_lossy().to_string();
    let target_path = target.to_string_lossy().to_string();
    officecli()
        .args(["create", &source_path])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &source_path,
            "/body",
            "--type",
            "p",
            "--prop",
            "text=Body replay",
        ])
        .assert()
        .success();
    let dump = officecli()
        .args(["dump", &source_path, "/body"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    officecli()
        .args(["create", &target_path])
        .assert()
        .success();
    officecli()
        .args(["batch", &target_path, std::str::from_utf8(&dump).unwrap()])
        .assert()
        .success();
    officecli()
        .args(["view", &target_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("Body replay"));
}

#[test]
fn test_dump_docx_paragraph_subtree_replays_into_fresh_document() {
    let tmp = temp_dir();
    let source = tmp.path().join("dump_paragraph_source.docx");
    let target = tmp.path().join("dump_paragraph_target.docx");
    let source_path = source.to_string_lossy().to_string();
    let target_path = target.to_string_lossy().to_string();
    officecli()
        .args(["create", &source_path])
        .assert()
        .success();
    officecli()
        .args(["set", &source_path, "/body/p[1]", "text=Paragraph replay"])
        .assert()
        .success();
    let dump = officecli()
        .args(["dump", &source_path, "/body/p[1]"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    officecli()
        .args(["create", &target_path])
        .assert()
        .success();
    officecli()
        .args(["batch", &target_path, std::str::from_utf8(&dump).unwrap()])
        .assert()
        .success();
    officecli()
        .args(["view", &target_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("Paragraph replay"));
}

#[test]
fn test_dump_docx_comments_replays_into_fresh_document() {
    let tmp = temp_dir();
    let source = tmp.path().join("dump_comments_source.docx");
    let target = tmp.path().join("dump_comments_target.docx");
    let source_path = source.to_string_lossy().to_string();
    let target_path = target.to_string_lossy().to_string();
    officecli()
        .args(["create", &source_path])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &source_path,
            "/body/p[1]",
            "--type",
            "comment",
            "--prop",
            "text=Comment replay",
            "--prop",
            "author=Ada",
        ])
        .assert()
        .success();
    let dump = officecli()
        .args(["dump", &source_path, "/comments"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    officecli()
        .args(["create", &target_path])
        .assert()
        .success();
    officecli()
        .args(["batch", &target_path, std::str::from_utf8(&dump).unwrap()])
        .assert()
        .success();
    officecli()
        .args(["raw", &target_path, "/comments"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Comment replay"));
}

#[test]
fn test_dump_xlsx_batch_replays_into_fresh_workbook() {
    let tmp = temp_dir();
    let source = tmp.path().join("dump_source.xlsx");
    let target = tmp.path().join("dump_target.xlsx");
    let source_path = source.to_string_lossy().to_string();
    let target_path = target.to_string_lossy().to_string();
    officecli()
        .args(["create", &source_path])
        .assert()
        .success();
    officecli()
        .args(["set", &source_path, "/Sheet1/A1", "value=Round trip"])
        .assert()
        .success();
    let dump = officecli()
        .args(["dump", &source_path])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    officecli()
        .args(["create", &target_path])
        .assert()
        .success();
    officecli()
        .args(["batch", &target_path, std::str::from_utf8(&dump).unwrap()])
        .assert()
        .success();
    officecli()
        .args(["get", &target_path, "/Sheet1/A1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Round trip"));
}

#[test]
fn test_dump_xlsx_batch_creates_missing_worksheets() {
    let tmp = temp_dir();
    let source = tmp.path().join("dump_workbook_source.xlsx");
    let target = tmp.path().join("dump_workbook_target.xlsx");
    let source_path = source.to_string_lossy().to_string();
    let target_path = target.to_string_lossy().to_string();
    officecli()
        .args(["create", &source_path])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &source_path,
            "/",
            "--type",
            "sheet",
            "--prop",
            "name=Second",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &source_path,
            "/Second/A1",
            "value=Second sheet replay",
        ])
        .assert()
        .success();
    let dump = officecli()
        .args(["dump", &source_path])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    officecli()
        .args(["create", &target_path])
        .assert()
        .success();
    officecli()
        .args(["batch", &target_path, std::str::from_utf8(&dump).unwrap()])
        .assert()
        .success();
    officecli()
        .args(["get", &target_path, "/Second/A1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Second sheet replay"));
}

#[test]
fn test_dump_xlsx_sheet_subtree_replays_into_matching_sheet() {
    let tmp = temp_dir();
    let source = tmp.path().join("dump_sheet_source.xlsx");
    let target = tmp.path().join("dump_sheet_target.xlsx");
    let source_path = source.to_string_lossy().to_string();
    let target_path = target.to_string_lossy().to_string();
    officecli()
        .args(["create", &source_path])
        .assert()
        .success();
    officecli()
        .args(["set", &source_path, "/Sheet1/A1", "value=Sheet replay"])
        .assert()
        .success();
    let dump = officecli()
        .args(["dump", &source_path, "/Sheet1"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    officecli()
        .args(["create", &target_path])
        .assert()
        .success();
    officecli()
        .args(["batch", &target_path, std::str::from_utf8(&dump).unwrap()])
        .assert()
        .success();
    officecli()
        .args(["get", &target_path, "/Sheet1/A1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Sheet replay"));
}

#[test]
fn test_dump_pptx_batch_replays_into_fresh_deck() {
    let tmp = temp_dir();
    let source = tmp.path().join("dump_source.pptx");
    let target = tmp.path().join("dump_target.pptx");
    let source_path = source.to_string_lossy().to_string();
    let target_path = target.to_string_lossy().to_string();
    officecli()
        .args(["create", &source_path])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &source_path,
            "/slide[1]",
            "--type",
            "shape",
            "--prop",
            "text=Round trip",
        ])
        .assert()
        .success();
    let dump = officecli()
        .args(["dump", &source_path])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    officecli()
        .args(["create", &target_path])
        .assert()
        .success();
    officecli()
        .args(["batch", &target_path, std::str::from_utf8(&dump).unwrap()])
        .assert()
        .success();
    officecli()
        .args(["view", &target_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("Round trip"));
}

#[test]
fn test_dump_pptx_batch_creates_missing_slides() {
    let tmp = temp_dir();
    let source = tmp.path().join("dump_multislide_source.pptx");
    let target = tmp.path().join("dump_multislide_target.pptx");
    let source_path = source.to_string_lossy().to_string();
    let target_path = target.to_string_lossy().to_string();
    officecli()
        .args(["create", &source_path])
        .assert()
        .success();
    officecli()
        .args(["add", &source_path, "/", "--type", "slide"])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &source_path,
            "/slide[2]",
            "--type",
            "shape",
            "--prop",
            "text=Second slide replay",
        ])
        .assert()
        .success();
    let dump = officecli()
        .args(["dump", &source_path])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    officecli()
        .args(["create", &target_path])
        .assert()
        .success();
    officecli()
        .args(["batch", &target_path, std::str::from_utf8(&dump).unwrap()])
        .assert()
        .success();
    officecli()
        .args(["view", &target_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("Second slide replay"));
}

#[test]
fn test_dump_pptx_slide_subtree_replays_into_matching_slide() {
    let tmp = temp_dir();
    let source = tmp.path().join("dump_slide_source.pptx");
    let target = tmp.path().join("dump_slide_target.pptx");
    let source_path = source.to_string_lossy().to_string();
    let target_path = target.to_string_lossy().to_string();
    officecli()
        .args(["create", &source_path])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &source_path,
            "/slide[1]",
            "--type",
            "shape",
            "--prop",
            "text=Slide replay",
        ])
        .assert()
        .success();
    let dump = officecli()
        .args(["dump", &source_path, "/slide[1]"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    officecli()
        .args(["create", &target_path])
        .assert()
        .success();
    officecli()
        .args(["batch", &target_path, std::str::from_utf8(&dump).unwrap()])
        .assert()
        .success();
    officecli()
        .args(["view", &target_path])
        .assert()
        .success()
        .stdout(predicate::str::contains("Slide replay"));
}

#[test]
fn test_dump_pptx_presentation_subtree_replays_into_fresh_deck() {
    let tmp = temp_dir();
    let source = tmp.path().join("dump_presentation_source.pptx");
    let target = tmp.path().join("dump_presentation_target.pptx");
    let source_path = source.to_string_lossy().to_string();
    let target_path = target.to_string_lossy().to_string();
    officecli()
        .args(["create", &source_path])
        .assert()
        .success();
    officecli()
        .args([
            "raw-set",
            &source_path,
            "/presentation",
            "--xpath",
            "/presentation",
            "--action",
            "setattr",
            "--xml",
            "firstSlideNum=7",
        ])
        .assert()
        .success();
    let dump = officecli()
        .args(["dump", &source_path, "/presentation"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    officecli()
        .args(["create", &target_path])
        .assert()
        .success();
    officecli()
        .args(["batch", &target_path, std::str::from_utf8(&dump).unwrap()])
        .assert()
        .success();
    officecli()
        .args(["raw", &target_path, "/presentation"])
        .assert()
        .success()
        .stdout(predicate::str::contains("firstSlideNum=\"7\""));
}

// ═══════════════════════════════════════════════════════════════════════
// Raw — read a part by name
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_raw_docx() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_raw.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["raw", &p, "word/document.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("w:body"));
}

#[test]
fn test_raw_accepts_csharp_default_part_and_row_options() {
    let tmp = temp_dir();
    let path = tmp.path().join("raw_csharp.docx");
    let p = path.to_string_lossy().to_string();
    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["raw", &p, "--start", "1", "--end", "1"])
        .assert()
        .success();
}

#[test]
fn test_xlsx_raw_and_raw_set_accept_csharp_semantic_sheet_paths() {
    let tmp = temp_dir();
    let path = tmp.path().join("raw_semantic.xlsx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["raw", &p, "/workbook"])
        .assert()
        .success()
        .stdout(predicate::str::contains("workbook"));
    officecli()
        .args(["raw", &p, "/Sheet1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("worksheet"));
    officecli()
        .args(["raw", &p, "/sheet[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("worksheet"));
    officecli()
        .args([
            "raw-set",
            &p,
            "/Sheet1",
            "--xpath",
            "/worksheet",
            "--action",
            "setattr",
            "--xml",
            "codeName=SemanticRaw",
        ])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "/Sheet1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("codeName=\"SemanticRaw\""));
}

#[test]
fn test_dump_xlsx_accepts_csharp_positional_sheet_path() {
    let tmp = temp_dir();
    let source = tmp.path().join("dump_sheet_index_source.xlsx");
    let target = tmp.path().join("dump_sheet_index_target.xlsx");
    let source_path = source.to_string_lossy().to_string();
    let target_path = target.to_string_lossy().to_string();
    officecli()
        .args(["create", &source_path])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &source_path,
            "/Sheet1/A1",
            "value=Indexed sheet replay",
        ])
        .assert()
        .success();
    let dump = officecli()
        .args(["dump", &source_path, "/sheet[1]"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    officecli()
        .args(["create", &target_path])
        .assert()
        .success();
    officecli()
        .args(["batch", &target_path, std::str::from_utf8(&dump).unwrap()])
        .assert()
        .success();
    officecli()
        .args(["get", &target_path, "/Sheet1/A1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Indexed sheet replay"));
}

#[test]
fn test_xlsx_raw_accepts_csharp_drawing_chart_and_relationship_paths() {
    let tmp = temp_dir();
    let path = tmp.path().join("raw_chart_paths.xlsx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "/Sheet1",
            "--type",
            "chart",
            "--prop",
            "sheet=Sheet1",
            "--prop",
            "title=Raw chart",
        ])
        .assert()
        .success();

    officecli()
        .args(["raw", &p, "/Sheet1/drawing"])
        .assert()
        .success()
        .stdout(predicate::str::contains("wsDr"));
    officecli()
        .args(["raw", &p, "/Sheet1/chart[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("chartSpace"));
    officecli()
        .args(["raw", &p, "/chart[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("chartSpace"));
    officecli()
        .args(["raw", &p, "/Sheet1/rId1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("wsDr"));
}

#[test]
fn test_xlsx_raw_filters_rows_and_columns_like_csharp() {
    let tmp = temp_dir();
    let path = tmp.path().join("raw_filters.xlsx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    for (cell, value) in [("A1", "keep"), ("B1", "drop column"), ("A2", "drop row")] {
        officecli()
            .args([
                "set",
                &p,
                &format!("/Sheet1/{cell}"),
                &format!("value={value}"),
            ])
            .assert()
            .success();
    }

    officecli()
        .args([
            "raw", &p, "/Sheet1", "--start", "1", "--end", "1", "--cols", "A",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("A1"))
        .stdout(predicate::str::contains("B1").not())
        .stdout(predicate::str::contains("A2").not());
}

// ═══════════════════════════════════════════════════════════════════════
// Extract-text — pull plain text
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_extract_text_docx() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_extract.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=Extract me",
        ])
        .assert()
        .success();

    officecli().args(["extract-text", &p]).assert().success();
}

// ═══════════════════════════════════════════════════════════════════════
// JSON output mode
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_view_json() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_json.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["--json", "view", &p, "-m", "stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"paragraphs\""));
}

// ═══════════════════════════════════════════════════════════════════════
// Sample file tests (use workspace-root relative paths)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_view_sample_docx() {
    let root = workspace_root();
    let sample = root.join("assets/showcase/annual-report.docx");
    if !sample.exists() {
        return;
    }

    officecli()
        .args(["view", sample.to_string_lossy().as_ref(), "-m", "stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Paragraphs"))
        .stdout(predicate::str::contains("Tables"));
}

#[test]
fn test_view_sample_xlsx() {
    let root = workspace_root();
    let sample = root.join("assets/showcase/budget-tracker.xlsx");
    if !sample.exists() {
        return;
    }

    officecli()
        .args(["view", sample.to_string_lossy().as_ref(), "-m", "stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Sheets"));
}

#[test]
fn test_query_sample_docx() {
    let root = workspace_root();
    let sample = root.join("assets/showcase/annual-report.docx");
    if !sample.exists() {
        return;
    }

    officecli()
        .args(["query", sample.to_string_lossy().as_ref(), "p"])
        .assert()
        .success();
}

#[test]
fn test_view_sample_docx_annotated() {
    let root = workspace_root();
    let sample = root.join("assets/showcase/annual-report.docx");
    if !sample.exists() {
        return;
    }

    officecli()
        .args(["view", sample.to_string_lossy().as_ref(), "-m", "annotated"])
        .assert()
        .success()
        .stdout(predicate::str::contains("/body/"));
}

#[test]
fn test_view_sample_docx_issues() {
    let root = workspace_root();
    let sample = root.join("assets/showcase/annual-report.docx");
    if !sample.exists() {
        return;
    }

    officecli()
        .args(["view", sample.to_string_lossy().as_ref(), "-m", "issues"])
        .assert()
        .success();
}

#[test]
fn test_view_sample_xlsx_outline() {
    let root = workspace_root();
    let sample = root.join("assets/showcase/budget-tracker.xlsx");
    if !sample.exists() {
        return;
    }

    officecli()
        .args(["view", sample.to_string_lossy().as_ref(), "-m", "outline"])
        .assert()
        .success();
}

// ═══════════════════════════════════════════════════════════════════════
// XLSX-specific: set cell value
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_xlsx_view_outline() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_xlsx.xlsx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["view", &p, "-m", "outline"])
        .assert()
        .success()
        .stdout(predicate::str::contains("/Sheet1"));
}

#[test]
fn test_xlsx_modern_formula_ooxml_round_trip() {
    let tmp = temp_dir();
    let path = tmp.path().join("modern_formula.xlsx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/Sheet1",
            "--type-name",
            "cell",
            "--properties",
            "ref=A1",
            "formula=SEQUENCE(2)",
        ])
        .assert()
        .success();
    officecli()
        .args(["set", &p, "/Sheet1/B1", "formula=SEQUENCE(3)"])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "xl/worksheets/sheet1.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_xlfn.SEQUENCE(2)"))
        .stdout(predicate::str::contains("cm=\"1\""))
        .stdout(predicate::str::contains("t=\"array\" ref=\"A1\""))
        .stdout(predicate::str::contains(
            r#"<c r="B1" cm="1"><f t="array" ref="B1">_xlfn.SEQUENCE(3)</f>"#,
        ));
    officecli()
        .args(["raw", &p, "xl/metadata.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("name=\"XLDAPR\""))
        .stdout(predicate::str::contains("xda:dynamicArrayProperties"));
    officecli()
        .args(["raw", &p, "xl/_rels/workbook.xml.rels"])
        .assert()
        .success()
        .stdout(predicate::str::contains("relationships/sheetMetadata"));
    officecli()
        .args(["--json", "get", &p, "/Sheet1/A1"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""formula": "SEQUENCE(2)""#));
}

#[test]
fn test_xlsx_workbook_settings_root_lifecycle() {
    let tmp = temp_dir();
    let path = tmp.path().join("workbook_settings.xlsx");
    let p = path.to_string_lossy().to_string();
    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/",
            "--type-name",
            "sheet",
            "--properties",
            "name=Second",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            &p,
            "/",
            "date1904=true",
            "codeName=Ledger",
            "calc.mode=manual",
            "calc.iterate=true",
            "calc.iterateCount=50",
            "calc.iterateDelta=0.01",
            "calc.fullPrecision=false",
            "calc.refMode=R1C1",
            "activeTab=Second",
            "firstSheet=1",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""workbook.date1904": true"#))
        .stdout(predicate::str::contains(r#""workbook.codeName": "Ledger""#))
        .stdout(predicate::str::contains(r#""calc.mode": "manual""#))
        .stdout(predicate::str::contains(r#""activeTab": "1""#));
    officecli()
        .args(["raw", &p, "xl/workbook.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<workbookPr"))
        .stdout(predicate::str::contains(r#"date1904="1""#))
        .stdout(predicate::str::contains(r#"codeName="Ledger""#))
        .stdout(predicate::str::contains("<calcPr"))
        .stdout(predicate::str::contains(r#"calcMode="manual""#))
        .stdout(predicate::str::contains(r#"iterateCount="50""#))
        .stdout(predicate::str::contains(r#"refMode="R1C1""#));
    officecli()
        .args(["set", &p, "/", "workbook.password=secret"])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""workbook.password": "***""#))
        .stdout(predicate::str::contains("workbook.passwordHash"));
    officecli()
        .args(["set", &p, "/", "workbook.password=none"])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "xl/workbook.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("workbookProtection").not());
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_xlsx_password_clear_keeps_session_explicit_structure_lock() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_xlsx_password_explicit_lock.xlsx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    // Batch deliberately keeps one handler open, matching the C# field's
    // session lifetime. The final password clear must only remove the lock it
    // auto-implied, never the caller's prior explicit structure lock.
    let batch_json = r#"[
        {"command":"set","path":"/","props":{"workbook.lockStructure":"true"}},
        {"command":"set","path":"/","props":{"workbook.password":"secret"}},
        {"command":"set","path":"/","props":{"workbook.password":"none"}}
    ]"#;
    officecli()
        .args(["batch", &p, batch_json])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "xl/workbook.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<workbookProtection"))
        .stdout(predicate::str::contains(r#"lockStructure="1""#))
        .stdout(predicate::str::contains("workbookPassword=").not());
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_xlsx_in_cell_rich_value_image_lifecycle() {
    let tmp = temp_dir();
    let path = tmp.path().join("in_cell_image.xlsx");
    let image = tmp.path().join("pixel.png");
    let p = path.to_string_lossy().to_string();
    std::fs::write(
        &image,
        [
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0, b'I', b'E', b'N', b'D',
            0xae, 0x42, 0x60, 0x82,
        ],
    )
    .unwrap();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/Sheet1",
            "--type-name",
            "cell",
            "--properties",
            "ref=B2",
            &format!("image={}", image.display()),
            "alt=Product photo",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/Sheet1",
            "--type-name",
            "cell",
            "--properties",
            "ref=A1",
            "formula=SEQUENCE(2)",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/Sheet1",
            "--type-name",
            "cell",
            "--properties",
            "ref=C3",
            &format!("image={}", image.display()),
        ])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "xl/worksheets/sheet1.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#"t="e" vm="1""#))
        .stdout(predicate::str::contains(r#"t="e" vm="2""#))
        .stdout(predicate::str::contains("#VALUE!"));
    officecli()
        .args(["raw", &p, "xl/metadata.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("XLRICHVALUE"))
        .stdout(predicate::str::contains("XLDAPR"))
        .stdout(predicate::str::contains("rvb"));
    officecli()
        .args(["raw", &p, "xl/richData/rdrichvalue.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Product photo"));
    officecli()
        .args(["--json", "get", &p, "/Sheet1/B2"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""type": "Image""#))
        .stdout(predicate::str::contains(r#""text": "[image]""#))
        .stdout(predicate::str::contains(r#""alt": "Product photo""#))
        .stdout(predicate::str::contains(
            r#""image.contentType": "image/png""#,
        ));
    officecli()
        .args(["--json", "query", &p, "type=image"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""path": "/Sheet1/B2""#))
        .stdout(predicate::str::contains(r#""type": "Image""#));
    officecli()
        .args(["get", &p, "/Sheet1/B2:image.fileSize"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Text: 20"));
    officecli()
        .args(["set", &p, "/Sheet1/B2", "image=none"])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "xl/worksheets/sheet1.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#"<c r="B2"/>"#))
        .stdout(predicate::str::contains(r#"<c r="C3" t="e" vm="2""#));
}

#[test]
fn test_xlsx_detected_table_query_and_row_predicate() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_xlsx_detected_table.xlsx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    let add_cell = |cell_ref: &str, value: &str| {
        officecli()
            .args([
                "add",
                &p,
                "--parent",
                "/Sheet1",
                "--type-name",
                "cell",
                "--properties",
                &format!("ref={cell_ref}"),
                &format!("value={value}"),
            ])
            .assert()
            .success();
    };
    add_cell("A1", "Name");
    add_cell("B1", "Amount, USD");
    add_cell("A2", "Ada");
    add_cell("B2", "12");

    officecli()
        .args(["--json", "query", &p, "table"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""type": "detectedtable""#))
        .stdout(predicate::str::contains(r#""path": "/Sheet1/A1:B2""#))
        .stdout(predicate::str::contains(r#""stable": false"#));

    officecli()
        .args(["--json", "query", &p, "row[Amount, USD > 10]"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""path": "/Sheet1/row[2]""#))
        .stdout(predicate::str::contains(r#""tableSource": "detected""#));

    officecli()
        .args(["--json", "query", &p, "listobject"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"matches\": 0"));

    officecli()
        .args(["help", "xlsx", "detectedtable"])
        .assert()
        .success()
        .stdout(predicate::str::contains("header-sniff"));
}

#[test]
fn test_xlsx_sheet_order_mutations_preserve_defined_name_scopes() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_xlsx_sheet_order.xlsx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/",
            "--type-name",
            "sheet",
            "--properties",
            "name=Second",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/",
            "--type-name",
            "sheet",
            "--position",
            "1",
            "--properties",
            "name=Inserted",
        ])
        .assert()
        .success();

    // Inject three sheet-scoped names so the following CLI mutations exercise
    // localSheetId remapping rather than only visible sheet order.
    {
        let mut package = oxml::OxmlPackage::open(&p, true).unwrap();
        let workbook = package.read_part_xml("xl/workbook.xml").unwrap();
        let defined_names = r#"<definedNames>
<definedName name="scopeSheet1" localSheetId="0">Sheet1!$A$1</definedName>
<definedName name="scopeInserted" localSheetId="1">Inserted!$A$1</definedName>
<definedName name="scopeSecond" localSheetId="2">Second!$A$1</definedName>
</definedNames>"#;
        let updated = workbook.replace("</workbook>", &format!("{}</workbook>", defined_names));
        package.write_part_xml("xl/workbook.xml", &updated).unwrap();
        package.save().unwrap();
    }

    officecli()
        .args(["move", &p, "/Sheet1", "--position", "after:/Second"])
        .assert()
        .success();

    {
        let package = oxml::OxmlPackage::open(&p, false).unwrap();
        let workbook = package.read_part_xml("xl/workbook.xml").unwrap();
        let inserted = workbook.find(r#"name="Inserted""#).unwrap();
        let second = workbook.find(r#"name="Second""#).unwrap();
        let sheet1 = workbook.find(r#"name="Sheet1""#).unwrap();
        assert!(inserted < second && second < sheet1);
        assert!(workbook.contains(r#"name="scopeSheet1" localSheetId="2""#));
        assert!(workbook.contains(r#"name="scopeInserted" localSheetId="0""#));
        assert!(workbook.contains(r#"name="scopeSecond" localSheetId="1""#));
    }

    officecli()
        .args(["remove", &p, "/Second"])
        .assert()
        .success();
    officecli()
        .args(["validate", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Validation passed: no errors found.",
        ));

    let package = oxml::OxmlPackage::open(&p, false).unwrap();
    let workbook = package.read_part_xml("xl/workbook.xml").unwrap();
    assert!(!workbook.contains(r#"name="Second""#));
    assert!(!workbook.contains(r#"name="scopeSecond""#));
    assert!(workbook.contains(r#"name="scopeSheet1" localSheetId="1""#));
    assert!(workbook.contains(r#"name="scopeInserted" localSheetId="0""#));
}

// ═══════════════════════════════════════════════════════════════════════
// PPTX-specific: add slide + textbox
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_pptx_add_slide() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_pptx.pptx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();

    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/presentation",
            "--type-name",
            "slide",
        ])
        .assert()
        .success();

    officecli()
        .args(["view", &p, "-m", "stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Slides"));
}

#[test]
fn test_pptx_raw_set_accepts_semantic_slide_path() {
    let tmp = temp_dir();
    let path = tmp.path().join("raw_slide.pptx");
    let p = path.to_string_lossy().to_string();
    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "raw-set",
            &p,
            "/slide[1]",
            "--xpath",
            "/sld",
            "--action",
            "setattr",
            "--xml",
            "showMasterSp=0",
        ])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "/slide[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("showMasterSp=\"0\""));
}

#[test]
fn test_pptx_raw_accepts_csharp_semantic_presentation_part_paths() {
    let tmp = temp_dir();
    let path = tmp.path().join("raw_semantic.pptx");
    let p = path.to_string_lossy().to_string();
    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["raw", &p, "/presentation"])
        .assert()
        .success()
        .stdout(predicate::str::contains("presentation"));
    officecli()
        .args(["raw", &p, "/theme"])
        .assert()
        .success()
        .stdout(predicate::str::contains("theme"));
    officecli()
        .args(["add", &p, "/slide[1]", "--type", "note"])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "/noteSlide[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("notes"));
    officecli()
        .args([
            "raw-set",
            &p,
            "/presentation",
            "--xpath",
            "/presentation",
            "--action",
            "setattr",
            "--xml",
            "firstSlideNum=7",
        ])
        .assert()
        .success();
    officecli()
        .args(["raw", &p, "/presentation"])
        .assert()
        .success()
        .stdout(predicate::str::contains("firstSlideNum=\"7\""));
}

#[test]
fn test_pptx_presentation_settings_lifecycle() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_pptx_presentation_settings.pptx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "set",
            &p,
            "/presentation",
            "firstSlideNum=4",
            "rtl=true",
            "compatMode=true",
            "removePersonalInfo=true",
            "print.what=handouts",
            "print.colorMode=grayscale",
            "print.hiddenSlides=true",
            "show.loop=true",
            "show.narration=false",
            "show.animation=true",
            "show.useTimings=false",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/presentation"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""firstSlideNum": "4""#))
        .stdout(predicate::str::contains(r#""direction": "rtl""#))
        .stdout(predicate::str::contains(r#""print.what": "handouts1""#))
        .stdout(predicate::str::contains(r#""show.narration": false"#));
    officecli()
        .args(["--json", "get", &p, "/"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""firstSlideNum": "4""#));
    officecli()
        .args(["raw", &p, "ppt/presProps.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<p:prnPr"))
        .stdout(predicate::str::contains(r#"prnWhat="handouts1""#))
        .stdout(predicate::str::contains(r#"clr="gray""#))
        .stdout(predicate::str::contains(r#"hiddenSlides="1""#))
        .stdout(predicate::str::contains("<p:showPr"))
        .stdout(predicate::str::contains(r#"loop="1""#))
        .stdout(predicate::str::contains(r#"showNarration="0""#))
        .stdout(predicate::str::contains(r#"showAnimation="1""#))
        .stdout(predicate::str::contains(r#"useTimings="0""#));
    officecli()
        .args(["raw", &p, "ppt/_rels/presentation.xml.rels"])
        .assert()
        .success()
        .stdout(predicate::str::contains("relationships/presProps"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_pptx_theme_and_default_font_lifecycle() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_pptx_theme.pptx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "set",
            &p,
            "/theme",
            "accent1=#123",
            "headingFont=Heading & Body",
            "bodyFont=Body Font",
            "majorFont.ea=等线",
            "minorFont.cs=Arial",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/theme"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""accent1": "112233""#))
        .stdout(predicate::str::contains(
            r#""headingFont": "Heading & Body""#,
        ))
        .stdout(predicate::str::contains(r#""headingFont.ea": "等线""#))
        .stdout(predicate::str::contains(r#""bodyFont.cs": "Arial""#));

    // C# also exposes dotted shared-theme props and the root defaultFont alias.
    officecli()
        .args([
            "set",
            &p,
            "/",
            "theme.color.accent2=ABC",
            "theme.font.major.eastAsia=宋体",
            "defaultFont=Tahoma",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/slide[1]",
            "--type-name",
            "shape",
            "--properties",
            "text=Themed preview",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "get", &p, "/theme"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""accent2": "AABBCC""#))
        .stdout(predicate::str::contains(r#""headingFont": "Tahoma""#))
        .stdout(predicate::str::contains(r#""bodyFont": "Tahoma""#))
        .stdout(predicate::str::contains(r#""headingFont.ea": "宋体""#));
    officecli()
        .args(["--json", "get", &p, "/"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#""theme.color.accent2": "AABBCC""#,
        ))
        .stdout(predicate::str::contains(
            r#""theme.font.major.latin": "Tahoma""#,
        ));
    officecli()
        .args(["raw", &p, "ppt/theme/theme1.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#"<a:accent1><a:srgbClr val="112233""#,
        ))
        .stdout(predicate::str::contains(r#"typeface="Tahoma""#));
    officecli()
        .args(["view", &p, "-m", "html"])
        .assert()
        .success()
        .stdout(predicate::str::contains("font-family:'Tahoma',sans-serif"));
    officecli().args(["validate", &p]).assert().success();
}

#[test]
fn test_pptx_linebreak_add_get_remove_lifecycle() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_pptx_linebreak.pptx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/presentation",
            "--type-name",
            "slide",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/slide[1]",
            "--type-name",
            "textbox",
            "--properties",
            "text=before",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/slide[1]/shape[1]/paragraph[1]",
            "--type-name",
            "br",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "/slide[1]/shape[1]/paragraph[1]/br[1]",
        ));
    officecli()
        .args(["--json", "get", &p, "/slide[1]/shape[1]/paragraph[1]/br[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"type\": \"linebreak\""));
    officecli()
        .args(["remove", &p, "/slide[1]/shape[1]/paragraph[1]/br[1]"])
        .assert()
        .success();
    officecli()
        .args(["get", &p, "/slide[1]/shape[1]/paragraph[1]/br[1]"])
        .assert()
        .failure();
    officecli().args(["validate", &p]).assert().success();
    officecli()
        .args(["help", "pptx", "linebreak"])
        .assert()
        .success()
        .stdout(predicate::str::contains("linebreak"));
}

#[test]
fn test_pptx_modern_comment_thread_lifecycle() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_pptx_modern_comments.pptx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/slide[1]",
            "--type-name",
            "modernComment",
            "--properties",
            "text=Root",
            "author=Ada Lovelace",
            "created=2026-01-02T03:04:05+08:00",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("/slide[1]/modernComment[1]"));
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/slide[1]",
            "--type-name",
            "modernComment",
            "--properties",
            "text=Reply",
            "parent=/slide[1]/modernComment[1]",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "/slide[1]/modernComment[1]/reply[1]",
        ));
    officecli()
        .args(["--json", "get", &p, "/slide[1]/modernComment[1]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"child_count\": 1"))
        .stdout(predicate::str::contains("2026-01-01T19:04:05Z"));
    officecli()
        .args([
            "set",
            &p,
            "/slide[1]/modernComment[1]",
            "text=Edited",
            "resolved=true",
        ])
        .assert()
        .success();
    officecli()
        .args(["--json", "query", &p, "modernComment"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Edited"))
        .stdout(predicate::str::contains("\"resolved\": true"));
    officecli()
        .args(["remove", &p, "/slide[1]/modernComment[1]/reply[1]"])
        .assert()
        .success();
    officecli()
        .args(["remove", &p, "/slide[1]/modernComment[1]"])
        .assert()
        .success();
    officecli().args(["validate", &p]).assert().success();
    officecli()
        .args(["help", "pptx", "modernComment"])
        .assert()
        .success()
        .stdout(predicate::str::contains("modernComment"));
}

#[test]
fn test_pptx_remove_middle_slide_then_edit_and_add() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_pptx_remove_middle.pptx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    for _ in 0..2 {
        officecli()
            .args([
                "add",
                &p,
                "--parent",
                "/presentation",
                "--type-name",
                "slide",
            ])
            .assert()
            .success();
    }

    officecli()
        .args(["remove", &p, "/slide[2]"])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/slide[2]",
            "--type-name",
            "rectangle",
            "--properties",
            "text=logical-slide-two",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/presentation",
            "--type-name",
            "slide",
        ])
        .assert()
        .success();

    officecli()
        .args(["validate", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Validation passed: no errors found.",
        ));
    officecli()
        .args(["view", &p, "-m", "stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Slides: 3"))
        .stdout(predicate::str::contains("Shapes: 1"));
}

#[test]
fn test_pptx_group_resize_axes_and_keep_aspect() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_pptx_group_resize.pptx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/slide[1]",
            "--type-name",
            "group",
            "--properties",
            "name=Diagram",
            "x=100",
            "y=200",
            "width=1000",
            "height=500",
        ])
        .assert()
        .success();

    officecli()
        .args(["set", &p, "/slide[1]/group[1]", "width=2000"])
        .assert()
        .success();

    {
        let package = oxml::OxmlPackage::open(&p, false).unwrap();
        let slide = package.read_part_xml("ppt/slides/slide1.xml").unwrap();
        assert!(slide.contains(r#"<a:ext cx="2000" cy="500"/>"#));
        assert!(slide.contains(r#"<a:chExt cx="1000" cy="500"/>"#));
    }

    officecli()
        .args([
            "set",
            &p,
            "/slide[1]/group[1]",
            "height=1000",
            "keepAspect=true",
        ])
        .assert()
        .success();

    let package = oxml::OxmlPackage::open(&p, false).unwrap();
    let slide = package.read_part_xml("ppt/slides/slide1.xml").unwrap();
    assert!(slide.contains(r#"<a:ext cx="4000" cy="1000"/>"#));
}

// ═══════════════════════════════════════════════════════════════════════
// Convert — docx → docx (re-save via oxide engine)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_convert_docx_resave() {
    let tmp = temp_dir();
    let src = tmp.path().join("convert_src.docx");
    let s = src.to_string_lossy().to_string();

    officecli().args(["create", &s]).assert().success();
    officecli()
        .args(["convert", &s, "--engine", "oxide", "--force"])
        .assert()
        .success();
}

#[test]
fn test_convert_pdf_to_docx_preserves_extractable_text() {
    let tmp = temp_dir();
    let src = workspace_root().join("examples/test.pdf");
    let dst = tmp.path().join("converted_pdf.docx");
    let src = src.to_string_lossy().to_string();
    let dst = dst.to_string_lossy().to_string();

    officecli()
        .args(["convert", &src, "-o", &dst, "--force"])
        .assert()
        .success();

    officecli()
        .args(["view", &dst, "-m", "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello World from OfficeCLI"))
        .stdout(predicate::str::contains("Second line of text"));
}

/// PDF→DOCX drives LibreOffice (two-hop bridge), which gets a private user
/// profile per invocation. Running several conversions in parallel must all
/// succeed — if the profile isolation regressed, concurrent `soffice`
/// processes would block on the shared default-profile lock and these would
/// hang or fail.
#[test]
fn test_convert_pdf_to_docx_concurrent_isolated() {
    let tmp = temp_dir();
    let src = workspace_root().join("examples/test.pdf");
    let src = src.to_string_lossy().to_string();

    let handles: Vec<_> = (0..3)
        .map(|i| {
            let src = src.clone();
            let dst = tmp
                .path()
                .join(format!("concurrent_{i}.docx"))
                .to_string_lossy()
                .to_string();
            std::thread::spawn(move || {
                officecli()
                    .args(["convert", &src, "-o", &dst, "--force"])
                    .assert()
                    .success();
                officecli()
                    .args(["view", &dst, "-m", "text"])
                    .assert()
                    .success()
                    .stdout(predicate::str::contains("Hello World from OfficeCLI"));
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("conversion thread panicked");
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Raw-set — modify a part's raw XML
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_raw_set_docx() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_rawset.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();

    // Use raw-set with 'setattr' action to set an attribute
    officecli()
        .args([
            "raw-set",
            &p,
            "word/document.xml",
            "/w:document",
            "setattr",
            "--xml",
            "w:rsidR=00000000",
        ])
        .assert()
        .success();

    officecli()
        .args([
            "raw-set",
            &p,
            "word/document.xml",
            "--xpath",
            "/w:document",
            "--action",
            "setattr",
            "--xml",
            "w:rsidDel=00000001",
        ])
        .assert()
        .success();
}

#[test]
fn test_add_part_accepts_csharp_type_and_json_envelope() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_add_part.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["--json", "add-part", &p, "/", "--type", "header"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"success\": true"))
        .stdout(predicate::str::contains("\"relId\""))
        .stdout(predicate::str::contains("Created header part"));
}

// ═══════════════════════════════════════════════════════════════════════
// Batch — run multiple operations
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_batch_docx() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_batch.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();

    // Batch: add paragraph then view stats
    let batch_json = r#"[{"command":"add","parent":"/body","type":"paragraph","properties":{"text":"Batch test"}},{"command":"view","mode":"stats"}]"#;

    officecli()
        .args(["batch", &p, batch_json])
        .assert()
        .success();
}

#[test]
fn test_batch_accepts_csharp_command_envelope() {
    let tmp = temp_dir();
    let path = tmp.path().join("batch_envelope.docx");
    let p = path.to_string_lossy().to_string();
    officecli().args(["create", &p]).assert().success();
    let envelope = r#"{"commands":[{"command":"set","path":"/body/p[1]","props":{"text":"Envelope replay"}}]}"#;
    officecli().args(["batch", &p, envelope]).assert().success();
    officecli()
        .args(["view", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains("Envelope replay"));
}

#[test]
fn test_batch_envelope_stop_on_error_false_continues() {
    let tmp = temp_dir();
    let path = tmp.path().join("batch_stop_on_error.docx");
    let p = path.to_string_lossy().to_string();
    officecli().args(["create", &p]).assert().success();
    let envelope = r#"{"stopOnError":false,"commands":[{"command":"set","path":"/missing","props":{"text":"ignored"}},{"command":"set","path":"/body/p[1]","props":{"text":"Continued"}}]}"#;
    officecli()
        .args(["batch", &p, envelope, "--best-effort"])
        .assert()
        .success();
    officecli()
        .args(["view", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains("Continued"));
}

#[test]
fn test_batch_accepts_csharp_dump_meta_and_raw_set_items() {
    let tmp = temp_dir();
    let path = tmp.path().join("batch_raw_set.docx");
    let p = path.to_string_lossy().to_string();
    officecli().args(["create", &p]).assert().success();
    let batch = r#"[
      {"command":"meta","dumpVersion":2},
      {"command":"raw-set","part":"/document","xpath":"/w:document","action":"setattr","xml":"w:rsidR=12345678"}
    ]"#;
    officecli().args(["batch", &p, batch]).assert().success();
    officecli()
        .args(["raw", &p, "/document"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rsidR=\"12345678\""));
}

#[test]
fn test_batch_docx_from_commands_file() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_batch_file.docx");
    let p = path.to_string_lossy().to_string();
    let commands_path = tmp.path().join("batch.json");
    let commands = r#"[{"command":"add","parent":"/body","type":"paragraph","properties":{"text":"Batch file test"}}]"#;
    std::fs::write(&commands_path, commands).unwrap();
    let commands_file = commands_path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["batch", &p, "--commands-file", &commands_file])
        .assert()
        .success();

    officecli()
        .args(["view", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains("Batch file test"));
}

#[test]
fn test_batch_docx_from_stdin() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_batch_stdin.docx");
    let p = path.to_string_lossy().to_string();
    let commands = r#"[{"command":"add","parent":"/body","type":"paragraph","properties":{"text":"Batch stdin test"}}]"#;

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["batch", &p, "--stdin"])
        .write_stdin(commands)
        .assert()
        .success();

    officecli()
        .args(["view", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains("Batch stdin test"));
}

#[test]
fn test_batch_docx_range_paths_with_props_replaces_text() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_batch_range_paths.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["set", &p, "/body/p[1]", "text=abcdef"])
        .assert()
        .success();

    let batch_json = r#"[{"command":"set","range_paths":"/body/p[1][1..4]","props":{"text":"X"}}]"#;

    officecli()
        .args(["batch", &p, batch_json, "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"Ok\": \"OK\""));

    officecli()
        .args(["view", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains("aXef"));
}

#[test]
fn test_batch_docx_range_paths_supports_run_paths() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_batch_run_paths.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["set", &p, "/body/p[1]", "text=abcdef"])
        .assert()
        .success();

    let batch_json = r#"[{"command":"set","range_paths":"/body/p[1]/r[1]","props":{"text":"X"}}]"#;

    officecli()
        .args(["batch", &p, batch_json, "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"Ok\": \"OK\""));

    officecli()
        .args(["view", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains("X"));
}

#[test]
fn test_batch_docx_set_range_paths_applies_multiple_format_ops() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_batch_range_format.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["set", &p, "/body/p[1]", "text=abcdef"])
        .assert()
        .success();

    let batch_json = r#"[
        {"command":"set","range_paths":"/body/p[1][0..2]","props":{"color":"FF4340","bgColor":"FFFDEB"}},
        {"command":"set","range_paths":"/body/p[1][2..4]","props":{"color":"0070C0","bgColor":"EAF4FF"}}
    ]"#;

    officecli()
        .args(["batch", &p, batch_json, "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"Ok\": \"OK\""));

    officecli()
        .args(["raw", &p, "word/document.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FF4340"))
        .stdout(predicate::str::contains("0070C0"))
        .stdout(predicate::str::contains("FFFDEB"))
        .stdout(predicate::str::contains("EAF4FF"));
}

#[test]
fn test_batch_docx_set_range_paths_applies_multiple_text_replacements() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_batch_range_replace.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["set", &p, "/body/p[1]", "text=abcdef"])
        .assert()
        .success();

    // Replacements are ordered from the end of the paragraph to the start,
    // matching the Java adapter's original-coordinate batch contract.
    let batch_json = r#"[
        {"command":"set","range_paths":"/body/p[1][4..6]","props":{"text":"Y","color":"FF4340"}},
        {"command":"set","range_paths":"/body/p[1][1..3]","props":{"text":"X","bgColor":"FFFDEB"}}
    ]"#;

    officecli()
        .args(["batch", &p, batch_json, "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"Ok\": \"OK\""));

    officecli()
        .args(["view", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains("aXdY"));
    officecli()
        .args(["raw", &p, "word/document.xml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FF4340"))
        .stdout(predicate::str::contains("FFFDEB"));
}

#[test]
fn test_batch_docx_set_range_paths_supports_hyperlink_paths() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_batch_hyperlink_range_set.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[1]",
            "--type-name",
            "hyperlink",
            "--properties",
            "text=abcdef",
            "url=https://example.com",
        ])
        .assert()
        .success();

    let batch_json =
        r#"[{"command":"set","range_paths":"/body/p[1]/hyperlink[1][1..4]","props":{"text":"X"}}]"#;

    officecli()
        .args(["batch", &p, batch_json, "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"Ok\": \"OK\""));

    officecli()
        .args(["view", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains("aXef"));
}

#[test]
fn test_batch_docx_bookmark_range_paths_supports_hyperlink_paths() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_batch_hyperlink_range_bookmark.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body/p[1]",
            "--type-name",
            "hyperlink",
            "--properties",
            "text=abcdef",
            "url=https://example.com",
        ])
        .assert()
        .success();

    let batch_json = r#"[{"command":"add","parent":"/body/p[1]","type":"bookmark","properties":{"name":"DSN_LINK"},"range_paths":"/body/p[1]/hyperlink[1][1..4]"}]"#;

    officecli()
        .args(["batch", &p, batch_json, "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"Ok\": \"created:"))
        .stdout(predicate::str::contains("DSN_LINK"));

    officecli()
        .args([
            "get",
            &p,
            "/body/p[1]/hyperlink[1]",
            "--depth",
            "3",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("bookmarkStart"))
        .stdout(predicate::str::contains("bookmarkEnd"))
        .stdout(predicate::str::contains("bcd"));
}

#[test]
fn test_batch_docx_bookmark_range_paths_supports_table_cell_paths() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_batch_table_cell_bookmark.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "table",
            "--properties",
            "rows=1",
            "cols=1",
            "r1c1=abcdef",
        ])
        .assert()
        .success();

    let batch_json = r#"[{"command":"add","parent":"/body/p[1]","type":"bookmark","properties":{"name":"DSN_CELL"},"range_paths":"/body/tbl[1]/tr[1]/tc[1][1..4]"}]"#;

    officecli()
        .args(["batch", &p, batch_json, "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"Ok\": \"created:"))
        .stdout(predicate::str::contains("DSN_CELL"));

    officecli()
        .args([
            "get",
            &p,
            "/body/tbl[1]/tr[1]/tc[1]/p[1]",
            "--depth",
            "2",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("bookmarkStart"))
        .stdout(predicate::str::contains("bookmarkEnd"))
        .stdout(predicate::str::contains("bcd"));
}

#[test]
fn test_batch_docx_bookmark_range_paths_applies_multiple_ops() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_batch_multiple_bookmarks.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=abcdef",
        ])
        .assert()
        .success();

    let batch_json = r#"[
        {"command":"add","parent":"/body/p[2]","type":"bookmark","properties":{"name":"DSN_MULTI_1"},"range_paths":"/body/p[2][0..2]"},
        {"command":"add","parent":"/body/p[2]","type":"bookmark","properties":{"name":"DSN_MULTI_2"},"range_paths":"/body/p[2][2..4]"}
    ]"#;

    officecli()
        .args(["batch", &p, batch_json, "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("DSN_MULTI_1"))
        .stdout(predicate::str::contains("DSN_MULTI_2"));

    officecli()
        .args(["get", &p, "/body/p[2]", "--depth", "4", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("bookmarkStart[1]"))
        .stdout(predicate::str::contains("bookmarkStart[2]"))
        .stdout(predicate::str::contains("bookmarkEnd[1]"))
        .stdout(predicate::str::contains("bookmarkEnd[2]"));
}

#[test]
fn test_batch_docx_bookmark_range_paths_ignores_virtual_table_separators() {
    let tmp = temp_dir();
    let path = tmp
        .path()
        .join("test_batch_table_cell_separator_bookmark.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "table",
            "--properties",
            "rows=1",
            "cols=2",
            "r1c1=abcdef",
            "r1c2=uvwxyz",
        ])
        .assert()
        .success();

    let batch_json = r#"[{"command":"add","parent":"/body/p[1]","type":"bookmark","properties":{"name":"DSN_CELL_SEP"},"range_paths":"/body/tbl[1]/tr[1]/tc[1][1..4],/body/tbl[1]/tr[1]/tc[1]/sep,/body/tbl[1]/tr[1]/tc[2][0..3]"}]"#;

    officecli()
        .args(["batch", &p, batch_json, "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"Ok\": \"created:"))
        .stdout(predicate::str::contains("DSN_CELL_SEP"));

    officecli()
        .args([
            "get",
            &p,
            "/body/tbl[1]/tr[1]/tc[1]/p[1]",
            "--depth",
            "2",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("bookmarkStart"));

    officecli()
        .args([
            "get",
            &p,
            "/body/tbl[1]/tr[1]/tc[2]/p[1]",
            "--depth",
            "2",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("bookmarkEnd"));
}

#[test]
fn test_batch_docx_set_range_paths_supports_span_index_table_cell_paths() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_batch_span_index_table_cell_set.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "table",
            "--properties",
            "rows=1",
            "cols=1",
            "r1c1=abcdef",
        ])
        .assert()
        .success();

    let batch_json = r#"[{"command":"set","range_paths":"/body/p[3][1..4]","props":{"text":"X"}}]"#;

    officecli()
        .args(["batch", &p, batch_json, "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"Ok\": \"OK\""));

    officecli()
        .args(["view", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains("aXef"));
}

#[test]
fn test_batch_docx_set_range_paths_suffix_fallback_for_stale_offsets() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_batch_stale_offsets.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["set", &p, "/body/p[1]", "text=prefix:1234567890"])
        .assert()
        .success();

    let batch_json =
        r#"[{"command":"set","range_paths":"/body/p[1][614..624]","props":{"text":"[DATE]"}}]"#;

    officecli()
        .args(["batch", &p, batch_json, "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"Ok\": \"OK\""));

    officecli()
        .args(["view", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains("prefix:[DATE]"));
}

// ═══════════════════════════════════════════════════════════════════════
// Save — explicit save (create already saves, but test the command)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_save_docx() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_save.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["save", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains("is already saved to disk."));
}

// ═══════════════════════════════════════════════════════════════════════
// Move — reorder elements
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn test_move_paragraph() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_move.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();

    // Add two paragraphs
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=First",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=Second",
        ])
        .assert()
        .success();

    // C# syntax infers /body from the sibling anchor and does not need --to.
    officecli()
        .args(["move", &p, "/body/p[3]", "--before", "/body/p[2]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Moved to /body/p[2]"));
    officecli()
        .args(["get", &p, "/body/p[2]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Second"));

    officecli()
        .args([
            "move",
            &p,
            "/body/p[2]",
            "--index",
            "0",
            "--after",
            "/body/p[1]",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("mutually exclusive"));
}

#[test]
fn test_get_and_query_json_use_csharp_node_envelope() {
    let tmp = temp_dir();
    let path = tmp.path().join("node_envelope.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();

    let get_output = officecli()
        .args(["--json", "get", &p, "/body/p[1]"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let get_json: serde_json::Value = serde_json::from_slice(&get_output).unwrap();
    assert_eq!(get_json["success"], true);
    assert_eq!(get_json["data"]["matches"], 1);
    assert_eq!(get_json["data"]["results"].as_array().unwrap().len(), 1);
    assert_eq!(get_json["data"]["results"][0]["path"], "/body/p[1]");

    let query_output = officecli()
        .args(["--json", "query", &p, "paragraph"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let query_json: serde_json::Value = serde_json::from_slice(&query_output).unwrap();
    let results = query_json["data"]["results"].as_array().unwrap();
    assert_eq!(query_json["success"], true);
    assert_eq!(query_json["data"]["matches"], results.len());
    let first_paragraph = results
        .iter()
        .find(|node| node["path"] == "/body/p[1]")
        .unwrap();
    assert!(first_paragraph["child_count"].as_u64().unwrap() > 0);
    assert!(!first_paragraph["children"].as_array().unwrap().is_empty());
}

#[test]
fn test_query_find_supports_csharp_literal_and_regex_filters() {
    let tmp = temp_dir();
    let path = tmp.path().join("query_find.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=Quarterly BULLET review",
        ])
        .assert()
        .success();

    officecli()
        .args(["--json", "query", &p, "paragraph", "--find", "bullet"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Quarterly BULLET review"));
    officecli()
        .args(["--json", "query", &p, "paragraph", "--find", "r\"bull.t\""])
        .assert()
        .success()
        .stdout(predicate::str::contains("Quarterly BULLET review"));
    officecli()
        .args(["query", &p, "paragraph", "--find", "r\"[\""])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid regex pattern"));
}

#[test]
fn test_json_envelopes_wrap_writes_and_failures() {
    let tmp = temp_dir();
    let path = tmp.path().join("json_envelope.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();

    let add_output = officecli()
        .args([
            "--json",
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=wrapped",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let add_json: serde_json::Value = serde_json::from_slice(&add_output).unwrap();
    assert_eq!(add_json["success"], true);
    assert_eq!(add_json["data"], "Created: /body/p[2]");
    assert_eq!(add_json["message"], "Created: /body/p[2]");
    assert_eq!(add_json["path"], "/body/p[2]");

    let error_output = officecli()
        .args(["--json", "get", &p, "/body/p[999]"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();
    let error_json: serde_json::Value = serde_json::from_slice(&error_output).unwrap();
    assert_eq!(error_json["success"], false);
    assert_eq!(error_json["error"]["code"], "not_found");
    assert!(error_json["error"]["error"]
        .as_str()
        .unwrap()
        .contains("not found"));
}

#[test]
fn test_set_json_reports_csharp_unsupported_property_warning() {
    let tmp = temp_dir();
    let path = tmp.path().join("set_warning.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    let output = officecli()
        .args(["--json", "set", &p, "/body/p[1]", "definitelyUnknown=value"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(json["success"], true);
    assert_eq!(json["warnings"][0]["code"], "unsupported_property");
    assert!(json["warnings"][0]["message"]
        .as_str()
        .unwrap()
        .contains("definitelyUnknown"));
}

#[test]
fn test_query_text_empty_result_prints_csharp_help_hint() {
    let tmp = temp_dir();
    let path = tmp.path().join("query_empty.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["query", &p, "table"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "No matches. Run 'officecli docx query' for selector syntax.",
        ));
}

#[test]
fn test_add_from_copies_existing_element_with_csharp_message() {
    let tmp = temp_dir();
    let path = tmp.path().join("add_from.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=Source",
        ])
        .assert()
        .success();
    officecli()
        .args(["add", &p, "--parent", "/body", "--from", "/body/p[2]"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Copied to /body/p[3]"));
    officecli()
        .args(["view", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains("Source\nSource"));
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--from",
            "/body/p[2]",
            "--properties",
            "text=ignored",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be combined with --from"));
}

#[test]
fn test_validate_uses_csharp_judgment_exit_and_streams() {
    let tmp = temp_dir();
    let path = tmp.path().join("validate_judgment.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["validate", &p])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Validation passed: no errors found.",
        ));

    officecli()
        .args(["set", &p, "/body/p[1]", "style=MissingStyle"])
        .assert()
        .success();
    officecli()
        .args(["validate", &p])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("Found 1 validation error(s):"))
        .stderr(predicate::str::contains("[dangling-reference]"));
    officecli()
        .args(["--json", "validate", &p])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"success\": false"))
        .stdout(predicate::str::contains("MissingStyle"));
}

#[test]
fn test_query_compact_uses_csharp_stable_docx_format() {
    let tmp = temp_dir();
    let path = tmp.path().join("query_compact.docx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args([
            "add",
            &p,
            "--parent",
            "/body",
            "--type-name",
            "paragraph",
            "--properties",
            "text=Alpha\tBeta",
        ])
        .assert()
        .success();
    officecli()
        .args(["set", &p, "/body/p[2]", "style=Heading1"])
        .assert()
        .success();

    officecli()
        .args(["query", &p, "paragraph", "--compact", "--fields", "style"])
        .assert()
        .success()
        .stdout(predicate::str::contains("/body/p[1]\t[p]\t(empty)\tstyle="))
        .stdout(predicate::str::contains(
            "/body/p[2]\t[Heading1]\t\"Alpha\\tBeta\"\tstyle=",
        ))
        .stdout(predicate::str::contains("total: 2 of 2 elements"));
    officecli()
        .args(["--json", "query", &p, "paragraph", "--compact"])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "--compact is a plain-text line format",
        ));
}

#[test]
fn test_query_compact_uses_csharp_pptx_total_suffix() {
    let tmp = temp_dir();
    let path = tmp.path().join("query_compact.pptx");
    let p = path.to_string_lossy().to_string();

    officecli().args(["create", &p]).assert().success();
    officecli()
        .args(["query", &p, "shape", "--compact"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "total: 0 of 0 elements / 1 slides",
        ));
}
