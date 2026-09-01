# 真实文档 HCD 转换样例

本目录使用 DOCX 与 PDF 真实文件验证以下纯 Rust 主链路：

1. 源文件转可独立打开的 HTML 预览。
2. 源文件导入分片 HCD（HTML 为唯一可编辑正文，JSON 保存索引、映射和 revision）。
3. 从 HCD 的 HTML 语义层分别导出 DOCX、XLSX、PPTX、PDF。
4. 对 HCD 和导出文件执行结构校验，并为每个导出文件生成 HTML 预览。

真实文件及其生成产物只保留在本机，不提交到 Git。脚本会生成校验报告和
`artifact-index.txt`，可用于检查每次运行的结果。

每个样例使用独立目录：

- `02-english-textbook-pdf/`
- `03-civil-complaint-docx/`
- `04-criminal-case-pdf/`

每个目录内部固定分为：

- `source/`：必要的源格式规范化产物。
- `html/source-preview.html`：沿用现有 `view html`/`watch` 渲染器的源文件高保真预览。
- `html/hcd-preview.html`：从 HCD 当前 revision 的 HTML 分片流式组装出的可直接打开中间产物，保留 `data-hcd-id`。
- `hcd/bundle/`：分片 HCD 包。
- `exports/`：从 HCD HTML 语义层导出的四种文件。
- `previews/`：DOCX/XLSX/PPTX 再次读取后的 HTML 预览；若系统提供 Poppler，PDF 会渲染前 3 页 PNG，完整内容查看 `exports/from-hcd.pdf`。
- `reports/`：导入事件、HCD 校验、文本抽取、导出保真报告、文件校验和 SHA-256。

运行：

```bash
cargo build --release
REAL_HDOC_TEXTBOOK_PDF="/path/to/textbook.pdf" \
REAL_HDOC_COMPLAINT_DOCX="/path/to/complaint.docx" \
REAL_HDOC_CRIMINAL_PDF="/path/to/criminal-case.pdf" \
  bash examples/hdoc/real-documents/run.sh
```

为了避免误覆盖，脚本发现目标 HCD bundle 已存在时会停止。需要重跑时，请指定一个新的输出根目录：

```bash
REAL_HDOC_OUTPUT_ROOT="$PWD/examples/hdoc/real-documents/output-rerun" \
  bash examples/hdoc/real-documents/run.sh
```

HTML/HCD 到 DOCX、XLSX、PPTX、PDF 的四条导出路径均为进程内 Rust 实现，不调用 LibreOffice。同格式输出会携带不可变源文件并走 source-backed 高保真回写；跨格式输出仍是语义转换。

PDF 的 HCD 预览使用只读、内容寻址的源图片层叠加可编辑文本层；DOCX 预览保留空段落、页面尺寸和页边距。HCD 会恢复安全链接并阻止 `javascript:` 等危险目标。
