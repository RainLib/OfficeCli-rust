# Office/PDF/HTML/Markdown/TXT ↔ 分片 HCD v1

本实现为 OfficeCLI 增加一条独立于既有 `DocumentHandler`/`view_as_html()` 的 HCD 链路，覆盖 DOCX、XLSX、PPTX、PDF、HTML、UTF-8 Markdown 和 UTF-8 TXT。HTML fragment 是唯一可编辑正文；JSON 仅包含清单、索引、节点 hash、源文件 anchor、revision 和保真信息，不保存正文副本。

各格式采用自然分片：

| 格式 | profile | HTML fidelity | 分片边界 | 当前可编辑节点 |
|---|---|---|---|---|
| DOCX | `semantic-flow` | `HIGH` | 段落、表格 row group、文档 region | `w:t` 文本 |
| XLSX | `grid` | `SEMANTIC` | sheet 的最多 128 行窗口 | 非公式单元格值 |
| PPTX | `slide-canvas` | `VISUAL` | slide/notes 内的 shape、picture、table row-group fragment | DrawingML 形状与表格单元格文本 |
| PDF | `fixed-layout` | `VISUAL` | 完整页面合成视觉层 + page 内透明 nodeId 文本层 | 可提取 PDF 文本层 |
| HTML/HTM | `semantic-flow` | `SEMANTIC` | 标题、段落、列表项；table 每片最多 128 行 | canonical 安全结构中的 UTF-8 文本 |
| TXT | `semantic-flow` | `SEMANTIC` | 最多 256 行 | UTF-8 行文本，包括空行 |
| Markdown | `semantic-flow` | `SEMANTIC` | 最多 256 个行级 block | 标题、引用、列表、代码与常用 inline 语义；未支持扩展保留为可编辑文本 |

这里的级别描述“导入后的 HCD HTML 能表达多少”，不是源文件导出级别。`manifest.fidelity` 明确列出 `preserved`、`flattened`、`dropped` 和 warnings。源文件未修改时，source-backed 导出仍可达到 `EXACT`；文本修改后通常为 `HIGH`，因为未修改 ZIP entry 原始复制、dirty XML part 只替换文本节点。

冻结的协议文件位于 `crates/hcd-core/schemas/hcd-1.schema.json` 与 `crates/hcd-core/schemas/hcd-patch-1.schema.json`，并以 `HCD_SCHEMA_JSON`/`HCD_PATCH_SCHEMA_JSON` 内嵌在 `hcd-core` 中。

## 边界与容量

- 单个源文件最大 256 MiB；HTML 入口进一步限制为 64 MiB，当前 lopdf/Hayro 对象图后端的 PDF 入口限制为 128 MiB。OOXML ZIP 解压总量最大 2 GiB，ZIP entry 最大 100,000 个。
- 普通分片软上限为 512 KiB 或 256 个顶层 block，硬上限为 2 MiB。
- 段落不跨分片；DOCX/XLSX/PPTX/HTML 表格均以完整 row group 拆分，每个普通 fragment 最多 128 行。PPTX 跨行合并组不会被切断；单个不可拆的行、文本节点或合并组超过 2 MiB 时在发布前返回 `NODE_TOO_LARGE`。HTML 单表最多 1,048,576 行、16,384 列和 1,000,000 个单元格，并在生成 chunk 前做一次有界结构预扫描。
- DOCX 单表 `tblGrid` 最多 16,384 列；列宽只以有界 twips 数组暂存，并在每个表格 fragment 中生成 `<colgroup>`，不会构造整表 DOM。
- `tblStyleRowBandSize`/`tblStyleColBandSize` 接受 1–1,024；支持 table style 的 `basedOn` 继承和文档内直接覆盖，超限在生成 HTML 前失败。
- 单个不可安全拆分的文本节点或 block 超过 2 MiB 时返回 `NODE_TOO_LARGE`。
- XML 控制部件以及 HCD manifest/index/map/revision/annotation/assets index/styles 单对象最多读取 16 MiB；读取器先检查并限制实际读入字节，不会在报错前完整缓冲恶意大对象。无源语义导出的图片单个最多 64 MiB、总计最多 256 MiB，并在转换前重新校验 byteLength、SHA-256 与安全栅格签名；PDF 图片额外限制为 PNG/JPEG、4,000 万像素和 128 MiB PNG 解码结果。
- patch JSON 最大 8 MiB；单 batch 最多 10,000 个 operation、正文插入总量最大 2 MiB。document/patch/annotation/node ID、SHA-256、actor/metadata 数量与总字节均在反序列化后再次执行运行时校验。
- revision 最大为 100,000；validator 会逐项校验 revision 文件连续性、parent、patchId 唯一性、patch/root hash、dirty set 和 head 对齐，避免 revision-chain/CPU bomb。
- XML 最大深度 256、单 part 最大 3,000,000 个元素；单个 HCD fragment 最大深度 256、最多 100,000 个元素，并采用元素/属性白名单、DOCTYPE 禁止和 asset 引用完整性校验。HTML `colspan/col span` 限制为 1–16,384、`rowspan` 限制为 1–1,048,576，拒绝指数、负数和越界值，避免浏览器表格布局放大攻击。HCD href 除拒绝 `..`/绝对路径外，也拒绝经符号链接逃逸 bundle 根目录。
- PDF 最多 100,000 页；进入 lopdf 前会用一个最大 128 MiB、随后立即释放的临时源缓冲扫描结构流。stream dictionary 最大 256 KiB，`/ObjStm`/`/XRef` 单个编码与解码结果最大 16 MiB、累计解码最大 64 MiB、结构流最多 100,000 个、xref 最多 1,000,000 项；异常 `/W`/`/Index` 预算在 lopdf 分配内存前失败。
- HCD 每页解码后的 PDF content stream 最大 16 MiB，ToUnicode 等单个辅助 stream 最大 8 MiB。Flate、ASCII85 和 LZW 在有界解码器内处理；不支持的 predictor/filter 直接失败，不回退到无界解压。页面视觉层固定按 CropBox 的 96 DPI CSS 尺度逐页合成；单页边长不超过 8,192 像素、总像素不超过 32 Mi、PNG 不超过 64 MiB。第 1 页 eager、后续页 lazy，`loading`/`decoding` 只允许出现在 `img` 且值为固定枚举。
- `semantic-flow` 供前端做虚拟分页，不表示 Word/浏览器物理页；其他格式使用表中对应的自然 profile。

