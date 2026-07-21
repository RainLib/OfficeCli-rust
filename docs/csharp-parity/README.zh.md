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
`plugins`、`help`、`install`、`skills` 和 `mcp`。

仍需对齐的命令行为：

- `--output-schema-crc` 缺失；
- `load_skill [name] [--path relpath]` 缺失；
- `config <key> [value]` 缺失；
- C# 的 `mcp list`、`mcp <target>`、`mcp uninstall <target>` 生命周期管理缺失；
- `skill`/`skills` 别名、skills 自动探测目标和引用文件安装语义不完整；
- C# 支持 `watch <file> mark|unmark|marks|goto`，Rust 目前主要保留隐藏的顶层
  兼容命令；
- `--help` 统一转发到 schema 驱动的 `help` 尚未完全对齐。

Rust 额外提供 `extract-text`、`convert`、`info` 和原生 PDF handler。这些是 Rust
扩展，不应为了 C# 对齐而删除。

### Help schema

C# 有 150 个 schema JSON，Rust 有 139 个。Rust 缺少：

- DOCX：`abstractNum`、`diagram`、`level`、`num`、`permStart`、`revision`、
  `shape`、`tab`、`textbox`；
- PPTX：`diagram`、`linebreak`；
- XLSX：`detectedtable`。

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

`PPTX-002` 已实现：

- `/slide[N]/group[M]` 使用专用 group setter，不再误走普通 shape 路径；
- 单独设置 width 或 height 时只改变传入轴，并保留 `chOff`/`chExt` 子坐标基线；
- `keepAspect=true` 且只传一个尺寸时按比例补齐另一轴；
- 子节点显式 run、段尾和默认字号按两个轴的最小缩放比例重算，最低保持 1pt；
- 首次修改缺少 child baseline 的外部文件时，先从原始 group transform 建立快照。

验证覆盖单轴放大、单轴缩小、keep-aspect、字号重算、缺失 baseline、非法尺寸，
以及 `create → add group → set width → set height keepAspect=true` 的 CLI 流程。
