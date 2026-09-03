#!/usr/bin/env bash
set -uo pipefail

DOCX_SUITE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DOCX_SUITE_REPO_ROOT="$(cd "$DOCX_SUITE_ROOT/../../.." && pwd)"
DOCX_SUITE_OUTPUT_ROOT="${1:-$DOCX_SUITE_ROOT/output}"
DOCX_SUITE_BIN="${DOCX_SUITE_BIN:-$DOCX_SUITE_REPO_ROOT/target/release/officecli}"

if [[ -e "$DOCX_SUITE_OUTPUT_ROOT" ]]; then
  echo "Output already exists: $DOCX_SUITE_OUTPUT_ROOT" >&2
  echo "Pass a new directory, for example: $0 /tmp/hdoc-docx-suite" >&2
  exit 2
fi

for DOCX_SUITE_TOOL in cargo git jq shasum unzip diff cmp; do
  if ! command -v "$DOCX_SUITE_TOOL" >/dev/null 2>&1; then
    echo "Required tool not found: $DOCX_SUITE_TOOL" >&2
    exit 2
  fi
done

if [[ "${DOCX_SUITE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build --release --manifest-path "$DOCX_SUITE_REPO_ROOT/Cargo.toml" -p officecli
fi
if [[ ! -x "$DOCX_SUITE_BIN" ]]; then
  echo "officecli binary not found: $DOCX_SUITE_BIN" >&2
  exit 2
fi

mkdir -p "$DOCX_SUITE_OUTPUT_ROOT/cases" "$DOCX_SUITE_OUTPUT_ROOT/reports"
DOCX_SUITE_RESULTS="$DOCX_SUITE_OUTPUT_ROOT/reports/results.ndjson"
DOCX_SUITE_MISSING="$DOCX_SUITE_OUTPUT_ROOT/reports/tracked-missing.txt"
: > "$DOCX_SUITE_RESULTS"
: > "$DOCX_SUITE_MISSING"

docx_suite_case_key() {
  printf '%s' "$1" | sed -E 's#^examples/##; s#\.docx$##I; s#[^[:alnum:]_.-]+#__#g'
}

docx_suite_validate_entry_contents() {
  local DOCX_SUITE_SOURCE="$1"
  local DOCX_SUITE_ROUNDTRIP="$2"
  local DOCX_SUITE_REPORT_DIR="$3"
  local DOCX_SUITE_ENTRY

  if ! diff -u \
    <(unzip -Z1 "$DOCX_SUITE_SOURCE") \
    <(unzip -Z1 "$DOCX_SUITE_ROUNDTRIP") \
    > "$DOCX_SUITE_REPORT_DIR/zip-entry-list.diff"; then
    return 1
  fi

  : > "$DOCX_SUITE_REPORT_DIR/zip-entry-content-mismatches.txt"
  while IFS= read -r DOCX_SUITE_ENTRY; do
    [[ "$DOCX_SUITE_ENTRY" == */ ]] && continue
    local DOCX_SUITE_ENTRY_PATTERN
    DOCX_SUITE_ENTRY_PATTERN="$(printf '%s' "$DOCX_SUITE_ENTRY" | sed 's/[][?*]/\\&/g')"
    if ! cmp -s \
      <(unzip -p "$DOCX_SUITE_SOURCE" "$DOCX_SUITE_ENTRY_PATTERN") \
      <(unzip -p "$DOCX_SUITE_ROUNDTRIP" "$DOCX_SUITE_ENTRY_PATTERN"); then
      printf '%s\n' "$DOCX_SUITE_ENTRY" \
        >> "$DOCX_SUITE_REPORT_DIR/zip-entry-content-mismatches.txt"
    fi
  done < <(unzip -Z1 "$DOCX_SUITE_SOURCE")

  [[ ! -s "$DOCX_SUITE_REPORT_DIR/zip-entry-content-mismatches.txt" ]]
}

docx_suite_validation_issues_preserved() {
  local DOCX_SUITE_SOURCE_VALIDATION="$1"
  local DOCX_SUITE_OUTPUT_VALIDATION="$2"
  local DOCX_SUITE_DIFF_PATH="$3"
  local DOCX_SUITE_SOURCE_NORMALIZED="${DOCX_SUITE_DIFF_PATH%.diff}-source.json"
  local DOCX_SUITE_OUTPUT_NORMALIZED="${DOCX_SUITE_DIFF_PATH%.diff}-output.json"

  jq -S '[.data[]? | {description, error_type, part}] | sort_by(.part, .error_type, .description)' \
    "$DOCX_SUITE_SOURCE_VALIDATION" > "$DOCX_SUITE_SOURCE_NORMALIZED"
  jq -S '[.data[]? | {description, error_type, part}] | sort_by(.part, .error_type, .description)' \
    "$DOCX_SUITE_OUTPUT_VALIDATION" > "$DOCX_SUITE_OUTPUT_NORMALIZED"
  diff -u "$DOCX_SUITE_SOURCE_NORMALIZED" "$DOCX_SUITE_OUTPUT_NORMALIZED" \
    > "$DOCX_SUITE_DIFF_PATH"
}

docx_suite_emit_result() {
  local DOCX_SUITE_STATUS="$1"
  local DOCX_SUITE_SOURCE_REL="$2"
  local DOCX_SUITE_CASE_KEY="$3"
  local DOCX_SUITE_CASE_DIR="$4"
  local DOCX_SUITE_FAILURES_FILE="$5"
  local DOCX_SUITE_MANIFEST="$DOCX_SUITE_CASE_DIR/hcd/bundle/manifest.json"
  local DOCX_SUITE_NOOP_FIDELITY="$DOCX_SUITE_CASE_DIR/reports/noop-fidelity.json"
  local DOCX_SUITE_PATCHED_FIDELITY="$DOCX_SUITE_CASE_DIR/reports/patched-fidelity.json"
  local DOCX_SUITE_SOURCE_VALIDATION="$DOCX_SUITE_CASE_DIR/reports/source-validate.json"

  jq -n -c \
    --arg status "$DOCX_SUITE_STATUS" \
    --arg source "$DOCX_SUITE_SOURCE_REL" \
    --arg caseKey "$DOCX_SUITE_CASE_KEY" \
    --arg artifactDir "${DOCX_SUITE_CASE_DIR#"$DOCX_SUITE_OUTPUT_ROOT"/}" \
    --rawfile failures "$DOCX_SUITE_FAILURES_FILE" \
    --slurpfile manifest "$DOCX_SUITE_MANIFEST" \
    --slurpfile noopFidelity "$DOCX_SUITE_NOOP_FIDELITY" \
    --slurpfile patchedFidelity "$DOCX_SUITE_PATCHED_FIDELITY" \
    --slurpfile sourceValidation "$DOCX_SUITE_SOURCE_VALIDATION" \
    '{
      status: $status,
      source: $source,
      caseKey: $caseKey,
      artifactDir: $artifactDir,
      failures: ($failures | split("\n") | map(select(length > 0))),
      profile: ($manifest[0].profile // null),
      chunkCount: ($manifest[0].chunkCount // null),
      importFidelity: ($manifest[0].fidelity.level // null),
      exactPagination: $manifest[0].capabilities.exactPagination,
      importWarningCount: (($manifest[0].warnings // []) | length),
      sourceValid: ($sourceValidation[0].success // false),
      sourceValidationIssueCount: (($sourceValidation[0].data // []) | length),
      noopExportFidelity: ($noopFidelity[0].level // null),
      patchedExportFidelity: ($patchedFidelity[0].level // null)
    }' >> "$DOCX_SUITE_RESULTS"
}

docx_suite_run_case() {
  local DOCX_SUITE_SOURCE_REL="$1"
  local DOCX_SUITE_SOURCE="$DOCX_SUITE_REPO_ROOT/$DOCX_SUITE_SOURCE_REL"
  local DOCX_SUITE_CASE_KEY
  DOCX_SUITE_CASE_KEY="$(docx_suite_case_key "$DOCX_SUITE_SOURCE_REL")"
  local DOCX_SUITE_CASE_DIR="$DOCX_SUITE_OUTPUT_ROOT/cases/$DOCX_SUITE_CASE_KEY"
  local DOCX_SUITE_REPORT_DIR="$DOCX_SUITE_CASE_DIR/reports"
  local DOCX_SUITE_FAILURES_FILE="$DOCX_SUITE_REPORT_DIR/failures.txt"
  local DOCX_SUITE_FAILED=0

  mkdir -p \
    "$DOCX_SUITE_CASE_DIR/hcd" \
    "$DOCX_SUITE_CASE_DIR/html" \
    "$DOCX_SUITE_CASE_DIR/roundtrip" \
    "$DOCX_SUITE_CASE_DIR/patched" \
    "$DOCX_SUITE_REPORT_DIR"
  : > "$DOCX_SUITE_FAILURES_FILE"

  printf '%s\n' "$DOCX_SUITE_SOURCE_REL" > "$DOCX_SUITE_REPORT_DIR/source-path.txt"
  shasum -a 256 "$DOCX_SUITE_SOURCE" > "$DOCX_SUITE_REPORT_DIR/source.sha256"

  if ! "$DOCX_SUITE_BIN" validate "$DOCX_SUITE_SOURCE" --json \
    > "$DOCX_SUITE_REPORT_DIR/source-validate.json" \
    2> "$DOCX_SUITE_REPORT_DIR/source-validate.stderr.txt"; then
    printf '%s\n' "source contains pre-existing validation issues; output must preserve the same issue signatures" \
      > "$DOCX_SUITE_REPORT_DIR/source-validation-note.txt"
  fi
  if ! "$DOCX_SUITE_BIN" view "$DOCX_SUITE_SOURCE" stats --json \
    > "$DOCX_SUITE_REPORT_DIR/source-stats.json" \
    2> "$DOCX_SUITE_REPORT_DIR/source-stats.stderr.txt"; then
    printf '%s\n' "source_stats" >> "$DOCX_SUITE_FAILURES_FILE"
    DOCX_SUITE_FAILED=1
  fi
  if ! "$DOCX_SUITE_BIN" view "$DOCX_SUITE_SOURCE" html \
    > "$DOCX_SUITE_CASE_DIR/html/source-preview.html" \
    2> "$DOCX_SUITE_REPORT_DIR/source-preview.stderr.txt"; then
    printf '%s\n' "source_preview" >> "$DOCX_SUITE_FAILURES_FILE"
    DOCX_SUITE_FAILED=1
  fi

  if ! "$DOCX_SUITE_BIN" hdoc import "$DOCX_SUITE_SOURCE" \
    --output "$DOCX_SUITE_CASE_DIR/hcd/bundle" \
    --events ndjson \
    > "$DOCX_SUITE_REPORT_DIR/import-events.ndjson" \
    2> "$DOCX_SUITE_REPORT_DIR/import.stderr.txt"; then
    printf '%s\n' "hcd_import" >> "$DOCX_SUITE_FAILURES_FILE"
    docx_suite_emit_result "failed" "$DOCX_SUITE_SOURCE_REL" "$DOCX_SUITE_CASE_KEY" "$DOCX_SUITE_CASE_DIR" "$DOCX_SUITE_FAILURES_FILE"
    return
  fi

  if ! "$DOCX_SUITE_BIN" hdoc validate "$DOCX_SUITE_CASE_DIR/hcd/bundle" --json \
    > "$DOCX_SUITE_REPORT_DIR/hcd-validate.json" \
    2> "$DOCX_SUITE_REPORT_DIR/hcd-validate.stderr.txt"; then
    printf '%s\n' "hcd_validate" >> "$DOCX_SUITE_FAILURES_FILE"
    DOCX_SUITE_FAILED=1
  fi
  if ! "$DOCX_SUITE_BIN" hdoc extract-text "$DOCX_SUITE_CASE_DIR/hcd/bundle" \
    --limit 100000 --json \
    > "$DOCX_SUITE_REPORT_DIR/extract-revision-0.json" \
    2> "$DOCX_SUITE_REPORT_DIR/extract-revision-0.stderr.txt"; then
    printf '%s\n' "hcd_extract" >> "$DOCX_SUITE_FAILURES_FILE"
    DOCX_SUITE_FAILED=1
  fi
  if ! "$DOCX_SUITE_BIN" hdoc render-html "$DOCX_SUITE_CASE_DIR/hcd/bundle" \
    --output "$DOCX_SUITE_CASE_DIR/html/hcd-preview.html" \
    --text-hitboxes on --image-hitboxes on --json \
    > "$DOCX_SUITE_REPORT_DIR/hcd-render.json" \
    2> "$DOCX_SUITE_REPORT_DIR/hcd-render.stderr.txt"; then
    printf '%s\n' "hcd_render" >> "$DOCX_SUITE_FAILURES_FILE"
    DOCX_SUITE_FAILED=1
  fi

  if "$DOCX_SUITE_BIN" hdoc import "$DOCX_SUITE_SOURCE" \
    --output "$DOCX_SUITE_CASE_DIR/hcd/stability-b" --json \
    > "$DOCX_SUITE_REPORT_DIR/stability-import.json" \
    2> "$DOCX_SUITE_REPORT_DIR/stability-import.stderr.txt" && \
    "$DOCX_SUITE_BIN" hdoc extract-text "$DOCX_SUITE_CASE_DIR/hcd/stability-b" \
    --limit 100000 --json \
    > "$DOCX_SUITE_REPORT_DIR/stability-extract.json" \
    2> "$DOCX_SUITE_REPORT_DIR/stability-extract.stderr.txt"; then
    jq -r '.data.entries[].nodeId' "$DOCX_SUITE_REPORT_DIR/extract-revision-0.json" \
      > "$DOCX_SUITE_REPORT_DIR/node-ids-a.txt"
    jq -r '.data.entries[].nodeId' "$DOCX_SUITE_REPORT_DIR/stability-extract.json" \
      > "$DOCX_SUITE_REPORT_DIR/node-ids-b.txt"
    if ! diff -u \
      "$DOCX_SUITE_REPORT_DIR/node-ids-a.txt" \
      "$DOCX_SUITE_REPORT_DIR/node-ids-b.txt" \
      > "$DOCX_SUITE_REPORT_DIR/node-ids.diff"; then
      printf '%s\n' "node_id_stability" >> "$DOCX_SUITE_FAILURES_FILE"
      DOCX_SUITE_FAILED=1
    fi
    if [[ "$(jq -r '.rootHash' "$DOCX_SUITE_CASE_DIR/hcd/bundle/manifest.json")" != \
          "$(jq -r '.rootHash' "$DOCX_SUITE_CASE_DIR/hcd/stability-b/manifest.json")" ]]; then
      printf '%s\n' "root_hash_stability" >> "$DOCX_SUITE_FAILURES_FILE"
      DOCX_SUITE_FAILED=1
    fi
  else
    printf '%s\n' "repeat_import" >> "$DOCX_SUITE_FAILURES_FILE"
    DOCX_SUITE_FAILED=1
  fi

  if "$DOCX_SUITE_BIN" hdoc export "$DOCX_SUITE_CASE_DIR/hcd/bundle" \
    --source "$DOCX_SUITE_SOURCE" \
    --output "$DOCX_SUITE_CASE_DIR/roundtrip/revision-0.docx" \
    --revision 0 \
    --fidelity-report "$DOCX_SUITE_REPORT_DIR/noop-fidelity.json" \
    --json > "$DOCX_SUITE_REPORT_DIR/noop-export.json" \
    2> "$DOCX_SUITE_REPORT_DIR/noop-export.stderr.txt"; then
    if ! "$DOCX_SUITE_BIN" validate "$DOCX_SUITE_CASE_DIR/roundtrip/revision-0.docx" --json \
      > "$DOCX_SUITE_REPORT_DIR/noop-validate.json" \
      2> "$DOCX_SUITE_REPORT_DIR/noop-validate.stderr.txt"; then
      if ! docx_suite_validation_issues_preserved \
        "$DOCX_SUITE_REPORT_DIR/source-validate.json" \
        "$DOCX_SUITE_REPORT_DIR/noop-validate.json" \
        "$DOCX_SUITE_REPORT_DIR/noop-validation-issues.diff"; then
        printf '%s\n' "noop_export_validation_regression" >> "$DOCX_SUITE_FAILURES_FILE"
        DOCX_SUITE_FAILED=1
      fi
    fi
    shasum -a 256 "$DOCX_SUITE_CASE_DIR/roundtrip/revision-0.docx" \
      > "$DOCX_SUITE_REPORT_DIR/noop-output.sha256"
    if ! docx_suite_validate_entry_contents \
      "$DOCX_SUITE_SOURCE" \
      "$DOCX_SUITE_CASE_DIR/roundtrip/revision-0.docx" \
      "$DOCX_SUITE_REPORT_DIR"; then
      printf '%s\n' "noop_zip_entry_identity" >> "$DOCX_SUITE_FAILURES_FILE"
      DOCX_SUITE_FAILED=1
    fi
  else
    printf '%s\n' "noop_export" >> "$DOCX_SUITE_FAILURES_FILE"
    DOCX_SUITE_FAILED=1
  fi

  if [[ -s "$DOCX_SUITE_REPORT_DIR/extract-revision-0.json" ]]; then
    local DOCX_SUITE_PATCH_NODE
    DOCX_SUITE_PATCH_NODE="$(jq -c 'first(.data.entries[] | select(.source.editable == true and .source.part == "word/document.xml" and (.text | length) > 0)) // empty' "$DOCX_SUITE_REPORT_DIR/extract-revision-0.json")"
    if [[ -n "$DOCX_SUITE_PATCH_NODE" ]]; then
      local DOCX_SUITE_DOCUMENT_ID DOCX_SUITE_NODE_ID DOCX_SUITE_NODE_HASH
      DOCX_SUITE_DOCUMENT_ID="$(jq -r '.data.documentId' "$DOCX_SUITE_REPORT_DIR/extract-revision-0.json")"
      DOCX_SUITE_NODE_ID="$(printf '%s' "$DOCX_SUITE_PATCH_NODE" | jq -r '.nodeId')"
      DOCX_SUITE_NODE_HASH="$(printf '%s' "$DOCX_SUITE_PATCH_NODE" | jq -r '.nodeHash')"
      jq -n \
        --arg documentId "$DOCX_SUITE_DOCUMENT_ID" \
        --arg nodeId "$DOCX_SUITE_NODE_ID" \
        --arg nodeHash "$DOCX_SUITE_NODE_HASH" \
        '{
          schemaVersion: "hcd-patch/1",
          documentId: $documentId,
          patchId: "examples-docx-suite-prefix-1",
          baseRevision: 0,
          operations: [{
            op: "text.splice",
            nodeId: $nodeId,
            start: 0,
            deleteCount: 0,
            insertText: "【HCD测试】",
            precondition: {nodeHash: $nodeHash}
          }]
        }' > "$DOCX_SUITE_CASE_DIR/patched/patch.json"

      if "$DOCX_SUITE_BIN" hdoc apply "$DOCX_SUITE_CASE_DIR/hcd/bundle" \
        --patch "$DOCX_SUITE_CASE_DIR/patched/patch.json" \
        --expected-revision 0 --json \
        > "$DOCX_SUITE_REPORT_DIR/patch-apply.json" \
        2> "$DOCX_SUITE_REPORT_DIR/patch-apply.stderr.txt" && \
        "$DOCX_SUITE_BIN" hdoc get-node "$DOCX_SUITE_CASE_DIR/hcd/bundle" \
        "$DOCX_SUITE_NODE_ID" --json \
        > "$DOCX_SUITE_REPORT_DIR/patched-node.json" \
        2> "$DOCX_SUITE_REPORT_DIR/patched-node.stderr.txt"; then
        if [[ "$(jq -r '.data.nodeId' "$DOCX_SUITE_REPORT_DIR/patched-node.json")" != "$DOCX_SUITE_NODE_ID" ]] || \
           [[ "$(jq -r '.data.text' "$DOCX_SUITE_REPORT_DIR/patched-node.json")" != "【HCD测试】"* ]]; then
          printf '%s\n' "patch_node_identity" >> "$DOCX_SUITE_FAILURES_FILE"
          DOCX_SUITE_FAILED=1
        fi
        if ! "$DOCX_SUITE_BIN" hdoc validate "$DOCX_SUITE_CASE_DIR/hcd/bundle" --json \
          > "$DOCX_SUITE_REPORT_DIR/patched-hcd-validate.json" \
          2> "$DOCX_SUITE_REPORT_DIR/patched-hcd-validate.stderr.txt"; then
          printf '%s\n' "patched_hcd_validate" >> "$DOCX_SUITE_FAILURES_FILE"
          DOCX_SUITE_FAILED=1
        fi
        if ! "$DOCX_SUITE_BIN" hdoc render-html "$DOCX_SUITE_CASE_DIR/hcd/bundle" \
          --revision 1 \
          --output "$DOCX_SUITE_CASE_DIR/html/hcd-patched-preview.html" \
          --text-hitboxes on --image-hitboxes on --json \
          > "$DOCX_SUITE_REPORT_DIR/patched-render.json" \
          2> "$DOCX_SUITE_REPORT_DIR/patched-render.stderr.txt"; then
          printf '%s\n' "patched_hcd_render" >> "$DOCX_SUITE_FAILURES_FILE"
          DOCX_SUITE_FAILED=1
        fi
        if "$DOCX_SUITE_BIN" hdoc export "$DOCX_SUITE_CASE_DIR/hcd/bundle" \
          --source "$DOCX_SUITE_SOURCE" \
          --output "$DOCX_SUITE_CASE_DIR/patched/revision-1.docx" \
          --revision 1 \
          --fidelity-report "$DOCX_SUITE_REPORT_DIR/patched-fidelity.json" \
          --json > "$DOCX_SUITE_REPORT_DIR/patched-export.json" \
          2> "$DOCX_SUITE_REPORT_DIR/patched-export.stderr.txt"; then
          if ! "$DOCX_SUITE_BIN" validate "$DOCX_SUITE_CASE_DIR/patched/revision-1.docx" --json \
            > "$DOCX_SUITE_REPORT_DIR/patched-docx-validate.json" \
            2> "$DOCX_SUITE_REPORT_DIR/patched-docx-validate.stderr.txt"; then
            if ! docx_suite_validation_issues_preserved \
              "$DOCX_SUITE_REPORT_DIR/source-validate.json" \
              "$DOCX_SUITE_REPORT_DIR/patched-docx-validate.json" \
              "$DOCX_SUITE_REPORT_DIR/patched-validation-issues.diff"; then
              printf '%s\n' "patched_export_validation_regression" >> "$DOCX_SUITE_FAILURES_FILE"
              DOCX_SUITE_FAILED=1
            fi
          fi
          if ! "$DOCX_SUITE_BIN" view "$DOCX_SUITE_CASE_DIR/patched/revision-1.docx" html \
            > "$DOCX_SUITE_CASE_DIR/html/patched-docx-preview.html" \
            2> "$DOCX_SUITE_REPORT_DIR/patched-docx-preview.stderr.txt"; then
            printf '%s\n' "patched_docx_preview" >> "$DOCX_SUITE_FAILURES_FILE"
            DOCX_SUITE_FAILED=1
          fi
        else
          printf '%s\n' "patched_export" >> "$DOCX_SUITE_FAILURES_FILE"
          DOCX_SUITE_FAILED=1
        fi
      else
        printf '%s\n' "patch_apply" >> "$DOCX_SUITE_FAILURES_FILE"
        DOCX_SUITE_FAILED=1
      fi
    else
      printf '%s\n' "patch_skipped_no_editable_body_text" \
        > "$DOCX_SUITE_REPORT_DIR/patch-skipped.txt"
    fi
  fi

  if [[ "$DOCX_SUITE_FAILED" -eq 0 ]]; then
    docx_suite_emit_result "passed" "$DOCX_SUITE_SOURCE_REL" "$DOCX_SUITE_CASE_KEY" "$DOCX_SUITE_CASE_DIR" "$DOCX_SUITE_FAILURES_FILE"
  else
    docx_suite_emit_result "failed" "$DOCX_SUITE_SOURCE_REL" "$DOCX_SUITE_CASE_KEY" "$DOCX_SUITE_CASE_DIR" "$DOCX_SUITE_FAILURES_FILE"
  fi
}

cd "$DOCX_SUITE_REPO_ROOT"
while IFS= read -r DOCX_SUITE_SOURCE_REL; do
  [[ -z "$DOCX_SUITE_SOURCE_REL" ]] && continue
  if [[ ! -f "$DOCX_SUITE_REPO_ROOT/$DOCX_SUITE_SOURCE_REL" ]]; then
    printf '%s\n' "$DOCX_SUITE_SOURCE_REL" >> "$DOCX_SUITE_MISSING"
    continue
  fi
  echo "Testing HCD pipeline: $DOCX_SUITE_SOURCE_REL"
  docx_suite_run_case "$DOCX_SUITE_SOURCE_REL"
done < <(git ls-files 'examples/**/*.docx' | sort)

jq -s \
  --rawfile missing "$DOCX_SUITE_MISSING" \
  '{
    schemaVersion: "officecli-hdoc-docx-suite/1",
    total: length,
    passed: (map(select(.status == "passed")) | length),
    failed: (map(select(.status == "failed")) | length),
    trackedMissing: ($missing | split("\n") | map(select(length > 0))),
    cases: .
  }' "$DOCX_SUITE_RESULTS" > "$DOCX_SUITE_OUTPUT_ROOT/summary.json"

{
  printf '%s\n' '<!doctype html><html><head><meta charset="utf-8">'
  printf '%s\n' '<title>OfficeCLI HCD DOCX suite</title>'
  printf '%s\n' '<style>body{font:15px system-ui;margin:32px;max-width:1200px}table{border-collapse:collapse;width:100%}th,td{border:1px solid #ccc;padding:8px;text-align:left}.passed{color:#087a32}.failed{color:#b42318}code{font-size:12px}</style></head><body>'
  printf '<h1>OfficeCLI HCD DOCX suite</h1><p>Total: %s; passed: %s; failed: %s.</p>\n' \
    "$(jq -r '.total' "$DOCX_SUITE_OUTPUT_ROOT/summary.json")" \
    "$(jq -r '.passed' "$DOCX_SUITE_OUTPUT_ROOT/summary.json")" \
    "$(jq -r '.failed' "$DOCX_SUITE_OUTPUT_ROOT/summary.json")"
  printf '%s\n' '<table><thead><tr><th>Source</th><th>Status</th><th>Chunks</th><th>Import</th><th>No-op export</th><th>Patched export</th><th>Previews</th></tr></thead><tbody>'
  jq -r --arg dash '-' '.cases[] | "<tr><td><code>\(.source)</code></td><td class=\"\(.status)\">\(.status)</td><td>\(.chunkCount // $dash)</td><td>\(.importFidelity // $dash)</td><td>\(.noopExportFidelity // $dash)</td><td>\(.patchedExportFidelity // $dash)</td><td><a href=\"\(.artifactDir)/html/source-preview.html\">source</a> · <a href=\"\(.artifactDir)/html/hcd-preview.html\">HCD</a> · <a href=\"\(.artifactDir)/html/hcd-patched-preview.html\">patched HCD</a> · <a href=\"\(.artifactDir)/html/patched-docx-preview.html\">patched DOCX</a></td></tr>"' \
    "$DOCX_SUITE_OUTPUT_ROOT/summary.json"
  printf '%s\n' '</tbody></table><p>“passed” means the automated structural, HCD, stable-nodeId, no-op package-entry identity, patch, export and validation checks passed. It is not a pixel-level Word pagination guarantee.</p></body></html>'
} > "$DOCX_SUITE_OUTPUT_ROOT/index.html"

find "$DOCX_SUITE_OUTPUT_ROOT" -type f -print | sort \
  > "$DOCX_SUITE_OUTPUT_ROOT/artifact-index.txt"

DOCX_SUITE_FAILED_COUNT="$(jq -r '.failed' "$DOCX_SUITE_OUTPUT_ROOT/summary.json")"
echo "HCD DOCX suite artifacts: $DOCX_SUITE_OUTPUT_ROOT"
echo "Summary: $(jq -c '{total, passed, failed, trackedMissing}' "$DOCX_SUITE_OUTPUT_ROOT/summary.json")"
if [[ "$DOCX_SUITE_FAILED_COUNT" -ne 0 ]]; then
  exit 1
fi