原文件不进入 HCD bundle。`manifest.json` 只记录源文件格式、SHA-256 和大小。要求保留源格式 opaque part、版式和未修改字节的 source-backed 导出必须再次提供完全相同的不可变源文件；若不提供源文件，可将选定 HCD revision 以纯 Rust `SEMANTIC` 模式重建为 DOCX、XLSX、PPTX 或 PDF，但不能声明 `EXACT/HIGH`。

## 包结构

```text
bundle/
├── manifest.json
├── styles.css
├── indexes/rev-00000000000000000000/000000.json
├── chunks/sha256/<hash>.html
├── maps/sha256/<hash>.json
├── annotations/sha256/<hash>.json
├── assets/sha256/<hash>.<ext>
├── assets/index.json
└── revisions/00000000000000000000.json
```

`chunks`、`maps`、`annotations` 和 `assets` 是不可变内容寻址对象。每个 index page 最多 128 个 `ChunkDescriptor`。正文 root 使用域分隔 Merkle 计算，同时覆盖 chunk/map descriptor、`styles.css` 和 `assets/index.json`；annotation 使用独立 root。revision 采用 append-only 写入，最后通过原子替换 `manifest.json` 推进本地 head；本地 patch 使用跨平台 OS 文件锁串行化，进程异常退出时锁由操作系统释放，不会留下永久锁文件状态。

每个正文节点在 HTML 中形如：

```html
<span data-hcd-id="n_..." data-hcd-node-hash="sha256...">唯一正文</span>
```

对应 map 只包含 `nodeId`、`nodeHash` 和 `source`，不存在正文 `text` 字段。OOXML/PDF source 定位到源 part 与文本序号；HTML/Markdown/TXT source 额外以 `textId=bytes:<start>:<end>` 记录不可变源文件中的安全文本字节范围。validator 会检查范围存在、按源顺序不重叠且不越界。节点 ID 优先使用格式原生稳定 ID，否则由 documentId、part、节点类型和源序号确定性生成。省略 `--document-id` 时，CLI 以不可变源文件 SHA-256 的前 128 位生成 `doc-...`，因此相同源字节重复导入会得到相同 documentId、nodeId 和 root；同一逻辑文档发生源内容变化后若仍需维持身份，Java 必须传入并永久保存自己的稳定 business document ID。

正文、表格、嵌套文本框、页眉、页脚、脚注、尾注和原 DOCX 批注依次导入。DOCX HTML 会表达直接段落/运行格式、文档内直接 table/row/cell 宽度、`tblGrid` 列宽、布局、对齐、底纹、边框、边距、行高、禁止跨页和垂直对齐，`styles.xml` 的 `basedOn` 继承、linked character style、常用 table style 与条件格式、文档 relationship 指向的主题字体/颜色及 `themeTint`/`themeShade`、超链接、横向合并单元格、真实 `rowspan` 纵向合并，以及 DrawingML 图片的内容寻址引用和布局信息。

直接 table/cell 格式以经过白名单校验的内联 CSS 覆盖样式表，列宽以 `<colgroup>` 表达，两者都随每个表格续片继承；table style 支持 whole/首尾行列、横纵奇偶带和四角条件，也支持从 table style 继承或由 `tblPr` 覆盖的非 1 行/列带宽。条件 CSS 按 Word 实际后覆盖前的次序输出：行带、列带、首尾列、首尾行、四角；四角同时要求相应行和列开关，文档没有 `tblLook` 时默认仅启用行列带纹。`cnfStyle` 的 12 位掩码和 Office 2010+ 独立属性会以固定大小状态解析到行、单元格和段落的 `data-hcd-*`，并驱动相应 table style 条件 CSS；`tblPrEx` 的行级边框、底纹和默认单元格边距会以内联 CSS 覆盖整表直接格式，而 cell 直接格式仍最后覆盖，`tblPrExChange` 历史快照不参与当前显示。解析器以整表逻辑行号和考虑 `gridSpan` 的逻辑列号给 `<tr>/<td>` 写入 band 元数据，因此分片和合并单元格不会让带纹重新起算；首行、尾行和角单元格样式也只在真正的首片/尾片启用。纵向合并 continuation 单元格中的源文本保留稳定 source-map 但标记为只读；跨行 merge group 不会被软分片边界切开，超过 2 MiB 时以 `NODE_TOO_LARGE` 失败。

