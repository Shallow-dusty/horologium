# Horologium

跨 Agent CLI 的统一状态栏与用量解析 Rust 工具。单二进制、冷启动毫秒级、零 Node 依赖。

`Horologium` 是拉丁语"时钟/计时器"，对应 5h 与 7d 配额窗口两台走针不停的钟。

## 项目定位

- **跨 Agent CLI 工具**：当前覆盖 Claude Code 与 Codex，后续评估 Gemini。不是 Claude Code 专属。
- 两个职责：
  1. **状态栏渲染**（`status`）——从 stdin JSON 渲染状态行，替代 bash 脚本，冷启动 <1 ms
  2. **用量解析**（`daily` / `sessions` / `blocks` / `windows` / `now`）——聚合本地 JSONL 日志，出成本/限额报表
- 单二进制同时承载两个职责。`rayon` / `chrono` / `toml` 进入 release 依赖是为 `stat` + `configure` 服务的；`status` 热路径本身不需要它们，但因为单二进制无法分离——这是"一个二进制做两件事"的耦合代价，刻意接受。
- **不是 ccusage 的机械翻译**：接口更克制、pipe-friendly、跨 CLI 统一。

## 当前阶段

| Phase | 状态 | 说明 |
|---|---|---|
| 1 `status` | ✅ v1.1，dogfooding 中 | 全功能冷启动 <1 ms（bash 35 ms → ~45× 提速）。125+ 单测 + 50 parity snapshots |
| 2 用量分析 | ✅ v2.1.0 | `daily` / `sessions` / `blocks` 覆盖 Claude + Codex JSONL |
| 3 `configure` | ✅ TOML MVP | `~/.config/horologium/config.toml`，render 开关 / segment 顺序 / 阈值 |
| 5 多 CLI 支持 | ✅ Codex MVP | `--source codex` 全命令支持；Gemini 待评估 |
| 4 发布工程 | ⏳ | cargo-dist / 多平台产物 / install 脚本 |
| 6 Plugin 壳 | ⏳ 最低 | Phase 5 完成后再评估 |

**dogfooding**：2026-04-23 起 `~/.claude/settings.json` statusLine 指向 `horologium status`；bash 原版备份于 `~/.backups/claude/`。

## 目录结构

```
src/
├── main.rs          # clap 分派；顶层命令 + hidden `stat` 别名
├── status.rs        # Phase 1: 状态栏渲染（stdin JSON → ANSI 行）
├── git.rs           # .git/HEAD + origin URL 手写解析（worktree-aware，零依赖）
├── source.rs        # Claude / Codex source enum + default_root
├── now.rs           # `now`: 当前 5h+7d 窗口快照（零输入）
├── config.rs        # Phase 3: TOML 配置 + `configure` CLI
└── stat/            # Phase 2/5: 用量分析
    ├── mod.rs       # CLI + dispatch + CommonArgs + 共享 helpers
    ├── walker.rs    # 递归扫 .jsonl
    ├── record.rs    # JSONL 行 → Record（Claude / Codex 双 schema）
    ├── pricing.rs   # 嵌入 LiteLLM 快照 + OpenAI 内置费率 + cost 算法
    ├── aggregate.rs # rayon fold/reduce + 跨文件 dedup + Report 类型 + ReportDiagnostics trait
    ├── windows.rs   # Codex rate-limit 窗口聚合（5h/7d，Tier/CostMode ValueEnum）
    └── format.rs    # align_table + format_diagnostics_notes + 各 report 的 table/NDJSON
data/
└── litellm-anthropic-pricing.json   # Anthropic 定价源（gen-pricing.py regen）
scripts/
└── gen-pricing.py                   # 从 LiteLLM full JSON 生成 slim 快照
docs/
├── roadmap.md       # 路线图 + 决策日志
└── intro.md         # 入门页
tests/parity/        # snapshot harness（10 fixtures × 5 modes = 50 cases）
```

## CLI 命令

```
horologium status          # stdin JSON → 状态栏（默认 Claude，--source codex 读 session JSONL）
horologium now             # 零输入：当前 5h+7d 窗口 used% / resets-in / 剩余 USD
horologium windows [5h|7d] # Codex 限额窗口反推（默认 7d）
horologium daily           # 按日聚合
horologium sessions        # 按会话聚合（--sort-cost）
horologium blocks          # 按 5h 块聚合
horologium configure ...   # TOML 配置管理
horologium stat <sub>      # [hidden] 旧 namespace 别名，脚本兼容
```

