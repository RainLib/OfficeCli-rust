use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00, 0x00, 0xb5, 0x1c, 0x0c,
    0x02, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0xf8, 0x0f, 0x00,
    0x01, 0x05, 0x01, 0x01, 0x27, 0x18, 0xe3, 0x66, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44,
    0xae, 0x42, 0x60, 0x82,
];

fn officecli() -> Command {
    Command::cargo_bin("officecli").unwrap()
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn import_and_extract(source: &Path, bundle: &Path, document_id: &str) -> Value {
    officecli()
        .args([
            "hdoc",
            "import",
            source.to_string_lossy().as_ref(),
            "--output",
            bundle.to_string_lossy().as_ref(),
            "--document-id",
            document_id,
            "--events",
            "ndjson",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""event":"chunk_ready""#))
        .stdout(predicate::str::contains(r#""event":"completed""#));
    officecli()
        .args([
            "hdoc",
            "validate",
            bundle.to_string_lossy().as_ref(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""valid": true"#));
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(bundle.join("manifest.json")).expect("published HCD manifest"),
    )
    .unwrap();
    let (expected_fidelity, expected_profile) =
        match source.extension().and_then(|value| value.to_str()) {
            Some("docx") => ("HIGH", "semantic-flow"),
            Some("xlsx") => ("SEMANTIC", "grid"),
            Some("pptx") => ("VISUAL", "slide-canvas"),
            Some("pdf") => ("VISUAL", "fixed-layout"),
            Some("html" | "htm" | "txt") => ("SEMANTIC", "semantic-flow"),
            other => panic!("unexpected test source format {other:?}"),
        };
    assert_eq!(manifest["fidelity"]["level"], expected_fidelity);
    assert_eq!(manifest["profile"], expected_profile);
    assert_eq!(manifest["capabilities"]["stylePatch"], false);
    assert_eq!(manifest["capabilities"]["structurePatch"], false);
    let output = officecli()
        .args([
            "hdoc",
            "extract-text",
            bundle.to_string_lossy().as_ref(),
            "--limit",
            "100",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn office_zip_contains_png(path: &Path, media_prefix: &str) -> bool {
    let file = std::fs::File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        if entry.name().starts_with(media_prefix) && !entry.is_dir() {
            let mut signature = [0u8; 8];
            if entry.read_exact(&mut signature).is_ok() && signature == ONE_PIXEL_PNG[..8] {
                return true;
            }
        }
    }
    false
}

fn office_zip_text(path: &Path, part: &str) -> String {
    let file = std::fs::File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut entry = archive.by_name(part).unwrap();
    let mut text = String::new();
    entry.read_to_string(&mut text).unwrap();
    text
}

fn office_zip_text_parts(path: &Path, prefix: &str, suffix: &str) -> Vec<String> {
    let file = std::fs::File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut parts = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        if entry.name().starts_with(prefix) && entry.name().ends_with(suffix) && !entry.is_dir() {
            let name = entry.name().to_string();
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            parts.push((name, text));
        }
    }
    parts.sort_by(|left, right| left.0.cmp(&right.0));
    parts.into_iter().map(|(_, text)| text).collect()
}

fn pdf_contains_image_xobject(path: &Path) -> bool {
    let document = lopdf::Document::load(path).unwrap();
    document.objects.values().any(|object| {
        let Ok(stream) = object.as_stream() else {
            return false;
        };
        stream
            .dict
            .get(b"Subtype")
            .and_then(|value| value.as_name_str())
            .is_ok_and(|name| name == "Image")
    })
}

fn apply_text_patch(
    root: &Path,
    bundle: &Path,
    document_id: &str,
    entry: &Value,
    start: usize,
    delete_count: usize,
    insert_text: &str,
) {
    let patch = root.join(format!("{document_id}-patch.json"));
    std::fs::write(
        &patch,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": "hcd-patch/1",
            "documentId": document_id,
            "patchId": format!("{document_id}-patch-1"),
            "baseRevision": 0,
            "operations": [{
                "op": "text.splice",
                "nodeId": entry["nodeId"],
                "start": start,
                "deleteCount": delete_count,
                "insertText": insert_text,
                "precondition": { "nodeHash": entry["nodeHash"] }
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    officecli()
        .args([
            "hdoc",
            "apply",
            bundle.to_string_lossy().as_ref(),
            "--patch",
            patch.to_string_lossy().as_ref(),
            "--expected-revision",
            "0",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""revision": 1"#));
}

fn export(source: &Path, bundle: &Path, output: &Path) {
    officecli()
        .args([
            "hdoc",
            "export",
            bundle.to_string_lossy().as_ref(),
            "--source",
            source.to_string_lossy().as_ref(),
            "--output",
            output.to_string_lossy().as_ref(),
            "--revision",
            "1",
            "--json",
        ])
        .assert()
        .success();
}

#[test]
fn xlsx_hcd_cell_patch_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.xlsx");
    let bundle = temp.path().join("bundle");
    let output = temp.path().join("output.xlsx");
    officecli()
        .args(["create", source.to_string_lossy().as_ref()])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            source.to_string_lossy().as_ref(),
            "/Sheet1/A1",
            "value=Secret 123",
        ])
        .assert()
        .success();
    let extracted = import_and_extract(&source, &bundle, "xlsx-doc");
    let entry = extracted["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "Secret 123")
        .unwrap();
    assert_eq!(entry["source"]["nodeKind"], "cell");
    apply_text_patch(temp.path(), &bundle, "xlsx-doc", entry, 7, 3, "***");
    export(&source, &bundle, &output);
    officecli()
        .args(["get", output.to_string_lossy().as_ref(), "/Sheet1/A1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Secret ***"));
}

#[test]
fn xlsx_hcd_formula_nodes_are_read_only() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("formula.xlsx");
    let bundle = temp.path().join("bundle");
    officecli()
        .args(["create", source.to_string_lossy().as_ref()])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            source.to_string_lossy().as_ref(),
            "/Sheet1/B1",
            "formula=SUM(1,2)",
        ])
        .assert()
        .success();
    let extracted = import_and_extract(&source, &bundle, "xlsx-formula-doc");
    let entry = extracted["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["source"]["paragraphId"] == "B1")
        .unwrap();
    assert_eq!(entry["source"]["editable"], false);

    let patch = temp.path().join("formula-patch.json");
    std::fs::write(
        &patch,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": "hcd-patch/1",
            "documentId": "xlsx-formula-doc",
            "patchId": "formula-patch-1",
            "baseRevision": 0,
            "operations": [{
                "op": "text.splice",
                "nodeId": entry["nodeId"],
                "start": 0,
                "deleteCount": 0,
                "insertText": "blocked",
                "precondition": { "nodeHash": entry["nodeHash"] }
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    officecli()
        .args([
            "hdoc",
            "apply",
            bundle.to_string_lossy().as_ref(),
            "--patch",
            patch.to_string_lossy().as_ref(),
            "--expected-revision",
            "0",
            "--json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("read-only"));
}

#[test]
fn xlsx_hcd_media_is_content_addressed() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("image.xlsx");
    let image = temp.path().join("pixel.png");
    let bundle = temp.path().join("bundle");
    std::fs::write(
        &image,
        [
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0, b'I', b'E', b'N', b'D',
            0xae, 0x42, 0x60, 0x82,
        ],
    )
    .unwrap();
    officecli()
        .args(["create", source.to_string_lossy().as_ref()])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            source.to_string_lossy().as_ref(),
            "--parent",
            "/Sheet1",
            "--type-name",
            "cell",
            "--properties",
            "ref=B2",
            &format!("image={}", image.display()),
            "alt=Sensitive image",
        ])
        .assert()
        .success();
    import_and_extract(&source, &bundle, "xlsx-image-doc");

    let index: Value =
        serde_json::from_slice(&std::fs::read(bundle.join("assets/index.json")).unwrap()).unwrap();
    let assets = index.as_array().unwrap();
    assert_eq!(assets.len(), 1);
    assert!(assets[0]["sourcePart"]
        .as_str()
        .unwrap()
        .starts_with("xl/media/"));
    assert!(bundle.join(assets[0]["href"].as_str().unwrap()).is_file());
    assert_eq!(assets[0]["byteLength"], 20);
}

#[test]
fn pptx_hcd_slide_patch_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.pptx");
    let bundle = temp.path().join("bundle");
    let output = temp.path().join("output.pptx");
    officecli()
        .args(["create", source.to_string_lossy().as_ref()])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            source.to_string_lossy().as_ref(),
            "/slide[1]",
            "--type",
            "shape",
            "--prop",
            "text=Slide Secret 123",
        ])
        .assert()
        .success();
    let extracted = import_and_extract(&source, &bundle, "pptx-doc");
    let entry = extracted["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "Slide Secret 123")
        .unwrap();
    apply_text_patch(temp.path(), &bundle, "pptx-doc", entry, 13, 3, "***");
    export(&source, &bundle, &output);
    officecli()
        .args(["view", output.to_string_lossy().as_ref()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Slide Secret ***"));
}

#[test]
fn pptx_hcd_table_cell_patch_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("table-source.pptx");
    let bundle = temp.path().join("table-bundle");
    let output = temp.path().join("table-output.pptx");
    officecli()
        .args(["create", source.to_string_lossy().as_ref()])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            source.to_string_lossy().as_ref(),
            "/slide[1]",
            "--type",
            "table",
            "--prop",
            "rows=2",
            "--prop",
            "cols=2",
            "--prop",
            "r1c1=Table Secret 123",
            "--prop",
            "r1c2=Public",
        ])
        .assert()
        .success();

    let extracted = import_and_extract(&source, &bundle, "pptx-table-doc");
    let entry = extracted["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "Table Secret 123")
        .unwrap();
    assert_eq!(entry["source"]["nodeKind"], "table-cell-text");
    apply_text_patch(temp.path(), &bundle, "pptx-table-doc", entry, 13, 3, "***");
    export(&source, &bundle, &output);
    officecli()
        .args([
            "raw",
            output.to_string_lossy().as_ref(),
            "ppt/slides/slide1.xml",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Table Secret ***"));
}

#[test]
fn pdf_hcd_page_patch_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let source = workspace_root().join("examples/test.pdf");
    let bundle = temp.path().join("bundle");
    let output = temp.path().join("output.pdf");
    let extracted = import_and_extract(&source, &bundle, "pdf-doc");
    let entry = extracted["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "Hello World from OfficeCLI")
        .unwrap();
    apply_text_patch(temp.path(), &bundle, "pdf-doc", entry, 17, 9, "Office***");
    export(&source, &bundle, &output);
    officecli()
        .args(["view", output.to_string_lossy().as_ref()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Hello World from Office***"));
}

#[test]
fn html_hcd_patch_roundtrip_is_source_backed_and_rust_only() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.html");
    let bundle = temp.path().join("bundle");
    let output = temp.path().join("output.html");
    std::fs::write(
        &source,
        "<!doctype html><style>.secret{color:red}</style><p class='secret'>Secret 123 &amp; 中文</p><script>window.original=true</script>",
    )
    .unwrap();
    let extracted = import_and_extract(&source, &bundle, "html-doc");
    let entry = extracted["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "Secret 123 & 中文")
        .unwrap();
    assert_eq!(entry["source"]["part"], "html/document");
    apply_text_patch(temp.path(), &bundle, "html-doc", entry, 7, 3, "<MASK>");
    export(&source, &bundle, &output);
    let result = std::fs::read_to_string(output).unwrap();
    assert!(result.contains("class='secret'"));
    assert!(result.contains("Secret &lt;MASK&gt; &amp; 中文"));
    assert!(result.contains("<script>window.original=true</script>"));
}

#[test]
fn edited_hcd_revision_semantically_exports_to_all_rust_targets_without_source() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("semantic-source.html");
    let bundle = temp.path().join("semantic-bundle");
    std::fs::write(
        &source,
        "<!doctype html><h1>HCD Cross Format</h1><p>Secret 123</p><table><tr><th>Name</th><th>Value</th></tr><tr><td>Account</td><td>6222</td></tr></table>",
    )
    .unwrap();
    let extracted = import_and_extract(&source, &bundle, "hcd-cross-format");
    let entry = extracted["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "Secret 123")
        .unwrap();
    apply_text_patch(temp.path(), &bundle, "hcd-cross-format", entry, 7, 3, "***");

    // A semantic export is deliberately independent of the original boundary
    // file. Removing it proves that no source-backed or external-office path is
    // consulted for these four targets.
    std::fs::remove_file(&source).unwrap();
    for extension in ["docx", "xlsx", "pptx", "pdf"] {
        let output = temp.path().join(format!("semantic-output.{extension}"));
        let fidelity = temp
            .path()
            .join(format!("semantic-{extension}-fidelity.json"));
        officecli()
            .args([
                "hdoc",
                "export",
                bundle.to_string_lossy().as_ref(),
                "--output",
                output.to_string_lossy().as_ref(),
                "--to",
                extension,
                "--revision",
                "1",
                "--fidelity-report",
                fidelity.to_string_lossy().as_ref(),
                "--json",
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains(r#""level": "SEMANTIC""#))
            .stdout(predicate::str::contains("HCD_CROSS_FORMAT_SEMANTIC_EXPORT"));
        officecli()
            .args(["validate", output.to_string_lossy().as_ref()])
            .assert()
            .success();
        officecli()
            .args(["view", output.to_string_lossy().as_ref()])
            .assert()
            .success()
            .stdout(predicate::str::contains("HCD Cross Format"))
            .stdout(predicate::str::contains("Secret ***"))
            .stdout(predicate::str::contains("Secret 123").not());
        match extension {
            "docx" => assert!(office_zip_text(&output, "word/document.xml").contains("<w:tbl>")),
            "xlsx" => {
                assert!(office_zip_text(&output, "xl/worksheets/sheet1.xml").contains("<c r=\"A4\""))
            }
            "pptx" => assert!(office_zip_text_parts(&output, "ppt/slides/slide", ".xml")
                .iter()
                .any(|slide| slide.contains("<a:tbl>"))),
            "pdf" => {}
            _ => unreachable!(),
        }
        let report: Value = serde_json::from_slice(&std::fs::read(&fidelity).unwrap()).unwrap();
        assert_eq!(report["level"], "SEMANTIC");
        assert!(report["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "HCD_CROSS_FORMAT_SEMANTIC_EXPORT"));
    }

    let mismatched = temp.path().join("mismatched.docx");
    officecli()
        .args([
            "hdoc",
            "export",
            bundle.to_string_lossy().as_ref(),
            "--output",
            mismatched.to_string_lossy().as_ref(),
            "--to",
            "pdf",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "does not match --output extension",
        ));
    assert!(!mismatched.exists());
}

#[test]
fn large_pptx_hcd_table_rebuilds_as_bounded_native_pptx_windows() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("large-table-source.pptx");
    let bundle = temp.path().join("large-table-bundle");
    let output = temp.path().join("large-table.pptx");
    officecli()
        .args(["create", source.to_string_lossy().as_ref()])
        .assert()
        .success();
    let mut add = vec![
        "add".to_string(),
        source.to_string_lossy().into_owned(),
        "/slide[1]".to_string(),
        "--type".to_string(),
        "table".to_string(),
        "--prop".to_string(),
        "rows=40".to_string(),
        "--prop".to_string(),
        "cols=14".to_string(),
        "--prop".to_string(),
        "x=0.5in".to_string(),
        "--prop".to_string(),
        "y=0.5in".to_string(),
        "--prop".to_string(),
        "width=9in".to_string(),
        "--prop".to_string(),
        "height=6in".to_string(),
    ];
    for row in 0..40 {
        for column in 0..14 {
            add.push("--prop".to_string());
            add.push(format!("r{}c{}=R{row}C{column}", row + 1, column + 1));
        }
    }
    officecli().args(add).assert().success();
    officecli()
        .args(["validate", source.to_string_lossy().as_ref()])
        .assert()
        .success();
    import_and_extract(&source, &bundle, "large-native-pptx-table");
    std::fs::remove_file(source).unwrap();

    officecli()
        .args([
            "hdoc",
            "export",
            bundle.to_string_lossy().as_ref(),
            "--output",
            output.to_string_lossy().as_ref(),
            "--to",
            "pptx",
            "--revision",
            "0",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""level": "SEMANTIC""#))
        .stdout(predicate::str::contains(
            "split into bounded native table slides",
        ));
    officecli()
        .args(["validate", output.to_string_lossy().as_ref()])
        .assert()
        .success();
    let slides = office_zip_text_parts(&output, "ppt/slides/slide", ".xml");
    assert_eq!(slides.len(), 6);
    assert!(slides.iter().all(|slide| slide.contains("<a:tbl>")));
    assert!(slides
        .iter()
        .all(|slide| slide.matches("<a:tr ").count() <= 18));
    assert!(slides
        .iter()
        .all(|slide| slide.matches("<a:gridCol ").count() <= 12));
    assert!(slides.iter().any(|slide| slide.contains("R39C13")));
}

#[test]
fn pptx_table_split_across_hcd_chunks_reassembles_before_rust_export() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("fragmented-table-source.pptx");
    let bundle = temp.path().join("fragmented-table-bundle");
    let output = temp.path().join("fragmented-table-output.pptx");
    officecli()
        .args(["create", source.to_string_lossy().as_ref()])
        .assert()
        .success();
    let mut add = vec![
        "add".to_string(),
        source.to_string_lossy().into_owned(),
        "/slide[1]".to_string(),
        "--type".to_string(),
        "table".to_string(),
        "--prop".to_string(),
        "rows=260".to_string(),
        "--prop".to_string(),
        "cols=2".to_string(),
        "--prop".to_string(),
        "width=9in".to_string(),
        "--prop".to_string(),
        "height=6in".to_string(),
    ];
    for row in 0..260 {
        for column in 0..2 {
            add.push("--prop".to_string());
            add.push(format!("r{}c{}=R{row}C{column}", row + 1, column + 1));
        }
    }
    officecli().args(add).assert().success();
    import_and_extract(&source, &bundle, "fragmented-native-pptx-table");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    assert!(manifest["chunkCount"].as_u64().unwrap() >= 3);
    std::fs::remove_file(source).unwrap();

    officecli()
        .args([
            "hdoc",
            "export",
            bundle.to_string_lossy().as_ref(),
            "--output",
            output.to_string_lossy().as_ref(),
            "--to",
            "pptx",
            "--revision",
            "0",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("reassembled HCD table"))
        .stdout(predicate::str::contains(
            "split into bounded native table slides",
        ));
    officecli()
        .args(["validate", output.to_string_lossy().as_ref()])
        .assert()
        .success();
    let slides = office_zip_text_parts(&output, "ppt/slides/slide", ".xml");
    assert_eq!(slides.len(), 16);
    assert!(slides.iter().all(|slide| slide.contains("<a:tbl>")));
    assert!(slides
        .iter()
        .all(|slide| slide.matches("<a:tr ").count() <= 18));
    assert!(slides.iter().any(|slide| slide.contains("R259C1")));
}

#[test]
fn hcd_content_addressed_image_exports_to_all_rust_targets_without_source() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("image-source.docx");
    let image = temp.path().join("pixel.png");
    let bundle = temp.path().join("image-bundle");
    std::fs::write(&image, ONE_PIXEL_PNG).unwrap();
    officecli()
        .args(["create", source.to_string_lossy().as_ref()])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            source.to_string_lossy().as_ref(),
            "/body/p[1]",
            "text=HCD Image Secret 123",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            source.to_string_lossy().as_ref(),
            "/body/p[1]",
            "--type",
            "image",
            "--prop",
            &format!("file={}", image.display()),
            "--prop",
            "alt=Content addressed pixel",
            "--prop",
            "width=2in",
            "--prop",
            "height=1in",
        ])
        .assert()
        .success();

    let extracted = import_and_extract(&source, &bundle, "hcd-cross-format-image");
    let entry = extracted["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "HCD Image Secret 123")
        .unwrap();
    apply_text_patch(
        temp.path(),
        &bundle,
        "hcd-cross-format-image",
        entry,
        17,
        3,
        "***",
    );

    let asset_index: Value =
        serde_json::from_slice(&std::fs::read(bundle.join("assets/index.json")).unwrap()).unwrap();
    assert_eq!(asset_index.as_array().unwrap().len(), 1);
    assert_eq!(asset_index[0]["byteLength"], ONE_PIXEL_PNG.len());

    std::fs::remove_file(&source).unwrap();
    std::fs::remove_file(&image).unwrap();
    for extension in ["docx", "xlsx", "pptx", "pdf"] {
        let output = temp.path().join(format!("image-output.{extension}"));
        let fidelity = temp.path().join(format!("image-{extension}-fidelity.json"));
        officecli()
            .args([
                "hdoc",
                "export",
                bundle.to_string_lossy().as_ref(),
                "--output",
                output.to_string_lossy().as_ref(),
                "--to",
                extension,
                "--revision",
                "1",
                "--fidelity-report",
                fidelity.to_string_lossy().as_ref(),
                "--json",
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains(r#""level": "SEMANTIC""#));
        officecli()
            .args(["validate", output.to_string_lossy().as_ref()])
            .assert()
            .success();
        officecli()
            .args(["view", output.to_string_lossy().as_ref()])
            .assert()
            .success()
            .stdout(predicate::str::contains("HCD Image Secret ***"));
        match extension {
            "docx" => {
                assert!(office_zip_contains_png(&output, "word/media/"));
                assert!(office_zip_text(&output, "word/document.xml")
                    .contains("<wp:extent cx=\"1828800\" cy=\"914400\""));
            }
            "xlsx" => {
                assert!(office_zip_contains_png(&output, "xl/media/"));
                assert!(office_zip_text(&output, "xl/drawings/drawing1.xml")
                    .contains("<a:ext cx=\"1828800\" cy=\"914400\""));
            }
            "pptx" => {
                assert!(office_zip_contains_png(&output, "ppt/media/"));
                assert!(office_zip_text(&output, "ppt/slides/slide1.xml")
                    .contains("<a:ext cx=\"1828800\" cy=\"914400\""));
            }
            "pdf" => {
                assert!(pdf_contains_image_xobject(&output));
                let document = lopdf::Document::load(&output).unwrap();
                let page = *document.get_pages().values().next().unwrap();
                let content = document.get_page_content(page).unwrap();
                assert!(String::from_utf8_lossy(&content).contains("144.00 0 0 72.00 54.00"));
            }
            _ => unreachable!(),
        }
        let report: Value = serde_json::from_slice(&std::fs::read(&fidelity).unwrap()).unwrap();
        assert!(report["preserved"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item
                .as_str()
                .unwrap_or("")
                .contains("1 of 1 content-addressed")));
    }

    let href = asset_index[0]["href"].as_str().unwrap();
    let asset_path = bundle.join(href);
    let mut corrupted = std::fs::read(&asset_path).unwrap();
    corrupted[0] ^= 0xff;
    std::fs::write(&asset_path, corrupted).unwrap();
    let rejected = temp.path().join("corrupt-asset.docx");
    officecli()
        .args([
            "hdoc",
            "export",
            bundle.to_string_lossy().as_ref(),
            "--output",
            rejected.to_string_lossy().as_ref(),
            "--to",
            "docx",
            "--revision",
            "1",
        ])
        .assert()
        .failure();
    assert!(!rejected.exists());
}

#[test]
fn pptx_hcd_picture_position_survives_source_free_rust_rebuild() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("position-source.pptx");
    let image = temp.path().join("position-pixel.png");
    let bundle = temp.path().join("position-bundle");
    let output = temp.path().join("position-output.pptx");
    std::fs::write(&image, ONE_PIXEL_PNG).unwrap();
    officecli()
        .args(["create", source.to_string_lossy().as_ref()])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            source.to_string_lossy().as_ref(),
            "/slide[1]",
            "--type",
            "shape",
            "--prop",
            "text=Positioned picture",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            source.to_string_lossy().as_ref(),
            "/slide[1]",
            "--type",
            "image",
            "--prop",
            &format!("file={}", image.display()),
            "--prop",
            "x=1in",
            "--prop",
            "y=2in",
            "--prop",
            "width=2in",
            "--prop",
            "height=1in",
        ])
        .assert()
        .success();
    import_and_extract(&source, &bundle, "pptx-position-image");
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(image).unwrap();

    officecli()
        .args([
            "hdoc",
            "export",
            bundle.to_string_lossy().as_ref(),
            "--output",
            output.to_string_lossy().as_ref(),
            "--to",
            "pptx",
            "--revision",
            "0",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""level": "SEMANTIC""#));
    officecli()
        .args(["validate", output.to_string_lossy().as_ref()])
        .assert()
        .success();
    let slide = office_zip_text(&output, "ppt/slides/slide1.xml");
    assert!(slide.contains("<a:off x=\"914400\" y=\"1828800\"/>"));
    assert!(slide.contains("<a:ext cx=\"1828800\" cy=\"914400\"/>"));
    assert!(office_zip_contains_png(&output, "ppt/media/"));
}

#[test]
fn txt_hcd_patch_roundtrip_preserves_bom_and_line_endings() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.txt");
    let bundle = temp.path().join("bundle");
    let output = temp.path().join("output.txt");
    std::fs::write(&source, b"\xef\xbb\xbfSecret 123\r\nSecond line\n").unwrap();
    let extracted = import_and_extract(&source, &bundle, "txt-doc");
    let entry = extracted["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "Secret 123")
        .unwrap();
    assert_eq!(entry["source"]["part"], "text/document");
    apply_text_patch(temp.path(), &bundle, "txt-doc", entry, 7, 3, "***");
    export(&source, &bundle, &output);
    assert_eq!(
        std::fs::read(output).unwrap(),
        b"\xef\xbb\xbfSecret ***\r\nSecond line\n"
    );
}

#[test]
fn supported_hcd_adapters_repeat_stable_ids_and_body_roots() {
    let temp = tempfile::tempdir().unwrap();
    let xlsx = temp.path().join("stable.xlsx");
    officecli()
        .args(["create", xlsx.to_string_lossy().as_ref()])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            xlsx.to_string_lossy().as_ref(),
            "/Sheet1/A1",
            "value=Stable spreadsheet",
        ])
        .assert()
        .success();

    let pptx = temp.path().join("stable.pptx");
    officecli()
        .args(["create", pptx.to_string_lossy().as_ref()])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            pptx.to_string_lossy().as_ref(),
            "/slide[1]",
            "--type",
            "shape",
            "--prop",
            "text=Stable slide",
        ])
        .assert()
        .success();

    let html = temp.path().join("stable.html");
    std::fs::write(&html, "<h1>Stable HTML</h1><p>Second node</p>").unwrap();
    let txt = temp.path().join("stable.txt");
    std::fs::write(&txt, "Stable text\nSecond line\n").unwrap();
    let pdf = workspace_root().join("examples/test.pdf");

    for (index, source) in [xlsx, pptx, pdf, html, txt].iter().enumerate() {
        let document_id = format!("stable-format-{index}");
        let first_bundle = temp.path().join(format!("stable-{index}-first"));
        let second_bundle = temp.path().join(format!("stable-{index}-second"));
        let first = import_and_extract(source, &first_bundle, &document_id);
        let second = import_and_extract(source, &second_bundle, &document_id);
        assert_eq!(first["data"]["entries"], second["data"]["entries"]);

        let first_manifest: Value =
            serde_json::from_slice(&std::fs::read(first_bundle.join("manifest.json")).unwrap())
                .unwrap();
        let second_manifest: Value =
            serde_json::from_slice(&std::fs::read(second_bundle.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(first_manifest["rootHash"], second_manifest["rootHash"]);
    }
}
