#!/usr/bin/env bash
set -euo pipefail

EXAMPLE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$EXAMPLE_ROOT/../.." && pwd)"
OUTPUT_ROOT="${1:-$EXAMPLE_ROOT/output}"
OFFICECLI_BIN="$REPO_ROOT/target/release/officecli"

if [[ -e "$OUTPUT_ROOT" ]]; then
  echo "Output already exists: $OUTPUT_ROOT" >&2
  echo "Pass a new directory path, for example: $0 /tmp/hdoc-output" >&2
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to build the nodeId patch fixture" >&2
  exit 2
fi

cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml"
mkdir -p \
  "$OUTPUT_ROOT/direct" \
  "$OUTPUT_ROOT/hcd" \
  "$OUTPUT_ROOT/patch" \
  "$OUTPUT_ROOT/previews" \
  "$OUTPUT_ROOT/reports" \
  "$OUTPUT_ROOT/semantic" \
  "$OUTPUT_ROOT/source-backed"

for extension in docx xlsx pptx pdf; do
  "$OFFICECLI_BIN" convert "$EXAMPLE_ROOT/source.html" \
    --output "$OUTPUT_ROOT/direct/from-html.$extension" \
    --json > "$OUTPUT_ROOT/reports/convert-html-$extension.json"
  "$OFFICECLI_BIN" validate "$OUTPUT_ROOT/direct/from-html.$extension" \
    > "$OUTPUT_ROOT/reports/validate-html-$extension.txt"
  "$OFFICECLI_BIN" view "$OUTPUT_ROOT/direct/from-html.$extension" html \
    > "$OUTPUT_ROOT/previews/from-html-$extension.html"
  "$OFFICECLI_BIN" view "$OUTPUT_ROOT/direct/from-html.$extension" text \
    > "$OUTPUT_ROOT/reports/view-html-$extension.txt"
  grep -F "Secret 123" "$OUTPUT_ROOT/reports/view-html-$extension.txt" >/dev/null
  if grep -F "活动脚本不应进入" "$OUTPUT_ROOT/reports/view-html-$extension.txt" >/dev/null; then
    echo "active HTML content leaked into direct $extension output" >&2
    exit 1
  fi
done

for extension in docx xlsx pptx pdf; do
  "$OFFICECLI_BIN" hdoc import "$OUTPUT_ROOT/direct/from-html.$extension" \
    --output "$OUTPUT_ROOT/hcd/from-$extension.hcd" \
    --document-id "hdoc-example-$extension" \
    --events ndjson > "$OUTPUT_ROOT/reports/import-$extension-events.ndjson"
  "$OFFICECLI_BIN" hdoc validate "$OUTPUT_ROOT/hcd/from-$extension.hcd" --json \
    > "$OUTPUT_ROOT/reports/validate-$extension-hcd.json"
  "$OFFICECLI_BIN" hdoc extract-text "$OUTPUT_ROOT/hcd/from-$extension.hcd" \
    --limit 10000 --json > "$OUTPUT_ROOT/reports/extract-$extension-hcd.json"
done

"$OFFICECLI_BIN" hdoc import "$EXAMPLE_ROOT/source.html" \
  --output "$OUTPUT_ROOT/hcd/html-revision.hcd" \
  --events ndjson > "$OUTPUT_ROOT/reports/import-html-events.ndjson"
"$OFFICECLI_BIN" hdoc validate "$OUTPUT_ROOT/hcd/html-revision.hcd" --json \
  > "$OUTPUT_ROOT/reports/validate-html-hcd-revision-0.json"
"$OFFICECLI_BIN" hdoc extract-text "$OUTPUT_ROOT/hcd/html-revision.hcd" \
  --limit 10000 --json > "$OUTPUT_ROOT/patch/extract-revision-0.json"

DOCUMENT_ID="$(jq -r '.data.documentId' "$OUTPUT_ROOT/patch/extract-revision-0.json")"
NODE_ID="$(jq -r '.data.entries[] | select(.text | contains("Secret 123")) | .nodeId' "$OUTPUT_ROOT/patch/extract-revision-0.json" | head -n 1)"
NODE_HASH="$(jq -r '.data.entries[] | select(.text | contains("Secret 123")) | .nodeHash' "$OUTPUT_ROOT/patch/extract-revision-0.json" | head -n 1)"

if [[ -z "$NODE_ID" || "$NODE_ID" == "null" ]]; then
  echo "Could not locate the Secret 123 test node" >&2
  exit 1
fi

"$OFFICECLI_BIN" hdoc get-node "$OUTPUT_ROOT/hcd/html-revision.hcd" "$NODE_ID" --json \
  > "$OUTPUT_ROOT/patch/node-before.json"

