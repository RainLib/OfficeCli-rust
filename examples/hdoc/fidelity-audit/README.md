# HCD DOCX / PPTX / XLSX 保真审核

这个审核器合并三套文件级测试的 `summary.json`，并采用保守门禁区分三类结论：

- 流水线是否通过：结构、HCD、nodeId、patch、导出和格式校验。
- 依赖不可变源文件的回写是否达到 `EXACT` / `HIGH`。
- 可编辑 HCD HTML 是否真正通过像素级 1:1 认证。

它不会把测试套件中的 `passed` 自动解释为视觉 1:1，也不会调用 LibreOffice。

## 运行

先分别运行三套测试，再传入它们的汇总文件：

```bash
./examples/hdoc/fidelity-audit/run.sh \
  examples/hdoc/docx-suite/output/summary.json \
  examples/hdoc/pptx-suite/output/summary.json \
  examples/hdoc/xlsx-suite/output/summary.json \
  /tmp/officecli-hcd-fidelity-audit

open /tmp/officecli-hcd-fidelity-audit/index.html
jq . /tmp/officecli-hcd-fidelity-audit/summary.json
```

当前门禁只有在存在独立视觉基准、自动比较通过且没有格式级阻断项时，才允许把 editable HCD HTML 标记为 1:1。revision 0 的 `EXACT` 是源文件支持的闭环保证，不等同于从 HTML 无源重建原始 Office 文件。

默认情况下，流水线、revision 0 或受支持 patch 的认证失败会返回非零退出码。需要在 CI 中同时强制可编辑 HTML 1:1 时，启用严格开关；当前实现会如实失败：

```bash
HCD_AUDIT_REQUIRE_1TO1=1 ./examples/hdoc/fidelity-audit/run.sh \
  path/to/docx-summary.json path/to/pptx-summary.json path/to/xlsx-summary.json \
  /tmp/officecli-hcd-fidelity-audit-strict
```
