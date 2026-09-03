#!/usr/bin/env bash
set -uo pipefail

PPTX_SUITE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PPTX_SUITE_REPO_ROOT="$(cd "$PPTX_SUITE_ROOT/../../.." && pwd)"
PPTX_SUITE_OUTPUT_ROOT="${1:-$PPTX_SUITE_ROOT/output}"
PPTX_SUITE_BIN="${PPTX_SUITE_BIN:-$PPTX_SUITE_REPO_ROOT/target/release/officecli}"
PPTX_SUITE_MATCH="${PPTX_SUITE_MATCH:-}"

if [[ -e "$PPTX_SUITE_OUTPUT_ROOT" ]]; then
  echo "Output already exists: $PPTX_SUITE_OUTPUT_ROOT" >&2
  echo "Pass a new directory, for example: $0 /tmp/hdoc-pptx-suite" >&2
  exit 2
fi

for PPTX_SUITE_TOOL in cargo git jq shasum unzip diff cmp rg; do
  if ! command -v "$PPTX_SUITE_TOOL" >/dev/null 2>&1; then
    echo "Required tool not found: $PPTX_SUITE_TOOL" >&2
    exit 2
  fi
done

if [[ "${PPTX_SUITE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build --release --manifest-path "$PPTX_SUITE_REPO_ROOT/Cargo.toml" -p officecli
fi
if [[ ! -x "$PPTX_SUITE_BIN" ]]; then
  echo "officecli binary not found: $PPTX_SUITE_BIN" >&2
  exit 2
fi

mkdir -p "$PPTX_SUITE_OUTPUT_ROOT/cases" "$PPTX_SUITE_OUTPUT_ROOT/reports"
PPTX_SUITE_RESULTS="$PPTX_SUITE_OUTPUT_ROOT/reports/results.ndjson"
PPTX_SUITE_MISSING="$PPTX_SUITE_OUTPUT_ROOT/reports/tracked-missing.txt"
: > "$PPTX_SUITE_RESULTS"
: > "$PPTX_SUITE_MISSING"

pptx_suite_case_key() {
  printf '%s' "$1" | sed -E 's#^examples/##; s#\.pptx$##I; s#[^[:alnum:]_.-]+#__#g'
}

pptx_suite_validate_entry_contents() {
  local PPTX_SUITE_SOURCE="$1"
  local PPTX_SUITE_ROUNDTRIP="$2"
  local PPTX_SUITE_REPORT_DIR="$3"
  local PPTX_SUITE_ENTRY

  if ! diff -u \
    <(unzip -Z1 "$PPTX_SUITE_SOURCE") \
    <(unzip -Z1 "$PPTX_SUITE_ROUNDTRIP") \
    > "$PPTX_SUITE_REPORT_DIR/zip-entry-list.diff"; then
    return 1
  fi

  : > "$PPTX_SUITE_REPORT_DIR/zip-entry-content-mismatches.txt"
  while IFS= read -r PPTX_SUITE_ENTRY; do
    [[ "$PPTX_SUITE_ENTRY" == */ ]] && continue
    local PPTX_SUITE_ENTRY_PATTERN
    PPTX_SUITE_ENTRY_PATTERN="$(printf '%s' "$PPTX_SUITE_ENTRY" | sed 's/[][?*]/\\&/g')"
    if ! cmp -s \
      <(unzip -p "$PPTX_SUITE_SOURCE" "$PPTX_SUITE_ENTRY_PATTERN") \
      <(unzip -p "$PPTX_SUITE_ROUNDTRIP" "$PPTX_SUITE_ENTRY_PATTERN"); then
      printf '%s\n' "$PPTX_SUITE_ENTRY" \
        >> "$PPTX_SUITE_REPORT_DIR/zip-entry-content-mismatches.txt"
    fi
  done < <(unzip -Z1 "$PPTX_SUITE_SOURCE")

  [[ ! -s "$PPTX_SUITE_REPORT_DIR/zip-entry-content-mismatches.txt" ]]
}

pptx_suite_validation_issues_preserved() {
  local PPTX_SUITE_SOURCE_VALIDATION="$1"
  local PPTX_SUITE_OUTPUT_VALIDATION="$2"
  local PPTX_SUITE_DIFF_PATH="$3"
  local PPTX_SUITE_SOURCE_NORMALIZED="${PPTX_SUITE_DIFF_PATH%.diff}-source.json"
  local PPTX_SUITE_OUTPUT_NORMALIZED="${PPTX_SUITE_DIFF_PATH%.diff}-output.json"

  jq -S '[.data[]? | {description, error_type, part}] | sort_by(.part, .error_type, .description)' \
    "$PPTX_SUITE_SOURCE_VALIDATION" > "$PPTX_SUITE_SOURCE_NORMALIZED"
  jq -S '[.data[]? | {description, error_type, part}] | sort_by(.part, .error_type, .description)' \
    "$PPTX_SUITE_OUTPUT_VALIDATION" > "$PPTX_SUITE_OUTPUT_NORMALIZED"
  diff -u "$PPTX_SUITE_SOURCE_NORMALIZED" "$PPTX_SUITE_OUTPUT_NORMALIZED" \
    > "$PPTX_SUITE_DIFF_PATH"
}

pptx_suite_image_node_ids() {
  local PPTX_SUITE_BUNDLE="$1"
  local PPTX_SUITE_REVISION="$2"
  local PPTX_SUITE_OUTPUT="$3"
  local PPTX_SUITE_INDEX_DIR
  PPTX_SUITE_INDEX_DIR="$(printf '%s/indexes/rev-%020d' "$PPTX_SUITE_BUNDLE" "$PPTX_SUITE_REVISION")"
  local PPTX_SUITE_INDEX PPTX_SUITE_HTML_HREF

  : > "$PPTX_SUITE_OUTPUT"
  for PPTX_SUITE_INDEX in "$PPTX_SUITE_INDEX_DIR"/*.json; do
    [[ -f "$PPTX_SUITE_INDEX" ]] || continue
    while IFS= read -r PPTX_SUITE_HTML_HREF; do
      (rg -o 'data-hcd-id="[^"]+" data-hcd-node-kind="image"' \
        "$PPTX_SUITE_BUNDLE/$PPTX_SUITE_HTML_HREF" || true) \
        | sed -E 's/^data-hcd-id="([^"]+)".*/\1/' >> "$PPTX_SUITE_OUTPUT"
    done < <(jq -r '.chunks[].htmlHref' "$PPTX_SUITE_INDEX")
  done
}

pptx_suite_take_screenshot() {
  local PPTX_SUITE_FILE="$1"
  local PPTX_SUITE_OUTPUT="$2"
  local PPTX_SUITE_LOG="$3"
  "$PPTX_SUITE_BIN" view "$PPTX_SUITE_FILE" screenshot --grid 4 \
    --out "$PPTX_SUITE_OUTPUT" > "$PPTX_SUITE_LOG" 2>&1
}

pptx_suite_emit_result() {
  local PPTX_SUITE_STATUS="$1"
  local PPTX_SUITE_SOURCE_REL="$2"
  local PPTX_SUITE_CASE_KEY="$3"
  local PPTX_SUITE_CASE_DIR="$4"
  local PPTX_SUITE_FAILURES_FILE="$5"
  local PPTX_SUITE_MANIFEST="$PPTX_SUITE_CASE_DIR/hcd/bundle/manifest.json"
  local PPTX_SUITE_NOOP_FIDELITY="$PPTX_SUITE_CASE_DIR/reports/noop-fidelity.json"
  local PPTX_SUITE_PATCHED_FIDELITY="$PPTX_SUITE_CASE_DIR/reports/patched-fidelity.json"
  local PPTX_SUITE_SEMANTIC_FIDELITY="$PPTX_SUITE_CASE_DIR/reports/semantic-fidelity.json"
  local PPTX_SUITE_SOURCE_VALIDATION="$PPTX_SUITE_CASE_DIR/reports/source-validate.json"
  local PPTX_SUITE_EXTRACT="$PPTX_SUITE_CASE_DIR/reports/extract-revision-0.json"
  local PPTX_SUITE_IMAGE_NODE_COUNT="$PPTX_SUITE_CASE_DIR/reports/image-node-count.txt"
  local PPTX_SUITE_ASSET_COUNT="$PPTX_SUITE_CASE_DIR/reports/asset-count.txt"

  jq -n -c \
    --arg status "$PPTX_SUITE_STATUS" \
    --arg source "$PPTX_SUITE_SOURCE_REL" \
    --arg caseKey "$PPTX_SUITE_CASE_KEY" \
    --arg artifactDir "${PPTX_SUITE_CASE_DIR#"$PPTX_SUITE_OUTPUT_ROOT"/}" \
    --rawfile failures "$PPTX_SUITE_FAILURES_FILE" \
    --slurpfile manifest "$PPTX_SUITE_MANIFEST" \
    --slurpfile noopFidelity "$PPTX_SUITE_NOOP_FIDELITY" \
    --slurpfile patchedFidelity "$PPTX_SUITE_PATCHED_FIDELITY" \
    --slurpfile semanticFidelity "$PPTX_SUITE_SEMANTIC_FIDELITY" \
    --slurpfile sourceValidation "$PPTX_SUITE_SOURCE_VALIDATION" \
    --slurpfile extracted "$PPTX_SUITE_EXTRACT" \
    --rawfile imageNodeCount "$PPTX_SUITE_IMAGE_NODE_COUNT" \
    --rawfile assetCount "$PPTX_SUITE_ASSET_COUNT" \
    '{
      status: $status,
      source: $source,
      caseKey: $caseKey,
      artifactDir: $artifactDir,
      failures: ($failures | split("\n") | map(select(length > 0))),
      profile: ($manifest[0].profile // null),
      chunkCount: ($manifest[0].chunkCount // null),
      importFidelity: ($manifest[0].fidelity.level // null),
      importWarningCount: (($manifest[0].warnings // []) | length),
      sourceValid: ($sourceValidation[0].success // false),
      sourceValidationIssueCount: (($sourceValidation[0].data // []) | length),
      nodeCount: (($extracted[0].data.entries // []) | length),
      editableNodeCount: (($extracted[0].data.entries // []) | map(select(.source.editable == true)) | length),
      imageNodeCount: (($imageNodeCount | rtrimstr("\n") | tonumber?) // 0),
      assetCount: (($assetCount | rtrimstr("\n") | tonumber?) // 0),
      noopExportFidelity: ($noopFidelity[0].level // null),
      patchedExportFidelity: ($patchedFidelity[0].level // null),
      semanticExportFidelity: ($semanticFidelity[0].level // null)
    }' >> "$PPTX_SUITE_RESULTS"
}

pptx_suite_write_comparison() {
  local PPTX_SUITE_CASE_DIR="$1"
  local PPTX_SUITE_SOURCE_REL="$2"
  local PPTX_SUITE_HAS_PATCH="$3"
  {
    printf '%s\n' '<!doctype html><html><head><meta charset="utf-8">'
    printf '<title>%s · HCD PPTX comparison</title>\n' "$PPTX_SUITE_SOURCE_REL"
    printf '%s\n' '<style>body{font:14px system-ui;margin:24px;background:#eef1f5;color:#182230}h1{font-size:22px}.grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:18px}.panel{background:#fff;padding:12px;border-radius:8px;box-shadow:0 2px 12px #0002}.panel h2{font-size:16px;margin:0 0 10px}.panel img,.panel iframe{display:block;width:100%;border:1px solid #ccd3dc;background:#fff}.panel iframe{height:720px}@media(max-width:900px){.grid{grid-template-columns:1fr}}</style></head><body>'
    printf '<h1><code>%s</code></h1><p>Source-backed revision 0 must be package-entry identical and render-identical. HCD is the editable slide-canvas view.</p><div class="grid">\n' "$PPTX_SUITE_SOURCE_REL"
    printf '%s\n' '<section class="panel"><h2>Source PPTX screenshot</h2><img src="screenshots/source.png"></section>'
    printf '%s\n' '<section class="panel"><h2>Revision 0 round-trip screenshot</h2><img src="screenshots/roundtrip.png"></section>'
    printf '%s\n' '<section class="panel"><h2>Source PPTX HTML renderer</h2><iframe src="html/source-preview.html"></iframe></section>'
    printf '%s\n' '<section class="panel"><h2>Editable HCD screenshot (actual HCD renderer)</h2><img src="screenshots/hcd.png"></section>'
    printf '%s\n' '<section class="panel"><h2>Editable HCD HTML</h2><iframe src="html/hcd-preview.html"></iframe></section>'
    if [[ "$PPTX_SUITE_HAS_PATCH" == "1" ]]; then
      printf '%s\n' '<section class="panel"><h2>Patched HCD revision 1</h2><iframe src="html/hcd-patched-preview.html"></iframe></section>'
      printf '%s\n' '<section class="panel"><h2>Patched PPTX screenshot</h2><img src="screenshots/patched.png"></section>'
      printf '%s\n' '<section class="panel"><h2>Patched PPTX HTML renderer</h2><iframe src="html/patched-pptx-preview.html"></iframe></section>'
    fi
    printf '%s\n' '<section class="panel"><h2>Source-free semantic PPTX screenshot</h2><img src="screenshots/semantic.png"></section>'
    printf '%s\n' '<section class="panel"><h2>Source-free semantic PPTX HTML renderer</h2><iframe src="html/semantic-pptx-preview.html"></iframe></section>'
    printf '%s\n' '</div></body></html>'
  } > "$PPTX_SUITE_CASE_DIR/comparison.html"
}

pptx_suite_run_case() {
  local PPTX_SUITE_SOURCE_REL="$1"
  local PPTX_SUITE_SOURCE="$PPTX_SUITE_REPO_ROOT/$PPTX_SUITE_SOURCE_REL"
  local PPTX_SUITE_CASE_KEY
  PPTX_SUITE_CASE_KEY="$(pptx_suite_case_key "$PPTX_SUITE_SOURCE_REL")"
  local PPTX_SUITE_CASE_DIR="$PPTX_SUITE_OUTPUT_ROOT/cases/$PPTX_SUITE_CASE_KEY"
  local PPTX_SUITE_REPORT_DIR="$PPTX_SUITE_CASE_DIR/reports"
  local PPTX_SUITE_FAILURES_FILE="$PPTX_SUITE_REPORT_DIR/failures.txt"
  local PPTX_SUITE_FAILED=0
  local PPTX_SUITE_HAS_PATCH=0

  mkdir -p \
    "$PPTX_SUITE_CASE_DIR/hcd" \
    "$PPTX_SUITE_CASE_DIR/html" \
    "$PPTX_SUITE_CASE_DIR/screenshots" \
    "$PPTX_SUITE_CASE_DIR/roundtrip" \
    "$PPTX_SUITE_CASE_DIR/patched" \
    "$PPTX_SUITE_CASE_DIR/semantic" \
    "$PPTX_SUITE_REPORT_DIR"
  : > "$PPTX_SUITE_FAILURES_FILE"
  printf '%s\n' '{}' > "$PPTX_SUITE_REPORT_DIR/noop-fidelity.json"
  printf '%s\n' '{}' > "$PPTX_SUITE_REPORT_DIR/patched-fidelity.json"
  printf '%s\n' '{}' > "$PPTX_SUITE_REPORT_DIR/semantic-fidelity.json"
  printf '%s\n' '{"data":{"entries":[]}}' > "$PPTX_SUITE_REPORT_DIR/extract-revision-0.json"
  printf '%s\n' '{"success":false,"data":[]}' > "$PPTX_SUITE_REPORT_DIR/source-validate.json"
  printf '%s\n' '0' > "$PPTX_SUITE_REPORT_DIR/image-node-count.txt"
  printf '%s\n' '0' > "$PPTX_SUITE_REPORT_DIR/asset-count.txt"

  printf '%s\n' "$PPTX_SUITE_SOURCE_REL" > "$PPTX_SUITE_REPORT_DIR/source-path.txt"
  shasum -a 256 "$PPTX_SUITE_SOURCE" > "$PPTX_SUITE_REPORT_DIR/source.sha256"

  if ! "$PPTX_SUITE_BIN" validate "$PPTX_SUITE_SOURCE" --json \
    > "$PPTX_SUITE_REPORT_DIR/source-validate.json" \
    2> "$PPTX_SUITE_REPORT_DIR/source-validate.stderr.txt"; then
    printf '%s\n' 'source contains pre-existing validation issues; output must preserve the same issue signatures' \
      > "$PPTX_SUITE_REPORT_DIR/source-validation-note.txt"
  fi
  if ! "$PPTX_SUITE_BIN" view "$PPTX_SUITE_SOURCE" stats --json \
    > "$PPTX_SUITE_REPORT_DIR/source-stats.json" \
    2> "$PPTX_SUITE_REPORT_DIR/source-stats.stderr.txt"; then
    printf '%s\n' source_stats >> "$PPTX_SUITE_FAILURES_FILE"
    PPTX_SUITE_FAILED=1
  fi
  if ! "$PPTX_SUITE_BIN" view "$PPTX_SUITE_SOURCE" html \
    > "$PPTX_SUITE_CASE_DIR/html/source-preview.html" \
    2> "$PPTX_SUITE_REPORT_DIR/source-preview.stderr.txt"; then
    printf '%s\n' source_preview >> "$PPTX_SUITE_FAILURES_FILE"
    PPTX_SUITE_FAILED=1
  fi
  if ! pptx_suite_take_screenshot "$PPTX_SUITE_SOURCE" \
    "$PPTX_SUITE_CASE_DIR/screenshots/source.png" \
    "$PPTX_SUITE_REPORT_DIR/source-screenshot.txt"; then
    printf '%s\n' source_screenshot >> "$PPTX_SUITE_FAILURES_FILE"
    PPTX_SUITE_FAILED=1
  fi

  if ! "$PPTX_SUITE_BIN" hdoc import "$PPTX_SUITE_SOURCE" \
    --output "$PPTX_SUITE_CASE_DIR/hcd/bundle" \
    --events ndjson \
    > "$PPTX_SUITE_REPORT_DIR/import-events.ndjson" \
    2> "$PPTX_SUITE_REPORT_DIR/import.stderr.txt"; then
    printf '%s\n' hcd_import >> "$PPTX_SUITE_FAILURES_FILE"
    mkdir -p "$PPTX_SUITE_CASE_DIR/hcd/bundle"
    printf '%s\n' '{}' > "$PPTX_SUITE_CASE_DIR/hcd/bundle/manifest.json"
    pptx_suite_emit_result failed "$PPTX_SUITE_SOURCE_REL" "$PPTX_SUITE_CASE_KEY" "$PPTX_SUITE_CASE_DIR" "$PPTX_SUITE_FAILURES_FILE"
    return
  fi

  local PPTX_SUITE_DOCUMENT_ID
  PPTX_SUITE_DOCUMENT_ID="$(jq -r .documentId "$PPTX_SUITE_CASE_DIR/hcd/bundle/manifest.json")"
  if ! "$PPTX_SUITE_BIN" hdoc validate "$PPTX_SUITE_CASE_DIR/hcd/bundle" --json \
    > "$PPTX_SUITE_REPORT_DIR/hcd-validate.json" \
    2> "$PPTX_SUITE_REPORT_DIR/hcd-validate.stderr.txt"; then
    printf '%s\n' hcd_validate >> "$PPTX_SUITE_FAILURES_FILE"
    PPTX_SUITE_FAILED=1
  fi
  if ! "$PPTX_SUITE_BIN" hdoc extract-text "$PPTX_SUITE_CASE_DIR/hcd/bundle" \
    --limit 100000 --json \
    > "$PPTX_SUITE_REPORT_DIR/extract-revision-0.json" \
    2> "$PPTX_SUITE_REPORT_DIR/extract-revision-0.stderr.txt"; then
    printf '%s\n' hcd_extract >> "$PPTX_SUITE_FAILURES_FILE"
    PPTX_SUITE_FAILED=1
  fi
  if ! "$PPTX_SUITE_BIN" hdoc render-html "$PPTX_SUITE_CASE_DIR/hcd/bundle" \
    --output "$PPTX_SUITE_CASE_DIR/html/hcd-preview.html" \
    --screenshot "$PPTX_SUITE_CASE_DIR/screenshots/hcd.png" \
    --screenshot-width 1600 --screenshot-height 1200 \
    --text-hitboxes on --image-hitboxes on --json \
    > "$PPTX_SUITE_REPORT_DIR/hcd-render.json" \
    2> "$PPTX_SUITE_REPORT_DIR/hcd-render.stderr.txt"; then
    printf '%s\n' hcd_render >> "$PPTX_SUITE_FAILURES_FILE"
    PPTX_SUITE_FAILED=1
  fi

  pptx_suite_image_node_ids "$PPTX_SUITE_CASE_DIR/hcd/bundle" 0 \
    "$PPTX_SUITE_REPORT_DIR/image-node-ids-a.txt"
  wc -l < "$PPTX_SUITE_REPORT_DIR/image-node-ids-a.txt" \
    | tr -d '[:space:]' > "$PPTX_SUITE_REPORT_DIR/image-node-count.txt"
  printf '\n' >> "$PPTX_SUITE_REPORT_DIR/image-node-count.txt"
  jq 'length' "$PPTX_SUITE_CASE_DIR/hcd/bundle/assets/index.json" \
    > "$PPTX_SUITE_REPORT_DIR/asset-count.txt"

  if "$PPTX_SUITE_BIN" hdoc import "$PPTX_SUITE_SOURCE" \
    --output "$PPTX_SUITE_CASE_DIR/hcd/stability-b" \
    --document-id "$PPTX_SUITE_DOCUMENT_ID" --json \
    > "$PPTX_SUITE_REPORT_DIR/stability-import.json" \
    2> "$PPTX_SUITE_REPORT_DIR/stability-import.stderr.txt" && \
    "$PPTX_SUITE_BIN" hdoc extract-text "$PPTX_SUITE_CASE_DIR/hcd/stability-b" \
    --limit 100000 --json \
    > "$PPTX_SUITE_REPORT_DIR/stability-extract.json" \
    2> "$PPTX_SUITE_REPORT_DIR/stability-extract.stderr.txt"; then
    jq -r '.data.entries[].nodeId' "$PPTX_SUITE_REPORT_DIR/extract-revision-0.json" \
      > "$PPTX_SUITE_REPORT_DIR/node-ids-a.txt"
    jq -r '.data.entries[].nodeId' "$PPTX_SUITE_REPORT_DIR/stability-extract.json" \
      > "$PPTX_SUITE_REPORT_DIR/node-ids-b.txt"
    if ! diff -u "$PPTX_SUITE_REPORT_DIR/node-ids-a.txt" \
      "$PPTX_SUITE_REPORT_DIR/node-ids-b.txt" \
      > "$PPTX_SUITE_REPORT_DIR/node-ids.diff"; then
      printf '%s\n' node_id_stability >> "$PPTX_SUITE_FAILURES_FILE"
      PPTX_SUITE_FAILED=1
    fi
    if [[ "$(jq -r .rootHash "$PPTX_SUITE_CASE_DIR/hcd/bundle/manifest.json")" != \
          "$(jq -r .rootHash "$PPTX_SUITE_CASE_DIR/hcd/stability-b/manifest.json")" ]]; then
      printf '%s\n' root_hash_stability >> "$PPTX_SUITE_FAILURES_FILE"
      PPTX_SUITE_FAILED=1
    fi
    pptx_suite_image_node_ids "$PPTX_SUITE_CASE_DIR/hcd/stability-b" 0 \
      "$PPTX_SUITE_REPORT_DIR/image-node-ids-b.txt"
    if ! diff -u "$PPTX_SUITE_REPORT_DIR/image-node-ids-a.txt" \
      "$PPTX_SUITE_REPORT_DIR/image-node-ids-b.txt" \
      > "$PPTX_SUITE_REPORT_DIR/image-node-ids.diff"; then
      printf '%s\n' image_node_id_stability >> "$PPTX_SUITE_FAILURES_FILE"
      PPTX_SUITE_FAILED=1
    fi
  else
    printf '%s\n' repeat_import >> "$PPTX_SUITE_FAILURES_FILE"
    PPTX_SUITE_FAILED=1
  fi

  if "$PPTX_SUITE_BIN" hdoc export "$PPTX_SUITE_CASE_DIR/hcd/bundle" \
    --source "$PPTX_SUITE_SOURCE" \
    --output "$PPTX_SUITE_CASE_DIR/roundtrip/revision-0.pptx" \
    --revision 0 \
    --fidelity-report "$PPTX_SUITE_REPORT_DIR/noop-fidelity.json" \
    --json > "$PPTX_SUITE_REPORT_DIR/noop-export.json" \
    2> "$PPTX_SUITE_REPORT_DIR/noop-export.stderr.txt"; then
    shasum -a 256 "$PPTX_SUITE_CASE_DIR/roundtrip/revision-0.pptx" \
      > "$PPTX_SUITE_REPORT_DIR/noop-output.sha256"
    if [[ "$(jq -r .level "$PPTX_SUITE_REPORT_DIR/noop-fidelity.json")" != EXACT ]]; then
      printf '%s\n' noop_fidelity_not_exact >> "$PPTX_SUITE_FAILURES_FILE"
      PPTX_SUITE_FAILED=1
    fi
    if ! pptx_suite_validate_entry_contents "$PPTX_SUITE_SOURCE" \
      "$PPTX_SUITE_CASE_DIR/roundtrip/revision-0.pptx" "$PPTX_SUITE_REPORT_DIR"; then
      printf '%s\n' noop_zip_entry_identity >> "$PPTX_SUITE_FAILURES_FILE"
      PPTX_SUITE_FAILED=1
    fi
    if ! "$PPTX_SUITE_BIN" validate "$PPTX_SUITE_CASE_DIR/roundtrip/revision-0.pptx" --json \
      > "$PPTX_SUITE_REPORT_DIR/noop-validate.json" \
      2> "$PPTX_SUITE_REPORT_DIR/noop-validate.stderr.txt"; then
      if ! pptx_suite_validation_issues_preserved \
        "$PPTX_SUITE_REPORT_DIR/source-validate.json" \
        "$PPTX_SUITE_REPORT_DIR/noop-validate.json" \
        "$PPTX_SUITE_REPORT_DIR/noop-validation-issues.diff"; then
        printf '%s\n' noop_export_validation_regression >> "$PPTX_SUITE_FAILURES_FILE"
        PPTX_SUITE_FAILED=1
      fi
    fi
    if ! "$PPTX_SUITE_BIN" view "$PPTX_SUITE_CASE_DIR/roundtrip/revision-0.pptx" html \
      > "$PPTX_SUITE_CASE_DIR/html/roundtrip-preview.html" \
      2> "$PPTX_SUITE_REPORT_DIR/roundtrip-preview.stderr.txt"; then
      printf '%s\n' roundtrip_preview >> "$PPTX_SUITE_FAILURES_FILE"
      PPTX_SUITE_FAILED=1
    fi
    if ! pptx_suite_take_screenshot "$PPTX_SUITE_CASE_DIR/roundtrip/revision-0.pptx" \
      "$PPTX_SUITE_CASE_DIR/screenshots/roundtrip.png" \
      "$PPTX_SUITE_REPORT_DIR/roundtrip-screenshot.txt"; then
      printf '%s\n' roundtrip_screenshot >> "$PPTX_SUITE_FAILURES_FILE"
      PPTX_SUITE_FAILED=1
    elif ! cmp -s "$PPTX_SUITE_CASE_DIR/screenshots/source.png" \
      "$PPTX_SUITE_CASE_DIR/screenshots/roundtrip.png"; then
      printf '%s\n' noop_screenshot_identity >> "$PPTX_SUITE_FAILURES_FILE"
      PPTX_SUITE_FAILED=1
    fi
  else
    printf '%s\n' noop_export >> "$PPTX_SUITE_FAILURES_FILE"
    PPTX_SUITE_FAILED=1
  fi

  if "$PPTX_SUITE_BIN" hdoc import "$PPTX_SUITE_CASE_DIR/roundtrip/revision-0.pptx" \
    --output "$PPTX_SUITE_CASE_DIR/hcd/roundtrip-reimport" \
    --document-id "$PPTX_SUITE_DOCUMENT_ID" --json \
    > "$PPTX_SUITE_REPORT_DIR/roundtrip-reimport.json" \
    2> "$PPTX_SUITE_REPORT_DIR/roundtrip-reimport.stderr.txt" && \
    "$PPTX_SUITE_BIN" hdoc extract-text "$PPTX_SUITE_CASE_DIR/hcd/roundtrip-reimport" \
    --limit 100000 --json > "$PPTX_SUITE_REPORT_DIR/roundtrip-reextract.json" \
    2> "$PPTX_SUITE_REPORT_DIR/roundtrip-reextract.stderr.txt"; then
    if [[ "$(jq -r .rootHash "$PPTX_SUITE_CASE_DIR/hcd/bundle/manifest.json")" != \
          "$(jq -r .rootHash "$PPTX_SUITE_CASE_DIR/hcd/roundtrip-reimport/manifest.json")" ]]; then
      printf '%s\n' noop_reimport_root_hash >> "$PPTX_SUITE_FAILURES_FILE"
      PPTX_SUITE_FAILED=1
    fi
    jq -r '.data.entries[].nodeId' "$PPTX_SUITE_REPORT_DIR/roundtrip-reextract.json" \
      > "$PPTX_SUITE_REPORT_DIR/roundtrip-node-ids.txt"
    if ! diff -u "$PPTX_SUITE_REPORT_DIR/node-ids-a.txt" \
      "$PPTX_SUITE_REPORT_DIR/roundtrip-node-ids.txt" \
      > "$PPTX_SUITE_REPORT_DIR/roundtrip-node-ids.diff"; then
      printf '%s\n' noop_reimport_node_ids >> "$PPTX_SUITE_FAILURES_FILE"
      PPTX_SUITE_FAILED=1
    fi
    pptx_suite_image_node_ids "$PPTX_SUITE_CASE_DIR/hcd/roundtrip-reimport" 0 \
      "$PPTX_SUITE_REPORT_DIR/roundtrip-image-node-ids.txt"
    if ! diff -u "$PPTX_SUITE_REPORT_DIR/image-node-ids-a.txt" \
      "$PPTX_SUITE_REPORT_DIR/roundtrip-image-node-ids.txt" \
      > "$PPTX_SUITE_REPORT_DIR/roundtrip-image-node-ids.diff"; then
      printf '%s\n' noop_reimport_image_node_ids >> "$PPTX_SUITE_FAILURES_FILE"
      PPTX_SUITE_FAILED=1
    fi
  else
    printf '%s\n' noop_reimport >> "$PPTX_SUITE_FAILURES_FILE"
    PPTX_SUITE_FAILED=1
  fi

  if [[ -s "$PPTX_SUITE_REPORT_DIR/extract-revision-0.json" ]]; then
    local PPTX_SUITE_PATCH_NODE
    PPTX_SUITE_PATCH_NODE="$(jq -c 'first(.data.entries[] | select(.source.editable == true and (.source.part | startswith("ppt/slides/slide")) and (.text | length) > 0)) // empty' "$PPTX_SUITE_REPORT_DIR/extract-revision-0.json")"
    if [[ -n "$PPTX_SUITE_PATCH_NODE" ]]; then
      PPTX_SUITE_HAS_PATCH=1
      local PPTX_SUITE_NODE_ID PPTX_SUITE_NODE_HASH
      PPTX_SUITE_NODE_ID="$(printf '%s' "$PPTX_SUITE_PATCH_NODE" | jq -r .nodeId)"
      PPTX_SUITE_NODE_HASH="$(printf '%s' "$PPTX_SUITE_PATCH_NODE" | jq -r .nodeHash)"
      jq -n \
        --arg documentId "$PPTX_SUITE_DOCUMENT_ID" \
        --arg nodeId "$PPTX_SUITE_NODE_ID" \
        --arg nodeHash "$PPTX_SUITE_NODE_HASH" \
        --arg patchId "examples-pptx-suite-$PPTX_SUITE_CASE_KEY" \
        '{
          schemaVersion: "hcd-patch/1",
          documentId: $documentId,
          patchId: $patchId,
          baseRevision: 0,
          operations: [{
            op: "text.splice",
            nodeId: $nodeId,
            start: 0,
            deleteCount: 0,
            insertText: "【HCD测试】",
            precondition: {nodeHash: $nodeHash}
          }]
        }' > "$PPTX_SUITE_CASE_DIR/patched/patch.json"

      if "$PPTX_SUITE_BIN" hdoc apply "$PPTX_SUITE_CASE_DIR/hcd/bundle" \
        --patch "$PPTX_SUITE_CASE_DIR/patched/patch.json" \
        --expected-revision 0 --json \
        > "$PPTX_SUITE_REPORT_DIR/patch-apply.json" \
        2> "$PPTX_SUITE_REPORT_DIR/patch-apply.stderr.txt" && \
        "$PPTX_SUITE_BIN" hdoc get-node "$PPTX_SUITE_CASE_DIR/hcd/bundle" \
        "$PPTX_SUITE_NODE_ID" --json \
        > "$PPTX_SUITE_REPORT_DIR/patched-node.json" \
        2> "$PPTX_SUITE_REPORT_DIR/patched-node.stderr.txt"; then
        if [[ "$(jq -r .data.nodeId "$PPTX_SUITE_REPORT_DIR/patched-node.json")" != "$PPTX_SUITE_NODE_ID" ]] || \
           [[ "$(jq -r .data.text "$PPTX_SUITE_REPORT_DIR/patched-node.json")" != '【HCD测试】'* ]]; then
          printf '%s\n' patch_node_identity >> "$PPTX_SUITE_FAILURES_FILE"
          PPTX_SUITE_FAILED=1
        fi
        if ! "$PPTX_SUITE_BIN" hdoc validate "$PPTX_SUITE_CASE_DIR/hcd/bundle" --json \
          > "$PPTX_SUITE_REPORT_DIR/patched-hcd-validate.json" \
          2> "$PPTX_SUITE_REPORT_DIR/patched-hcd-validate.stderr.txt"; then
          printf '%s\n' patched_hcd_validate >> "$PPTX_SUITE_FAILURES_FILE"
          PPTX_SUITE_FAILED=1
        fi
        if ! "$PPTX_SUITE_BIN" hdoc render-html "$PPTX_SUITE_CASE_DIR/hcd/bundle" \
          --revision 1 --output "$PPTX_SUITE_CASE_DIR/html/hcd-patched-preview.html" \
          --text-hitboxes on --image-hitboxes on --json \
          > "$PPTX_SUITE_REPORT_DIR/patched-render.json" \
          2> "$PPTX_SUITE_REPORT_DIR/patched-render.stderr.txt"; then
          printf '%s\n' patched_hcd_render >> "$PPTX_SUITE_FAILURES_FILE"
          PPTX_SUITE_FAILED=1
        fi
        if "$PPTX_SUITE_BIN" hdoc export "$PPTX_SUITE_CASE_DIR/hcd/bundle" \
          --source "$PPTX_SUITE_SOURCE" \
          --output "$PPTX_SUITE_CASE_DIR/patched/revision-1.pptx" \
          --revision 1 \
          --fidelity-report "$PPTX_SUITE_REPORT_DIR/patched-fidelity.json" \
          --json > "$PPTX_SUITE_REPORT_DIR/patched-export.json" \
          2> "$PPTX_SUITE_REPORT_DIR/patched-export.stderr.txt"; then
          if [[ "$(jq -r .level "$PPTX_SUITE_REPORT_DIR/patched-fidelity.json")" != HIGH ]]; then
            printf '%s\n' patched_fidelity_not_high >> "$PPTX_SUITE_FAILURES_FILE"
            PPTX_SUITE_FAILED=1
          fi
          if ! "$PPTX_SUITE_BIN" validate "$PPTX_SUITE_CASE_DIR/patched/revision-1.pptx" --json \
            > "$PPTX_SUITE_REPORT_DIR/patched-pptx-validate.json" \
            2> "$PPTX_SUITE_REPORT_DIR/patched-pptx-validate.stderr.txt"; then
            if ! pptx_suite_validation_issues_preserved \
              "$PPTX_SUITE_REPORT_DIR/source-validate.json" \
              "$PPTX_SUITE_REPORT_DIR/patched-pptx-validate.json" \
              "$PPTX_SUITE_REPORT_DIR/patched-validation-issues.diff"; then
              printf '%s\n' patched_export_validation_regression >> "$PPTX_SUITE_FAILURES_FILE"
              PPTX_SUITE_FAILED=1
            fi
          fi
          if ! "$PPTX_SUITE_BIN" view "$PPTX_SUITE_CASE_DIR/patched/revision-1.pptx" html \
            > "$PPTX_SUITE_CASE_DIR/html/patched-pptx-preview.html" \
            2> "$PPTX_SUITE_REPORT_DIR/patched-pptx-preview.stderr.txt"; then
            printf '%s\n' patched_pptx_preview >> "$PPTX_SUITE_FAILURES_FILE"
            PPTX_SUITE_FAILED=1
          fi
          if ! pptx_suite_take_screenshot "$PPTX_SUITE_CASE_DIR/patched/revision-1.pptx" \
            "$PPTX_SUITE_CASE_DIR/screenshots/patched.png" \
            "$PPTX_SUITE_REPORT_DIR/patched-screenshot.txt"; then
            printf '%s\n' patched_screenshot >> "$PPTX_SUITE_FAILURES_FILE"
            PPTX_SUITE_FAILED=1
          fi
          if "$PPTX_SUITE_BIN" hdoc import "$PPTX_SUITE_CASE_DIR/patched/revision-1.pptx" \
            --output "$PPTX_SUITE_CASE_DIR/hcd/patched-reimport" \
            --document-id "$PPTX_SUITE_DOCUMENT_ID" --json \
            > "$PPTX_SUITE_REPORT_DIR/patched-reimport.json" \
            2> "$PPTX_SUITE_REPORT_DIR/patched-reimport.stderr.txt" && \
            "$PPTX_SUITE_BIN" hdoc extract-text "$PPTX_SUITE_CASE_DIR/hcd/patched-reimport" \
            --limit 100000 --json \
            > "$PPTX_SUITE_REPORT_DIR/patched-reextract.json" \
            2> "$PPTX_SUITE_REPORT_DIR/patched-reextract.stderr.txt"; then
            if [[ "$(jq -r .rootHash "$PPTX_SUITE_CASE_DIR/hcd/bundle/manifest.json")" != \
                  "$(jq -r .rootHash "$PPTX_SUITE_CASE_DIR/hcd/patched-reimport/manifest.json")" ]]; then
              printf '%s\n' patched_reimport_root_hash >> "$PPTX_SUITE_FAILURES_FILE"
              PPTX_SUITE_FAILED=1
            fi
            local PPTX_SUITE_REIMPORTED_TEXT
            PPTX_SUITE_REIMPORTED_TEXT="$(jq -r --arg id "$PPTX_SUITE_NODE_ID" '.data.entries[] | select(.nodeId == $id) | .text' "$PPTX_SUITE_REPORT_DIR/patched-reextract.json")"
            if [[ "$PPTX_SUITE_REIMPORTED_TEXT" != '【HCD测试】'* ]]; then
              printf '%s\n' patched_reimport_node >> "$PPTX_SUITE_FAILURES_FILE"
              PPTX_SUITE_FAILED=1
            fi
            pptx_suite_image_node_ids "$PPTX_SUITE_CASE_DIR/hcd/patched-reimport" 0 \
              "$PPTX_SUITE_REPORT_DIR/patched-reimport-image-node-ids.txt"
            if ! diff -u "$PPTX_SUITE_REPORT_DIR/image-node-ids-a.txt" \
              "$PPTX_SUITE_REPORT_DIR/patched-reimport-image-node-ids.txt" \
              > "$PPTX_SUITE_REPORT_DIR/patched-reimport-image-node-ids.diff"; then
              printf '%s\n' patched_reimport_image_node_ids >> "$PPTX_SUITE_FAILURES_FILE"
              PPTX_SUITE_FAILED=1
            fi
          else
            printf '%s\n' patched_reimport >> "$PPTX_SUITE_FAILURES_FILE"
            PPTX_SUITE_FAILED=1
          fi
        else
          printf '%s\n' patched_export >> "$PPTX_SUITE_FAILURES_FILE"
          PPTX_SUITE_FAILED=1
        fi
      else
        printf '%s\n' patch_apply >> "$PPTX_SUITE_FAILURES_FILE"
        PPTX_SUITE_FAILED=1
      fi
    else
      printf '%s\n' patch_skipped_no_editable_slide_text \
        > "$PPTX_SUITE_REPORT_DIR/patch-skipped.txt"
    fi
  fi

  local PPTX_SUITE_SEMANTIC_REVISION="$PPTX_SUITE_HAS_PATCH"
  if "$PPTX_SUITE_BIN" hdoc export "$PPTX_SUITE_CASE_DIR/hcd/bundle" \
    --output "$PPTX_SUITE_CASE_DIR/semantic/rebuilt.pptx" \
    --revision "$PPTX_SUITE_SEMANTIC_REVISION" \
    --fidelity-report "$PPTX_SUITE_REPORT_DIR/semantic-fidelity.json" \
    --json > "$PPTX_SUITE_REPORT_DIR/semantic-export.json" \
    2> "$PPTX_SUITE_REPORT_DIR/semantic-export.stderr.txt"; then
    if [[ "$(jq -r .level "$PPTX_SUITE_REPORT_DIR/semantic-fidelity.json")" != SEMANTIC ]]; then
      printf '%s\n' semantic_fidelity_not_semantic >> "$PPTX_SUITE_FAILURES_FILE"
      PPTX_SUITE_FAILED=1
    fi
    if ! "$PPTX_SUITE_BIN" validate "$PPTX_SUITE_CASE_DIR/semantic/rebuilt.pptx" --json \
      > "$PPTX_SUITE_REPORT_DIR/semantic-validate.json" \
      2> "$PPTX_SUITE_REPORT_DIR/semantic-validate.stderr.txt"; then
      printf '%s\n' semantic_validate >> "$PPTX_SUITE_FAILURES_FILE"
      PPTX_SUITE_FAILED=1
    fi
    if ! "$PPTX_SUITE_BIN" view "$PPTX_SUITE_CASE_DIR/semantic/rebuilt.pptx" html \
      > "$PPTX_SUITE_CASE_DIR/html/semantic-pptx-preview.html" \
      2> "$PPTX_SUITE_REPORT_DIR/semantic-preview.stderr.txt"; then
      printf '%s\n' semantic_preview >> "$PPTX_SUITE_FAILURES_FILE"
      PPTX_SUITE_FAILED=1
    fi
    if ! pptx_suite_take_screenshot "$PPTX_SUITE_CASE_DIR/semantic/rebuilt.pptx" \
      "$PPTX_SUITE_CASE_DIR/screenshots/semantic.png" \
      "$PPTX_SUITE_REPORT_DIR/semantic-screenshot.txt"; then
      printf '%s\n' semantic_screenshot >> "$PPTX_SUITE_FAILURES_FILE"
      PPTX_SUITE_FAILED=1
    fi
  else
    printf '%s\n' semantic_export >> "$PPTX_SUITE_FAILURES_FILE"
    PPTX_SUITE_FAILED=1
  fi

  pptx_suite_write_comparison "$PPTX_SUITE_CASE_DIR" "$PPTX_SUITE_SOURCE_REL" "$PPTX_SUITE_HAS_PATCH"
  if [[ "$PPTX_SUITE_FAILED" -eq 0 ]]; then
    pptx_suite_emit_result passed "$PPTX_SUITE_SOURCE_REL" "$PPTX_SUITE_CASE_KEY" "$PPTX_SUITE_CASE_DIR" "$PPTX_SUITE_FAILURES_FILE"
  else
    pptx_suite_emit_result failed "$PPTX_SUITE_SOURCE_REL" "$PPTX_SUITE_CASE_KEY" "$PPTX_SUITE_CASE_DIR" "$PPTX_SUITE_FAILURES_FILE"
  fi
}

cd "$PPTX_SUITE_REPO_ROOT"
while IFS= read -r PPTX_SUITE_SOURCE_REL; do
  [[ -z "$PPTX_SUITE_SOURCE_REL" ]] && continue
  if [[ -n "$PPTX_SUITE_MATCH" && ! "$PPTX_SUITE_SOURCE_REL" =~ $PPTX_SUITE_MATCH ]]; then
    continue
  fi
  if [[ ! -f "$PPTX_SUITE_REPO_ROOT/$PPTX_SUITE_SOURCE_REL" ]]; then
    printf '%s\n' "$PPTX_SUITE_SOURCE_REL" >> "$PPTX_SUITE_MISSING"
    continue
  fi
  echo "Testing HCD PPTX pipeline: $PPTX_SUITE_SOURCE_REL"
  pptx_suite_run_case "$PPTX_SUITE_SOURCE_REL"
done < <(git ls-files 'examples/*.pptx' 'examples/**/*.pptx' | sort -u)

jq -s \
  --rawfile missing "$PPTX_SUITE_MISSING" \
  '{
    schemaVersion: "officecli-hdoc-pptx-suite/1",
    total: length,
    passed: (map(select(.status == "passed")) | length),
    failed: (map(select(.status == "failed")) | length),
    trackedMissing: ($missing | split("\n") | map(select(length > 0))),
    aggregate: {
      nodeCount: (map(.nodeCount // 0) | add // 0),
      editableNodeCount: (map(.editableNodeCount // 0) | add // 0),
      imageNodeCount: (map(.imageNodeCount // 0) | add // 0),
      assetCount: (map(.assetCount // 0) | add // 0),
      patchedCases: (map(select(.patchedExportFidelity != null)) | length)
    },
    cases: .
  }' "$PPTX_SUITE_RESULTS" > "$PPTX_SUITE_OUTPUT_ROOT/summary.json"

{
  printf '%s\n' '<!doctype html><html><head><meta charset="utf-8">'
  printf '%s\n' '<title>OfficeCLI HCD PPTX suite</title>'
  printf '%s\n' '<style>body{font:15px system-ui;margin:32px;max-width:1400px}table{border-collapse:collapse;width:100%}th,td{border:1px solid #ccc;padding:8px;text-align:left}.passed{color:#087a32}.failed{color:#b42318}code{font-size:12px}</style></head><body>'
  printf '<h1>OfficeCLI HCD PPTX suite</h1><p>Total: %s; passed: %s; failed: %s.</p>\n' \
    "$(jq -r .total "$PPTX_SUITE_OUTPUT_ROOT/summary.json")" \
    "$(jq -r .passed "$PPTX_SUITE_OUTPUT_ROOT/summary.json")" \
    "$(jq -r .failed "$PPTX_SUITE_OUTPUT_ROOT/summary.json")"
  printf '<p>Text nodes: %s; editable: %s; image placements: %s; content-addressed assets: %s; patched cases: %s.</p>\n' \
    "$(jq -r .aggregate.nodeCount "$PPTX_SUITE_OUTPUT_ROOT/summary.json")" \
    "$(jq -r .aggregate.editableNodeCount "$PPTX_SUITE_OUTPUT_ROOT/summary.json")" \
    "$(jq -r .aggregate.imageNodeCount "$PPTX_SUITE_OUTPUT_ROOT/summary.json")" \
    "$(jq -r .aggregate.assetCount "$PPTX_SUITE_OUTPUT_ROOT/summary.json")" \
    "$(jq -r .aggregate.patchedCases "$PPTX_SUITE_OUTPUT_ROOT/summary.json")"
  printf '%s\n' '<table><thead><tr><th>Source</th><th>Status</th><th>Nodes</th><th>Chunks</th><th>Import</th><th>Revision 0</th><th>Revision 1</th><th>Source-free</th><th>Artifacts</th></tr></thead><tbody>'
  jq -r --arg dash '-' '.cases[] | "<tr><td><code>\(.source)</code></td><td class=\"\(.status)\">\(.status)</td><td>\(.nodeCount // 0) / editable \(.editableNodeCount // 0) / images \(.imageNodeCount // 0) / assets \(.assetCount // 0)</td><td>\(.chunkCount // $dash)</td><td>\(.importFidelity // $dash)</td><td>\(.noopExportFidelity // $dash)</td><td>\(.patchedExportFidelity // $dash)</td><td>\(.semanticExportFidelity // $dash)</td><td><a href=\"\(.artifactDir)/comparison.html\">comparison</a> · <a href=\"\(.artifactDir)/html/hcd-preview.html\">HCD</a> · <a href=\"\(.artifactDir)/roundtrip/revision-0.pptx\">revision 0 PPTX</a>" + (if .patchedExportFidelity != null then " · <a href=\"\(.artifactDir)/patched/revision-1.pptx\">revision 1 PPTX</a>" else " · patched n/a" end) + " · <a href=\"\(.artifactDir)/semantic/rebuilt.pptx\">semantic PPTX</a></td></tr>"' \
    "$PPTX_SUITE_OUTPUT_ROOT/summary.json"
  printf '%s\n' '</tbody></table><p>“passed” means package identity, source/HCD/nodeId, source-backed export, patch, validation and re-import checks passed. It is not a pixel-level HCD fidelity claim. Every comparison page now includes an actual HCD screenshot beside the source screenshot; unresolved visual limitations are listed in each manifest.</p></body></html>'
} > "$PPTX_SUITE_OUTPUT_ROOT/index.html"

find "$PPTX_SUITE_OUTPUT_ROOT" -type f -print | sort \
  > "$PPTX_SUITE_OUTPUT_ROOT/artifact-index.txt"

PPTX_SUITE_FAILED_COUNT="$(jq -r .failed "$PPTX_SUITE_OUTPUT_ROOT/summary.json")"
echo "HCD PPTX suite artifacts: $PPTX_SUITE_OUTPUT_ROOT"
echo "Summary: $(jq -c '{total, passed, failed, trackedMissing}' "$PPTX_SUITE_OUTPUT_ROOT/summary.json")"
if [[ "$PPTX_SUITE_FAILED_COUNT" -ne 0 ]]; then
  exit 1
fi