`daily` / `sessions` / `blocks` / `windows` / `now` 默认 `--source codex`（Codex 才有 rate-limit 字段、Max 订阅不计费）。切 Claude 用 `--src claude`。

默认日志路径：`claude → ~/.claude/projects`，`codex → ~/.codex/sessions`。

## 架构约定

- **每加一个子命令 = 一个独立 module**（不往 main.rs 塞逻辑）
- **CLI arg 复用**：`daily` / `sessions` / `blocks` 共享 `CommonArgs`（flatten）；`windows` 因不接受 `--since/--until/--project` 而独立
- **表格渲染统一走 `format::align_table`**（header + body + optional total），5 个 table 函数复用；`now` 也复用（传 `total = None`）
- **diagnostics 统一**：`aggregate::ReportDiagnostics` trait 让 `Report` / `BlockReport` / `SessionReport` 共享 malformed / divergent / unknown-model 渲染路径（stderr + in-table notes 两处）
- **`Tier` / `CostMode` 直接 `derive(ValueEnum)`**，不再包一层 Arg 别名 enum
- **schema 变更追踪**：Claude Code stdin JSON 字段演进用 serde `#[serde(alias = "...")]` 兼容，不丢旧字段
- **零警告**：`cargo clippy -- -D warnings` 必须过
- **测试**：纯函数必测；IO 用 fixture / tempfile；`tests/parity/` 50 snapshot 守 status 输出
- **git 走手写 `.git/HEAD` + `commondir`**（worktree-aware），不引入 `git2` / `gix` / subprocess
- **定价表 `include_str!` 嵌入** + `scripts/gen-pricing.py` 发版 regen，无运行时网络依赖

## 依赖

| 层 | 选型 | 服务于 |
|---|---|---|
| CLI | `clap` derive | 全部 |
| JSON | `serde` + `serde_json` | 全部 |
| TOML | `toml` | `configure` |
| 颜色 | `owo-colors` | `status` |
| 时间 | `chrono`（clock+std） | `stat` 桶键 / `now` 倒计时 |
| 并行 | `rayon` | `stat` JSONL 扫描 |
| 错误 | `anyhow` | 全部 |
| dev | `tempfile` | IO 测试 |

release profile：`lto = "thin"` + `codegen-units = 1` + `strip = "symbols"` + `panic = "abort"`。

## 后续项（重构备忘）

- **session 聚合的 dedup 语义分裂**（correctness，未修）：`daily` / `blocks` 走 `LocalAccumulator` + `PerIdSummary` map，碰撞检测 `divergent_duplicates`；`session` 走独立 `aggregate_one_session` + `HashSet`，**不检测 divergent**，`SessionReport` 也没有该字段。这是 session 后加时没复用 daily pipeline 的遗留。`ReportDiagnostics::divergent_duplicates` 默认返回 0 是这个分裂的临时遮盖。修复需让 session 走 `LocalAccumulator`，是语义改动，不在本轮纯结构重构范围内。
- **`Now` vs `Windows` disclaimer 文案分叉**：两处 cost disclaimer 因语义不同（windows 是累计 cost + EstLimit，now 是剩余 cost）文案不同，未强行合并。可后续抽 `cost_disclaimer(mode, mult, scope)` 参数化。
- **`Stat` hidden 子命令**：保留为脚本兼容别名，代码路径与顶层 `daily/sessions/blocks/windows` 共享底层，无重复。确认无脚本依赖后可删。
- **`status` 的 `--source` 默认值**：当前 `status` 默认 Claude，其余默认 Codex。跨 agent 定位下"默认 agent"是个产品决策，待 Gemini 落地后重新评估是否需要 `configure` 里设全局默认 source。

## 参考资料

- Claude Code statusline 官方文档: https://code.claude.com/docs/en/statusline
- ccusage 源码（TS 对照）: `references/ccusage/`
- ccstatusline 源码（TS 对照）: `references/ccstatusline/`
- 路线图与决策日志: `docs/roadmap.md`

## 未完事项备忘

- 已发布：v1.0.0 / v1.1.0（Phase 1）、v2.0.0 / v2.0.1 / v2.0.2 / v2.1.0（Phase 2）均 push 至 `origin/main` 并建 GitHub Release
- 2 周 dogfooding 观测期进行中（起始 2026-04-23）；回退方案：`~/.backups/claude/statusline.sh.bash-v1.20260423.bak`
- ratatui TUI、写入 `~/.claude/settings.json`、git-status widget、Gemini source 后续再评估