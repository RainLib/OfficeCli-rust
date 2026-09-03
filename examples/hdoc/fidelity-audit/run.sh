#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  cat >&2 <<'USAGE'
Usage: run.sh <docx-summary.json> <pptx-summary.json> <xlsx-summary.json> <output-dir>

Combines the three format suites into one conservative fidelity audit. A suite's
"passed" count is treated as pipeline coverage, never as editable-HTML 1:1 proof.
USAGE
  exit 2
fi

DOCX_SUMMARY=$1
PPTX_SUMMARY=$2
XLSX_SUMMARY=$3
OUTPUT_DIR=$4

for input in "$DOCX_SUMMARY" "$PPTX_SUMMARY" "$XLSX_SUMMARY"; do
  if [[ ! -f "$input" ]]; then
    printf 'Missing suite summary: %s\n' "$input" >&2
    exit 2
  fi
  jq -e '.cases | type == "array"' "$input" >/dev/null
done

if [[ -e "$OUTPUT_DIR" ]]; then
  printf 'Output directory already exists: %s\n' "$OUTPUT_DIR" >&2
  exit 2
fi

mkdir -p "$OUTPUT_DIR"

summary_path() {
  local value=$1
  if [[ "$value" = /* ]]; then
    printf '%s\n' "$value"
  else
    printf '%s/%s\n' "$PWD" "$value"
  fi
}

DOCX_ABS=$(summary_path "$DOCX_SUMMARY")
PPTX_ABS=$(summary_path "$PPTX_SUMMARY")
XLSX_ABS=$(summary_path "$XLSX_SUMMARY")

jq -n \
  --slurpfile docx "$DOCX_SUMMARY" \
  --slurpfile pptx "$PPTX_SUMMARY" \
  --slurpfile xlsx "$XLSX_SUMMARY" \
  --arg docxSummary "$DOCX_ABS" \
  --arg pptxSummary "$PPTX_ABS" \
  --arg xlsxSummary "$XLSX_ABS" '
  def count_value($cases; $field; $value):
    [$cases[] | select(.[$field] == $value)] | length;
  def patchable($cases):
    [$cases[] | select(.patchedExportFidelity != null)] | length;
  def format_result($name; $summary; $disqualifiers; $conclusion; $summary_path):
    ($summary.cases // []) as $cases |
    {
      format: $name,
      suiteSchemaVersion: $summary.schemaVersion,
      suiteSummary: $summary_path,
      suiteIndex: ($summary_path | sub("summary\\.json$"; "index.html")),
      pipeline: {
        total: ($summary.total // ($cases | length)),
        passed: ($summary.passed // count_value($cases; "status"; "passed")),
        failed: ($summary.failed // count_value($cases; "status"; "failed")),
        certified: (($summary.failed // 0) == 0 and ($summary.passed // 0) == ($summary.total // -1))
      },
      sourceBackedNoOp: {
        exact: count_value($cases; "noopExportFidelity"; "EXACT"),
        expected: ($cases | length),
        certified: (count_value($cases; "noopExportFidelity"; "EXACT") == ($cases | length))
      },
      sourceBackedPatched: {
        high: count_value($cases; "patchedExportFidelity"; "HIGH"),
        expected: patchable($cases),
        certified: (count_value($cases; "patchedExportFidelity"; "HIGH") == patchable($cases))
      },
      editableHtml: {
        declaredLevels: ($cases | map(.importFidelity // "UNDECLARED") | unique),
        profiles: ($cases | map(.profile // "UNDECLARED") | unique),
        oneToOneCertified: false,
        disqualifiers: $disqualifiers,
        conclusion: $conclusion
      }
    };
  [
    format_result(
      "DOCX";
      $docx[0];
      [
        "semantic-flow profile does not reproduce Word physical pagination",
        "opaque Office objects and unsupported structure/style edits remain source-authoritative",
        "the suite has no independent Word-native pixel comparison gate"
      ];
      "High semantic/text fidelity for the supported patch surface; editable HTML is not certified as Word pixel-level 1:1.";
      $docxSummary
    ),
    format_result(
      "PPTX";
      $pptx[0];
      [
        "master/layout inheritance and grouped transforms are incomplete",
        "native charts, SmartArt, media, animations and several effects remain source-authoritative",
        "source-vs-HCD screenshots are review evidence but are not required to be pixel-identical"
      ];
      "Slide-canvas visual preview is available; advanced slide content prevents a universal 1:1 certification.";
      $pptxSummary
    ),
    format_result(
      "XLSX";
      $xlsx[0];
      [
        "the manifest declares SEMANTIC rather than HIGH or VISUAL fidelity",
        "native chart theme, axes, effects, conditional formatting, shapes and pivot visuals are incomplete",
        "external, named, 3D or oversized chart references and native chart styling remain source-authoritative"
      ];
      "Grid semantics, styles, sheet metadata and drawing anchors are checked; editable HTML is not certified as Excel pixel-level 1:1.";
      $xlsxSummary
    )
  ] as $formats |
  {
    schemaVersion: "officecli-hcd-fidelity-audit/1",
    generatedAt: (now | todateiso8601),
    policy: {
      pipelinePassIsVisualCertification: false,
      sourceBackedNoOpRequirement: "Every case must export revision 0 as EXACT.",
      sourceBackedPatchedRequirement: "Every patchable case must export the supported text edit as HIGH.",
      editableHtmlOneToOneRequirement: "Requires an independent visual oracle and zero format-specific disqualifiers."
    },
    overall: {
      pipelineCertified: ([$formats[].pipeline.certified] | all),
      sourceBackedNoOpCertified: ([$formats[].sourceBackedNoOp.certified] | all),
      sourceBackedPatchedCertified: ([$formats[].sourceBackedPatched.certified] | all),
      editableHtmlOneToOneCertified: ([$formats[].editableHtml.oneToOneCertified] | all),
      verdict: "PARTIAL_HIGH_FIDELITY"
    },
    formats: $formats
  }
' > "$OUTPUT_DIR/summary.json"

jq -r '
  def yesno: if . then "yes" else "no" end;
  def esc: tostring | @html;
  "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>OfficeCLI HCD fidelity audit</title><style>body{font:15px/1.55 system-ui,sans-serif;max-width:1180px;margin:32px auto;padding:0 20px;color:#18212b}h1{margin-bottom:4px}.good{color:#087b39}.bad{color:#b42318}table{border-collapse:collapse;width:100%;margin:20px 0}th,td{border:1px solid #ccd3da;padding:9px;vertical-align:top;text-align:left}th{background:#f3f5f7}code{background:#f3f5f7;padding:2px 4px}ul{margin:4px 0;padding-left:20px}.note{background:#fff7e6;border-left:4px solid #f79009;padding:12px 16px}</style></head><body>",
  "<h1>OfficeCLI HCD 三格式保真审核</h1>",
  "<p>结论：<strong>" + (.overall.verdict|esc) + "</strong></p>",
  "<p class=\"note\">源文件支持 revision 0 精确闭环；可编辑 HCD HTML 尚未通过 DOCX/PPTX/XLSX 全格式像素级 1:1 认证。表中的 pipeline passed 只代表结构、nodeId、patch、导出和校验门禁通过。</p>",
  "<table><thead><tr><th>格式</th><th>流水线</th><th>revision 0</th><th>修改后回写</th><th>HCD HTML 1:1</th><th>声明等级 / profile</th><th>阻断项</th><th>证据</th></tr></thead><tbody>",
  (.formats[] |
    "<tr><td><strong>" + (.format|esc) + "</strong></td>" +
    "<td>" + (.pipeline.passed|tostring) + "/" + (.pipeline.total|tostring) + "</td>" +
    "<td class=\"" + (if .sourceBackedNoOp.certified then "good" else "bad" end) + "\">" + (.sourceBackedNoOp.certified|yesno) + " (" + (.sourceBackedNoOp.exact|tostring) + "/" + (.sourceBackedNoOp.expected|tostring) + ")</td>" +
    "<td class=\"" + (if .sourceBackedPatched.certified then "good" else "bad" end) + "\">" + (.sourceBackedPatched.certified|yesno) + " (" + (.sourceBackedPatched.high|tostring) + "/" + (.sourceBackedPatched.expected|tostring) + ")</td>" +
    "<td class=\"bad\">no</td>" +
    "<td><code>" + (.editableHtml.declaredLevels|join(", ")|esc) + "</code><br><code>" + (.editableHtml.profiles|join(", ")|esc) + "</code></td>" +
    "<td><ul>" + ([.editableHtml.disqualifiers[] | "<li>" + (.|esc) + "</li>"] | join("")) + "</ul></td>" +
    "<td><a href=\"" + (.suiteIndex|esc) + "\">对比索引</a> · <a href=\"" + (.suiteSummary|esc) + "\">summary.json</a></td></tr>"
  ),
  "</tbody></table><h2>判定原则</h2><ul><li><code>EXACT</code>：依赖不可变源文件的 revision 0 包内容精确保留。</li><li><code>HIGH</code>：受支持的 nodeId 文本修改可高保真写回源格式，但不代表所有结构或样式均可编辑。</li><li>HTML 1:1：必须另有独立视觉基准，且格式阻断项为零；本次三种格式均未满足。</li></ul></body></html>"
' "$OUTPUT_DIR/summary.json" > "$OUTPUT_DIR/index.html"

printf 'Audit JSON: %s/summary.json\n' "$OUTPUT_DIR"
printf 'Audit HTML: %s/index.html\n' "$OUTPUT_DIR"

if ! jq -e '.overall.pipelineCertified and .overall.sourceBackedNoOpCertified and .overall.sourceBackedPatchedCertified' "$OUTPUT_DIR/summary.json" >/dev/null; then
  printf 'Required pipeline/source-backed fidelity gate failed.\n' >&2
  exit 1
fi
if [[ "${HCD_AUDIT_REQUIRE_1TO1:-0}" == 1 ]] \
  && ! jq -e '.overall.editableHtmlOneToOneCertified' "$OUTPUT_DIR/summary.json" >/dev/null; then
  printf 'Editable HTML 1:1 fidelity gate failed. See %s/index.html\n' "$OUTPUT_DIR" >&2
  exit 1
fi
