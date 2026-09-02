# 独立 HTML 转换 v1

OfficeCLI 现在可以不依赖原始 Office/PDF 文件，直接将普通 `.html`/`.htm` 新建为 DOCX、XLSX、PPTX 或 PDF：

```bash
officecli convert input.html --output output.docx
officecli convert input.html --output output.xlsx
officecli convert input.html --output output.pptx
officecli convert input.html --output output.pdf
```

该入口固定走 OfficeCLI 进程内的 Rust parser 与 DOCX/XLSX/PPTX/PDF handler，不调用 LibreOffice、WPS、`pdf2docx` 或浏览器打印。即使传入 `--engine libreoffice` 等外部 engine，HTML 分派也不会切换到外部进程；`--engine` 只影响非 HTML 输入。

这条链路与 HCD 的 source-backed 增量回写不同：DOCX/XLSX/PPTX 使用 `semantic` 结构映射；PDF 直接对 canonical HTML/CSS 做分页排版并报告 `high`。HCD 在提供不可变原文件时仍依赖 source-map 将修改增量回写为原格式。已编辑 HCD revision 也可在不提供源文件时新建 DOCX、XLSX、PPTX 或 PDF：

```bash
officecli hdoc export document.hcd --output revision.pdf --to pdf --revision 3 --json
```

该 HCD 无源路径按目标报告 `SEMANTIC` 或 PDF `HIGH`，不会利用源格式 opaque part 或精确物理布局；materialized canonical HTML 最大 64 MiB。与普通 HTML 不同，HCD 中通过 `asset://sha256/...` 引用、且通过 index/大小/hash/文件签名校验的栅格图片可以原生嵌入目标文件。`data-hcd-width-emu`/`data-hcd-height-emu` 在 1–100,000,000 EMU 内会映射到四种目标；PPTX picture 容器的有界 `data-hcd-x-emu`/`data-hcd-y-emu` 也会用于新 slide，PDF 则按 CSS/有界尺寸分页。这仍不代表原格式的裁剪、环绕、相对锚点、文本碰撞或物理分页被精确复刻。

HTML-source HCD 会把安全的标题、段落、列表项和表格转换为 canonical 结构，同时让每个文本 span 保持原 HTML UTF-8 byte range source-map。大表每片最多 128 行，并通过稳定 table ID、连续 row range 和 final/总行数元数据供前端虚拟化及无源 Office 导出重组。源 CSS、任意属性、活动内容和不支持的嵌套结构不会进入可编辑 fragment；source-backed HTML 导出仍只替换被 patch 的文本范围，其余源字节原样复制。

因此当前不能笼统宣称“HTML ↔ 所有文档高保真”：DOCX 的 source-backed HCD HTML 是 `HIGH`，XLSX 是 `SEMANTIC`，PPTX 与 PDF 导入文本层是 `VISUAL`。无源 HTML 新建 DOCX/XLSX/PPTX 仍是结构语义映射；无源 PDF 对 canonical HTML/CSS 的常用排版可达到 `HIGH`，但不是 Chromium/WebKit 的任意 CSS/JavaScript 像素级复刻，也不是原 Office/PDF 的物理分页回放。

## 语义映射

| HTML | DOCX | XLSX | PPTX | PDF |
|---|---|---|---|---|
| `h1`–`h6` | Heading 段落 | 顺序行 | 标题文本；`h1` 可开始新 slide | CSS 标题与分页规则 |
| `p`、`div` | 普通段落 | A 列顺序行 | slide 正文 | CSS flow 自动换行 |
| `ul`、`ol`、`li` | 列表文本 | A 列顺序行 | slide 正文 | CSS 列表与嵌套结构 |
| `table`、`tr`、`td`、`th` | 原生 Word 表格 | worksheet 行列 | 原生可编辑 DrawingML 表格 | CSS 表格、边框、表头和单元格对齐 |
| `section`、`main` | 语义分隔 | 空行 | 新 slide 边界 | CSS 分页布局 |
| `a` | 文本附安全 URL | 同左 | 同左 | 可见锚文本与原生 URI annotation |
| `img` | HCD 安全资产写入 `word/media`；其他为 alt | HCD 安全资产写入 `xl/media`；其他为 alt | HCD 安全资产写入 `ppt/media`；其他为 alt | HCD PNG/JPEG 写为 Image XObject；其他为 alt |

DOCX/XLSX/PPTX 当前不复刻 CSS 布局；PDF 支持纯 Rust 排版器已实现的 CSS cascade、盒模型、字体、表格、链接和分页，但不执行 JavaScript，也不承诺浏览器专属 CSS 的像素一致。`script`、`iframe`、`object`、`embed` 等活动内容不会执行；`javascript:` 等危险链接不会保留目标地址。普通 HTML 的任意本地路径、网络 URL 和 data URL 图片均不会读取或下载；只有完成内容寻址校验的 HCD 资产获准嵌入。

## 资源与发布边界

- HTML 文件最大 64 MiB，单个语义文本块最大 2 MiB，最多 1,000,000 个 block。
- HCD 无源导出图片单个最大 64 MiB、总计最大 256 MiB；发布前再次核对索引大小与 SHA-256。当前签名白名单为 PNG、JPEG、GIF、BMP、TIFF、WebP、ICO；PDF 进一步只接受 PNG/JPEG，并限制单图 4,000 万像素、PNG 解码结果 128 MiB。SVG/EMF/WMF 和未知编码不嵌入，改为 alt warning。图片几何只接受有界整数 EMU；缺失或越界时使用 4×3 英寸语义默认值，PDF 超出页框时等比缩小。
- XLSX 遵守 1,048,576 行、16,384 列和单元格 32,767 字符限制；越界直接失败，不截断。
- PPTX 表格以 frame 实际宽高计算 grid/row 尺寸；单张 slide 最多 18 行×12 列，超出时按行列窗口拆分并重复第一行。窗口只保存原 table block 的索引范围，不复制整张大表。跨多个 HCD fragment 的同一逻辑表会按 `data-hcd-table-node-id`、fragment 序号和连续 row range 有界重组；缺片、重复、乱序、列数漂移、错误 final/row count 或实际 `<tr>` 数不一致均在输出发布前失败。
- 输出先写同目录临时文件，重新打开并验证后再替换目标；转换失败不会发布半成品。
- PDF 使用纯 Rust HTML/CSS 排版器流式写入输出，嵌入所需字体子集并生成可提取 Unicode 文本层；PDF reader 会按视觉几何合并同一行中因字体、脚本或链接样式切开的 text run。Markdown HCD 的 Mermaid flowchart/graph 与 sequenceDiagram 会从当前 revision 的可编辑源码派生内联 SVG，矢量写入 PDF，不执行 Mermaid JavaScript。
- DOCX/XLSX/PPTX JSON 输出包含 `engine=rust-html-semantic`、`fidelity=semantic`；PDF 输出包含 `engine=rust-html-css-pdf`、`fidelity=high`，由进程内纯 Rust HTML/CSS 排版器直接分页并保留常见样式、表格和链接。两条链路都输出 block 数量、`imageCount`/`embeddedImageCount` 和 fidelity warnings。

## 验证

```bash
cargo test -p officecli commands::html_convert::tests --offline
cargo test -p officecli --test html_convert --offline
cargo test -p officecli --test hdoc_multi_format edited_hcd_revision_semantically_exports_to_all_rust_targets_without_source --offline
cargo test -p officecli --test hdoc_multi_format hcd_content_addressed_image_exports_to_all_rust_targets_without_source --offline
cargo test -p pdf-handler image_tests --offline
cargo clippy --all-targets --offline -- -D warnings
```
