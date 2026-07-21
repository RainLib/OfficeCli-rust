# OfficeCLI C# → Rust 对齐基线

本文档是 Rust 迁移的持久入口。后续对齐工作先读取本目录，不再重新扫描整个
C# 仓库；只有 `source/OfficeCLI` 的上游提交发生变化时，才检查基线之后的增量。

## 当前基线

| 项目 | 值 |
|---|---|
| 记录日期 | 2026-07-20 |
| C# 源码目录 | `source/OfficeCLI` |
| C# 上游 | `iOfficeAI/OfficeCLI` 的 `origin/main` |
| C# 基线提交 | `0b3557bbec29f073f5df6b92b4b8dcefa7e3c160` |
| C# 版本 | `1.0.139` |
| Rust 基线提交 | `06f0d89cd8d033b04e3fa6ca9ce3497bbbde55d6` |
| Rust 版本 | `0.1.17` |
| 同步前 C# 本地快照 | 分支 `local/pre-upstream-sync-20260720`，提交 `9f69b9b1` |

`source/OfficeCLI/main` 已快进到 `origin/main`，工作区干净。同步前发现的 327 个
暂存文件没有丢失，已经保存在上述本地快照分支。

## 后续增量流程

1. 获取上游，但不要覆盖本地改动：

   ```bash
   git -C source/OfficeCLI fetch --prune origin main
   git -C source/OfficeCLI status --short --branch
   ```

2. 仅检查本基线之后的变化：

   ```bash
   git -C source/OfficeCLI log --reverse --oneline \
     0b3557bbec29f073f5df6b92b4b8dcefa7e3c160..origin/main
   git -C source/OfficeCLI diff --stat \
     0b3557bbec29f073f5df6b92b4b8dcefa7e3c160..origin/main
   ```

3. 按提交和文件格式更新 `migration-ledger.tsv`。一个迁移批次只处理一个可独立
   验证的行为，避免 DOCX、XLSX、PPTX 逻辑混在同一修改中。

4. 完成并验证增量后，更新本页的 C# 基线提交和版本。若基线未变化，直接从
   ledger 中首个 `missing` 或 `partial` 条目继续。

## 已完成的静态盘点

本次已经检查：

- C# 与 Rust 根命令面；
- `schemas/help` 的逐文件差异；
- DOCX、XLSX、PPTX handler 模块与最新 C# 修复提交；
- Rust 中显式的 `TODO`、`unimplemented!` 和 “not implemented”；
- skills 目录及安装/加载命令面。

这是一份功能面和高风险行为盘点，不代表所有属性组合已完成行为级测试。

### 根命令

Rust 已有主要文档命令：

`open`、`close`、`watch`、`unwatch`、`view`、`get`、`query`、`set`、`add`、
`remove`、`move`、`swap`、`refresh`、`raw`、`raw-set`、`add-part`、
`validate`、`save`、`batch`、`dump`、`import`、`create`、`merge`、
`plugins`、`help`、`install`、`load_skill`、`skills` 和 `mcp`。

仍需对齐的命令行为：

- `--output-schema-crc` 缺失；
- `config <key> [value]` 缺失；
- C# 的 `mcp list`、`mcp <target>`、`mcp uninstall <target>` 生命周期管理缺失；
- `skill`/`skills` 别名、skills 自动探测目标和引用文件安装语义不完整；
- C# 支持 `watch <file> mark|unmark|marks|goto`，Rust 目前主要保留隐藏的顶层
  兼容命令；
- `--help` 统一转发到 schema 驱动的 `help` 尚未完全对齐。

Rust 额外提供 `extract-text`、`convert`、`info` 和原生 PDF handler。这些是 Rust
扩展，不应为了 C# 对齐而删除。

### Help schema

C# 有 150 个 schema JSON，Rust 有 140 个。Rust 缺少：

- DOCX：`abstractNum`、`diagram`、`level`、`num`、`permStart`、`revision`、
  `shape`、`tab`、`textbox`；
- PPTX：`diagram`、`linebreak`。

Rust 独有 `docx/trackedchange.json`；需要先确认它能否完整替代 C# 的
`revision.json`，不能只按文件名直接覆盖。

### Handler 风险排序

