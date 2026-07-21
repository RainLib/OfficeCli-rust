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
fn test_info() {
    officecli()
        .args(["info"])
        .assert()
        .success()
        .stdout(predicate::str::contains("OfficeCLI"));
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
fn test_create_xlsx() {
    let tmp = temp_dir();
    let path = tmp.path().join("test_create.xlsx");
    let path_str = path.to_string_lossy().to_string();

    officecli().args(["create", &path_str]).assert().success();
    assert!(path.exists(), "created file should exist");
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
        .stdout(predicate::str::is_match(r"(?s)^\s*\[\s*\]\s*$").unwrap());

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
        .stdout(predicate::str::contains("No validation errors"));

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
        .stdout(predicate::str::contains("No validation errors"));
    officecli()
        .args(["view", &p, "-m", "stats"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Slides: 3"))
        .stdout(predicate::str::contains("Shapes: 1"));
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
            "mc:Ignorable=wp",
        ])
        .assert()
        .success();
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
    officecli().args(["save", &p]).assert().success();
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

    // Move p[3] before p[2] using --target
    officecli()
        .args(["move", &p, "/body/p[3]", "--target", "/body/p[2]"])
        .assert()
        .success();
}
