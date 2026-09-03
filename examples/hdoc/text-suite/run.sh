#!/usr/bin/env bash
set -euo pipefail

SUITE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SUITE_ROOT/../../.." && pwd)"
OUTPUT_ROOT="${1:-$SUITE_ROOT/output}"
OFFICECLI_BIN="$REPO_ROOT/target/debug/officecli"

if [[ -e "$OUTPUT_ROOT" ]]; then
  echo "Output already exists: $OUTPUT_ROOT" >&2
  echo "Remove it or pass a new output path." >&2
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to construct nodeId patch fixtures" >&2
  exit 2
fi

cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p officecli
mkdir -p "$OUTPUT_ROOT/hcd" "$OUTPUT_ROOT/html" "$OUTPUT_ROOT/patch" \
  "$OUTPUT_ROOT/reports" "$OUTPUT_ROOT/source-backed" "$OUTPUT_ROOT/export"

for kind in markdown markdown-all text; do
  if [[ "$kind" == "markdown" ]]; then
    source="$SUITE_ROOT/source.md"
    source_extension="md"
  elif [[ "$kind" == "markdown-all" ]]; then
    source="$SUITE_ROOT/source-all-formats.md"
    source_extension="md"
  else
    source="$SUITE_ROOT/source.txt"
    source_extension="txt"
  fi
  bundle="$OUTPUT_ROOT/hcd/$kind.hcd"
  export_root="$OUTPUT_ROOT/export/$kind"
  mkdir -p "$export_root"

  "$OFFICECLI_BIN" hdoc import "$source" --output "$bundle" \
    --document-id "hdoc-$kind-example" --events ndjson \
    > "$OUTPUT_ROOT/reports/$kind-import-events.ndjson"
  "$OFFICECLI_BIN" hdoc validate "$bundle" --json \
    > "$OUTPUT_ROOT/reports/$kind-validate-revision-0.json"
  "$OFFICECLI_BIN" hdoc extract-text "$bundle" --limit 10000 --json \
    > "$OUTPUT_ROOT/reports/$kind-extract-revision-0.json"
  "$OFFICECLI_BIN" hdoc render-html "$bundle" \
    --output "$OUTPUT_ROOT/html/$kind-revision-0.html" --revision 0 --json \
    > "$OUTPUT_ROOT/reports/$kind-render-revision-0.json"

  exact="$OUTPUT_ROOT/source-backed/$kind-revision-0.$source_extension"
  "$OFFICECLI_BIN" hdoc export "$bundle" --source "$source" --output "$exact" \
    --revision 0 --fidelity-report "$OUTPUT_ROOT/reports/$kind-fidelity-revision-0.json" --json \
    > "$OUTPUT_ROOT/reports/$kind-export-revision-0.json"
  cmp "$source" "$exact"

  document_id="$(jq -r '.data.documentId' "$OUTPUT_ROOT/reports/$kind-extract-revision-0.json")"
  node_id="$(jq -r '.data.entries[] | select(.text == "Secret 123") | .nodeId' "$OUTPUT_ROOT/reports/$kind-extract-revision-0.json" | head -n 1)"
  node_hash="$(jq -r '.data.entries[] | select(.text == "Secret 123") | .nodeHash' "$OUTPUT_ROOT/reports/$kind-extract-revision-0.json" | head -n 1)"
  if [[ -z "$node_id" || "$node_id" == "null" ]]; then
    echo "Could not locate Secret 123 in $kind HCD" >&2
    exit 1
  fi
  jq -n --arg documentId "$document_id" --arg nodeId "$node_id" --arg nodeHash "$node_hash" \
    '{schemaVersion:"hcd-patch/1",documentId:$documentId,patchId:("text-suite-"+$documentId),baseRevision:0,operations:[{op:"text.splice",nodeId:$nodeId,start:7,deleteCount:3,insertText:"[MASKED]",precondition:{nodeHash:$nodeHash}}]}' \
    > "$OUTPUT_ROOT/patch/$kind-mask.json"
  "$OFFICECLI_BIN" hdoc apply "$bundle" --patch "$OUTPUT_ROOT/patch/$kind-mask.json" \
    --expected-revision 0 --json > "$OUTPUT_ROOT/reports/$kind-apply.json"
  "$OFFICECLI_BIN" hdoc validate "$bundle" --json \
    > "$OUTPUT_ROOT/reports/$kind-validate-revision-1.json"
  "$OFFICECLI_BIN" hdoc get-node "$bundle" "$node_id" --json \
    > "$OUTPUT_ROOT/reports/$kind-node-revision-1.json"
  "$OFFICECLI_BIN" hdoc render-html "$bundle" \
    --output "$OUTPUT_ROOT/html/$kind-revision-1.html" --revision 1 --json \
    > "$OUTPUT_ROOT/reports/$kind-render-revision-1.json"

  patched="$OUTPUT_ROOT/source-backed/$kind-revision-1.$source_extension"
  "$OFFICECLI_BIN" hdoc export "$bundle" --source "$source" --output "$patched" \
    --revision 1 --fidelity-report "$OUTPUT_ROOT/reports/$kind-fidelity-revision-1.json" --json \
    > "$OUTPUT_ROOT/reports/$kind-export-revision-1.json"

  for target in docx xlsx pptx pdf md txt; do
    artifact="$export_root/revision-1.$target"
    "$OFFICECLI_BIN" hdoc export "$bundle" --output "$artifact" --to "$target" \
      --revision 1 --fidelity-report "$OUTPUT_ROOT/reports/$kind-fidelity-$target.json" --json \
      > "$OUTPUT_ROOT/reports/$kind-export-$target.json"
    if [[ "$target" =~ ^(docx|xlsx|pptx|pdf)$ ]]; then
      "$OFFICECLI_BIN" validate "$artifact" --json \
        > "$OUTPUT_ROOT/reports/$kind-validate-$target.json"
      "$OFFICECLI_BIN" view "$artifact" html \
        > "$export_root/preview-$target.html"
      "$OFFICECLI_BIN" view "$artifact" text \
        > "$OUTPUT_ROOT/reports/$kind-view-$target.txt"
      grep -F "Secret [MASKED]" "$OUTPUT_ROOT/reports/$kind-view-$target.txt" >/dev/null
    else
      grep -F "Secret [MASKED]" "$artifact" >/dev/null
    fi
  done
