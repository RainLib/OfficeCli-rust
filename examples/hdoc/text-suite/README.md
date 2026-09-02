# Markdown / TXT HCD 闭环示例

该示例完全使用 OfficeCLI 的 Rust 实现，不调用 LibreOffice 或其他外部文档转换程序。覆盖：

- UTF-8 `.md`、`.markdown` 有界标准语法解析，以及 `.txt` 流式导入 HCD；
- 稳定 `nodeId`、source-map、HCD 校验和独立 HTML 预览；
- 按 `nodeId` 应用 `text.splice`，revision 0/1 预览；
- Markdown/TXT 基于不可变源文件的 EXACT/HIGH 回写；
- HCD 无源语义导出 DOCX、XLSX、PPTX、PDF、Markdown 和 TXT；
- PDF 将安全链接绑定到原锚文本（不追加 `(URL)`），并保留代码块矢量面板和表格矢量网格，HTML 预览同步还原；
- PDF 使用字符级字体回退保留 emoji；示例中的 `😀` 可渲染、可提取且不会降级为 `?`；
- Office/PDF 结构校验，以及导出结果的 HTML/text 检查。
- `source-all-formats.md` 覆盖完整 CommonMark、GFM 和显式启用的安全扩展，并生成机器可读能力矩阵。

运行：

```bash
./examples/hdoc/text-suite/run.sh
```

也可指定新的输出目录：

```bash
./examples/hdoc/text-suite/run.sh /tmp/officecli-text-suite
```

重点产物：

| 路径 | 内容 |
|---|---|
| `output/hcd/*.hcd/` | 分片、内容寻址的 HCD 包 |
| `output/html/*-revision-0.html` | 修改前 HCD HTML |
| `output/html/*-revision-1.html` | nodeId 修改后 HCD HTML |
| `output/source-backed/*-revision-0.*` | 与输入逐字节一致的 EXACT 回写 |
| `output/source-backed/*-revision-1.*` | 仅改目标节点的 HIGH 回写 |
| `output/export/<source>/revision-1.*` | 六种目标格式产物 |
| `output/export/<source>/preview-*.html` | DOCX/XLSX/PPTX/PDF 导出效果预览 |
| `output/reports/` | validate、events、extract、patch 和 fidelity JSON |
| `output/reports/markdown-all-feature-matrix.json` | Markdown 语法族的支持/安全净化证据 |

保真说明：同格式、带 `--source` 的 revision 0 回写为 `EXACT`；revision 1 为 `HIGH`，只重写被修改节点的源字节范围。跨格式导出为 `SEMANTIC`，保留正文语义，不承诺不同排版模型之间的物理分页 1:1。

## Markdown 能力边界

全格式样例验证 CommonMark 全部结构族，以及 GFM 表格、任务列表、删除线、自动链接、提示块和脚注。额外启用 Setext/标题属性、引用式链接、定义列表、数学、上下标、Wiki 链接、YAML/TOML 元数据、Mermaid 围栏和冒号 admonition。嵌套引用、列表和脚注保持真实 HTML 层级，每个可编辑文本节点保留稳定 nodeId 与源字节范围。

安全边界仍然生效：危险 URL 不进入 `href`；安全行内 HTML 只允许 `mark/kbd/u/s/sub/sup/small/br`；脚本和其他活动 HTML 作为转义源码显示，绝不执行。图片语法生成稳定、可定位的图片 nodeId 和 URL/alt 信息，但导入阶段不主动下载远程资产。
