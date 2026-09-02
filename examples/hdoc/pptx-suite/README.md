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
3. 输出带文字/图片 nodeId hover 的 HCD HTML。
4. 相同源文件重复导入，比较 root hash 和全部文本 `nodeId`。
5. revision 0 source-backed 导出 PPTX，逐 ZIP entry 比较解压内容，并比较源/回写截图。
6. 使用相同 documentId 重新导入 revision 0 PPTX，比较 HCD root hash 和 nodeId。
7. 选择首个可编辑 slide 文本 nodeId，插入 `【HCD测试】`，验证 nodeId 不变。
8. revision 1 source-backed 导出、PPTX 校验、重新导入，并验证 revision 1 root hash 和目标 nodeId/text。
9. 为每个样例生成源 PPTX、HCD、无修改回写和修改后回写的可视化对比页面。

打开总索引查看所有样例：

```bash
open examples/hdoc/pptx-suite/output/index.html
jq . examples/hdoc/pptx-suite/output/summary.json
```

`passed` 表示结构、HCD、nodeId 稳定性、revision 0 package-entry identity、源/回写截图、patch、revision 1 回写和重新导入检查全部通过。revision 0 是 `EXACT` 闭环；HCD HTML 当前仍是 `slide-canvas` 的渐进可编辑视图，尚未完整渲染 master/theme 继承、图表、SmartArt、3D、视频、动画和切换效果，因此不能把 HCD HTML 与 PowerPoint 播放器视为像素级 1:1。