done

markdown_all_html="$OUTPUT_ROOT/html/markdown-all-revision-0.html"
for marker in \
  '<h1 class="hcd-source-block hcd-markdown-heading"' \
  '<h6 class="hcd-source-block hcd-markdown-heading">' \
  '<strong>' '<em>' '<del>' '<code>' '<br/>' \
  'class="hcd-markdown-task"' \
  'class="hcd-markdown-task hcd-markdown-task-checked"' \
  '<ol class="hcd-markdown-list" start="3">' \
  'class="hcd-source-block hcd-markdown-quote"' \
  'class="hcd-source-block hcd-markdown-code"' \
  'hcd-markdown-rule"' \
  'class="hcd-markdown-table"' \
  'class="hcd-markdown-image"' \
  'data-hcd-metadata="yaml"' \
  'id="markdown-all"' \
  'data-hcd-markdown-classes="hcd-demo"' \
  'class="hcd-source-block hcd-markdown-footnotes"' \
  'class="hcd-source-block hcd-markdown-definition-list"' \
  'data-hcd-alert="warning"' \
  'data-hcd-math="inline"' \
  'data-hcd-math="display"' \
  'class="hcd-markdown-wikilink"' \
  'class="language-mermaid"' \
  'data-hcd-fenced="false"' \
  '<mark>' '<kbd>' '<sub>' '<sup>' \
  'class="hcd-markdown-raw-html"' \
  'href="https://example.com/docs"' \
  'href="https://example.org/guide"' \
  'href="mailto:editor@example.org"' \
  'href="https://example.com/reference"'; do
  grep -F "$marker" "$markdown_all_html" >/dev/null
done
if grep -F 'href="javascript:' "$markdown_all_html" >/dev/null; then
  echo "Unsafe JavaScript URL escaped the Markdown safety boundary" >&2
  exit 1
fi
for marker in \
  'class="hcd-mermaid-preview"' \
  'data-hcd-node-kind="diagram"' \
  'data-hcd-source-node-id=' \
  'data-hcd-mermaid-svg="true"' \
  '>Markdown</text>' \
  '>HCD</text>' \
  '>HTML</text>'; do
  grep -F "$marker" "$OUTPUT_ROOT/html/markdown-all-revision-1.html" >/dev/null
done
grep -F '😀' "$OUTPUT_ROOT/reports/markdown-all-view-pdf.txt" >/dev/null
grep -F '表格链接' "$OUTPUT_ROOT/reports/markdown-all-view-pdf.txt" >/dev/null
grep -F 'Markdown' "$OUTPUT_ROOT/reports/markdown-all-view-pdf.txt" >/dev/null
grep -F 'HCD' "$OUTPUT_ROOT/reports/markdown-all-view-pdf.txt" >/dev/null
grep -F 'HTML' "$OUTPUT_ROOT/reports/markdown-all-view-pdf.txt" >/dev/null
table_link_count="$(grep -o 'href="https://example.com/table"' \
  "$OUTPUT_ROOT/export/markdown-all/preview-pdf.html" | wc -l | tr -d ' ')"