`wp:inline/wp:anchor`、EMU 尺寸、水平/垂直 `relativeFrom`、`posOffset`/align、simplePos、四向距离、wrap 类型/侧向、relativeHeight、behindDoc、layoutInCell 与 allowOverlap 会进入安全 `data-hcd-*`；anchor 以相对偏移和受控 float 提供语义预览，Word 物理页碰撞、精确 z-order 和自定义 wrap polygon 仍为 best-effort。位置文本限制为 128 bytes，坐标和尺寸执行有界数值解析。

`w:ins`/`w:del`/`w:moveFrom`/`w:moveTo` 会物化为带 revision 元数据的安全 span，删除历史不进入可编辑正文，`pPrChange/rPrChange/tblPrChange/tblPrExChange/trPrChange/tcPrChange` 历史快照不会覆盖当前格式。主题字体会依据 `w:lang` 的东亚/双向语言槽选择 theme supplemental font，未识别脚本才回退通用主题字体。主题颜色变换优先采用 Word 已物化到 `w:val` 的有限精度结果；手写 OOXML 缺失该值时才以 HSL luminance 公式回退，并遵循 tint 与 shade 同时存在时 tint 优先。theme/styles 控制部件均限制为 16 MiB，并执行 XML 深度/元素预算、继承环路保护和字体/CSS 白名单校验。`numbering.xml` 以最多 16 MiB 的控制部件读取，常见十进制、字母、罗马数字和项目符号会生成只读的可见 list marker；start override 与多级 `%1`–`%9` 引用会维持流式计数，地区化编号和高级 level restart 仍为 best-effort。正文引用的媒体先做内容寻址以保证图片 chunk 可立即使用，未被正文 region 引用的媒体延后到正文 chunk 之后发布。未识别语言脚本、对角边框、`tblPrEx` 的宽度/对齐/间距/缩进/布局/`tblLook` 例外、旧版 Word table-style 兼容模式和 Word 物理分页仍是 best-effort。域、宏、OLE 和不支持的 DrawingML 不可编辑，保留在外部源 DOCX 中，并写入 fidelity warning。

HCD v1 的 `capabilities.stylePatch=false`、`structurePatch=false`：样式和结构可在 HTML 中渲染，但前端不得提交样式/结构修改。当前可写闭环只接受正文 splice 和独立 annotation；这样可以保证不会把只读渲染能力误报为可编辑能力。

XLSX sharedStrings 通过磁盘 offset/value store 解析，不构造全量字符串表；HTML 会表达 `cellXfs` 中可安全映射的直接字体、填充、边框、对齐，以及行高和列宽。常见内置/自定义 `numFmt` 的数字、日期、时间、百分比、货币、科学计数与分数，以及 1900/1904 日期系统已物化为显示文本；地区化、条件式和高级 token 继续以 warning 标记 best-effort。编辑后的单元格写成 inline string，公式节点只读。worksheet DrawingML 图片会在媒体内容寻址完成后生成独立 `sheet` 图片层：图片使用稳定 `data-hcd-id`，记录 worksheet/drawing source part、picture source path、单元格 anchor 和有界 EMU 几何，并可继续参与无源 HCD→DOCX/XLSX/PPTX/PDF 的图片导出。由于 Excel 实际列宽/行高可能受样式和 DPI 影响，图片层中的绝对像素位置仍明确视为近似值。

由于 `mergeCells` 通常位于 `sheetData` 之后，importer 会对每个 worksheet 做一次有界事件流预扫描，再进行正文流式输出；同一次预扫描还读取 `sheetView/pane`，只保留固定大小的视图状态并优先选择 `workbookViewId=0`。每个 worksheet chunk（包括空工作表）都会重复冻结/拆分窗格、RTL、网格线、行列标题、零值、公式显示标志、缩放和初始可见单元格元数据，便于前端独立虚拟化任意分片。按照 Office 实际语义，冻结状态的 `xSplit` 表示左侧可见列数，映射为冻结列；`ySplit` 表示顶部可见行数，映射为冻结行。`showFormulas` 只作为视图元数据保留，HCD 正文仍显示缓存单元格值，公式表达式保持只读。

合并区域最多接受 1,000,000 个固定坐标范围，不构造 worksheet DOM。已物化锚点使用 `rowspan`/`colspan` 和 `data-hcd-merge`，跨多行的 merge row group 不会被普通分片边界切开；若该不可拆分 row group 超过 2 MiB 则返回 `NODE_TOO_LARGE`。worksheet chunk 会先于只读媒体资产发布。条件格式、图表和 drawing 仍在 fidelity warning 中明确标记为未完全物化。

