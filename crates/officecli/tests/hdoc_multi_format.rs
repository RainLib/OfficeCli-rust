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

const SECOND_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00, 0x00, 0xb5, 0x1c, 0x0c,
    0x02, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xfc, 0xff, 0x1f, 0x00,
    0x02, 0xeb, 0x01, 0xf5, 0x8f, 0x59, 0x97, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44,
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
            Some("html" | "htm" | "md" | "markdown" | "txt") => ("SEMANTIC", "semantic-flow"),
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

fn office_zip_contains_bytes(path: &Path, media_prefix: &str, expected: &[u8]) -> bool {
    let file = std::fs::File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        if entry.name().starts_with(media_prefix) && !entry.is_dir() {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            if bytes == expected {
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

fn first_image_map_entry(bundle: &Path) -> Value {
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    let prefix = manifest["indexPrefix"].as_str().unwrap();
    for page in 0..manifest["indexPageCount"].as_u64().unwrap() {
        let index: Value = serde_json::from_slice(
            &std::fs::read(bundle.join(prefix).join(format!("{page:06}.json"))).unwrap(),
        )
        .unwrap();
        for chunk in index["chunks"].as_array().unwrap() {
            let map: Value = serde_json::from_slice(
                &std::fs::read(bundle.join(chunk["mapHref"].as_str().unwrap())).unwrap(),
            )
            .unwrap();
            if let Some(entry) = map["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["source"]["nodeKind"] == "image")
            {
                return entry.clone();
            }
        }
    }
    panic!("bundle has no mapped image node")
}

fn bundle_html(bundle: &Path) -> String {
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    let mut output = String::new();
    for page in 0..manifest["indexPageCount"].as_u64().unwrap() {
        let index: Value = serde_json::from_slice(
            &std::fs::read(
                bundle
                    .join(manifest["indexPrefix"].as_str().unwrap())
                    .join(format!("{page:06}.json")),
            )
            .unwrap(),
        )
        .unwrap();
        for chunk in index["chunks"].as_array().unwrap() {
            output.push_str(
                &std::fs::read_to_string(bundle.join(chunk["htmlHref"].as_str().unwrap())).unwrap(),
            );
        }
    }
    output
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

fn pdf_contains_image_placement(path: &Path, width: f32, height: f32) -> bool {
    let document = lopdf::Document::load(path).unwrap();
    document.get_pages().into_values().any(|page| {
        let Ok(bytes) = document.get_page_content(page) else {
            return false;
        };
        let Ok(content) = lopdf::content::Content::decode(&bytes) else {
            return false;
        };
        content.operations.iter().any(|operation| {
            operation.operator == "cm"
                && operation.operands.len() == 6
                && operation.operands[0]
                    .as_float()
                    .is_ok_and(|value| (value.abs() - width).abs() < 0.5)
                && operation.operands[3]
                    .as_float()
                    .is_ok_and(|value| (value.abs() - height).abs() < 0.5)
        })
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
    let repeated_bundle = temp.path().join("repeated-bundle");
    std::fs::write(&image, ONE_PIXEL_PNG).unwrap();
    officecli()
        .args(["create", source.to_string_lossy().as_ref()])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            source.to_string_lossy().as_ref(),
            "/Sheet1",
            "--type",
            "image",
            "--prop",
            &format!("file={}", image.display()),
            "--prop",
            "anchor=B2",
            "--prop",
            "width=2in",
            "--prop",
            "height=1in",
            "--prop",
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
    assert_eq!(assets[0]["byteLength"], ONE_PIXEL_PNG.len());
    let copied_asset = temp.path().join("copied-pixel.png");
    officecli()
        .args([
            "hdoc",
            "get-asset",
            bundle.to_string_lossy().as_ref(),
            assets[0]["hash"].as_str().unwrap(),
            "--output",
            copied_asset.to_string_lossy().as_ref(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""byteLength": 68"#))
        .stdout(predicate::str::contains("copied-pixel.png"));
    assert_eq!(std::fs::read(&copied_asset).unwrap(), ONE_PIXEL_PNG);
    let html = bundle_html(&bundle);
    assert!(html.contains("class=\"hcd-sheet-picture\""));
    assert!(html.contains("data-hcd-node-kind=\"image\""));
    assert!(html.contains("data-hcd-editable=\"true\""));
    assert!(html.contains("data-hcd-source-part=\"xl/worksheets/sheet1.xml\""));
    assert!(html.contains("data-hcd-anchor-from=\"B2\""));
    assert!(html.contains("data-hcd-width-emu=\"1828800\""));
    assert!(html.contains("data-hcd-height-emu=\"914400\""));
    assert!(html.contains("<img src=\"asset://sha256/"));
    let styles = std::fs::read_to_string(bundle.join("styles.css")).unwrap();
    assert!(styles.contains("data-hcd-image-hitboxes"));
    assert!(styles.contains("data-hcd-text-hitboxes"));

    import_and_extract(&source, &repeated_bundle, "xlsx-image-doc");
    assert_eq!(html, bundle_html(&repeated_bundle));
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    let repeated_manifest: Value =
        serde_json::from_slice(&std::fs::read(repeated_bundle.join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["rootHash"], repeated_manifest["rootHash"]);

    let docx = temp.path().join("from-xlsx.docx");
    officecli()
        .args([
            "hdoc",
            "export",
            bundle.to_string_lossy().as_ref(),
            "--output",
            docx.to_string_lossy().as_ref(),
            "--to",
            "docx",
            "--revision",
            "0",
            "--json",
        ])
        .assert()
        .success();
    assert!(office_zip_contains_png(&docx, "word/media/"));
}

#[test]
fn hcd_image_node_replace_and_geometry_are_revisioned_and_hash_guarded() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("image.xlsx");
    let original = temp.path().join("original.png");
    let replacement = temp.path().join("replacement.png");
    let bundle = temp.path().join("bundle");
    std::fs::write(&original, ONE_PIXEL_PNG).unwrap();
    std::fs::write(&replacement, SECOND_PIXEL_PNG).unwrap();
    officecli()
        .args(["create", source.to_string_lossy().as_ref()])
        .assert()
        .success();
    officecli()
        .args([
            "add",
            source.to_string_lossy().as_ref(),
            "/Sheet1",
            "--type",
            "image",
            "--prop",
            &format!("file={}", original.display()),
            "--prop",
            "anchor=B2",
            "--prop",
            "width=2in",
            "--prop",
            "height=1in",
        ])
        .assert()
        .success();
    import_and_extract(&source, &bundle, "image-patch-doc");
    let image_entry = first_image_map_entry(&bundle);
    let node_id = image_entry["nodeId"].as_str().unwrap();
    officecli()
        .args([
            "hdoc",
            "list-images",
            bundle.to_string_lossy().as_ref(),
            "--limit",
            "1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(node_id))
        .stdout(predicate::str::contains(r#""unit": "emu""#));
    let before = officecli()
        .args([
            "hdoc",
            "get-image",
            bundle.to_string_lossy().as_ref(),
            node_id,
            "--json",
        ])
        .output()
        .unwrap();
    assert!(before.status.success());
    let before: Value = serde_json::from_slice(&before.stdout).unwrap();
    let visual_hash = before["data"]["visualHash"].as_str().unwrap();
    let original_asset_hash = before["data"]["assetHash"].as_str().unwrap();

    let staged = officecli()
        .args([
            "hdoc",
            "put-asset",
            bundle.to_string_lossy().as_ref(),
            replacement.to_string_lossy().as_ref(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(staged.status.success());
    let staged: Value = serde_json::from_slice(&staged.stdout).unwrap();
    let replacement_hash = staged["data"]["hash"].as_str().unwrap();
    assert_ne!(replacement_hash, original_asset_hash);
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(bundle.join("assets/index.json")).unwrap())
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1,
        "staging must not mutate revision 0's asset index"
    );

    let patch = temp.path().join("image-patch.json");
    std::fs::write(
        &patch,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": "hcd-patch/3",
            "documentId": "image-patch-doc",
            "patchId": "image-replace-geometry-1",
            "baseRevision": 0,
            "operations": [
                {
                    "op": "image.replace",
                    "nodeId": node_id,
                    "assetHash": replacement_hash,
                    "precondition": { "visualHash": visual_hash }
                },
                {
                    "op": "image.geometry",
                    "nodeId": node_id,
                    "geometry": { "x": 914400, "y": 457200, "width": 2743200, "height": 1371600, "unit": "emu" },
                    "precondition": { "visualHash": visual_hash }
                }
            ]
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
        .stdout(predicate::str::contains("HCD_IMAGE_PATCH_SEMANTIC_EXPORT"));

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
    let after = officecli()
        .args([
            "hdoc",
            "get-image",
            bundle.to_string_lossy().as_ref(),
            node_id,
            "--json",
        ])
        .output()
        .unwrap();
    assert!(after.status.success());
    let after: Value = serde_json::from_slice(&after.stdout).unwrap();
    assert_eq!(after["data"]["assetHash"], replacement_hash);
    assert_eq!(after["data"]["geometry"]["x"].as_f64(), Some(914400.0));
    assert_eq!(after["data"]["geometry"]["width"].as_f64(), Some(2743200.0));
    assert_ne!(after["data"]["visualHash"], visual_hash);

    officecli()
        .args([
            "hdoc",
            "get-asset",
            bundle.to_string_lossy().as_ref(),
            replacement_hash,
            "--json",
        ])
        .assert()
        .success();
    officecli()
        .args([
            "hdoc",
            "get-asset",
            bundle.to_string_lossy().as_ref(),
            replacement_hash,
            "--revision",
            "0",
            "--json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("path not found"));
    let preview = temp.path().join("revision-1.html");
    officecli()
        .args([
            "hdoc",
            "render-html",
            bundle.to_string_lossy().as_ref(),
            "--output",
            preview.to_string_lossy().as_ref(),
            "--revision",
            "1",
            "--json",
        ])
        .assert()
        .success();
    let preview = std::fs::read_to_string(preview).unwrap();
    assert!(preview.contains(replacement_hash));
    assert!(preview.contains("data-hcd-x=\"914400\""));
    assert!(preview.contains("width:288px"));

    let source_backed = temp.path().join("source-backed.xlsx");
    officecli()
        .args([
            "hdoc",
            "export",
            bundle.to_string_lossy().as_ref(),
            "--source",
            source.to_string_lossy().as_ref(),
            "--output",
            source_backed.to_string_lossy().as_ref(),
            "--revision",
            "1",
            "--json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("stopped before writing output"));
    assert!(!source_backed.exists());

    let semantic = temp.path().join("semantic.pptx");
    officecli()
        .args([
            "hdoc",
            "export",
            bundle.to_string_lossy().as_ref(),
            "--output",
            semantic.to_string_lossy().as_ref(),
            "--revision",
            "1",
            "--json",
        ])
        .assert()
        .success();
    assert!(office_zip_contains_bytes(
        &semantic,
        "ppt/media/",
        SECOND_PIXEL_PNG
    ));
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
        let expected_level = if extension == "pdf" {
            r#""level": "HIGH""#
        } else {
            r#""level": "SEMANTIC""#
        };
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
            .stdout(predicate::str::contains(expected_level))
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
        assert_eq!(
            report["level"],
            if extension == "pdf" {
                "HIGH"
            } else {
                "SEMANTIC"
            }
        );
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
        let expected_level = if extension == "pdf" {
            r#""level": "HIGH""#
        } else {
            r#""level": "SEMANTIC""#
        };
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
            .stdout(predicate::str::contains(expected_level));
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
                assert!(pdf_contains_image_placement(&output, 144.0, 72.0));
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
fn markdown_hcd_patch_roundtrip_and_semantic_text_exports() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.md");
    let bundle = temp.path().join("bundle");
    let output = temp.path().join("output.markdown");
    let text_output = temp.path().join("semantic.txt");
    std::fs::write(
        &source,
        "# Customer\n\nAccount **6222**\n\n- First\n- Second\n",
    )
    .unwrap();
    let extracted = import_and_extract(&source, &bundle, "markdown-doc");
    let entry = extracted["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "6222")
        .unwrap();
    assert_eq!(entry["source"]["part"], "markdown/document");
    apply_text_patch(temp.path(), &bundle, "markdown-doc", entry, 0, 4, "****");
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
        .success()
        .stdout(predicate::str::contains(r#""level": "HIGH""#));
    let markdown = std::fs::read_to_string(&output).unwrap();
    assert!(markdown.contains("Account **\\*\\*\\*\\***"));
    assert!(markdown.contains("- First\n- Second"));

    officecli()
        .args([
            "hdoc",
            "export",
            bundle.to_string_lossy().as_ref(),
            "--output",
            text_output.to_string_lossy().as_ref(),
            "--to",
            "txt",
            "--revision",
            "1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""level": "SEMANTIC""#));
    let text = std::fs::read_to_string(text_output).unwrap();
    assert!(text.contains("Customer"));
    assert!(text.contains("Account ****"));
}

#[test]
fn markdown_mermaid_preview_tracks_nodeid_patch_in_html_and_pdf() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("diagram.md");
    let bundle = temp.path().join("bundle");
    let first_html = temp.path().join("revision-0.html");
    let second_html = temp.path().join("revision-1.html");
    let pdf = temp.path().join("revision-1.pdf");
    std::fs::write(&source, "```mermaid\ngraph LR\nA[Alpha] --> B[Beta]\n```\n").unwrap();
    let extracted = import_and_extract(&source, &bundle, "markdown-mermaid-doc");
    let entry = extracted["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| {
            entry["source"]["nodeKind"] == "markdown-code"
                && entry["text"].as_str().unwrap_or("").contains("B[Beta]")
        })
        .unwrap();

    officecli()
        .args([
            "hdoc",
            "render-html",
            bundle.to_string_lossy().as_ref(),
            "--output",
            first_html.to_string_lossy().as_ref(),
            "--revision",
            "0",
            "--json",
        ])
        .assert()
        .success();
    let first = std::fs::read_to_string(&first_html).unwrap();
    assert!(first.contains("class=\"hcd-mermaid-preview\""));
    assert!(first.contains(">Beta</text>"));
    let diagram_marker = "data-hcd-node-kind=\"diagram\"";
    let first_diagram_id = first[..first.find(diagram_marker).unwrap()]
        .rsplit_once("data-hcd-id=\"")
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap()
        .to_string();

    let source_text = entry["text"].as_str().unwrap();
    let byte_start = source_text.find("Beta").unwrap();
    let scalar_start = source_text[..byte_start].chars().count();
    apply_text_patch(
        temp.path(),
        &bundle,
        "markdown-mermaid-doc",
        entry,
        scalar_start,
        "Beta".chars().count(),
        "Gamma",
    );
    officecli()
        .args([
            "hdoc",
            "render-html",
            bundle.to_string_lossy().as_ref(),
            "--output",
            second_html.to_string_lossy().as_ref(),
            "--revision",
            "1",
            "--json",
        ])
        .assert()
        .success();
    let second = std::fs::read_to_string(&second_html).unwrap();
    assert!(second.contains(">Gamma</text>"));
    assert!(!second.contains(">Beta</text>"));
    assert!(second.contains(&format!("data-hcd-id=\"{first_diagram_id}\"")));
    assert!(second.contains(&format!(
        "data-hcd-source-node-id=\"{}\"",
        entry["nodeId"].as_str().unwrap()
    )));

    officecli()
        .args([
            "hdoc",
            "export",
            bundle.to_string_lossy().as_ref(),
            "--output",
            pdf.to_string_lossy().as_ref(),
            "--to",
            "pdf",
            "--revision",
            "1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""level": "HIGH""#));
    officecli()
        .args(["view", pdf.to_string_lossy().as_ref(), "text"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Alpha"))
        .stdout(predicate::str::contains("Gamma"))
        .stdout(predicate::str::contains("Beta").not());
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
    let markdown = temp.path().join("stable.md");
    std::fs::write(&markdown, "# Stable Markdown\n\nSecond **node**\n").unwrap();
    let pdf = workspace_root().join("examples/test.pdf");

    for (index, source) in [xlsx, pptx, pdf, html, txt, markdown].iter().enumerate() {
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