jq -n \
  --arg documentId "$DOCUMENT_ID" \
  --arg nodeId "$NODE_ID" \
  --arg nodeHash "$NODE_HASH" \
  '{
    schemaVersion: "hcd-patch/1",
    documentId: $documentId,
    patchId: "hdoc-example-mask-1",
    baseRevision: 0,
    operations: [{
      op: "text.splice",
      nodeId: $nodeId,
      start: 12,
      deleteCount: 3,
      insertText: "***",
      precondition: {nodeHash: $nodeHash}
    }]
  }' > "$OUTPUT_ROOT/patch/mask-secret-123.json"

"$OFFICECLI_BIN" hdoc apply "$OUTPUT_ROOT/hcd/html-revision.hcd" \
  --patch "$OUTPUT_ROOT/patch/mask-secret-123.json" \
  --expected-revision 0 --json > "$OUTPUT_ROOT/patch/apply-result.json"
"$OFFICECLI_BIN" hdoc get-node "$OUTPUT_ROOT/hcd/html-revision.hcd" "$NODE_ID" --json \
  > "$OUTPUT_ROOT/patch/node-after.json"
"$OFFICECLI_BIN" hdoc validate "$OUTPUT_ROOT/hcd/html-revision.hcd" --json \
  > "$OUTPUT_ROOT/reports/validate-html-hcd-revision-1.json"

BEFORE_NODE_ID="$(jq -r '.data.nodeId' "$OUTPUT_ROOT/patch/node-before.json")"
AFTER_NODE_ID="$(jq -r '.data.nodeId' "$OUTPUT_ROOT/patch/node-after.json")"
BEFORE_NODE_HASH="$(jq -r '.data.nodeHash' "$OUTPUT_ROOT/patch/node-before.json")"
AFTER_NODE_HASH="$(jq -r '.data.nodeHash' "$OUTPUT_ROOT/patch/node-after.json")"
AFTER_NODE_TEXT="$(jq -r '.data.text' "$OUTPUT_ROOT/patch/node-after.json")"
if [[ "$BEFORE_NODE_ID" != "$AFTER_NODE_ID" ]]; then
  echo "nodeId changed after text patch" >&2
  exit 1
fi
if [[ "$BEFORE_NODE_HASH" == "$AFTER_NODE_HASH" ]]; then
  echo "nodeHash did not change after text patch" >&2
  exit 1
fi
if [[ "$AFTER_NODE_TEXT" != *"Secret ***"* ]]; then
  echo "patched node does not contain the masked text" >&2
  exit 1
fi
jq -n \
  --arg nodeId "$AFTER_NODE_ID" \
  --arg beforeNodeHash "$BEFORE_NODE_HASH" \
  --arg afterNodeHash "$AFTER_NODE_HASH" \
  --arg afterText "$AFTER_NODE_TEXT" \
  '{
    nodeIdStableAfterPatch: true,
    nodeHashChanged: true,
    revisionAdvanced: true,
    nodeId: $nodeId,
    beforeNodeHash: $beforeNodeHash,
    afterNodeHash: $afterNodeHash,
    afterText: $afterText
  }' > "$OUTPUT_ROOT/reports/node-operation-summary.json"

"$OFFICECLI_BIN" hdoc export "$OUTPUT_ROOT/hcd/html-revision.hcd" \
  --source "$EXAMPLE_ROOT/source.html" \
  --output "$OUTPUT_ROOT/source-backed/patched-source.html" \
  --revision 1 \
  --fidelity-report "$OUTPUT_ROOT/source-backed/fidelity.json" \
  --json > "$OUTPUT_ROOT/reports/export-source-backed-html.json"

for extension in docx xlsx pptx pdf; do
  "$OFFICECLI_BIN" hdoc export "$OUTPUT_ROOT/hcd/html-revision.hcd" \
    --output "$OUTPUT_ROOT/semantic/revision-1.$extension" \
    --to "$extension" \
    --revision 1 \
    --fidelity-report "$OUTPUT_ROOT/semantic/fidelity-$extension.json" \
    --json > "$OUTPUT_ROOT/reports/export-hcd-$extension.json"
  "$OFFICECLI_BIN" validate "$OUTPUT_ROOT/semantic/revision-1.$extension" \
    > "$OUTPUT_ROOT/reports/validate-hcd-$extension.txt"
  "$OFFICECLI_BIN" view "$OUTPUT_ROOT/semantic/revision-1.$extension" html \
    > "$OUTPUT_ROOT/previews/hcd-revision-1-$extension.html"
  "$OFFICECLI_BIN" view "$OUTPUT_ROOT/semantic/revision-1.$extension" text \
    > "$OUTPUT_ROOT/reports/view-hcd-revision-1-$extension.txt"
  grep -F "Secret ***" "$OUTPUT_ROOT/reports/view-hcd-revision-1-$extension.txt" >/dev/null
  if grep -F "活动脚本不应进入" "$OUTPUT_ROOT/reports/view-hcd-revision-1-$extension.txt" >/dev/null; then
    echo "active HTML content leaked into HCD $extension output" >&2
    exit 1
  fi
