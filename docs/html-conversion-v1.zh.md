# 独立 HTML 语义转换 v1

OfficeCLI 现在可以不依赖原始 Office/PDF 文件，直接将普通 `.html`/`.htm` 新建为 DOCX、XLSX、PPTX 或 PDF：

```bash
officecli convert input.html --output output.docx
officecli convert input.html --output output.xlsx
officecli convert input.html --output output.pptx
officecli convert input.html --output output.pdf
```

该入口固定走 OfficeCLI 进程内的 Rust parser 与 DOCX/XLSX/PPTX/PDF handler，不调用 LibreOffice、WPS、`pdf2docx` 或浏览器打印。即使传入 `--engine libreoffice` 等外部 engine，HTML 分派也不会切换到外部进程；`--engine` 只影响非 HTML 输入。

这条链路与 HCD 的 source-backed 增量回写不同：独立 HTML 转换的保真级别固定为 `semantic`；HCD 在提供不可变原文件时依赖 source-map 将修改增量回写为原格式。已编辑 HCD revision 也可在不提供源文件时通过同一组 Rust semantic handlers 新建 DOCX、XLSX、PPTX 或 PDF：

```bash
officecli hdoc export document.hcd --output revision.pdf --to pdf --revision 3 --json
```

该 HCD 无源路径同样固定报告 `SEMANTIC`，不会利用源格式 opaque part 或精确布局；materialized canonical HTML 最大 64 MiB。与普通 HTML 不同，HCD 中通过 `asset://sha256/...` 引用、且通过 index/大小/hash/文件签名校验的栅格图片可以原生嵌入目标文件。`data-hcd-width-emu`/`data-hcd-height-emu` 在 1–100,000,000 EMU 内会映射到四种目标；PPTX picture 容器的有界 `data-hcd-x-emu`/`data-hcd-y-emu` 也会用于新 slide，PDF 则等比缩放到可用页框。这仍不代表原格式的裁剪、环绕、相对锚点、文本碰撞或物理分页被精确复刻。

HTML-source HCD 会把安全的标题、段落、列表项和表格转换为 canonical 结构，同时让每个文本 span 保持原 HTML UTF-8 byte range source-map。大表每片最多 128 行，并通过稳定 table ID、连续 row range 和 final/总行数元数据供前端虚拟化及无源 Office 导出重组。源 CSS、任意属性、活动内容和不支持的嵌套结构不会进入可编辑 fragment；source-backed HTML 导出仍只替换被 patch 的文本范围，其余源字节原样复制。

因此当前不能笼统宣称“HTML ↔ 所有文档高保真”：DOCX 的 source-backed HCD HTML 是 `HIGH`，XLSX 是 `SEMANTIC`，PPTX 与 PDF 文本层是 `VISUAL`；PPTX 的 `VISUAL` 仅表示普通文本形状的直接几何、文本样式和关系绑定的嵌入图片已物化，不包含完整母版/主题/图形栈。而下面这条无源 HTML 新建链路对四种目标格式全部是 `SEMANTIC`。

## 语义映射

| HTML | DOCX | XLSX | PPTX | PDF |
|---|---|---|---|---|
| `h1`–`h6` | Heading 段落 | 顺序行 | 标题文本；`h1` 可开始新 slide | 大字号文本 |
| `p`、`div` | 普通段落 | A 列顺序行 | slide 正文 | 自动换行文本 |
| `ul`、`ol`、`li` | 列表文本 | A 列顺序行 | slide 正文 | 列表文本 |
| `table`、`tr`、`td`、`th` | 原生 Word 表格 | worksheet 行列 | 原生可编辑 DrawingML 表格 | 以行文本表达 |
| `section`、`main` | 语义分隔 | 空行 | 新 slide 边界 | 新页边界 |
| `a` | 文本附安全 URL | 同左 | 同左 | 同左 |
| `img` | HCD 安全资产写入 `word/media`；其他为 alt | HCD 安全资产写入 `xl/media`；其他为 alt | HCD 安全资产写入 `ppt/media`；其他为 alt | HCD PNG/JPEG 写为 Image XObject；其他为 alt |

当前版本不复刻 CSS 布局、浏览器像素分页、动画和活动内容。`script`、`style`、`iframe`、`object`、`embed` 等内容不会进入目标文档；`javascript:` 等危险链接不会保留目标地址。普通 HTML 的本地路径、网络 URL 和 data URL 图片均不会读取或下载，只生成 alt 文本。段落内图片会在语义输出中重排为独立 block；表格单元格内图片目前仍退化为 alt。

## 资源与发布边界

- HTML 文件最大 64 MiB，单个语义文本块最大 2 MiB，最多 1,000,000 个 block。
- HCD 无源导出图片单个最大 64 MiB、总计最大 256 MiB；发布前再次核对索引大小与 SHA-256。当前签名白名单为 PNG、JPEG、GIF、BMP、TIFF、WebP、ICO；PDF 进一步只接受 PNG/JPEG，并限制单图 4,000 万像素、PNG 解码结果 128 MiB。SVG/EMF/WMF 和未知编码不嵌入，改为 alt warning。图片几何只接受有界整数 EMU；缺失或越界时使用 4×3 英寸语义默认值，PDF 超出页框时等比缩小。
- XLSX 遵守 1,048,576 行、16,384 列和单元格 32,767 字符限制；越界直接失败，不截断。
- PPTX 表格以 frame 实际宽高计算 grid/row 尺寸；单张 slide 最多 18 行×12 列，超出时按行列窗口拆分并重复第一行。窗口只保存原 table block 的索引范围，不复制整张大表。跨多个 HCD fragment 的同一逻辑表会按 `data-hcd-table-node-id`、fragment 序号和连续 row range 有界重组；缺片、重复、乱序、列数漂移、错误 final/row count 或实际 `<tr>` 数不一致均在输出发布前失败。
- 输出先写同目录临时文件，重新打开并验证后再替换目标；转换失败不会发布半成品。
- PDF 预先收集文档字符并一次性嵌入统一 Unicode 字形子集，后续页面复用同一字体资源；同时读取压缩的 ToUnicode CMap，保证中英文和脱敏文本可连续写入及重新提取。
- JSON 输出包含 `engine=rust-html-semantic`、`fidelity=semantic`、block 数量、`imageCount`/`embeddedImageCount` 和 fidelity warnings。

## 验证

```bash
cargo test -p officecli commands::html_convert::tests --offline
cargo test -p officecli --test html_convert --offline
cargo test -p officecli --test hdoc_multi_format edited_hcd_revision_semantically_exports_to_all_rust_targets_without_source --offline
cargo test -p officecli --test hdoc_multi_format hcd_content_addressed_image_exports_to_all_rust_targets_without_source --offline
cargo test -p pdf-handler image_tests --offline
cargo clippy --all-targets --offline -- -D warnings
```