PPTX 按 `presentation.xml` relationship 顺序生成 slide，随后生成 notes；HTML 会按演示文稿尺寸建立画布，并表达普通文本形状的直接位置、大小、段落对齐和 run 字体/字号/颜色/强调。通过页面 relationship 引用的嵌入图片会复用内容寻址资产并表达直接位置和尺寸。

DOCX、PPTX、XLSX 与 PDF 的文字 hover 和图片 visual node hover 使用独立开关：`hdoc render-html` 默认同时开启，可分别用 `--text-hitboxes off` 与 `--image-hitboxes off` 关闭；HTML body 公开 `data-hcd-text-hitboxes`、`data-hcd-image-hitboxes`，前端也可独立切换。图片节点统一提供稳定 `data-hcd-id`、`data-hcd-node-kind=image`、`data-hcd-source-part/path`、`data-hcd-editable=false` 和格式可提供的 geometry/anchor 元数据。同一源文件、documentId 与 importer 版本重跑时 ID 和 root hash 保持一致。这些 visual node 当前是结构标定，不进入仅面向 canonical text 的 `hcd/1` source-map，也不能使用 `text.splice`；图片内容/几何 patch 需要后续独立 schema。

DrawingML 表格使用同一事件流解析器物化为 HTML table，不构造整页 DOM；会保留 graphic frame 位置/尺寸、`tblGrid` 列宽、行高、`gridSpan/rowSpan`、`hMerge/vMerge`、直接单元格填充、四边边框、内边距、垂直对齐和文本方向元数据。大表按安全 row group 渐进输出，每个 fragment 最多 128 行并重复 graphic-frame 几何和 `<colgroup>`，因此首个 `chunk_ready` 可早于 slide XML EOF。fragment 通过稳定的 `data-hcd-table-node-id`、`data-hcd-table-fragment`、`data-hcd-row-start/end`、`data-hcd-table-continuation` 和末片 `data-hcd-table-final` 拼接；source-map 仍保持原 `a:t` ordinal，不因分片改变节点 ID。

合并 continuation 中的异常源文本仍进入 source-map，但以隐藏只读节点保留，只有可见 merge anchor 可编辑。跨行合并组不会被 fragment 边界切断；单表最多 16,384 列、1,000,000 个单元格，单个 fragment 和不可拆行组的 HTML 硬限制均为 2 MiB，任一超限都在相关 chunk 发布前以 `NODE_TOO_LARGE`/resource limit 失败。表格文字 patch 仍按原 `a:t` ordinal 流式回写，因此不改变表格结构和未修改 ZIP entry。

母版与主题继承、table style 继承、组合形状变换、图片裁剪/效果、图表、SmartArt 和动画仍保留在源 OOXML 中，不计为 HTML 已完整呈现。PDF 当前仍由 lopdf 在内存保留压缩对象图，但已经移除打开时的全量 `doc.decompress()`；`/ObjStm` 和 `/XRef` 在 lopdf 之前先做有界结构预检，HCD 随后逐页有界解码文本 stream，并用固定 Hayro 版本完整解释当页绘制指令生成内容寻址 PNG，不再以 XObject 拼图作为正常路径。PNG 是只读视觉权威层，nodeId 文本作为透明交互层；编辑文字若需继续保持同等视觉保真，必须重新合成 dirty 页。不同 PDF 引擎对 ICC/JPEG2000/抗锯齿可能产生像素差，`VISUAL` 不表示跨引擎零像素差。manifest 使用 `PDF_OBJECT_GRAPH_IN_MEMORY` 明示剩余边界；这份 163 页教材的实测 maximum RSS 仍超过 512 MiB，因此严格 RSS 保证目前仍只适用于 OOXML 适配器。

原生 HTML 导入不会把 `script`、`style`、`iframe`、`object`、`embed`、`template`、`noscript` 内容放入可编辑 chunk，源标签和任意属性也不会原样进入前端；前端只读取经过 HCD 元素/属性/CSS 白名单校验的 canonical fragment。安全的 `h1`–`h6`、段落、`pre`、引用、列表项和 table/row/cell 语义会保留；源 CSS、任意 class/属性和不支持或嵌套的 markup 会降为 canonical 安全结构。每个可编辑 span 仍以 `bytes:<start>:<end>` 精确锚定原 UTF-8 文本，因此结构化表达没有改变 source-backed patch 契约。HTML 大表使用稳定 `data-hcd-table-node-id` 和连续 row-range 元数据分片，可被无源 DOCX/XLSX/PPTX 导出重新识别为原生表格。source-backed HTML 导出只在记录的文本字节范围内写入实体转义后的修改值，其余原始 HTML 字节保持不变，因此导出的源 HTML 仍应按不可信活动内容处理。Markdown/TXT 都按行有界读取，不构造全文字符串；导出保留 UTF-8 BOM、CRLF/LF 和所有未修改字节。Markdown patch 会将 dirty 节点安全转义为普通 Markdown 文本，未修改节点的原语法字节不变。

## CLI