done

"$OFFICECLI_BIN" hdoc import "$EXAMPLE_ROOT/source.txt" \
  --output "$OUTPUT_ROOT/hcd/text.hcd" \
  --document-id hdoc-example-text \
  --events ndjson > "$OUTPUT_ROOT/reports/import-text-events.ndjson"
"$OFFICECLI_BIN" hdoc validate "$OUTPUT_ROOT/hcd/text.hcd" --json \
  > "$OUTPUT_ROOT/reports/validate-text-hcd.json"
"$OFFICECLI_BIN" hdoc extract-text "$OUTPUT_ROOT/hcd/text.hcd" \
  --limit 10000 --json > "$OUTPUT_ROOT/reports/extract-text-hcd.json"

for extension in docx xlsx pptx pdf; do
  "$OFFICECLI_BIN" hdoc export "$OUTPUT_ROOT/hcd/text.hcd" \
    --output "$OUTPUT_ROOT/semantic/from-text.$extension" \
    --to "$extension" \
    --revision 0 \
    --fidelity-report "$OUTPUT_ROOT/semantic/fidelity-text-$extension.json" \
    --json > "$OUTPUT_ROOT/reports/export-text-hcd-$extension.json"
  "$OFFICECLI_BIN" validate "$OUTPUT_ROOT/semantic/from-text.$extension" \
    > "$OUTPUT_ROOT/reports/validate-text-hcd-$extension.txt"
  "$OFFICECLI_BIN" view "$OUTPUT_ROOT/semantic/from-text.$extension" text \
    > "$OUTPUT_ROOT/reports/view-text-hcd-$extension.txt"
  grep -F "Secret 789" "$OUTPUT_ROOT/reports/view-text-hcd-$extension.txt" >/dev/null
done

"$OFFICECLI_BIN" hdoc import "$EXAMPLE_ROOT/source.html" \
  --output "$OUTPUT_ROOT/hcd/stability-a.hcd" --json \
  > "$OUTPUT_ROOT/reports/import-stability-a.json"
"$OFFICECLI_BIN" hdoc import "$EXAMPLE_ROOT/source.html" \
  --output "$OUTPUT_ROOT/hcd/stability-b.hcd" --json \
  > "$OUTPUT_ROOT/reports/import-stability-b.json"
"$OFFICECLI_BIN" hdoc extract-text "$OUTPUT_ROOT/hcd/stability-a.hcd" \
  --limit 10000 --json | jq -r '.data.entries[].nodeId' \
  > "$OUTPUT_ROOT/reports/stability-a-node-ids.txt"
"$OFFICECLI_BIN" hdoc extract-text "$OUTPUT_ROOT/hcd/stability-b.hcd" \
  --limit 10000 --json | jq -r '.data.entries[].nodeId' \
  > "$OUTPUT_ROOT/reports/stability-b-node-ids.txt"
diff -u \
  "$OUTPUT_ROOT/reports/stability-a-node-ids.txt" \
  "$OUTPUT_ROOT/reports/stability-b-node-ids.txt" \
  > "$OUTPUT_ROOT/reports/stability-node-ids.diff"

STABLE_A_DOCUMENT_ID="$(jq -r '.documentId' "$OUTPUT_ROOT/hcd/stability-a.hcd/manifest.json")"
STABLE_B_DOCUMENT_ID="$(jq -r '.documentId' "$OUTPUT_ROOT/hcd/stability-b.hcd/manifest.json")"
STABLE_A_ROOT_HASH="$(jq -r '.rootHash' "$OUTPUT_ROOT/hcd/stability-a.hcd/manifest.json")"
STABLE_B_ROOT_HASH="$(jq -r '.rootHash' "$OUTPUT_ROOT/hcd/stability-b.hcd/manifest.json")"
if [[ "$STABLE_A_DOCUMENT_ID" != "$STABLE_B_DOCUMENT_ID" ]]; then
  echo "documentId changed across byte-identical imports" >&2
  exit 1
fi
if [[ "$STABLE_A_ROOT_HASH" != "$STABLE_B_ROOT_HASH" ]]; then
  echo "rootHash changed across byte-identical imports" >&2
  exit 1
fi

jq -n \
  --arg documentId "$STABLE_A_DOCUMENT_ID" \
  --arg rootHash "$STABLE_A_ROOT_HASH" \
  '{
    byteIdenticalImports: true,
    documentIdStable: true,
    rootHashStable: true,
    nodeIdsStable: true,
    documentId: $documentId,
    rootHash: $rootHash
  }' > "$OUTPUT_ROOT/reports/stability-summary.json"

echo "Generated and validated HCD examples in: $OUTPUT_ROOT"
