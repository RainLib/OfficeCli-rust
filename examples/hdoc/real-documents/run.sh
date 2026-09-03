#!/usr/bin/env bash
set -euo pipefail

# Real-file HCD conversion fixture.
# Source paths are supplied explicitly so this reproducible script never embeds
# workstation-specific paths or private document names.

REAL_HDOC_REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
REAL_HDOC_BIN="${REAL_HDOC_BIN:-$REAL_HDOC_REPO_ROOT/target/release/officecli}"
REAL_HDOC_OUTPUT_ROOT="${REAL_HDOC_OUTPUT_ROOT:-$REAL_HDOC_REPO_ROOT/examples/hdoc/real-documents/output}"

: "${REAL_HDOC_TEXTBOOK_PDF:?set REAL_HDOC_TEXTBOOK_PDF to the textbook PDF path}"
: "${REAL_HDOC_COMPLAINT_DOCX:?set REAL_HDOC_COMPLAINT_DOCX to the complaint DOCX path}"
: "${REAL_HDOC_CRIMINAL_PDF:?set REAL_HDOC_CRIMINAL_PDF to the criminal-case PDF path}"

if [[ ! -x "$REAL_HDOC_BIN" ]]; then
  echo "officecli binary not found: $REAL_HDOC_BIN" >&2
  exit 2
fi

for REAL_HDOC_SOURCE in \
  "$REAL_HDOC_TEXTBOOK_PDF" \
  "$REAL_HDOC_COMPLAINT_DOCX" \
  "$REAL_HDOC_CRIMINAL_PDF"; do
  if [[ ! -f "$REAL_HDOC_SOURCE" ]]; then
    echo "source file not found: $REAL_HDOC_SOURCE" >&2
    exit 2
  fi
done

prepare_case() {
  local REAL_HDOC_CASE_DIR="$1"
  mkdir -p \
    "$REAL_HDOC_CASE_DIR/source" \
    "$REAL_HDOC_CASE_DIR/html" \
    "$REAL_HDOC_CASE_DIR/hcd" \
    "$REAL_HDOC_CASE_DIR/exports" \
    "$REAL_HDOC_CASE_DIR/previews" \
    "$REAL_HDOC_CASE_DIR/reports"
}

