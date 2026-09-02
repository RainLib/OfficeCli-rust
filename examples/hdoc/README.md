# HCD 手工验收样例

本目录用于直接查看 OfficeCLI 的分片 HCD、稳定 `nodeId`、文本 patch、source-backed HTML 回写，以及纯 Rust HTML/HCD → DOCX/XLSX/PPTX/PDF 的实际效果。

仓库内全部现存 DOCX 样例的批量 HCD 文件测试位于 [`docx-suite/`](docx-suite/README.md)。它会为每个文件生成源预览、HCD 预览、稳定 nodeId 对比、revision 0 无修改回写、nodeId patch 和 revision 1 DOCX 回写产物。

仓库内全部现存 PPTX 样例的批量 HCD 闭环测试位于 [`pptx-suite/`](pptx-suite/README.md)。它会生成 PPTX/HCD HTML、整套幻灯片截图、revision 0 精确回写、nodeId patch、revision 1 回写和重新导入对比报告。

Markdown/TXT 的低内存 HCD 闭环测试位于 [`text-suite/`](text-suite/README.md)。它覆盖稳定 `nodeId` patch、同格式 EXACT/HIGH 回写，以及无源导出 DOCX/XLSX/PPTX/PDF/Markdown/TXT。

## 已提交内容

- `source.html`：标题、段落、Unicode 中文、列表、表格、敏感文本和活动脚本输入。
- `source.txt`：TXT → HCD → 四格式语义导出输入。
- `generate.sh`：完整、可重复的生成及验证命令。
- `output/`：由当前 OfficeCLI release binary 生成并验证的参考产物。

## 重新生成

脚本要求 Rust/Cargo 和 `jq`，目标目录必须不存在：

```bash
./examples/hdoc/generate.sh /tmp/officecli-hdoc-output
```

若要重建仓库内参考输出，请先将现有 `examples/hdoc/output` 移到备份位置，再运行：

```bash
./examples/hdoc/generate.sh ./examples/hdoc/output
```

## 输出目录

| 路径 | 内容 |
|---|---|
| `output/direct/` | 普通 HTML 直接转换得到的 DOCX/XLSX/PPTX/PDF |
| `output/previews/` | 上述文件及 HCD revision 输出重新生成的完整 HTML 预览 |
| `output/hcd/from-*.hcd/` | DOCX/XLSX/PPTX/PDF → 分片 HCD |
| `output/hcd/html-revision.hcd/` | 已应用 revision 1 脱敏 patch 的 HTML-source HCD |
| `output/hcd/text.hcd/` | TXT-source HCD |
| `output/patch/` | patch 前后节点、patch JSON 和 ApplyResult |
| `output/source-backed/` | revision 1 回写原 HTML 的 HIGH fidelity 结果 |
| `output/semantic/` | HTML/TXT HCD 无源导出的四种纯 Rust `SEMANTIC` 产物及 fidelity 报告 |
| `output/reports/` | import NDJSON、extract、validate、export 和 nodeId 稳定性报告 |

## 重点查看

```bash
# patch 前后 nodeId 应相同，nodeHash 和正文应变化
jq . examples/hdoc/output/patch/node-before.json
jq . examples/hdoc/output/patch/node-after.json
jq . examples/hdoc/output/reports/node-operation-summary.json

# 相同源字节重复导入的 documentId/rootHash/nodeId 稳定性
jq . examples/hdoc/output/reports/stability-summary.json
wc -c examples/hdoc/output/reports/stability-node-ids.diff

# source-backed HTML 仅替换目标文本范围
open examples/hdoc/output/source-backed/patched-source.html

# 四种目标的 HTML 预览
open examples/hdoc/output/previews/from-html-docx.html
open examples/hdoc/output/previews/from-html-xlsx.html
open examples/hdoc/output/previews/from-html-pptx.html
open examples/hdoc/output/previews/from-html-pdf.html

# HCD revision 1 无源跨格式重建预览
open examples/hdoc/output/previews/hcd-revision-1-docx.html
open examples/hdoc/output/previews/hcd-revision-1-xlsx.html
open examples/hdoc/output/previews/hcd-revision-1-pptx.html
open examples/hdoc/output/previews/hcd-revision-1-pdf.html
```

## 保真边界

`output/source-backed/patched-source.html` 是基于不可变原 HTML 的范围回写；除 dirty 文本外，源字节保持不变。`output/direct` 和 `output/semantic` 使用进程内 Rust handler：DOCX/XLSX/PPTX 为 `SEMANTIC` 结构映射，PDF 为 canonical HTML/CSS 的 `HIGH` 分页排版。PDF 会保留常见 CSS、表格、代码块、链接和 Unicode 文本层，但不代表 Chromium/WebKit 专属 CSS 或原 Office 物理分页能像素级一致。