P0 表示可能导致文件损坏、引用悬空或作用域错误：

- PPTX 删除幻灯片的包级引用清理；
- PPTX 逻辑幻灯片索引与物理 part 名称解耦，避免删除中间页后覆盖或编辑错页；
- XLSX 插入、移动、删除工作表时 `definedName@localSheetId` 作用域调整；
- OOXML 修改的崩溃原子性和 resident 请求期间的生命周期。

P1 表示主要功能缺口：

- XLSX 动态数组、现代公式边界语义和 in-cell image/richValue；
- DOCX 编号定义、修订/权限范围、diagram/shape/textbox 的结构化增删改；
- PPTX diagram、line break、动画/过渡、现代评论及演示文稿级设置的行为覆盖；
- 缺失 schema 与 handler 支持同步迁移；
- `load_skill`、schema CRC 和 MCP 安装生命周期。

P2 表示兼容性和体验：

- help 路由、命令别名、skills 多客户端安装布局；
- C# 的更新检查、配置和遥测类外围能力；
- HTML/SVG 预览与 Office 实际渲染的细节差异。

详细状态、源码位置和下一步见
[`migration-ledger.tsv`](migration-ledger.tsv)。

## 每批验证门槛

每个条目至少需要：

```bash
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

文档修改还必须提供一个命令级 before/after 流程，并在适用时执行：

```bash
cargo run -- validate <output-file>
```

对 DOCX、XLSX、PPTX 的包级修改，测试必须分别检查该格式的 XML part、
relationship part 和 `[Content_Types].xml`，不能用另一格式的实现推断已对齐。

## 当前迁移批次

`PPTX-001` 已实现：

- 删除逻辑幻灯片时通过 presentation relationship 找到真实 part；
- 删除 `p:sldId`、presentation relationship、slide part、slide rels 和 content
  type override；
- 清除 custom show 中的悬空引用，并删除空 show/list；
- 后续编辑通过关系解析逻辑索引，不再拼接 `slideN.xml`；
- 删除中间页后再新增时使用未占用的 part、slide ID 和 relationship ID，并注册
  content type，避免覆盖现存幻灯片。

验证覆盖纯包级 custom-show 场景，以及
`create → add → remove middle → edit logical slide → add → validate` 的 CLI 流程。

`XLSX-001` 已实现：

- 在指定位置插入工作表时调整后续 `definedName@localSheetId`，并分别分配未占用的
  worksheet part、`sheetId` 和 workbook relationship ID；
- 移动工作表时按工作表身份重映射本地名称作用域，不把名称错误地绑定到原索引上的
  另一张工作表；
- 删除工作表时移除该表作用域的 defined name，递减后续作用域，并清理 worksheet
  part、worksheet rels、workbook relationship 和 content type override；
- `view --mode issues` 会报告越界的 `localSheetId`，避免损坏状态静默通过。

验证覆盖非连续物理 part/ID 的包级插入、移动和删除场景，以及
`create → add at index → move → remove → validate` 的 CLI 流程。下一批从 ledger 中
尚未实现的高优先级条目继续，且仍按 DOCX、XLSX、PPTX 分支隔离。

`SCHEMA-XLSX-001` / `XLSX-004` 已实现：

- `query table` 同时返回真实 ListObject 和严格启发式识别的 `detectedtable`；
- `query listobject` 只返回真实 ListObject，避免把推断范围伪装成稳定 `/table[N]`；
- 检测仅接受左上锚点、至少两列连续表头和至少一行数据的稀疏单元格块，且排除与
  真实 ListObject 重叠的范围；
- 推断节点使用诚实的 `/Sheet/A1:C10` 路径，并报告 `source=header-sniff`、
  `stable=false`、`ref`、`columns` 和 `dataRange`；
- `row[Column op value]` 在没有匹配真实 ListObject 时回退到 detected table，列名以
  结构化列表解析，因此 `Amount, USD` 这类带逗号表头不会错位；
- 新增 `detectedtable` help schema，并在 `table` schema 中明确 table/listobject 的
  查询差异。

验证覆盖数值年份表头、单列误报拒绝、真实表去重、带逗号表头的 row predicate、
sheet-scoped selector，以及真实 CLI 的 create/add/query/help 流程。