```bash
officecli hdoc import input.docx \
  --output document.hcd \
  --document-id business-document-id \
  --events ndjson

officecli hdoc validate document.hcd --json

officecli hdoc extract-text document.hcd \
  --cursor '0:0' \
  --limit 1000 \
  --json

# 使用稳定 nodeId 随机读取当前 revision 的单个文本节点，不扫描/缓冲全文
officecli hdoc get-node document.hcd n_0123456789abcdef0123456789abcdef --json

# 分页列出 append-only revision，并读取一条 revision 记录
officecli hdoc list-revisions document.hcd --cursor 0 --limit 100 --json
officecli hdoc get-revision document.hcd 1 --json

# 校验 asset 的 size/hash；可选地原子复制到新路径
ASSET_HASH=$(jq -r '.[0].hash' document.hcd/assets/index.json)
officecli hdoc get-asset document.hcd "$ASSET_HASH" --json
officecli hdoc get-asset document.hcd "$ASSET_HASH" --output image.png --json

# 分页发现图片节点，并读取一个节点当前的 asset/geometry/visualHash
officecli hdoc list-images document.hcd --limit 100 --json
officecli hdoc get-image document.hcd n_0123456789abcdef0123456789abcdef --json

# 暂存替换图片；暂存本身不推进 revision，也不改变正文 root hash
officecli hdoc put-asset document.hcd replacement.png --json

# 只物化从第 128 个 chunk 开始的 16 个 chunk；省略窗口参数仍输出全文
officecli hdoc render-html document.hcd \
  --output partial.html \
  --revision 1 \
  --chunk-start 128 \
  --chunk-limit 16 \
  --json

officecli hdoc apply document.hcd \
  --patch patch.json \
  --expected-revision 0 \
  --json

officecli hdoc export document.hcd \
  --source input.docx \
  --output output.docx \
  --revision 1 \
  --fidelity-report fidelity.json \
  --json

# 无需原源文件：将已编辑 HCD revision 纯 Rust 语义重建为另一目标格式
officecli hdoc export document.hcd \
  --output output.pdf \
  --to pdf \
  --revision 1 \
  --fidelity-report semantic-fidelity.json \
  --json
```

同一组命令也接受 `.xlsx`、`.pptx`、`.pdf`、`.html`/`.htm`、UTF-8 `.md`/`.markdown` 与 UTF-8 `.txt`。提供 `--source` 且输出扩展名与 bundle source format 相同时走 source-backed 回写；省略 `--source` 或选择不同目标时，`.docx/.xlsx/.pptx/.pdf/.md/.txt` 走进程内 Rust semantic handler。`--to` 可省略并由 `--output` 推断，但显式值与扩展名不一致会在创建输出前失败。HTML/Markdown/TXT 的逐字节回写仍要求原源文件。

Markdown/HTML 导出 PDF 时，canonical HTML/CSS 由进程内纯 Rust 排版器直接分页；安全的 HTTP(S)/mailto 链接保留可见锚文本并写为原生 URI annotation，不把 `(URL)` 追加到正文。标题、inline style、fenced code、表格、引用、提示块和安全图片按 CSS 盒模型渲染。`mermaid` fenced block 的源码仍是带稳定 nodeId 的唯一可编辑正文；`render-html` 与 PDF 导出按当前 revision 逐 chunk 派生安全 SVG，当前覆盖 `flowchart`/`graph` 和 `sequenceDiagram`，不把派生图固化进 HCD root。JavaScript、任意外部资源和浏览器专属 CSS 保持禁用。PDF 的 `view html` 会继续还原文字、可点击链接、填充矩形、线段和图片。

语义 PDF 同一页可嵌入多个字体子集并按字符切换；emoji 或罕见符号会优先使用可覆盖该字符的系统/配置回退字体，不再因为主中文字体缺字直接替换成 `?`。生产环境可通过 `OFFICECLI_SEMANTIC_PDF_EMOJI_FONT_FILE` 或按平台路径分隔符传入的 `OFFICECLI_SEMANTIC_PDF_FALLBACK_FONT_FILES` 固定回退字体集合。

`--events ndjson` 与全局 `--json` 互斥。事件按实际完成顺序输出。正文直接引用的资产可能先于其 chunk；未引用资产会延后，不阻塞首批正文：

```json
{"event":"import_started","documentId":"...","sourceSha256":"..."}
{"event":"asset_ready","hash":"...","href":"...","byteLength":123}
{"event":"chunk_ready","descriptor":{"sequence":0}}
{"event":"asset_ready","hash":"...deferred...","href":"...","byteLength":456}
{"event":"completed","manifest":{"state":"COMPLETE"}}
```

导入期间不会发布 `manifest.json`，输出目录带有 `.importing` 标记。任意解析或事件回调错误都会清理未完成目录；只有最终 manifest 原子发布后 bundle 才可作为 authoritative head 使用。`completed` 发生在发布之后，因此 completed 事件输出失败不会把已经提交的 bundle 伪装成失败导入。

## Patch 契约

正文偏移是节点内 Unicode scalar 索引，不是 UTF-8 byte、UTF-16 code unit 或全文 offset：

