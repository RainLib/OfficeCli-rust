#!/usr/bin/env bash
set -uo pipefail

XLSX_SUITE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
XLSX_SUITE_REPO_ROOT="$(cd "$XLSX_SUITE_ROOT/../../.." && pwd)"
XLSX_SUITE_OUTPUT_ROOT="${1:-$XLSX_SUITE_ROOT/output}"
XLSX_SUITE_BIN="${XLSX_SUITE_BIN:-$XLSX_SUITE_REPO_ROOT/target/release/officecli}"

if [[ -e "$XLSX_SUITE_OUTPUT_ROOT" ]]; then
  echo "Output already exists: $XLSX_SUITE_OUTPUT_ROOT" >&2
  echo "Pass a new directory, for example: $0 /tmp/hdoc-xlsx-suite" >&2
  exit 2
fi

for XLSX_SUITE_TOOL in cargo git jq shasum unzip diff cmp grep; do
  if ! command -v "$XLSX_SUITE_TOOL" >/dev/null 2>&1; then
    echo "Required tool not found: $XLSX_SUITE_TOOL" >&2
    exit 2
  fi
done

if [[ "${XLSX_SUITE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build --release --manifest-path "$XLSX_SUITE_REPO_ROOT/Cargo.toml" -p officecli
fi
if [[ ! -x "$XLSX_SUITE_BIN" ]]; then
  echo "officecli binary not found: $XLSX_SUITE_BIN" >&2
  exit 2
fi

mkdir -p "$XLSX_SUITE_OUTPUT_ROOT/cases" "$XLSX_SUITE_OUTPUT_ROOT/reports"
XLSX_SUITE_RESULTS="$XLSX_SUITE_OUTPUT_ROOT/reports/results.ndjson"
XLSX_SUITE_MISSING="$XLSX_SUITE_OUTPUT_ROOT/reports/tracked-missing.txt"
: > "$XLSX_SUITE_RESULTS"
: > "$XLSX_SUITE_MISSING"

xlsx_suite_case_key() {
  printf '%s' "$1" | sed -E 's#^examples/##; s#\.xlsx$##I; s#[^[:alnum:]_.-]+#__#g'
}

xlsx_suite_validate_entry_contents() {
  local XLSX_SUITE_SOURCE="$1"
  local XLSX_SUITE_ROUNDTRIP="$2"
  local XLSX_SUITE_REPORT_DIR="$3"
  local XLSX_SUITE_ENTRY

  if ! diff -u \
    <(unzip -Z1 "$XLSX_SUITE_SOURCE") \
    <(unzip -Z1 "$XLSX_SUITE_ROUNDTRIP") \
    > "$XLSX_SUITE_REPORT_DIR/zip-entry-list.diff"; then
    return 1
  fi

  : > "$XLSX_SUITE_REPORT_DIR/zip-entry-content-mismatches.txt"
  while IFS= read -r XLSX_SUITE_ENTRY; do
    [[ "$XLSX_SUITE_ENTRY" == */ ]] && continue
    local XLSX_SUITE_ENTRY_PATTERN
    XLSX_SUITE_ENTRY_PATTERN="$(printf '%s' "$XLSX_SUITE_ENTRY" | sed 's/[][?*]/\\&/g')"
    if ! cmp -s \
      <(unzip -p "$XLSX_SUITE_SOURCE" "$XLSX_SUITE_ENTRY_PATTERN") \
      <(unzip -p "$XLSX_SUITE_ROUNDTRIP" "$XLSX_SUITE_ENTRY_PATTERN"); then
      printf '%s\n' "$XLSX_SUITE_ENTRY" \
        >> "$XLSX_SUITE_REPORT_DIR/zip-entry-content-mismatches.txt"
    fi
  done < <(unzip -Z1 "$XLSX_SUITE_SOURCE")

  [[ ! -s "$XLSX_SUITE_REPORT_DIR/zip-entry-content-mismatches.txt" ]]
}

xlsx_suite_validation_issues_preserved() {
  local XLSX_SUITE_SOURCE_VALIDATION="$1"
  local XLSX_SUITE_OUTPUT_VALIDATION="$2"
  local XLSX_SUITE_DIFF_PATH="$3"
  local XLSX_SUITE_SOURCE_NORMALIZED="${XLSX_SUITE_DIFF_PATH%.diff}-source.json"
  local XLSX_SUITE_OUTPUT_NORMALIZED="${XLSX_SUITE_DIFF_PATH%.diff}-output.json"

  jq -S '[.data[]? | {description, error_type, part}] | sort_by(.part, .error_type, .description)' \
    "$XLSX_SUITE_SOURCE_VALIDATION" > "$XLSX_SUITE_SOURCE_NORMALIZED"
  jq -S '[.data[]? | {description, error_type, part}] | sort_by(.part, .error_type, .description)' \
    "$XLSX_SUITE_OUTPUT_VALIDATION" > "$XLSX_SUITE_OUTPUT_NORMALIZED"
  diff -u "$XLSX_SUITE_SOURCE_NORMALIZED" "$XLSX_SUITE_OUTPUT_NORMALIZED" \
    > "$XLSX_SUITE_DIFF_PATH"
}

xlsx_suite_emit_result() {
  local XLSX_SUITE_STATUS="$1"
  local XLSX_SUITE_SOURCE_REL="$2"
  local XLSX_SUITE_CASE_KEY="$3"
  local XLSX_SUITE_CASE_DIR="$4"
  local XLSX_SUITE_FAILURES_FILE="$5"
  local XLSX_SUITE_MANIFEST="$XLSX_SUITE_CASE_DIR/hcd/bundle/manifest.json"
  local XLSX_SUITE_NOOP_FIDELITY="$XLSX_SUITE_CASE_DIR/reports/noop-fidelity.json"
  local XLSX_SUITE_PATCHED_FIDELITY="$XLSX_SUITE_CASE_DIR/reports/patched-fidelity.json"
  local XLSX_SUITE_SEMANTIC_FIDELITY="$XLSX_SUITE_CASE_DIR/reports/semantic-fidelity.json"
  local XLSX_SUITE_EXTRACT="$XLSX_SUITE_CASE_DIR/reports/extract-revision-0.json"

  jq -n -c \
    --arg status "$XLSX_SUITE_STATUS" \
    --arg source "$XLSX_SUITE_SOURCE_REL" \
    --arg caseKey "$XLSX_SUITE_CASE_KEY" \
    --arg artifactDir "${XLSX_SUITE_CASE_DIR#"$XLSX_SUITE_OUTPUT_ROOT"/}" \
    --rawfile failures "$XLSX_SUITE_FAILURES_FILE" \
    --slurpfile manifest "$XLSX_SUITE_MANIFEST" \
    --slurpfile noopFidelity "$XLSX_SUITE_NOOP_FIDELITY" \
    --slurpfile patchedFidelity "$XLSX_SUITE_PATCHED_FIDELITY" \
    --slurpfile semanticFidelity "$XLSX_SUITE_SEMANTIC_FIDELITY" \
    --slurpfile extracted "$XLSX_SUITE_EXTRACT" \
    '{
      status: $status,
      source: $source,
      caseKey: $caseKey,
      artifactDir: $artifactDir,
      failures: ($failures | split("\n") | map(select(length > 0))),
      profile: ($manifest[0].profile // null),
      chunkCount: ($manifest[0].chunkCount // null),
      importFidelity: ($manifest[0].fidelity.level // null),
      warningCodes: (($manifest[0].warnings // []) | map(.code) | unique),
      nodeCount: (($extracted[0].data.entries // []) | length),
      editableNodeCount: (($extracted[0].data.entries // []) | map(select(.source.editable == true)) | length),
      formulaNodeCount: (($extracted[0].data.entries // []) | map(select(.source.nodeKind == "cell" and .source.editable == false)) | length),
      imageNodeCount: (($extracted[0].data.entries // []) | map(select(.source.nodeKind == "image")) | length),
      chartNodeCount: (($extracted[0].data.entries // []) | map(select(.source.nodeKind == "chart")) | length),
      noopExportFidelity: (($noopFidelity[0] | if type == "object" then .level else null end) // null),
      patchedExportFidelity: (($patchedFidelity[0] | if type == "object" then .level else null end) // null),
      semanticExportFidelity: (($semanticFidelity[0] | if type == "object" then .level else null end) // null)
    }' >> "$XLSX_SUITE_RESULTS"
}

xlsx_suite_run_case() {
  local XLSX_SUITE_SOURCE_REL="$1"
  local XLSX_SUITE_SOURCE="$XLSX_SUITE_REPO_ROOT/$XLSX_SUITE_SOURCE_REL"
  local XLSX_SUITE_CASE_KEY
  XLSX_SUITE_CASE_KEY="$(xlsx_suite_case_key "$XLSX_SUITE_SOURCE_REL")"
  local XLSX_SUITE_CASE_DIR="$XLSX_SUITE_OUTPUT_ROOT/cases/$XLSX_SUITE_CASE_KEY"
  local XLSX_SUITE_REPORT_DIR="$XLSX_SUITE_CASE_DIR/reports"
  local XLSX_SUITE_FAILURES_FILE="$XLSX_SUITE_REPORT_DIR/failures.txt"
  local XLSX_SUITE_FAILED=0

  mkdir -p \
    "$XLSX_SUITE_CASE_DIR/hcd" \
    "$XLSX_SUITE_CASE_DIR/html" \
    "$XLSX_SUITE_CASE_DIR/roundtrip" \
    "$XLSX_SUITE_CASE_DIR/patched" \
    "$XLSX_SUITE_CASE_DIR/semantic" \
    "$XLSX_SUITE_REPORT_DIR"
  : > "$XLSX_SUITE_FAILURES_FILE"

  printf '%s\n' "$XLSX_SUITE_SOURCE_REL" > "$XLSX_SUITE_REPORT_DIR/source-path.txt"
  shasum -a 256 "$XLSX_SUITE_SOURCE" > "$XLSX_SUITE_REPORT_DIR/source.sha256"

  if ! "$XLSX_SUITE_BIN" validate "$XLSX_SUITE_SOURCE" --json \
    > "$XLSX_SUITE_REPORT_DIR/source-validate.json" \
    2> "$XLSX_SUITE_REPORT_DIR/source-validate.stderr.txt"; then
    printf '%s\n' "source contains pre-existing validation issues; source-backed output must preserve the same issue signatures" \
      > "$XLSX_SUITE_REPORT_DIR/source-validation-note.txt"
  fi
  if ! "$XLSX_SUITE_BIN" view "$XLSX_SUITE_SOURCE" stats --json \
    > "$XLSX_SUITE_REPORT_DIR/source-stats.json" \
    2> "$XLSX_SUITE_REPORT_DIR/source-stats.stderr.txt"; then
    printf '%s\n' "source_stats" >> "$XLSX_SUITE_FAILURES_FILE"
    XLSX_SUITE_FAILED=1
  fi
  if ! "$XLSX_SUITE_BIN" view "$XLSX_SUITE_SOURCE" html \
    > "$XLSX_SUITE_CASE_DIR/html/source-preview.html" \
    2> "$XLSX_SUITE_REPORT_DIR/source-preview.stderr.txt"; then
    printf '%s\n' "source_preview" >> "$XLSX_SUITE_FAILURES_FILE"
    XLSX_SUITE_FAILED=1
  fi

  if ! "$XLSX_SUITE_BIN" hdoc import "$XLSX_SUITE_SOURCE" \
    --output "$XLSX_SUITE_CASE_DIR/hcd/bundle" --events ndjson \
    > "$XLSX_SUITE_REPORT_DIR/import-events.ndjson" \
    2> "$XLSX_SUITE_REPORT_DIR/import.stderr.txt"; then
    printf '%s\n' "hcd_import" >> "$XLSX_SUITE_FAILURES_FILE"
    xlsx_suite_emit_result "failed" "$XLSX_SUITE_SOURCE_REL" "$XLSX_SUITE_CASE_KEY" "$XLSX_SUITE_CASE_DIR" "$XLSX_SUITE_FAILURES_FILE"
    return
  fi

  if ! "$XLSX_SUITE_BIN" hdoc validate "$XLSX_SUITE_CASE_DIR/hcd/bundle" --json \
    > "$XLSX_SUITE_REPORT_DIR/hcd-validate.json" \
    2> "$XLSX_SUITE_REPORT_DIR/hcd-validate.stderr.txt"; then
    printf '%s\n' "hcd_validate" >> "$XLSX_SUITE_FAILURES_FILE"
    XLSX_SUITE_FAILED=1
  fi
  if ! "$XLSX_SUITE_BIN" hdoc extract-text "$XLSX_SUITE_CASE_DIR/hcd/bundle" \
    --limit 100000 --json \
    > "$XLSX_SUITE_REPORT_DIR/extract-revision-0.json" \
    2> "$XLSX_SUITE_REPORT_DIR/extract-revision-0.stderr.txt"; then
    printf '%s\n' "hcd_extract" >> "$XLSX_SUITE_FAILURES_FILE"
    XLSX_SUITE_FAILED=1
  fi
  if ! "$XLSX_SUITE_BIN" hdoc render-html "$XLSX_SUITE_CASE_DIR/hcd/bundle" \
    --output "$XLSX_SUITE_CASE_DIR/html/hcd-preview.html" \
    --text-hitboxes on --image-hitboxes on --json \
    > "$XLSX_SUITE_REPORT_DIR/hcd-render.json" \
    2> "$XLSX_SUITE_REPORT_DIR/hcd-render.stderr.txt"; then
    printf '%s\n' "hcd_render" >> "$XLSX_SUITE_FAILURES_FILE"
    XLSX_SUITE_FAILED=1
  fi

  local XLSX_SUITE_SOURCE_SHEET_COUNT XLSX_SUITE_HCD_SHEET_COUNT XLSX_SUITE_HCD_SHEET_METADATA_COUNT
  XLSX_SUITE_SOURCE_SHEET_COUNT="$(
    unzip -p "$XLSX_SUITE_SOURCE" xl/workbook.xml 2>/dev/null \
      | grep -oE '<([[:alnum:]_.-]+:)?sheet([[:space:]>])' \
      | wc -l \
      | tr -d '[:space:]'
  )"
  XLSX_SUITE_HCD_SHEET_COUNT="$(
    grep -rhoE 'data-hcd-sheet="[^"]+"' "$XLSX_SUITE_CASE_DIR/hcd/bundle/chunks/sha256" 2>/dev/null \
      | sort -u \
      | wc -l \
      | tr -d '[:space:]'
  )"
  XLSX_SUITE_HCD_SHEET_METADATA_COUNT="$(
    grep -rhoE \
      'data-hcd-sheet="[^"]+" data-hcd-sheet-index="[0-9]+" data-hcd-sheet-state="(visible|hidden|very-hidden)"' \
      "$XLSX_SUITE_CASE_DIR/hcd/bundle/chunks/sha256" 2>/dev/null \
      | sort -u \
      | wc -l \
      | tr -d '[:space:]'
  )"
  printf 'source=%s\nhcd=%s\nhcdWithMetadata=%s\n' \
    "$XLSX_SUITE_SOURCE_SHEET_COUNT" "$XLSX_SUITE_HCD_SHEET_COUNT" \
    "$XLSX_SUITE_HCD_SHEET_METADATA_COUNT" \
    > "$XLSX_SUITE_REPORT_DIR/sheet-counts.txt"
  if [[ "$XLSX_SUITE_SOURCE_SHEET_COUNT" -lt 1 ]] || \
    [[ "$XLSX_SUITE_SOURCE_SHEET_COUNT" != "$XLSX_SUITE_HCD_SHEET_COUNT" ]]; then
    printf '%s\n' "sheet_count" >> "$XLSX_SUITE_FAILURES_FILE"
    XLSX_SUITE_FAILED=1
  fi
  if [[ "$XLSX_SUITE_HCD_SHEET_METADATA_COUNT" != "$XLSX_SUITE_HCD_SHEET_COUNT" ]]; then
    printf '%s\n' "sheet_metadata" >> "$XLSX_SUITE_FAILURES_FILE"
    XLSX_SUITE_FAILED=1
  fi

  local XLSX_SUITE_GRID_INDEX_REPORT="$XLSX_SUITE_REPORT_DIR/grid-index.json"
  if find "$XLSX_SUITE_CASE_DIR/hcd/bundle/indexes" -type f -name '*.json' -print0 \
    | xargs -0 jq -s \
      --argjson sourceSheetCount "$XLSX_SUITE_SOURCE_SHEET_COUNT" \
      '{
        descriptors: ([.[].chunks[]] | length),
        addressed: ([.[].chunks[] | select(.grid != null)] | length),
        sheets: ([.[].chunks[].grid.sheetId] | unique | length),
        valid: (
          all(.[].chunks[];
            (.region != "sheet") or
            (.grid != null and
             (.grid.sheetId | test("^s_[0-9a-f]{32}$")) and
             (.grid.sheetIndex >= 0) and
             (.grid.sheetState == "visible" or .grid.sheetState == "hidden" or .grid.sheetState == "veryHidden") and
             (.grid.kind == "cells" or .grid.kind == "picture" or .grid.kind == "chart") and
             ((.grid.rowStart == null or .grid.rowEnd == null) or .grid.rowStart <= .grid.rowEnd) and
             ((.grid.columnStart == null or .grid.columnEnd == null) or .grid.columnStart <= .grid.columnEnd))) and
          ([.[].chunks[].grid.sheetId] | unique | length) == $sourceSheetCount
        )
      }' > "$XLSX_SUITE_GRID_INDEX_REPORT" && \
    [[ "$(jq -r '.valid' "$XLSX_SUITE_GRID_INDEX_REPORT")" == "true" ]]; then
    :
  else
    printf '%s\n' "grid_random_access_index" >> "$XLSX_SUITE_FAILURES_FILE"
    XLSX_SUITE_FAILED=1
  fi

  local XLSX_SUITE_SOURCE_CHART_COUNT=0
  local XLSX_SUITE_DRAWING_PART XLSX_SUITE_DRAWING_CHART_COUNT
  while IFS= read -r XLSX_SUITE_DRAWING_PART; do
    XLSX_SUITE_DRAWING_CHART_COUNT="$(
      unzip -p "$XLSX_SUITE_SOURCE" "$XLSX_SUITE_DRAWING_PART" 2>/dev/null \
        | grep -oE '<([[:alnum:]_.-]+:)?chart([[:space:]/>])' \
        | wc -l \
        | tr -d '[:space:]'
    )"
    XLSX_SUITE_SOURCE_CHART_COUNT=$((XLSX_SUITE_SOURCE_CHART_COUNT + XLSX_SUITE_DRAWING_CHART_COUNT))
  done < <(unzip -Z1 "$XLSX_SUITE_SOURCE" | grep -E '^xl/drawings/drawing[0-9]+\.xml$' || true)
  local XLSX_SUITE_SOURCE_PREVIEW_CHART_COUNT XLSX_SUITE_HCD_CHART_COUNT
  XLSX_SUITE_SOURCE_PREVIEW_CHART_COUNT="$(
    grep -o 'class="chart-container"' "$XLSX_SUITE_CASE_DIR/html/source-preview.html" 2>/dev/null \
      | wc -l \
      | tr -d '[:space:]'
  )"
  XLSX_SUITE_HCD_CHART_COUNT="$(
    jq '[.data.entries[]? | select(.source.nodeKind == "chart")] | length' \
      "$XLSX_SUITE_REPORT_DIR/extract-revision-0.json"
  )"
  printf 'source=%s\nsourcePreview=%s\nhcd=%s\n' \
    "$XLSX_SUITE_SOURCE_CHART_COUNT" "$XLSX_SUITE_SOURCE_PREVIEW_CHART_COUNT" \
    "$XLSX_SUITE_HCD_CHART_COUNT" \
    > "$XLSX_SUITE_REPORT_DIR/chart-counts.txt"
  if [[ "$XLSX_SUITE_SOURCE_CHART_COUNT" != "$XLSX_SUITE_SOURCE_PREVIEW_CHART_COUNT" ]] || \
    [[ "$XLSX_SUITE_SOURCE_CHART_COUNT" != "$XLSX_SUITE_HCD_CHART_COUNT" ]]; then
    printf '%s\n' "chart_materialization" >> "$XLSX_SUITE_FAILURES_FILE"
    XLSX_SUITE_FAILED=1
  fi
  if ! grep -F 'hcd-grid-tabs' "$XLSX_SUITE_CASE_DIR/html/hcd-preview.html" >/dev/null || \
    ! grep -F 'hcd-grid-title' "$XLSX_SUITE_CASE_DIR/html/hcd-preview.html" >/dev/null || \
    ! grep -F 'hcd-grid-sheet-content' "$XLSX_SUITE_CASE_DIR/html/hcd-preview.html" >/dev/null || \
    ! grep -F 'hcd-row-header' "$XLSX_SUITE_CASE_DIR/html/hcd-preview.html" >/dev/null || \
    ! grep -F 'sheet.regions.push(region)' "$XLSX_SUITE_CASE_DIR/html/hcd-preview.html" >/dev/null || \
    ! grep -F 'content.append(region)' "$XLSX_SUITE_CASE_DIR/html/hcd-preview.html" >/dev/null; then
    printf '%s\n' "grid_presentation_chrome" >> "$XLSX_SUITE_FAILURES_FILE"
    XLSX_SUITE_FAILED=1
  fi

  if [[ -s "$XLSX_SUITE_REPORT_DIR/extract-revision-0.json" ]] && \
    jq -e 'all(.data.entries[] | select(.source.nodeKind == "cell" and .source.editable == false); .source.editable == false)' \
      "$XLSX_SUITE_REPORT_DIR/extract-revision-0.json" >/dev/null; then
    :
  else
    printf '%s\n' "formula_node_editability" >> "$XLSX_SUITE_FAILURES_FILE"
    XLSX_SUITE_FAILED=1
  fi

  if "$XLSX_SUITE_BIN" hdoc import "$XLSX_SUITE_SOURCE" \
    --output "$XLSX_SUITE_CASE_DIR/hcd/stability-b" --json \
    > "$XLSX_SUITE_REPORT_DIR/stability-import.json" \
    2> "$XLSX_SUITE_REPORT_DIR/stability-import.stderr.txt" && \
    "$XLSX_SUITE_BIN" hdoc extract-text "$XLSX_SUITE_CASE_DIR/hcd/stability-b" \
    --limit 100000 --json \
    > "$XLSX_SUITE_REPORT_DIR/stability-extract.json" \
    2> "$XLSX_SUITE_REPORT_DIR/stability-extract.stderr.txt"; then
    jq -r '.data.entries[].nodeId' "$XLSX_SUITE_REPORT_DIR/extract-revision-0.json" \
      > "$XLSX_SUITE_REPORT_DIR/node-ids-a.txt"
    jq -r '.data.entries[].nodeId' "$XLSX_SUITE_REPORT_DIR/stability-extract.json" \
      > "$XLSX_SUITE_REPORT_DIR/node-ids-b.txt"
    if ! diff -u \
      "$XLSX_SUITE_REPORT_DIR/node-ids-a.txt" \
      "$XLSX_SUITE_REPORT_DIR/node-ids-b.txt" \
      > "$XLSX_SUITE_REPORT_DIR/node-ids.diff"; then
      printf '%s\n' "node_id_stability" >> "$XLSX_SUITE_FAILURES_FILE"
      XLSX_SUITE_FAILED=1
    fi
    if [[ "$(jq -r '.rootHash' "$XLSX_SUITE_CASE_DIR/hcd/bundle/manifest.json")" != \
          "$(jq -r '.rootHash' "$XLSX_SUITE_CASE_DIR/hcd/stability-b/manifest.json")" ]]; then
      printf '%s\n' "root_hash_stability" >> "$XLSX_SUITE_FAILURES_FILE"
      XLSX_SUITE_FAILED=1
    fi
  else
    printf '%s\n' "repeat_import" >> "$XLSX_SUITE_FAILURES_FILE"
    XLSX_SUITE_FAILED=1
  fi

  if "$XLSX_SUITE_BIN" hdoc export "$XLSX_SUITE_CASE_DIR/hcd/bundle" \
    --source "$XLSX_SUITE_SOURCE" \
    --output "$XLSX_SUITE_CASE_DIR/roundtrip/revision-0.xlsx" \
    --revision 0 --fidelity-report "$XLSX_SUITE_REPORT_DIR/noop-fidelity.json" --json \
    > "$XLSX_SUITE_REPORT_DIR/noop-export.json" \
    2> "$XLSX_SUITE_REPORT_DIR/noop-export.stderr.txt"; then
    if ! "$XLSX_SUITE_BIN" validate "$XLSX_SUITE_CASE_DIR/roundtrip/revision-0.xlsx" --json \
      > "$XLSX_SUITE_REPORT_DIR/noop-validate.json" \
      2> "$XLSX_SUITE_REPORT_DIR/noop-validate.stderr.txt"; then
      if ! xlsx_suite_validation_issues_preserved \
        "$XLSX_SUITE_REPORT_DIR/source-validate.json" \
        "$XLSX_SUITE_REPORT_DIR/noop-validate.json" \
        "$XLSX_SUITE_REPORT_DIR/noop-validation-issues.diff"; then
        printf '%s\n' "noop_export_validation_regression" >> "$XLSX_SUITE_FAILURES_FILE"
        XLSX_SUITE_FAILED=1
      fi
    fi
    if ! xlsx_suite_validate_entry_contents \
      "$XLSX_SUITE_SOURCE" \
      "$XLSX_SUITE_CASE_DIR/roundtrip/revision-0.xlsx" \
      "$XLSX_SUITE_REPORT_DIR"; then
      printf '%s\n' "noop_zip_entry_identity" >> "$XLSX_SUITE_FAILURES_FILE"
      XLSX_SUITE_FAILED=1
    fi
  else
    printf '%s\n' "noop_export" >> "$XLSX_SUITE_FAILURES_FILE"
    XLSX_SUITE_FAILED=1
  fi

  if [[ -s "$XLSX_SUITE_REPORT_DIR/extract-revision-0.json" ]]; then
    local XLSX_SUITE_PATCH_NODE
    XLSX_SUITE_PATCH_NODE="$(jq -c 'first(.data.entries[] | select(.source.editable == true and .source.nodeKind == "cell" and (.text | length) > 0)) // empty' "$XLSX_SUITE_REPORT_DIR/extract-revision-0.json")"
    if [[ -n "$XLSX_SUITE_PATCH_NODE" ]]; then
      local XLSX_SUITE_DOCUMENT_ID XLSX_SUITE_NODE_ID XLSX_SUITE_NODE_HASH
      XLSX_SUITE_DOCUMENT_ID="$(jq -r '.data.documentId' "$XLSX_SUITE_REPORT_DIR/extract-revision-0.json")"
      XLSX_SUITE_NODE_ID="$(printf '%s' "$XLSX_SUITE_PATCH_NODE" | jq -r '.nodeId')"
      XLSX_SUITE_NODE_HASH="$(printf '%s' "$XLSX_SUITE_PATCH_NODE" | jq -r '.nodeHash')"
      jq -n \
        --arg documentId "$XLSX_SUITE_DOCUMENT_ID" \
        --arg nodeId "$XLSX_SUITE_NODE_ID" \
        --arg nodeHash "$XLSX_SUITE_NODE_HASH" \
        '{
          schemaVersion: "hcd-patch/1",
          documentId: $documentId,
          patchId: "examples-xlsx-suite-prefix-1",
          baseRevision: 0,
          operations: [{
            op: "text.splice",
            nodeId: $nodeId,
            start: 0,
            deleteCount: 0,
            insertText: "【HCD测试】",
            precondition: {nodeHash: $nodeHash}
          }]
        }' > "$XLSX_SUITE_CASE_DIR/patched/patch.json"

      if "$XLSX_SUITE_BIN" hdoc apply "$XLSX_SUITE_CASE_DIR/hcd/bundle" \
        --patch "$XLSX_SUITE_CASE_DIR/patched/patch.json" \
        --expected-revision 0 --json \
        > "$XLSX_SUITE_REPORT_DIR/patch-apply.json" \
        2> "$XLSX_SUITE_REPORT_DIR/patch-apply.stderr.txt" && \
        "$XLSX_SUITE_BIN" hdoc get-node "$XLSX_SUITE_CASE_DIR/hcd/bundle" \
        "$XLSX_SUITE_NODE_ID" --json \
        > "$XLSX_SUITE_REPORT_DIR/patched-node.json" \
        2> "$XLSX_SUITE_REPORT_DIR/patched-node.stderr.txt"; then
        if [[ "$(jq -r '.data.nodeId' "$XLSX_SUITE_REPORT_DIR/patched-node.json")" != "$XLSX_SUITE_NODE_ID" ]] || \
           [[ "$(jq -r '.data.text' "$XLSX_SUITE_REPORT_DIR/patched-node.json")" != "【HCD测试】"* ]]; then
          printf '%s\n' "patch_node_identity" >> "$XLSX_SUITE_FAILURES_FILE"
          XLSX_SUITE_FAILED=1
        fi
        if ! "$XLSX_SUITE_BIN" hdoc validate "$XLSX_SUITE_CASE_DIR/hcd/bundle" --json \
          > "$XLSX_SUITE_REPORT_DIR/patched-hcd-validate.json" \
          2> "$XLSX_SUITE_REPORT_DIR/patched-hcd-validate.stderr.txt"; then
          printf '%s\n' "patched_hcd_validate" >> "$XLSX_SUITE_FAILURES_FILE"
          XLSX_SUITE_FAILED=1
        fi
        if ! "$XLSX_SUITE_BIN" hdoc render-html "$XLSX_SUITE_CASE_DIR/hcd/bundle" \
          --revision 1 --output "$XLSX_SUITE_CASE_DIR/html/hcd-patched-preview.html" \
          --text-hitboxes on --image-hitboxes on --json \
          > "$XLSX_SUITE_REPORT_DIR/patched-render.json" \
          2> "$XLSX_SUITE_REPORT_DIR/patched-render.stderr.txt"; then
          printf '%s\n' "patched_hcd_render" >> "$XLSX_SUITE_FAILURES_FILE"
          XLSX_SUITE_FAILED=1
        fi
        if "$XLSX_SUITE_BIN" hdoc export "$XLSX_SUITE_CASE_DIR/hcd/bundle" \
          --source "$XLSX_SUITE_SOURCE" \
          --output "$XLSX_SUITE_CASE_DIR/patched/revision-1.xlsx" \
          --revision 1 --fidelity-report "$XLSX_SUITE_REPORT_DIR/patched-fidelity.json" --json \
          > "$XLSX_SUITE_REPORT_DIR/patched-export.json" \
          2> "$XLSX_SUITE_REPORT_DIR/patched-export.stderr.txt"; then
          if ! "$XLSX_SUITE_BIN" validate "$XLSX_SUITE_CASE_DIR/patched/revision-1.xlsx" --json \
            > "$XLSX_SUITE_REPORT_DIR/patched-xlsx-validate.json" \
            2> "$XLSX_SUITE_REPORT_DIR/patched-xlsx-validate.stderr.txt"; then
            if ! xlsx_suite_validation_issues_preserved \
              "$XLSX_SUITE_REPORT_DIR/source-validate.json" \
              "$XLSX_SUITE_REPORT_DIR/patched-xlsx-validate.json" \
              "$XLSX_SUITE_REPORT_DIR/patched-validation-issues.diff"; then
              printf '%s\n' "patched_export_validation_regression" >> "$XLSX_SUITE_FAILURES_FILE"
              XLSX_SUITE_FAILED=1
            fi
          fi
          if ! "$XLSX_SUITE_BIN" view "$XLSX_SUITE_CASE_DIR/patched/revision-1.xlsx" html \
            > "$XLSX_SUITE_CASE_DIR/html/patched-xlsx-preview.html" \
            2> "$XLSX_SUITE_REPORT_DIR/patched-xlsx-preview.stderr.txt"; then
            printf '%s\n' "patched_xlsx_preview" >> "$XLSX_SUITE_FAILURES_FILE"
            XLSX_SUITE_FAILED=1
          fi
          if ! "$XLSX_SUITE_BIN" view "$XLSX_SUITE_CASE_DIR/patched/revision-1.xlsx" text \
            > "$XLSX_SUITE_REPORT_DIR/patched-xlsx-text.txt" \
            2> "$XLSX_SUITE_REPORT_DIR/patched-xlsx-text.stderr.txt" || \
            ! grep -F '【HCD测试】' "$XLSX_SUITE_REPORT_DIR/patched-xlsx-text.txt" >/dev/null; then
            printf '%s\n' "patched_xlsx_text" >> "$XLSX_SUITE_FAILURES_FILE"
            XLSX_SUITE_FAILED=1
          fi
        else
          printf '%s\n' "patched_export" >> "$XLSX_SUITE_FAILURES_FILE"
          XLSX_SUITE_FAILED=1
        fi

        if "$XLSX_SUITE_BIN" hdoc export "$XLSX_SUITE_CASE_DIR/hcd/bundle" \
          --output "$XLSX_SUITE_CASE_DIR/semantic/revision-1.xlsx" --to xlsx \
          --revision 1 --fidelity-report "$XLSX_SUITE_REPORT_DIR/semantic-fidelity.json" --json \
          > "$XLSX_SUITE_REPORT_DIR/semantic-export.json" \
          2> "$XLSX_SUITE_REPORT_DIR/semantic-export.stderr.txt" && \
          "$XLSX_SUITE_BIN" validate "$XLSX_SUITE_CASE_DIR/semantic/revision-1.xlsx" --json \
          > "$XLSX_SUITE_REPORT_DIR/semantic-validate.json" \
          2> "$XLSX_SUITE_REPORT_DIR/semantic-validate.stderr.txt" && \
          "$XLSX_SUITE_BIN" view "$XLSX_SUITE_CASE_DIR/semantic/revision-1.xlsx" html \
          > "$XLSX_SUITE_CASE_DIR/html/semantic-xlsx-preview.html" \
          2> "$XLSX_SUITE_REPORT_DIR/semantic-xlsx-preview.stderr.txt"; then
          :
        else
          printf '%s\n' "semantic_export" >> "$XLSX_SUITE_FAILURES_FILE"
          XLSX_SUITE_FAILED=1
        fi
      else
        printf '%s\n' "patch_apply" >> "$XLSX_SUITE_FAILURES_FILE"
        XLSX_SUITE_FAILED=1
      fi
    else
      printf '%s\n' "patch_skipped_no_editable_cell" \
        > "$XLSX_SUITE_REPORT_DIR/patch-skipped.txt"
      printf '[]\n' > "$XLSX_SUITE_REPORT_DIR/patched-fidelity.json"
      printf '[]\n' > "$XLSX_SUITE_REPORT_DIR/semantic-fidelity.json"
    fi
  fi

  if [[ ! -f "$XLSX_SUITE_REPORT_DIR/patched-fidelity.json" ]]; then
    printf '[]\n' > "$XLSX_SUITE_REPORT_DIR/patched-fidelity.json"
  fi
  if [[ ! -f "$XLSX_SUITE_REPORT_DIR/semantic-fidelity.json" ]]; then
    printf '[]\n' > "$XLSX_SUITE_REPORT_DIR/semantic-fidelity.json"
  fi

  if [[ "$XLSX_SUITE_FAILED" -eq 0 ]]; then
    xlsx_suite_emit_result "passed" "$XLSX_SUITE_SOURCE_REL" "$XLSX_SUITE_CASE_KEY" "$XLSX_SUITE_CASE_DIR" "$XLSX_SUITE_FAILURES_FILE"
  else
    xlsx_suite_emit_result "failed" "$XLSX_SUITE_SOURCE_REL" "$XLSX_SUITE_CASE_KEY" "$XLSX_SUITE_CASE_DIR" "$XLSX_SUITE_FAILURES_FILE"
  fi
}

xlsx_suite_run_formula_readonly() {
  local XLSX_SUITE_FORMULA_DIR="$XLSX_SUITE_OUTPUT_ROOT/synthetic/formula-readonly"
  local XLSX_SUITE_FORMULA_SOURCE="$XLSX_SUITE_FORMULA_DIR/source.xlsx"
  local XLSX_SUITE_FORMULA_BUNDLE="$XLSX_SUITE_FORMULA_DIR/bundle"
  local XLSX_SUITE_FORMULA_EXTRACT="$XLSX_SUITE_FORMULA_DIR/extract.json"
  local XLSX_SUITE_FORMULA_PATCH="$XLSX_SUITE_FORMULA_DIR/patch.json"
  local XLSX_SUITE_FORMULA_PASSED=true
  mkdir -p "$XLSX_SUITE_FORMULA_DIR"

  if ! "$XLSX_SUITE_BIN" create "$XLSX_SUITE_FORMULA_SOURCE" >/dev/null || \
    ! "$XLSX_SUITE_BIN" set "$XLSX_SUITE_FORMULA_SOURCE" '/Sheet1/A1' \
      --prop value='Editable value' >/dev/null || \
    ! "$XLSX_SUITE_BIN" set "$XLSX_SUITE_FORMULA_SOURCE" '/Sheet1/B1' \
      --prop 'formula=SUM(1,2)' >/dev/null || \
    ! "$XLSX_SUITE_BIN" hdoc import "$XLSX_SUITE_FORMULA_SOURCE" \
      --output "$XLSX_SUITE_FORMULA_BUNDLE" \
      --document-id xlsx-suite-formula --json \
      > "$XLSX_SUITE_FORMULA_DIR/import.json" || \
    ! "$XLSX_SUITE_BIN" hdoc extract-text "$XLSX_SUITE_FORMULA_BUNDLE" \
      --limit 1000 --json > "$XLSX_SUITE_FORMULA_EXTRACT"; then
    XLSX_SUITE_FORMULA_PASSED=false
  fi

  local XLSX_SUITE_FORMULA_NODE=""
  if [[ "$XLSX_SUITE_FORMULA_PASSED" == true ]]; then
    XLSX_SUITE_FORMULA_NODE="$(jq -c 'first(.data.entries[] | select(.source.paragraphId == "B1")) // empty' "$XLSX_SUITE_FORMULA_EXTRACT")"
    if [[ -z "$XLSX_SUITE_FORMULA_NODE" ]] || \
      [[ "$(printf '%s' "$XLSX_SUITE_FORMULA_NODE" | jq -r '.source.editable')" != "false" ]]; then
      XLSX_SUITE_FORMULA_PASSED=false
    fi
  fi

  if [[ "$XLSX_SUITE_FORMULA_PASSED" == true ]]; then
    jq -n \
      --arg nodeId "$(printf '%s' "$XLSX_SUITE_FORMULA_NODE" | jq -r '.nodeId')" \
      --arg nodeHash "$(printf '%s' "$XLSX_SUITE_FORMULA_NODE" | jq -r '.nodeHash')" \
      '{
        schemaVersion: "hcd-patch/1",
        documentId: "xlsx-suite-formula",
        patchId: "xlsx-suite-formula-must-fail",
        baseRevision: 0,
        operations: [{
          op: "text.splice",
          nodeId: $nodeId,
          start: 0,
          deleteCount: 0,
          insertText: "blocked",
          precondition: {nodeHash: $nodeHash}
        }]
      }' > "$XLSX_SUITE_FORMULA_PATCH"
    if "$XLSX_SUITE_BIN" hdoc apply "$XLSX_SUITE_FORMULA_BUNDLE" \
      --patch "$XLSX_SUITE_FORMULA_PATCH" --expected-revision 0 --json \
      > "$XLSX_SUITE_FORMULA_DIR/apply.stdout.json" \
      2> "$XLSX_SUITE_FORMULA_DIR/apply.stderr.txt"; then
      XLSX_SUITE_FORMULA_PASSED=false
    fi
  fi

  jq -n \
    --argjson passed "$XLSX_SUITE_FORMULA_PASSED" \
    --arg nodeId "$(printf '%s' "$XLSX_SUITE_FORMULA_NODE" | jq -r '.nodeId // ""' 2>/dev/null)" \
    '{name:"formula-node-is-read-only",passed:$passed,nodeId:$nodeId}' \
    > "$XLSX_SUITE_FORMULA_DIR/result.json"
  XLSX_SUITE_FORMULA_READONLY="$XLSX_SUITE_FORMULA_PASSED"
}

cd "$XLSX_SUITE_REPO_ROOT"
while IFS= read -r XLSX_SUITE_SOURCE_REL; do
  [[ -z "$XLSX_SUITE_SOURCE_REL" ]] && continue
  if [[ ! -f "$XLSX_SUITE_REPO_ROOT/$XLSX_SUITE_SOURCE_REL" ]]; then
    printf '%s\n' "$XLSX_SUITE_SOURCE_REL" >> "$XLSX_SUITE_MISSING"
    continue
  fi
  echo "Testing HCD pipeline: $XLSX_SUITE_SOURCE_REL"
  xlsx_suite_run_case "$XLSX_SUITE_SOURCE_REL"
done < <(git ls-files 'examples/**/*.xlsx' | sort)

XLSX_SUITE_FORMULA_READONLY=false
xlsx_suite_run_formula_readonly

jq -s \
  --rawfile missing "$XLSX_SUITE_MISSING" \
  --argjson formulaReadonly "$XLSX_SUITE_FORMULA_READONLY" \
  '{
    schemaVersion: "officecli-hdoc-xlsx-suite/1",
    total: length,
    passed: (map(select(.status == "passed")) | length),
    failed: (map(select(.status == "failed")) | length),
    trackedMissing: ($missing | split("\n") | map(select(length > 0))),
    synthetic: {formulaReadonly: $formulaReadonly},
    aggregate: {
      nodeCount: (map(.nodeCount // 0) | add // 0),
      editableNodeCount: (map(.editableNodeCount // 0) | add // 0),
      formulaNodeCount: (map(.formulaNodeCount // 0) | add // 0),
      imageNodeCount: (map(.imageNodeCount // 0) | add // 0),
      chartNodeCount: (map(.chartNodeCount // 0) | add // 0),
      noEditableNodeCases: (map(select((.editableNodeCount // 0) == 0)) | length)
    },
    cases: .
  }' "$XLSX_SUITE_RESULTS" > "$XLSX_SUITE_OUTPUT_ROOT/summary.json"

{
  printf '%s\n' '<!doctype html><html><head><meta charset="utf-8">'
  printf '%s\n' '<title>OfficeCLI HCD XLSX suite</title>'
  printf '%s\n' '<style>body{font:15px system-ui;margin:32px;max-width:1400px}table{border-collapse:collapse;width:100%}th,td{border:1px solid #ccc;padding:8px;text-align:left;vertical-align:top}.passed{color:#087a32}.failed{color:#b42318}code{font-size:12px}</style></head><body>'
  printf '<h1>OfficeCLI HCD XLSX suite</h1><p>Total: %s; passed: %s; failed: %s.</p>\n' \
    "$(jq -r '.total' "$XLSX_SUITE_OUTPUT_ROOT/summary.json")" \
    "$(jq -r '.passed' "$XLSX_SUITE_OUTPUT_ROOT/summary.json")" \
    "$(jq -r '.failed' "$XLSX_SUITE_OUTPUT_ROOT/summary.json")"
  printf '<p>Formula node read-only rejection: <strong>%s</strong>.</p>\n' \
    "$(jq -r '.synthetic.formulaReadonly' "$XLSX_SUITE_OUTPUT_ROOT/summary.json")"
  printf '<p>Nodes: %s; editable: %s; formulas/read-only cells: %s; images: %s; charts: %s; cases without editable cells: %s.</p>\n' \
    "$(jq -r '.aggregate.nodeCount' "$XLSX_SUITE_OUTPUT_ROOT/summary.json")" \
    "$(jq -r '.aggregate.editableNodeCount' "$XLSX_SUITE_OUTPUT_ROOT/summary.json")" \
    "$(jq -r '.aggregate.formulaNodeCount' "$XLSX_SUITE_OUTPUT_ROOT/summary.json")" \
    "$(jq -r '.aggregate.imageNodeCount' "$XLSX_SUITE_OUTPUT_ROOT/summary.json")" \
    "$(jq -r '.aggregate.chartNodeCount' "$XLSX_SUITE_OUTPUT_ROOT/summary.json")" \
    "$(jq -r '.aggregate.noEditableNodeCases' "$XLSX_SUITE_OUTPUT_ROOT/summary.json")"
  printf '%s\n' '<table><thead><tr><th>Source</th><th>Status</th><th>Nodes</th><th>Formula/Image/Chart</th><th>Import</th><th>Exports</th><th>Previews</th></tr></thead><tbody>'
  jq -r --arg dash '-' '.cases[] | "<tr><td><code>\(.source)</code></td><td class=\"\(.status)\">\(.status)</td><td>\(.nodeCount // 0) / editable \(.editableNodeCount // 0)</td><td>\(.formulaNodeCount // 0) / \(.imageNodeCount // 0) / \(.chartNodeCount // 0)</td><td>\(.importFidelity // $dash)<br><code>\((.warningCodes // []) | join(", "))</code></td><td>no-op \(.noopExportFidelity // $dash)<br>patched \(.patchedExportFidelity // $dash)<br>semantic \(.semanticExportFidelity // $dash)</td><td><a href=\"\(.artifactDir)/html/source-preview.html\">source</a> · <a href=\"\(.artifactDir)/html/hcd-preview.html\">HCD</a>" + (if .patchedExportFidelity != null then " · <a href=\"\(.artifactDir)/html/hcd-patched-preview.html\">patched HCD</a> · <a href=\"\(.artifactDir)/html/patched-xlsx-preview.html\">patched XLSX</a> · <a href=\"\(.artifactDir)/html/semantic-xlsx-preview.html\">semantic XLSX</a>" else " · <span title=\"no editable cell\">patched n/a</span>" end) + "</td></tr>"' \
    "$XLSX_SUITE_OUTPUT_ROOT/summary.json"
  printf '%s\n' '</tbody></table><p>“passed” means the automated structure, HCD, stable-nodeId, formula read-only, no-op ZIP-entry identity, patch and export checks passed. It is not a pixel-level Excel chart/layout guarantee.</p></body></html>'
} > "$XLSX_SUITE_OUTPUT_ROOT/index.html"

find "$XLSX_SUITE_OUTPUT_ROOT" -type f -print | sort \
  > "$XLSX_SUITE_OUTPUT_ROOT/artifact-index.txt"

XLSX_SUITE_FAILED_COUNT="$(jq -r '.failed' "$XLSX_SUITE_OUTPUT_ROOT/summary.json")"
echo "HCD XLSX suite artifacts: $XLSX_SUITE_OUTPUT_ROOT"
echo "Summary: $(jq -c '{total, passed, failed, trackedMissing}' "$XLSX_SUITE_OUTPUT_ROOT/summary.json")"
if [[ "$XLSX_SUITE_FAILED_COUNT" -ne 0 ]]; then
  exit 1
fi
if [[ "$XLSX_SUITE_FORMULA_READONLY" != true ]]; then
  exit 1
fi
