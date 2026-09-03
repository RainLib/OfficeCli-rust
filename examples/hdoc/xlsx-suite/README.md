# Examples XLSX → HCD 文件测试

本目录对仓库中 Git 跟踪且实际存在的 `examples/**/*.xlsx` 执行 XLSX → HCD → XLSX 文件级闭环验证。源文件保持只读，所有产物写入独立输出目录。

## 运行

```bash
./examples/hdoc/xlsx-suite/run.sh
```

默认输出到 `examples/hdoc/xlsx-suite/output/`，目标目录必须不存在。也可以指定新目录：

```bash
./examples/hdoc/xlsx-suite/run.sh /tmp/officecli-hdoc-xlsx-suite
```

复用已有二进制：

```bash
XLSX_SUITE_SKIP_BUILD=1 \
XLSX_SUITE_BIN="$PWD/target/debug/officecli" \
  ./examples/hdoc/xlsx-suite/run.sh /tmp/officecli-hdoc-xlsx-suite
```

默认 HCD 预览从 XLSX 的 `styles.xml`、列宽、行高、合并单元格和 sheet view 生成样式。需要在不改变 HCD、nodeId、root hash 和导出文件的前提下覆盖展示样式时，可传入一个经过安全校验的 CSS 文件：

```bash
target/debug/officecli hdoc render-html \
  examples/hdoc/xlsx-suite/output/cases/word__tables/hcd/bundle \
  --output /tmp/tables-custom-style.html \
  --style examples/hdoc/xlsx-suite/excel-style-override.css \
  --text-hitboxes on --image-hitboxes on --json
```

覆盖 CSS 在 bundle 自带样式之后加载，因此相同或更高优先级的选择器可重写展示值。`url(...)`、`@import`、`expression(...)` 等主动内容会被拒绝，文件上限为 1 MiB。

## 大表格与 nodeId

standalone HCD 预览采用混合渲染：当前视口的空白网格、行号和列号由 viewport-sized Canvas 绘制，实际有内容且可编辑的单元格仍保留 HTML 节点。Canvas 不生成业务标识，也不会改变 bundle；单元格的 `nodeId`、`nodeHash` 和 source-map 仍以 HCD 为权威。页面提供 `window.hcdGridHitTest(clientX, clientY)`，返回 `{sheet, cell, row, column, nodeId, nodeHash, nodeKind, loaded}`；图片和图表命中时直接返回对应 visual nodeId，尚未加载的行窗口返回坐标且 `loaded=false`，由前端再按 sheet/row window 拉取索引与分片。

Canvas 始终只分配当前滚动视口大小（设备像素比最多 2），不为图表覆盖范围合成大批空白 `<td>`。生产前端仍应按 `indexes/*.json` 分页加载 HCD 行窗口并回收离屏 DOM；standalone HTML 是文件验收工具，不代替服务端的分片 API。

生产参考前端位于 [`../xlsx-univer-viewer/`](../xlsx-univer-viewer/README.md)。它使用 Univer Canvas 工作簿，并直接按每个 `ChunkDescriptor.grid` 的稳定 sheetId、行列范围和内容类型选择可视分片；Rust/HCD nodeId 与 patch/revision 语义不交给组件管理。

## 每个文件的检查

1. 源 XLSX 的 `validate`、`stats` 和 `view html`。
2. XLSX → 分片 HCD，并校验内容寻址对象与 root hash。
3. 输出带稳定 cell/image/chart nodeId 的 HCD HTML；预览头部、行列标题和工作表标签页与 `view html` 的 workbook 外壳保持一致，不再额外伪造空公式栏。
4. 对比源 `workbook.xml` 与 HCD 的工作表数量，验证每个 sheet 的顺序和 visible/hidden/veryHidden 状态；同一 sheet 的表格、图片和图表分片会组合到一个共享坐标层并由同一个标签页切换。
5. 逐 drawing 统计源图表引用，要求 source HTML 与 HCD 都物化相同数量的图表；即使 sheet 没有任何单元格、只有图表，也不能显示为空白页。
6. 重复导入并比较 root hash 和全部 nodeId。
7. revision 0 source-backed 导出 XLSX，逐 ZIP entry 比较未压缩内容，确保图表、透视表和其他不透明 OOXML 部件原样保留。
8. 公式节点必须保持只读；存在普通单元格时，按 nodeId 插入 `【HCD测试】`，nodeId 必须不变。
9. 存在普通单元格时，执行 revision 1 source-backed 导出、校验、文本回读和 HTML 预览。
10. 存在普通单元格时，执行 revision 1 source-free 语义重建 XLSX 并校验；该产物只承诺 HCD 网格语义，不承诺重建源图表或透视表。

打开总索引查看每个案例：

```bash
open examples/hdoc/xlsx-suite/output/index.html
jq . examples/hdoc/xlsx-suite/output/summary.json
```

`passed` 表示自动化结构、HCD、sheet 数量/元数据、图表数量、Excel 风格 grid presentation、nodeId 稳定性、公式只读、patch、source-backed/source-free 导出和校验全部通过。稀疏行会写入不可编辑空白单元格占位，确保 `C9` 仍显示在 C 列而不是挤到 B 列；没有显式 `<cols>` 的工作表使用稳定的默认列宽，不再被长标题撑开。图表优先使用 OOXML 缓存 series；缓存缺失时，导入器按每个 series 最多 2048 个单元格的有界范围流式回读工作表引用，再由纯 Rust 渲染为 SVG。图表仍保留只读 chart nodeId、chart part、drawing part 和单元格锚点；同一 sheet 的多个图表会在共享 drawing layer 中按 anchor 叠加到网格上。这可正确显示图表专用 sheet，但不是 Excel 图表主题、阴影、字体和轴布局的像素级复刻。外部、命名、3D 或超限图表引用继续由不可变源文件保真保存。XLSX HCD 当前 profile 为 `grid`；条件格式、形状、透视表、图表高级效果和精确 Excel 绘图布局仍以不可变源 XLSX 为权威。