```json
{
  "schemaVersion": "hcd-patch/1",
  "documentId": "business-document-id",
  "patchId": "request-uuid",
  "baseRevision": 0,
  "operations": [
    {
      "op": "text.splice",
      "nodeId": "n_...",
      "start": 7,
      "deleteCount": 3,
      "insertText": "***",
      "precondition": { "nodeHash": "..." }
    }
  ]
}
```

- `expectedRevision` 必须命中当前 head，供调用方执行 CAS。
- 相同 `patchId` 和相同 payload 是幂等重放；相同 ID 对应不同 payload 会被拒绝。
- `baseRevision` 过期时，不同正文节点可自动 rebase；同一节点已变化则冲突。
- 每个 `text.splice` 必须命中 map 中的 `nodeHash` 前置条件。
- `hdoc get-node` 返回当前 `documentId`、`revision`、`nodeId`、`nodeHash`、正文和 source anchor；实现先用 chunk bloom 排除无关分片，再读取候选 map/HTML，不建立全文 offset map。调用方应以返回的 revision/nodeHash 构造后续 patch。
- 同一 HCD revision 链中的正文 patch 只更新 `nodeHash`，不会改变 `nodeId`。完全相同的不可变源字节在省略 `--document-id` 时也会确定性重放相同 ID；内容变化后的逻辑文档必须复用外部 business document ID，且缺少原生 ID、依赖源序号的节点在前方结构增删后仍可能漂移。
- 单 batch 最多 10,000 个 operation，插入正文总量最大 2 MiB，节点内 splice 不可重叠。
- annotation 只记录 node-local 范围、规则、置信度等元数据，不记录敏感原文；annotation root hash 与正文 root hash 独立。
- patch JSON 与 annotation 对未知字段采用拒绝策略，避免把 `originalText` 一类敏感旁路字段静默带入 bundle。

Java 生产服务仍应以数据库 revision/head 为权威，在对象上传完成后做数据库 CAS。OfficeCLI 的本地 manifest head 只负责 bundle 内原子可见性。

## 导出与保真

导出先校验源文件 SHA-256，再从目标 revision 的 HTML fragment 取正文，通过 revision 的 `dirtyNodeIds` 精确收集实际修改节点，不会因为一个节点变化而缓冲整个 dirty part。OOXML 只有 dirty XML parts 被事件流重写，其他 ZIP entry 使用 raw compressed copy；候选 ZIP 的全部 XML/.rels 会在目标文件原子发布前执行流式 well-formed、DOCTYPE、深度和元素预算校验。PDF 在同目录临时副本上修改后原子发布。HTML/TXT 只流式复制并替换 dirty source range；revision 0 导出与源文件逐字节一致。目标路径必须不存在。

HCD 脱敏识别 annotation 默认不写回源格式。导出后会重新打开输出做结构检查并生成 `FidelityReport`。不支持的变更在写目标文件前报错，不做静默降级。source-free/cross-format 路径先按 revision index 顺序校验并物化 canonical chunks，整体 HTML 受 64 MiB 上限约束，再固定使用 OfficeCLI 进程内 Rust handler，不调用 LibreOffice/WPS/pdf2docx/Chromium；Profile 专属源格式几何、opaque part 和 annotation 会在 report 中标记为 flattened。跨 chunk 的 PPTX canonical table 会先按稳定 table node ID、连续 fragment/row range、固定列数和 final 总行数重组；实际 `<tr>` 数也必须与元数据一致。重组后的表格在 PPTX 中建立为原生 DrawingML 表格；单 slide 限制为 18×12，超出时拆为行列窗口并重复首行。PDF 则直接排版 canonical HTML/CSS，保留常见样式、表格、代码块、链接、Unicode/emoji 文本层和分页，并报告 `HIGH`；它不等同于恢复源 Office/PDF 的物理页面，也不宣称任意浏览器 CSS 像素一致。通过 HCD 内容寻址校验和栅格签名白名单的图片会原生写入四种目标；1–100,000,000 EMU 的图片宽高会传递到输出，PPTX 直接 picture 的有界 x/y 会复用。

## Java 与前端接入

1. Java 创建 `IMPORTING` 记录并启动 `hdoc import --events ndjson`。
2. 收到 `chunk_ready` 后即可上传该 descriptor 指向的 chunk/map，并让前端按 sequence 增量读取。
3. `manifest.json` 尚未出现时不得把 bundle 标为可发布；收到 `completed` 后上传 manifest，数据库 CAS 切换 head。
4. 前端按 region 和 sequence 虚拟滚动。通用 chunk descriptor 的 `continuation=true` 表示同一 source part 的后续 chunk；任何跨 fragment 的 canonical 表格必须按 HTML 内相同的 `data-hcd-table-node-id` 和连续 `data-hcd-row-start/end` 拼接，并以 `data-hcd-table-final=true` 判断末片，不能仅依赖 chunk continuation。
5. patch 成功后先上传新内容寻址对象和 revision，再由 Java 数据库 CAS 推进 authoritative head。
6. 任意 `failed` 或子进程异常都使整个导入失效，已上传的无引用对象由后台 GC 清理。

