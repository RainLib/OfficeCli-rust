#!/usr/bin/env bash
set -euo pipefail

stress_mib="${HCD_STRESS_MIB:-2040}"
rss_limit_mib="${HCD_STRESS_RSS_MIB:-512}"
stress_root="$(mktemp -d "${TMPDIR:-/tmp}/officecli-hcd-stress.XXXXXX")"
source_docx="$stress_root/source.docx"
bundle="$stress_root/bundle"
time_report="$stress_root/time.txt"
patch_file="$stress_root/patch.json"
exported_docx="$stress_root/exported.docx"
export_time_report="$stress_root/export-time.txt"

cleanup() {
  rm -rf "$stress_root"
}
trap cleanup EXIT

cargo build --release -p officecli
cargo run --release -p hcd-docx --example generate_stress_docx -- "$source_docx" "$stress_mib"

source_bytes="$(wc -c < "$source_docx" | tr -d ' ')"
if [ "$source_bytes" -gt $((256 * 1024 * 1024)) ]; then
  echo "compressed fixture exceeds 256 MiB: $source_bytes bytes" >&2
  exit 1
fi

if /usr/bin/time --version 2>&1 | grep -q 'GNU time'; then
  /usr/bin/time -v -o "$time_report" \
    target/release/officecli hdoc import "$source_docx" --output "$bundle" --document-id hcd-stress --events ndjson \
    > "$stress_root/events.ndjson"
  rss_kib="$(awk -F: '/Maximum resident set size/ {gsub(/ /, "", $2); print $2}' "$time_report")"
  rss_bytes=$((rss_kib * 1024))
else
  /usr/bin/time -l -o "$time_report" \
    target/release/officecli hdoc import "$source_docx" --output "$bundle" --document-id hcd-stress --events ndjson \
    > "$stress_root/events.ndjson"
  rss_bytes="$(awk '/maximum resident set size/ {print $1}' "$time_report")"
fi

if [ -z "${rss_bytes:-}" ]; then
  echo "could not read peak RSS from $time_report" >&2
  exit 1
fi
if [ "$rss_bytes" -gt $((rss_limit_mib * 1024 * 1024)) ]; then
  echo "peak RSS exceeded ${rss_limit_mib} MiB: $rss_bytes bytes" >&2
  exit 1
fi
if find "$bundle/chunks/sha256" -type f -size +2097152c -print -quit | grep -q .; then
  echo "a generated HCD chunk exceeds the 2 MiB hard limit" >&2
  exit 1
fi
if [ "$(grep -c '"event":"chunk_ready"' "$stress_root/events.ndjson")" -lt 2 ]; then
  echo "stress import did not produce multiple progressive chunks" >&2
  exit 1
fi

target/release/officecli hdoc validate "$bundle" --json > "$stress_root/validation.json"
cargo run --release -p hcd-docx --example generate_stress_patch -- "$bundle" "$patch_file"
target/release/officecli hdoc apply "$bundle" --patch "$patch_file" --expected-revision 0 --json \
  > "$stress_root/apply.json"

if /usr/bin/time --version 2>&1 | grep -q 'GNU time'; then
  /usr/bin/time -v -o "$export_time_report" \
    target/release/officecli hdoc export "$bundle" --source "$source_docx" \
      --output "$exported_docx" --revision 1 --json \
    > "$stress_root/export.json"
  export_rss_kib="$(awk -F: '/Maximum resident set size/ {gsub(/ /, "", $2); print $2}' "$export_time_report")"
  export_rss_bytes=$((export_rss_kib * 1024))
else
  /usr/bin/time -l -o "$export_time_report" \
    target/release/officecli hdoc export "$bundle" --source "$source_docx" \
      --output "$exported_docx" --revision 1 --json \
    > "$stress_root/export.json"
  export_rss_bytes="$(awk '/maximum resident set size/ {print $1}' "$export_time_report")"
fi

if [ -z "${export_rss_bytes:-}" ]; then
  echo "could not read export peak RSS from $export_time_report" >&2
  exit 1
fi
if [ "$export_rss_bytes" -gt $((rss_limit_mib * 1024 * 1024)) ]; then
  echo "export peak RSS exceeded ${rss_limit_mib} MiB: $export_rss_bytes bytes" >&2
  exit 1
fi
if [ ! -s "$exported_docx" ]; then
  echo "stress export did not publish a DOCX" >&2
  exit 1
fi

echo "HCD stress passed: source=$source_bytes bytes, import_peak_rss=$rss_bytes bytes, export_peak_rss=$export_rss_bytes bytes"
