# Examples PPTX → HCD → PPTX 文件测试

这个目录对仓库中由 Git 跟踪且当前实际存在的 `examples/**/*.pptx` 执行完整 HCD 闭环测试。源 PPTX 只读，所有产物写入独立输出目录，转换过程使用 OfficeCLI 的纯 Rust 实现，不调用 LibreOffice。

## 运行

```bash
./examples/hdoc/pptx-suite/run.sh
```

默认输出到 `examples/hdoc/pptx-suite/output/`，目标目录必须不存在。也可以指定新目录：

```bash
./examples/hdoc/pptx-suite/run.sh /tmp/officecli-hdoc-pptx-suite
```

已有 binary 且不希望重复构建时：

```bash
PPTX_SUITE_SKIP_BUILD=1 \
PPTX_SUITE_BIN="$PWD/target/debug/officecli" \
  ./examples/hdoc/pptx-suite/run.sh /tmp/officecli-hdoc-pptx-suite
```

开发时可用正则只运行部分样例：

```bash
PPTX_SUITE_MATCH='textboxes-advanced|pictures-basic' \
PPTX_SUITE_SKIP_BUILD=1 \
PPTX_SUITE_BIN="$PWD/target/debug/officecli" \
  ./examples/hdoc/pptx-suite/run.sh /tmp/officecli-hdoc-pptx-subset
```

## 每个文件的检查

1. 源 PPTX 的 `validate`、`stats`、HTML 预览和整套幻灯片截图。
2. PPTX → 分片 HCD，校验内容寻址对象、source-map 和 root hash。
3. 输出带文字/图片 nodeId hover 的 HCD HTML，并由浏览器后端生成真实 HCD 截图，避免把 source-backed ZIP 恒等误当作 HCD 视觉一致。
4. 相同源文件重复导入，比较 root hash、全部文本 `nodeId` 和图片 `nodeId`。
5. revision 0 source-backed 导出 PPTX，逐 ZIP entry 比较解压内容，并比较源/回写截图。
6. 使用相同 documentId 重新导入 revision 0 PPTX，比较 HCD root hash、文本 nodeId 和图片 nodeId。
7. 选择首个可编辑 slide 文本 nodeId，插入 `【HCD测试】`，验证 nodeId 不变。
8. revision 1 source-backed 导出、PPTX 校验、重新导入，并验证 revision 1 root hash、目标 nodeId/text 和图片 nodeId。
9. 不提供源 PPTX，使用纯 Rust 将当前 HCD revision 语义重建为 PPTX，并执行结构校验、HTML 预览和截图。
10. 为每个样例生成源 PPTX、真实 HCD 截图/HCD HTML、无修改回写、修改后回写和无源语义重建的可视化对比页面。

打开总索引查看所有样例：

```bash
open examples/hdoc/pptx-suite/output/index.html
jq . examples/hdoc/pptx-suite/output/summary.json
```

`passed` 表示结构、HCD、nodeId 稳定性、revision 0 package-entry identity、源/回写截图、patch、revision 1 回写、重新导入和无源语义导出检查全部通过；它不是 HCD 像素级一致声明。revision 0 是 `EXACT` 闭环；修改后的 source-backed revision 1 为 `HIGH`；无源重建为 `SEMANTIC`。HCD `slide-canvas` 已直接渲染 solid/gradient 幻灯片背景、形状填充/描边/圆角/旋转、文本框垂直布局、字符间距、主题色、图片和表格；master/layout 继承、组合变换、裁剪/效果、图表、SmartArt、3D、视频、动画和切换效果仍需继续补齐。每个 comparison 页面中的 HCD 截图才是当前 HCD 视觉能力的实际证据。
