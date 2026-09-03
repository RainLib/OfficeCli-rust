# HCD XLSX · Univer Canvas 前端

这是 OfficeCLI 的生产参考适配器：使用 Apache-2.0 的 [Univer](https://docs.univer.ai/) 作为 Canvas 工作簿内核，Rust/HCD 继续负责导入、稳定 nodeId、source-map、revision、patch 冲突和 XLSX 导出。

它不调用 Univer 的 XLSX 导入/导出能力，也不改变纯 Rust 转换边界。

## 为什么使用 Univer

- Canvas2D 工作表渲染，适合百万行/百万级单元格的滚动场景。
- 原生多 Sheet、合并单元格、冻结窗格、行列尺寸、单元格样式和编辑体验。
- HCD 只把当前可视区前后各 128 行对应的内容分片送入 Univer；不会生成百万个 `<td>`。离开窗口且没有待确认 patch 的单元格/图片分片会从 Univer 模型回收，因此内存不会随着用户浏览完整工作簿而持续累积。
- HCD 图片和 Rust 生成的图表 SVG 使用工作表锚点进入同一 Canvas drawing 层；两单元格锚点按真实列宽/行高求和，并应用 `colOff`/`rowOff` 的 EMU 级单元格内偏移，位置和尺寸不再依赖固定网格估算。

所有 `@univerjs/*` 包严格固定为相同的 `0.25.1` 版本，避免跨版本内部协议不一致。

## 使用实际 HCD bundle 验证

先用当前 Rust 二进制重新生成带 `ChunkDescriptor.grid` 随机访问索引的 bundle：

```bash
cargo build -p officecli
rm -rf /tmp/officecli-univer-hcd
target/debug/officecli hdoc import examples/excel/charts.xlsx \
  --output /tmp/officecli-univer-hcd --events ndjson
target/debug/officecli hdoc validate /tmp/officecli-univer-hcd --json
```

启动前端。开发服务器只允许读取 `HCD_BUNDLE_DIR` 指定目录下的文件，防止任意路径访问：

```bash
cd examples/hdoc/xlsx-univer-viewer
npm install
HCD_BUNDLE_DIR=/tmp/officecli-univer-hcd npm run dev
```

打开 [http://127.0.0.1:4174/?bundle=/hcd/](http://127.0.0.1:4174/?bundle=/hcd/)；可切换 `Sheet1`、`StockData`、`Analysis`、`Assessment` 验证多 Sheet、图表锚点与滚动加载。默认是 `mode=readonly`：隐藏功能区、工具栏和公式栏，并拦截所有持久化 mutation；使用 `?bundle=/hcd/&mode=editable` 或页面顶部的模式选择器切换到可编辑界面。生产环境必须由 Java 鉴权结果决定模式，不能把 URL 参数当作授权依据。

生产环境直接把 `?bundle=` 指向 Java/OSS 提供的 HCD 对象前缀。

## nodeId 与编辑协议

Univer 的行列坐标不是业务 ID。适配器维护两张仅覆盖已加载内容的稀疏映射：

```text
sheetId + row + column -> nodeId/nodeHash/chunkId
nodeId -> sheetId/row/column
```

可编辑模式下，只有同时具有稳定 `nodeId`、有效 `nodeHash` 且 source-map 标记为 `editable=true` 的单元格允许进入编辑。单元格编辑后页面发送 `hcd-patch` 事件，`event.detail.patch` 是 `hcd-patch/1` 的 `text.splice`，并携带旧 `nodeHash`。Java 服务执行 revision/CAS 后必须确认或拒绝：

```js
window.addEventListener('hcd-patch', async (event) => {
  try {
    const result = await fetch('/api/hcd/apply', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(event.detail.patch),
    }).then((response) => response.json());
    window.hcdUniver.acknowledgePatch(
      event.detail.patch.patchId,
      result.revision,
      result.nodeHashes,
    );
  } catch (error) {
    window.hcdUniver.rejectPatch(event.detail.patch.patchId, String(error));
  }
});
```

可从外部按 nodeId 定位：

```js
await window.hcdUniver.focusNode('n_0123456789abcdef0123456789abcdef');
window.hcdUniver.getNodeAt('s_0123456789abcdef0123456789abcdef', 0, 0);
```

公式、空白未建模单元格和等待服务端确认的节点禁止编辑；组件不会自行生成或替换 HCD nodeId。

## 验证前端构建

```bash
cd examples/hdoc/xlsx-univer-viewer
npm run build
```

当前适配器重点解决大网格、样式、Sheet 与 nodeId 编辑闭环。条件格式、透视表以及 Excel 图表主题的像素级复刻仍属于后续独立能力，不应由前端组件静默伪造。