if [[ "$table_link_count" != "1" ]]; then
  echo "Expected exactly one clickable PDF annotation for the Markdown table link, found $table_link_count" >&2
  exit 1
fi
grep -F '[文字上的安全链接](https://example.com/docs "文档站点")' \
  "$OUTPUT_ROOT/source-backed/markdown-all-revision-0.md" >/dev/null
grep -F '[表格链接](https://example.com/table)' \
  "$OUTPUT_ROOT/export/markdown-all/revision-1.md" >/dev/null

jq -n '{
  scope: "Complete CommonMark, GFM and explicitly enabled safe Markdown extensions exercised by OfficeCLI",
  statuses: {supported: "semantic HCD/HTML rendering plus source-backed editing", sanitized: "semantic content is preserved while active behavior is removed at the HCD security boundary"},
  features: [
    {feature:"ATX headings h1-h6",status:"supported",evidence:"h1-h6 semantic elements"},
    {feature:"paragraphs and blank lines",status:"supported",evidence:"source order and line endings retained"},
    {feature:"Unicode, CJK and emoji",status:"supported",evidence:"HTML and PDF text checks"},
    {feature:"emphasis, strong and combined emphasis",status:"supported",evidence:"em/strong nested elements"},
    {feature:"GFM strikethrough",status:"supported",evidence:"del element"},
    {feature:"inline code and escapes",status:"supported",evidence:"code element and literal escaped punctuation"},
    {feature:"hard line break",status:"supported",evidence:"br element"},
    {feature:"unordered and ordered lists with start",status:"supported",evidence:"ul/ol/li and start=3"},
    {feature:"GFM task lists",status:"supported",evidence:"checked and unchecked semantic task markers"},
    {feature:"blockquotes",status:"supported",evidence:"blockquote element"},
    {feature:"thematic break",status:"supported",evidence:"hr element"},
    {feature:"backtick and tilde fenced code",status:"supported",evidence:"pre/code presentation; language info remains source metadata only"},
    {feature:"inline links with optional title",status:"supported",evidence:"clickable anchor on label; URL is not appended"},
    {feature:"angle, email and raw URL autolinks",status:"supported",evidence:"safe href and mailto anchors"},
    {feature:"image syntax",status:"supported",evidence:"safe image node with source URL and alt label; remote bytes are not fetched"},
    {feature:"GFM pipe tables",status:"supported",evidence:"table rows and editable cell nodeIds"},
    {feature:"unsafe link schemes",status:"supported",evidence:"flattened to label with no dangerous href"},
    {feature:"Setext headings",status:"supported",evidence:"semantic h1/h2 with nodeId text"},
    {feature:"reference, collapsed and shortcut links",status:"supported",evidence:"resolved clickable anchors with editable labels"},
    {feature:"footnotes",status:"supported",evidence:"linked references and semantic definition sections"},
    {feature:"nested containers",status:"supported",evidence:"nested blockquote/list/item structure"},
    {feature:"definition lists",status:"supported",evidence:"dl/dt/dd semantic structure"},
    {feature:"table alignment",status:"supported",evidence:"left, center and right cell alignment"},
    {feature:"indented code",status:"supported",evidence:"non-fenced pre/code structure"},
    {feature:"heading attributes and metadata",status:"supported",evidence:"safe id/classes plus YAML/TOML metadata blocks"},
    {feature:"math, superscript and subscript",status:"supported",evidence:"semantic math/sup/sub elements"},
    {feature:"wikilinks",status:"supported",evidence:"safe local wiki anchors"},
    {feature:"GFM alerts and colon admonitions",status:"supported",evidence:"typed blockquote and aside containers"},
    {feature:"Mermaid fenced blocks",status:"supported",evidence:"editable source nodeId plus derived safe SVG/PDF preview for flowchart/graph and sequenceDiagram; no JavaScript execution"},
    {feature:"safe inline HTML",status:"supported",evidence:"allowlisted mark/kbd/u/s/sub/sup/small/br tags"},
    {feature:"active or unsafe raw HTML",status:"sanitized",evidence:"escaped editable source; never executed"}
  ]
}' > "$OUTPUT_ROOT/reports/markdown-all-feature-matrix.json"

echo "Generated and validated Markdown/TXT HCD examples in: $OUTPUT_ROOT"
