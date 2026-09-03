# Examples DOCX → HCD 文件测试

这个目录对仓库中由 Git 跟踪且当前实际存在的 `examples/**/*.docx` 执行同一套 HCD 文件测试。源 DOCX 只读，所有产物写入独立输出目录。

## 运行

```bash
./examples/hdoc/docx-suite/run.sh
```

默认输出到 `examples/hdoc/docx-suite/output/`，目标目录必须不存在。也可以指定新目录：

```bash
./examples/hdoc/docx-suite/run.sh /tmp/officecli-hdoc-docx-suite
```

已有 release binary 且不希望重复构建时：

```bash
DOCX_SUITE_SKIP_BUILD=1 \
  ./examples/hdoc/docx-suite/run.sh /tmp/officecli-hdoc-docx-suite
```

## 每个文件的检查

1. 源 DOCX `validate`、`stats` 和现有 `view html` 预览。
2. DOCX → 分片 HCD，校验所有内容寻址对象与 root hash。
3. 输出带文字/图片 nodeId hover 的完整 HCD HTML。
4. 对相同源文件重复导入，比较 root hash 和全部文本 `nodeId`。
5. revision 0 source-backed 导出 DOCX，重新校验，并逐 ZIP entry 比较未压缩内容。
6. 选择正文中首个可编辑文本 nodeId，插入 `【HCD测试】`，验证 nodeId 不变。
7. revision 1 source-backed 导出、DOCX 校验，并分别生成 patched HCD/DOCX HTML 预览。

打开总索引即可逐文件查看：

```bash
open examples/hdoc/docx-suite/output/index.html
jq . examples/hdoc/docx-suite/output/summary.json
```

`passed` 表示结构、HCD、nodeId 稳定性、patch、source-backed 回写和 DOCX 校验全部通过。HCD DOCX 当前 profile 仍是 `semantic-flow`，因此通过不等于 Word 物理分页或浏览器像素级 1:1。
