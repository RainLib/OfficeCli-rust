use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::path::Path;

fn officecli() -> Command {
    Command::cargo_bin("officecli").unwrap()
}

fn manifest(bundle: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(bundle.join("manifest.json")).unwrap()).unwrap()
}

fn bundle_node_ids(bundle: &Path) -> Vec<String> {
    let manifest = manifest(bundle);
    let index_prefix = manifest["indexPrefix"].as_str().unwrap();
    let mut ids = Vec::new();
    for page in 0..manifest["indexPageCount"].as_u64().unwrap() {
        let index: Value = serde_json::from_slice(
            &std::fs::read(bundle.join(index_prefix).join(format!("{page:06}.json"))).unwrap(),
        )
        .unwrap();
        for chunk in index["chunks"].as_array().unwrap() {
            let map: Value = serde_json::from_slice(
                &std::fs::read(bundle.join(chunk["mapHref"].as_str().unwrap())).unwrap(),
            )
            .unwrap();
            ids.extend(
                map["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| entry["nodeId"].as_str().unwrap().to_string()),
            );
        }
    }
    ids
}

#[test]
fn hdoc_cli_import_patch_validate_export_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.docx");
    let bundle = temp.path().join("bundle");
    let patch_path = temp.path().join("patch.json");
    let preview = temp.path().join("preview.html");
    let preview_style = temp.path().join("preview.css");
    let preview_custom_style = temp.path().join("preview-custom-style.html");
    let preview_independent_hitboxes = temp.path().join("preview-independent-hitboxes.html");
    let exported = temp.path().join("exported.docx");
    let report = temp.path().join("fidelity.json");
    let source_arg = source.to_string_lossy();
    let bundle_arg = bundle.to_string_lossy();

    officecli()
        .args(["create", source_arg.as_ref()])
        .assert()
        .success();
    officecli()
        .args(["set", source_arg.as_ref(), "/body/p[1]", "text=Secret 123"])
        .assert()
        .success();

    officecli()
        .args([
            "hdoc",
            "import",
            source_arg.as_ref(),
            "--output",
            bundle_arg.as_ref(),
            "--document-id",
            "cli-doc",
            "--events",
            "ndjson",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""event":"import_started""#))
        .stdout(predicate::str::contains(r#""event":"chunk_ready""#))
        .stdout(predicate::str::contains(r#""event":"completed""#));

    officecli()
        .args(["hdoc", "validate", bundle_arg.as_ref(), "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""valid": true"#));
    let bundle_manifest: Value =
        serde_json::from_slice(&std::fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(bundle_manifest["fidelity"]["level"], "HIGH");
    assert_eq!(bundle_manifest["capabilities"]["stylePatch"], false);

    officecli()
        .args([
            "hdoc",
            "render-html",
            bundle_arg.as_ref(),
            "--output",
            preview.to_string_lossy().as_ref(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""chunkCount": 1"#));
    let preview_html = std::fs::read_to_string(&preview).unwrap();
    assert!(preview_html.contains("data-hcd-profile=\"semantic-flow\""));
    assert!(preview_html.contains("data-hcd-text-hitboxes=\"on\""));
    assert!(preview_html.contains("data-hcd-image-hitboxes=\"on\""));
    assert!(preview_html.contains("data-hcd-id="));
    assert!(preview_html.contains("Secret 123"));

    let root_before_preview_style = bundle_manifest["rootHash"].as_str().unwrap().to_string();
    std::fs::write(&preview_style, ".hcd-chunk{color:#123456}").unwrap();
    officecli()
        .args([
            "hdoc",
            "render-html",
            bundle_arg.as_ref(),
            "--output",
            preview_custom_style.to_string_lossy().as_ref(),
            "--style",
            preview_style.to_string_lossy().as_ref(),
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("preview.css"));
    let custom_preview_html = std::fs::read_to_string(&preview_custom_style).unwrap();
    assert!(custom_preview_html.contains(".hcd-chunk{color:#123456}"));
    assert_eq!(
        manifest(&bundle)["rootHash"].as_str().unwrap(),
        root_before_preview_style
    );

    officecli()
        .args([
            "hdoc",
            "render-html",
            bundle_arg.as_ref(),
            "--output",
            preview_independent_hitboxes.to_string_lossy().as_ref(),
            "--text-hitboxes",
            "off",
            "--image-hitboxes",
            "on",
            "--json",
        ])
        .assert()
        .success();
    let preview_independent_hitboxes =
        std::fs::read_to_string(&preview_independent_hitboxes).unwrap();
    assert!(preview_independent_hitboxes.contains("data-hcd-text-hitboxes=\"off\""));
    assert!(preview_independent_hitboxes.contains("data-hcd-image-hitboxes=\"on\""));

    let extract = officecli()
        .args([
            "hdoc",
            "extract-text",
            bundle_arg.as_ref(),
            "--limit",
            "10",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        extract.status.success(),
        "{}",
        String::from_utf8_lossy(&extract.stderr)
    );
    let envelope: Value = serde_json::from_slice(&extract.stdout).unwrap();
    let entry = envelope["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["text"] == "Secret 123")
        .unwrap();
    let node_id = entry["nodeId"].as_str().unwrap();
    let node_hash = entry["nodeHash"].as_str().unwrap();

    officecli()
        .args(["hdoc", "get-node", bundle_arg.as_ref(), node_id, "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""nodeId": "#))
        .stdout(predicate::str::contains(r#""text": "Secret 123""#));

    std::fs::write(
        &patch_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": "hcd-patch/1",
            "documentId": "cli-doc",
            "patchId": "mask-1",
            "baseRevision": 0,
            "operations": [{
                "op": "text.splice",
                "nodeId": node_id,
                "start": 7,
                "deleteCount": 3,
                "insertText": "***",
                "precondition": { "nodeHash": node_hash }
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    officecli()
        .args([
            "hdoc",
            "apply",
            bundle_arg.as_ref(),
            "--patch",
            patch_path.to_string_lossy().as_ref(),
            "--expected-revision",
            "0",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""revision": 1"#));

    let current = officecli()
        .args(["hdoc", "get-node", bundle_arg.as_ref(), node_id, "--json"])
        .output()
        .unwrap();
    assert!(current.status.success());
    let current: Value = serde_json::from_slice(&current.stdout).unwrap();
    assert_eq!(current["data"]["documentId"], "cli-doc");
    assert_eq!(current["data"]["revision"], 1);
    assert_eq!(current["data"]["nodeId"], node_id);
    assert_eq!(current["data"]["text"], "Secret ***");
    assert_ne!(current["data"]["nodeHash"], node_hash);
    officecli()
        .args([
            "hdoc",
            "get-node",
            bundle_arg.as_ref(),
            "n_ffffffffffffffffffffffffffffffff",
            "--json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains(r#""code": "not_found""#))
        .stdout(predicate::str::contains(
            "n_ffffffffffffffffffffffffffffffff",
        ));

    officecli()
        .args([
            "hdoc",
            "export",
            bundle_arg.as_ref(),
            "--source",
            source_arg.as_ref(),
            "--output",
            exported.to_string_lossy().as_ref(),
            "--revision",
            "1",
            "--fidelity-report",
            report.to_string_lossy().as_ref(),
            "--json",
        ])
        .assert()
        .success();
    officecli()
        .args(["view", exported.to_string_lossy().as_ref()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Secret ***"));
    assert!(report.exists());

    let map_path = std::fs::read_dir(bundle.join("maps/sha256"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let map: Value = serde_json::from_slice(&std::fs::read(map_path).unwrap()).unwrap();
    assert!(map["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry.get("text").is_none()));
}

#[test]
fn identical_source_uses_deterministic_default_document_and_node_ids() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("stable.docx");
    let first_bundle = temp.path().join("first-bundle");
    let second_bundle = temp.path().join("second-bundle");
    officecli()
        .args(["create", source.to_string_lossy().as_ref()])
        .assert()
        .success();
    officecli()
        .args([
            "set",
            source.to_string_lossy().as_ref(),
            "/body/p[1]",
            "text=Stable node",
        ])
        .assert()
        .success();

    for bundle in [&first_bundle, &second_bundle] {
        officecli()
            .args([
                "hdoc",
                "import",
                source.to_string_lossy().as_ref(),
                "--output",
                bundle.to_string_lossy().as_ref(),
                "--json",
            ])
            .assert()
            .success();
    }

    let first = manifest(&first_bundle);
    let second = manifest(&second_bundle);
    let source_hash = first["source"]["sha256"].as_str().unwrap();
    assert_eq!(first["documentId"], format!("doc-{}", &source_hash[..32]));
    assert_eq!(first["documentId"], second["documentId"]);
    assert_eq!(first["rootHash"], second["rootHash"]);
    assert_eq!(
        bundle_node_ids(&first_bundle),
        bundle_node_ids(&second_bundle)
    );
}

#[test]
fn hdoc_chunk_window_and_revision_queries_are_cursor_addressable() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.txt");
    let bundle = temp.path().join("bundle");
    let patch_path = temp.path().join("patch.json");
    let preview = temp.path().join("chunk-1.html");
    std::fs::write(&source, "alpha\nbeta\ngamma\n").unwrap();

    officecli()
        .args([
            "hdoc",
            "import",
            source.to_string_lossy().as_ref(),
            "--output",
            bundle.to_string_lossy().as_ref(),
            "--document-id",
            "chunk-window",
            "--chunk-blocks",
            "1",
            "--json",
        ])
        .assert()
        .success();
    let imported = manifest(&bundle);
    assert_eq!(imported["chunkCount"], 3);

    officecli()
        .args([
            "hdoc",
            "render-html",
            bundle.to_string_lossy().as_ref(),
            "--output",
            preview.to_string_lossy().as_ref(),
            "--chunk-start",
            "1",
            "--chunk-limit",
            "1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""firstChunk": 1"#))
        .stdout(predicate::str::contains(r#""chunkCount": 1"#))
        .stdout(predicate::str::contains(r#""totalChunkCount": 3"#))
        .stdout(predicate::str::contains(r#""nextChunk": 2"#));
    let preview = std::fs::read_to_string(preview).unwrap();
    assert!(!preview.contains("alpha"));
    assert!(preview.contains("beta"));
    assert!(!preview.contains("gamma"));

    let extract = officecli()
        .args([
            "hdoc",
            "extract-text",
            bundle.to_string_lossy().as_ref(),
            "--limit",
            "1",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(extract.status.success());
    let extract: Value = serde_json::from_slice(&extract.stdout).unwrap();
    let node = &extract["data"]["entries"][0];
    std::fs::write(
        &patch_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": "hcd-patch/1",
            "documentId": "chunk-window",
            "patchId": "chunk-window-1",
            "baseRevision": 0,
            "operations": [{
                "op": "text.splice",
                "nodeId": node["nodeId"],
                "start": 0,
                "deleteCount": 5,
                "insertText": "ALPHA",
                "precondition": { "nodeHash": node["nodeHash"] }
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
            patch_path.to_string_lossy().as_ref(),
            "--expected-revision",
            "0",
            "--json",
        ])
        .assert()
        .success();

    officecli()
        .args([
            "hdoc",
            "list-revisions",
            bundle.to_string_lossy().as_ref(),
            "--limit",
            "1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""headRevision": 1"#))
        .stdout(predicate::str::contains(r#""nextCursor": 1"#));
    officecli()
        .args([
            "hdoc",
            "list-revisions",
            bundle.to_string_lossy().as_ref(),
            "--cursor",
            "1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""revision": 1"#))
        .stdout(predicate::str::contains("chunk-window-1"));
    officecli()
        .args([
            "hdoc",
            "get-revision",
            bundle.to_string_lossy().as_ref(),
            "1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""revision": 1"#))
        .stdout(predicate::str::contains("chunk-window-1"));
}

#[test]
fn hdoc_patch_v2_supports_atomic_cross_node_text_and_presentation_style() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.txt");
    let bundle = temp.path().join("bundle");
    let patch_path = temp.path().join("cross-node-style.json");
    let preview = temp.path().join("revision-1.html");
    std::fs::write(&source, "First bbox\nSecond bbox\n").unwrap();

    officecli()
        .args([
            "hdoc",
            "import",
            source.to_string_lossy().as_ref(),
            "--output",
            bundle.to_string_lossy().as_ref(),
            "--document-id",
            "cross-node-style",
            "--json",
        ])
        .assert()
        .success();

    let extract = officecli()
        .args([
            "hdoc",
            "extract-text",
            bundle.to_string_lossy().as_ref(),
            "--limit",
            "10",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(extract.status.success());
    let extract: Value = serde_json::from_slice(&extract.stdout).unwrap();
    let entries = extract["data"]["entries"].as_array().unwrap();
    let first = entries
        .iter()
        .find(|entry| entry["text"] == "First bbox")
        .unwrap();
    let second = entries
        .iter()
        .find(|entry| entry["text"] == "Second bbox")
        .unwrap();

    std::fs::write(
        &patch_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": "hcd-patch/2",
            "documentId": "cross-node-style",
            "patchId": "cross-node-style-1",
            "baseRevision": 0,
            "operations": [
                {
                    "op": "text.splice",
                    "nodeId": first["nodeId"],
                    "start": 0,
                    "deleteCount": 10,
                    "insertText": "Combined across bbox",
                    "precondition": { "nodeHash": first["nodeHash"] }
                },
                {
                    "op": "text.splice",
                    "nodeId": second["nodeId"],
                    "start": 0,
                    "deleteCount": 11,
                    "insertText": "",
                    "precondition": { "nodeHash": second["nodeHash"] }
                },
                {
                    "op": "node.style",
                    "nodeId": first["nodeId"],
                    "style": {
                        "textColor": "#D70015",
                        "backgroundColor": "#FFF2A8",
                        "border": { "color": "#0A84FF", "widthPt": 2, "style": "dashed" }
                    },
                    "precondition": { "nodeHash": first["nodeHash"] }
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
            patch_path.to_string_lossy().as_ref(),
            "--expected-revision",
            "0",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("HCD_PRESENTATION_STYLE_ONLY"));
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
    assert!(preview.contains("Combined across bbox"));
    assert!(!preview.contains("Second bbox"));
    assert!(preview.contains("color:#d70015"));
    assert!(preview.contains("background-color:#fff2a8"));
    assert!(preview.contains("border-top:2pt dashed #0a84ff"));
    assert!(preview.contains("data-hcd-style-patched=\"true\""));

    let rejected_export = temp.path().join("style-must-not-be-dropped.txt");
    officecli()
        .args([
            "hdoc",
            "export",
            bundle.to_string_lossy().as_ref(),
            "--source",
            source.to_string_lossy().as_ref(),
            "--output",
            rejected_export.to_string_lossy().as_ref(),
            "--revision",
            "1",
            "--json",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "source-backed export cannot preserve",
        ));
    assert!(!rejected_export.exists());
}