仓库中的 `examples/hdoc/lazy-viewer` 是通用只读参考实现：index page 渐进读取，视口前后预取 chunk，超过驻留上限后用等高 placeholder 卸载正文，重新进入视口时按 `htmlHash` 校验并恢复 canonical fragment，因此 nodeId 不因卸载而变化。它对 PDF/PPTX 的自然 page/slide 分片最直接；DOCX 是语义 chunk 虚拟化，不等于 Word 物理页。XLSX 使用 `examples/hdoc/xlsx-univer-viewer`，按 sheet/row window 进入 Univer Canvas 并保留 cell→nodeId 映射。

`hdoc render-html --chunk-start/--chunk-limit` 生成可离线打开的部分独立 HTML；默认仍为全量输出。部分 HTML 的 `firstChunk/chunkCount/totalChunkCount/nextChunk` 可供 Java API 返回下一窗口游标。完整 `render-html` 与部分 `render-html` 都只在 Rust 端常驻一个有界 chunk；区别是完整输出最终仍包含全部 DOM，而 lazy viewer 只下载并驻留视口附近的 canonical chunk。

### 图片节点与 `hcd-patch/3`

DOCX、XLSX、PPTX、PDF 中可定位的图片进入 source-map，使用稳定 `nodeId`。图片正文与几何状态使用独立 `visualHash`，不会把二进制内容误当作文本 `nodeHash`。`list-images` 可分页返回 `nodeId`、`assetHash`、完整矩形和单位；Office 坐标使用 `emu`，PDF 固定布局使用 `pt`。

`put-asset` 仅接受经过文件头检查的 PNG/JPEG/GIF/WebP，单文件最多 64 MiB。它只写入不可变暂存对象；`image.replace` 成功时，新 asset 才进入该 revision 的内容寻址 asset index。旧 revision 仍读取自己的 asset index。

```json
{
  "schemaVersion": "hcd-patch/3",
  "documentId": "document-id",
  "patchId": "replace-picture-001",
  "baseRevision": 0,
  "operations": [
    {
      "op": "image.replace",
      "nodeId": "n_0123456789abcdef0123456789abcdef",
      "assetHash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      "precondition": { "visualHash": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789" }
    },
    {
      "op": "image.geometry",
      "nodeId": "n_0123456789abcdef0123456789abcdef",
      "geometry": {
        "x": 914400,
        "y": 457200,
        "width": 2743200,
        "height": 1371600,
        "unit": "emu"
      },
      "precondition": { "visualHash": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789" }
    }
  ]
}
```

同一 patch 中的替换和几何操作使用修改前相同的 `visualHash`，作为一次原子修改提交。同一图片已被其他 revision 修改时返回冲突。当前图片 patch 可在 HCD HTML 和不带 `--source` 的纯 Rust 语义导出中生效；DOCX/XLSX/PPTX/PDF 的原包增量媒体重写尚未实现，带 `--source` 导出会在创建输出文件前明确失败，不会静默保留旧图片。

## 后续保真优先级

当前链路已经具备六种 source format 的 source-backed 文本闭环，但“可转”不等于“HTML 已完整复刻原应用渲染”。后续按以下顺序继续收敛：

1. DOCX：常见主题字体/颜色、tint/shade、`basedOn`/linked style、文档内直接 table/row/cell 格式、table style 的继承/直接 band size、Word 条件级联、跨分片 `tblLook`、`cnfStyle` 12 位/独立条件属性、常见 `tblPrEx` 行级视觉例外、按语言选择东亚/双向主题字体、纵向合并 row group、插入/删除/移动修订语义，以及浮动 DrawingML 的 anchor/wrap/坐标元数据已物化；下一步继续对角边框、`tblPrEx` 的布局类例外和旧版兼容模式、wrap polygon 与更完整的编号 restart/地区化格式。物理分页仍不作为 HCD v1 承诺。
2. XLSX：常见内置/自定义 `numFmt`（数字、日期、时间、百分比、货币、科学计数、分数）、1900/1904 日期系统、冻结/拆分窗格和常用 worksheet view 元数据已物化到 HTML；下一步完善地区化数字格式、条件格式及 drawing/chart 的可见布局，公式继续只读。
3. PPTX：DrawingML 表格的直接几何、网格、合并、单元格格式、文本闭环及渐进 row-group fragment 已物化；下一步处理母版/版式/主题与 table style 继承、组合形状变换、图片裁剪、图表与 SmartArt，动画和切换保持源文件权威。
4. PDF：当前已做到 lopdf 前置 `/ObjStm`/`/XRef` 有界预检、压缩对象图保留、逐页 content/字体 stream 有界解码、纯 Rust 完整页面合成视觉层以及透明 nodeId 文本交互层；下一步仍需替换整包 lopdf/Hayro 对象图，按 xref/page 真正随机读取，并以独立受限 worker 或缩放源图解码把 RSS 压到 512 MiB 内。扫描 PDF 需要独立 OCR 能力，dirty 文本页需要重新合成视觉层。
5. 普通 HTML/HCD revision→Office/PDF：DOCX/XLSX/PPTX 的无源纯 Rust `SEMANTIC` 重建与 PDF 的 canonical HTML/CSS `HIGH` 排版均已覆盖；HCD 内容寻址栅格图片、宽高、直接 PPTX picture 坐标、原生表格窗口、HTML-source 结构化 source-map，以及跨 fragment 大表均已安全映射。下一步完善 Office 裁剪/相对锚点和表格内图片；PDF 继续扩展 CSS 覆盖面，但不把 canonical HTML 排版等级误写成源格式物理布局 `EXACT`。