process_case() {
  local REAL_HDOC_CASE_NAME="$1"
  local REAL_HDOC_SOURCE_PATH="$2"
  local REAL_HDOC_DOCUMENT_ID="$3"
  local REAL_HDOC_CASE_DIR="$REAL_HDOC_OUTPUT_ROOT/$REAL_HDOC_CASE_NAME"
  local REAL_HDOC_BUNDLE="$REAL_HDOC_CASE_DIR/hcd/bundle"

  prepare_case "$REAL_HDOC_CASE_DIR"

  if [[ -e "$REAL_HDOC_BUNDLE" ]]; then
    echo "bundle already exists; use a new REAL_HDOC_OUTPUT_ROOT: $REAL_HDOC_BUNDLE" >&2
    exit 2
  fi

  printf '%s\n' "$REAL_HDOC_SOURCE_PATH" > "$REAL_HDOC_CASE_DIR/reports/source-path.txt"
  shasum -a 256 "$REAL_HDOC_SOURCE_PATH" > "$REAL_HDOC_CASE_DIR/reports/source.sha256"
  file "$REAL_HDOC_SOURCE_PATH" > "$REAL_HDOC_CASE_DIR/reports/source-file-type.txt"

  "$REAL_HDOC_BIN" view "$REAL_HDOC_SOURCE_PATH" html > "$REAL_HDOC_CASE_DIR/html/source-preview.html"
  "$REAL_HDOC_BIN" hdoc import "$REAL_HDOC_SOURCE_PATH" \
    --output "$REAL_HDOC_BUNDLE" \
    --document-id "$REAL_HDOC_DOCUMENT_ID" \
    --events ndjson > "$REAL_HDOC_CASE_DIR/reports/import-events.ndjson"
  "$REAL_HDOC_BIN" hdoc validate "$REAL_HDOC_BUNDLE" --json > "$REAL_HDOC_CASE_DIR/reports/hcd-validation.json"
  "$REAL_HDOC_BIN" hdoc extract-text "$REAL_HDOC_BUNDLE" --limit 200 --json > "$REAL_HDOC_CASE_DIR/reports/extract-text-page1.json"
  "$REAL_HDOC_BIN" hdoc render-html "$REAL_HDOC_BUNDLE" \
    --output "$REAL_HDOC_CASE_DIR/html/hcd-preview.html" \
    --json > "$REAL_HDOC_CASE_DIR/reports/hcd-html.json"

  local REAL_HDOC_SOURCE_FORMAT
  REAL_HDOC_SOURCE_FORMAT="$(printf '%s' "${REAL_HDOC_SOURCE_PATH##*.}" | tr '[:upper:]' '[:lower:]')"
  for REAL_HDOC_TARGET in docx xlsx pptx pdf; do
    local REAL_HDOC_EXPORT_ARGS=(
      hdoc export "$REAL_HDOC_BUNDLE"
      --output "$REAL_HDOC_CASE_DIR/exports/from-hcd.$REAL_HDOC_TARGET"
      --to "$REAL_HDOC_TARGET"
      --revision 0
      --fidelity-report "$REAL_HDOC_CASE_DIR/reports/export-$REAL_HDOC_TARGET-fidelity.json"
      --json
    )
    if [[ "$REAL_HDOC_TARGET" == "$REAL_HDOC_SOURCE_FORMAT" ]]; then
      REAL_HDOC_EXPORT_ARGS+=(--source "$REAL_HDOC_SOURCE_PATH")
    fi
    "$REAL_HDOC_BIN" "${REAL_HDOC_EXPORT_ARGS[@]}" \
      > "$REAL_HDOC_CASE_DIR/reports/export-$REAL_HDOC_TARGET.json"
    "$REAL_HDOC_BIN" validate "$REAL_HDOC_CASE_DIR/exports/from-hcd.$REAL_HDOC_TARGET" \
      --json > "$REAL_HDOC_CASE_DIR/reports/validate-$REAL_HDOC_TARGET.json"
    if [[ "$REAL_HDOC_TARGET" == "pdf" ]]; then
      if command -v pdftoppm >/dev/null 2>&1; then
        pdftoppm -f 1 -l 3 -r 110 -png \
          "$REAL_HDOC_CASE_DIR/exports/from-hcd.pdf" \
          "$REAL_HDOC_CASE_DIR/previews/from-hcd-pdf-page"
      fi
      if command -v pdfinfo >/dev/null 2>&1; then
        pdfinfo "$REAL_HDOC_CASE_DIR/exports/from-hcd.pdf" \
          > "$REAL_HDOC_CASE_DIR/reports/export-pdf-info.txt"
      fi
    else
      "$REAL_HDOC_BIN" view "$REAL_HDOC_CASE_DIR/exports/from-hcd.$REAL_HDOC_TARGET" html \
        > "$REAL_HDOC_CASE_DIR/previews/from-hcd-$REAL_HDOC_TARGET.html"
    fi
  done

  shasum -a 256 "$REAL_HDOC_CASE_DIR"/exports/from-hcd.* \
    > "$REAL_HDOC_CASE_DIR/reports/export-artifacts.sha256"
}

mkdir -p "$REAL_HDOC_OUTPUT_ROOT"

process_case \
  "02-english-textbook-pdf" \
  "$REAL_HDOC_TEXTBOOK_PDF" \
  "real-english-textbook-pdf"
process_case \
  "03-civil-complaint-docx" \
  "$REAL_HDOC_COMPLAINT_DOCX" \
  "real-civil-complaint-docx"
process_case \
  "04-criminal-case-pdf" \
  "$REAL_HDOC_CRIMINAL_PDF" \
  "real-criminal-case-pdf"

find "$REAL_HDOC_OUTPUT_ROOT" -type f -print | sort > "$REAL_HDOC_OUTPUT_ROOT/artifact-index.txt"
echo "real document artifacts written to: $REAL_HDOC_OUTPUT_ROOT"