## 验证

日常闭环测试：

```bash
cargo test -p hcd-core -p oxml -p hcd-docx -p hcd-formats --offline
cargo test -p officecli --test hdoc_cli --offline
cargo test -p officecli --test hdoc_multi_format --offline
cargo test -p officecli --test html_convert --offline
cargo test -p pdf-handler reader::tests --offline
```

显式大文件验收（默认生成约 2040 MiB 的解压 XML，可能耗时并占用数 GiB 临时磁盘）：

```bash
make hcd-stress
```

该目标验证压缩输入不超过 256 MiB、`chunk_ready` 多次渐进产生、分片不超过 2 MiB、bundle 校验通过，并进一步执行单节点 patch、source-backed DOCX export 和输出 XML 流式校验；importer 与 exporter 峰值 RSS 都必须低于 512 MiB。调试时可用 `HCD_STRESS_MIB=64 make hcd-stress` 缩小样本；正式验收保持默认值。

2026-09-01 正式值实测通过：`document.xml=2038 MiB`、460,685 个段落、压缩源 53,604,387 bytes，import 峰值 RSS 24,928,256 bytes，patch 后 export 峰值 RSS 8,826,880 bytes。

## 实现与验收矩阵

| 原方案交付项 | 当前状态 | 权威验收 |
|---|---|---|
| `hcd/1`、`hcd-patch/1`、内容寻址 bundle、Merkle root | 已实现 | `hcd-core` schema、bundle、root/validator 单元测试 |
| 分片 index、稳定 nodeId、source-map、manifest 最后发布 | 已实现 | DOCX 重跑稳定测试；XLSX/PPTX/PDF/HTML/Markdown/TXT 重跑 node/root 一致测试；page/ObjStm/XRef bomb 均不留下 bundle |
| 流式 OOXML archive reader/rewriter、raw compressed copy | 已实现 | `oxml::archive` payload 一致与候选结构校验测试 |
| DOCX 渐进 import、正文/表格/文本框/页眉页脚/脚注尾注/批注 | HCD v1 文本范围已实现 | importer golden/语言主题字体/颜色/tint-shade/linked style/直接 table-row-cell 格式/table style 继承与 band size/跨分片及 gridSpan 条件/修订语义/纵向 merge group/浮动 DrawingML anchor-wrap/坐标边界/大节点/首 chunk 早于 part EOF 测试；高级 Word 渲染仍按 fidelity warning 管理 |
| append-only patch、幂等、冲突、annotation 独立 root | 已实现 | `hcd-core::patch` 与 revision-chain 测试 |
| source-backed DOCX export 与 dirty part rewrite | 已实现 | DOCX patch/export 闭环、dirty-node 精确收集、未修改 ZIP payload 测试 |
| XLSX/PPTX/PDF/HTML/Markdown/TXT 扩展闭环 | 已实现当前声明的文本能力；PPTX 额外覆盖直接 DrawingML 表格视觉、渐进分片与文本回写 | `hdoc_multi_format` 文件测试；Markdown/TXT nodeId patch 与 EXACT/HIGH source-backed 回写；PPTX 表格 geometry/grid/merge/style/只读 continuation/patch-export、首片早于 part EOF、跨 rowSpan 不拆分、超大合并组发布前失败测试；各 manifest fidelity 不超报 |
| 已编辑 HCD revision 无源跨格式导出 | DOCX/XLSX/PPTX 纯 Rust `SEMANTIC` 重建和 PDF canonical HTML/CSS `HIGH` 排版已实现；内容寻址安全栅格图片可原生嵌入；source-backed 路径保持不变 | 删除原 HTML 后从 revision 1 导出四种目标，逐个结构校验、重新提取脱敏文本并校验 fidelity report；另验证 2×1 英寸图片的目标 EMU 与 PDF Image XObject/绘图矩阵，损坏 asset 不发布输出；PPTX 大表窗口、跨 chunk 重组及 validator 错误输入覆盖保持不变 |
| DOCX 2 GiB 解压量、worker RSS ≤512 MiB | 已实现脚本验收 | `make hcd-stress`（正式值） |
| PDF 真正 xref/page 随机读取 | 尚未完成 | 当前前置结构流预检和有界逐页解码已阻断已知解压 bomb，但 lopdf 仍整包读取并保留压缩对象图；不能声明严格流式完成 |
| Java OSS、鉴权、数据库 authoritative head/CAS、前端虚拟分页 | OfficeCLI 仓库外责任 | 需要 Java/前端仓库分别实现与联调，本仓库只提供 NDJSON、revision 和 chunk 协议 |
