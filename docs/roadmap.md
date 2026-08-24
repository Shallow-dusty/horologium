# Roadmap

按"先热路径、后冷路径"顺序迭代。每个 phase 有明确出口条件。

## Phase 1 — `status`（状态栏渲染） ✅ v1.0

**目标**：替代 `~/.claude/statusline.sh` 的 bash 实现，冷启动 <20ms。

| 里程碑 | 状态 |
|---|---|
| stdin JSON 解析（model / workspace / context / cost / rate_limits）| ✅ |
| 5h / 7d 倒计时 + 百分比 + 三档颜色 | ✅ |
| 单测覆盖 `fmt_countdown` | ✅ |
| `cargo build --release` 产物可直接配到 settings.json | ✅ |
| 冷启动 benchmark（`hyperfine` 对比 bash 与 ccusage statusline） | ✅ 2026-04-22：Horologium 1.9 ms ± 0.5 / bash statusline.sh 35.2 ms ± 2.4；v1.0 全功能 732 µs；v1.1 默认 560 µs / 全功能 478 µs（Fix B 跳过 IO 后 ~30% 再提速）|
| Codex 交叉审查 + parity 修复 | ✅ 2026-04-22：修 4 项（rate pct 四舍五入 / context 0 默认 / cost 0 默认 / rate 门控对齐）+ basename edge case，测试从 1 扩到 6 |
| git branch（modified 计数延后到 Phase 3 widget） | ✅ 2026-04-23：手写 `.git/HEAD` 解析，支持 worktree，零新依赖 |
| Powerline 分段渲染开关（`--powerline`）| ✅ 2026-04-23：Segment struct + 256 色调色板 + U+E0B0 箭头 |
| 多行输出支持（`--multiline`）| ✅ 2026-04-23：row 分组，与 --powerline 正交组合 |
| OSC 8 clickable links（git 远端 / cwd） | ✅ 2026-04-23：`--hyperlinks` 开关，dir → `file://`，branch → origin web URL |

**出口条件**：日常用 Horologium 的 `status` 替代当前 bash 脚本 ≥ 2 周无退化。

2026-04-23：切换启动。`~/.claude/settings.json` statusLine.command 已改为 `horologium status`；bash 原版备份至 `~/.backups/claude/statusline.sh.bash-v1.20260423.bak`。实测冷启动 25 ms → 512 µs（~49×）。2 周观测期始于本日。

## 版本号策略

- **`vX.0`**：新 Phase 完成（如 v1.0 = Phase 1 完成，v2.0 = Phase 2 完成）
- **`vX.Y`**：小 bug 修复 / 小功能增强（Y 递增）

## Phase 2 — `stat`（用量解析） ✅ v2.1.0

**目标**（重定位于 2026-04-23）：**填补 Max 订阅的历史统计空白**。Claude Code
`/usage` TUI 只看当前会话 + 当前窗口；Max 用户无法看到"这周花了多少 / 哪个项目最
费 / 今天跟昨天对比"。Horologium `stat` 把这些信息从本地 JSONL 里算出来，单二进制、
零 Node 依赖。**不是 ccusage 的机械翻译**——接口更克制、更贴 pipe。

| 里程碑 | 状态 |
|---|---|
| 发现并读取 `~/.claude/projects/*.jsonl` | ✅ 2026-04-23：`walker::find_jsonl` DFS 手写 |
| 并行解析（`rayon`）| ✅ 2026-04-23：`par_iter().fold().reduce()` |
| 计数：input / output / cache-read / cache-write (5m + 1h) tokens | ✅ 2026-04-23 |
| 成本估算（对齐 Anthropic 定价表，单独 `pricing.rs`） | ✅ 2026-04-23：LiteLLM 快照 4 KB 嵌入 |
| 子命令：`stat daily` | ✅ 2026-04-23：按日 rollup + `--since/--until/--project/--json/--root` |
| 子命令：`stat session` | ✅ 2026-04-25：按 JSONL 文件聚合，session 级过滤，`--sort-cost`，Codex 审核修正 |
| 子命令：`stat blocks`（5h 窗口）| ✅ 2026-04-25：5h 固定窗口（00-05/05-10/10-15/15-20/20-00），复用 dedup pipeline |
| `--json` 输出 | ✅ 2026-04-23：NDJSON 每行一对象 |
| 与 ccusage 黄金样本对照验证 | ⏳ 延后到 v2.x |

**实测**：本机 665 JSONL / 517 MB / 14 天历史 → **0.058 s**（rayon 8 核 7.4× 并行度），
扫出 14,824 条 unique assistant 记录 / TOTAL $1761.36（v2.0.1 口径；v2.0.2
修正 1h cache 定价后同语料会略低 ~6-7%）。二进制从 802 KB 涨到 1082 KB
（chrono + rayon 进入代码路径）。

**定价更新**：`scripts/gen-pricing.py` 从 LiteLLM 的
`model_prices_and_context_window.json` 过滤 claude-* + slim 到 4 个成本字段。发版时
重跑一次即可。

**不做**：
- 定价表运行时网络拉取（选择每次发版 regen，规避启动时网络依赖）
- 成本模式切换（官方定价一种；ccusage 的 hybrid 非 Max 刚需）

## Phase 3 — `configure`（配置文件 MVP；TUI 后续）

**目标**：先用可审计的 TOML 覆盖状态栏布局与渲染开关，满足日常配置需求；TUI
配置器作为后续增强，不阻塞 Codex 兼容性分支。

| 里程碑 | 状态 |
|---|---|
| `status` 自动读取 `~/.config/horologium/config.toml` | ✅ 2026-05-03：缺失则默认，坏配置 stderr 告警并回退默认 |
| `configure init/check/path` | ✅ 2026-05-03：生成默认 TOML、校验阈值/空列表/重复 segment、打印路径 |
| 渲染开关持久化：powerline / multiline / hyperlinks | ✅ 2026-05-03：CLI flag 仍可临时覆盖为 true |
| Segment 顺序 / 隐藏 / row / 256 色覆盖 | ✅ 2026-05-03：`[[segments]]` 数组即渲染顺序，删除即隐藏 |
| Rate limit 阈值配置 | ✅ 2026-05-03：`green_below` / `red_above` 驱动 plain 与 Powerline 颜色 |
| 测试隔离配置路径 | ✅ 2026-05-03：`HOROLOGIUM_CONFIG` 覆盖，parity harness 默认 `/dev/null` |
| ratatui 骨架 + crossterm 事件循环 | ⏳ 后续，如 TOML 手改仍不够再做 |
| Widget 列表扩展：git-status 等 | ⏳ |
| 拖拽/上下移动排序 | ⏳ |
| 颜色选择器（16/256/truecolor） | ⏳ |
| 实时预览 | ⏳ |
| 一键写入 `~/.claude/settings.json` | ⏳ |

**决策**：先落地 TOML 配置 MVP。ratatui 的开发量约是 Ink 的 2-3 倍；如果手改
TOML + `configure check` 足够，TUI 不作为当前兼容性工作的前置条件。

## v2.3 开发线 — `heatmap` ✅ 本地完成

`ab2be48 feat(heatmap): add usage activity heatmap` 已在 workspace 中实现：

- `horologium heatmap` 支持 year/month/week/day 四种视图
- `--metric cost|tokens`、`--at`、`--plain`、`--json`
- core 提供去重后 cell 聚合与分位阈值；CLI 负责参数和输出
- 当前 Cargo 版本为 `2.3.0`，尚未打 tag、建 Release 或 push；发布工程仍归 Phase 4

## Phase 4 — 发布工程

| 里程碑 | 状态 |
|---|---|
| `cargo-dist` 配置 | ⏳ |
| GitHub Actions: clippy + fmt + test + release | ⏳ |
| 多平台产物：linux x86_64/aarch64, macos x86_64/aarch64, windows x86_64 | ⏳ |
| `install.sh` 一键安装脚本 | ⏳ |
| Homebrew tap 或 cargo-binstall 支持 | ⏳ |

## Phase 5 — 多 Agent CLI 支持

**目标**：将 Horologium 从 Claude Code 专属工具扩展为跨 Agent CLI 的统一状态栏 + 用量分析工具。

当前项目状态：

- 已完成 Codex 兼容 MVP：`status`、`daily`、`sessions`、`blocks`、`windows`、`now` 均支持 `--source codex`。
- Codex 数据来源限定为本地 `~/.codex/sessions/**/*.jsonl`，核心口径是 `turn_context` 与 `token_count.last_token_usage`。
- Codex 状态显示分两层：Codex TUI 内部使用官方原生 `/statusline` / `[tui].status_line`；Horologium 只做外部渲染和报告。
- `configure codex-statusline` 是配置辅助，不是 external command hook，也不会让 Codex TUI 调用 Horologium。
- Pi 已进入 WSL MVP 维护阶段：独立 package 提供 `/stats`、`/usage`、`/status`、`/statusline` 与 footer；实现状态见 [`pi-integration-design.md`](pi-integration-design.md) 和 Pi package README。
- 2026-08-24 已完成 Pi 资源稳定性修复：串行 poller、失败退避、进程内 Promise 合并、footer fast path、per-file 增量 cache、跨进程锁和原子写入。
- Windows Pi、版本化稳定接口、跨 Harness 混合聚合仍是后续方向；不回头拆 Claude/Codex 现有实现。

| 里程碑 | 状态 |
|---|---|
| `status --source codex`：适配 Codex session JSONL | ✅ 2026-05-03：折叠 `turn_context` + `token_count`，用于调试/外部渲染；Codex TUI 仍使用官方原生 status line |
| `configure codex-statusline`：输出 Codex 原生状态栏推荐配置 | ✅ 2026-05-03：输出 `[tui].status_line` 片段；不伪装成 Claude external command hook |
| `status --source gemini`：适配 Gemini CLI stdin schema | ⏳ |
| `stat daily --source codex`：扫描 Codex 日志目录 | ✅ 2026-05-03：默认 `~/.codex/sessions`，解析 `token_count.last_token_usage` |
| `stat session --source codex`：按 Codex rollout 文件聚合 | ✅ 2026-05-03：session 过滤/排序复用 |
| `stat blocks --source codex`：按 5h block 聚合 Codex token usage | ✅ 2026-05-03：block 逻辑复用 |
| OpenAI / Codex 模型成本估算 | ✅ 2026-05-03：GPT-5.5/5.4/5.4 mini + GPT-5.3-Codex/5.2 |
| `daily --source gemini`：扫描 Gemini CLI 日志目录 | ⏳ |
| `Horologium-Pi` 独立 package / Adapter MVP | ✅ 2026-08-24：WSL MVP 已实现 |
| Pi `/stats`、`/usage`、`/status`、`/statusline`、footer | ✅ 2026-08-24：文本卡 + explorer + footer |
| Pi 资源稳定性（poller/cache/lock/fast path） | ✅ 2026-08-24：commit `5c33764` |
| Horologium 通过稳定接口汇总 Pi 数据 | ⏳ 尚未定义/实现版本化协议 |
| 多源混合聚合（跨 CLI 的统一 daily 视图） | ⏳ |

**优先级**：当前收口 v2.3 heatmap 的发布工程，同时维护 Pi WSL MVP；Windows 支持和稳定协议在实测需求明确后推进。Phase 4 发布工程不再作为 Codex 兼容性的前置条件。

**设计原则**：核心渲染 / 聚合逻辑复用，只有输入解析和日志路径按 source 分流。

## Phase 6 — Claude Code Plugin 壳

**目标**：为纯 Claude Code 用户提供一键安装体验（plugin marketplace 分发）。

| 里程碑 | 状态 |
|---|---|
| `plugin.json` 声明 statusLine 指向二进制 | ⏳ |
| `/stat` slash command 包装 `horologium daily` | ⏳ |
| 配置切换 skill（powerline/multiline/hyperlinks） | ⏳ |

**优先级**：最低，Phase 5 完成后再评估。Plugin 壳是薄包装，不维护双版本。

## 非目标（明确不做）

- Codex TUI 注入：官方开放外部 statusline 接口前，不做 `Codex -> horologium status` 这种嵌入式调用。
- MCP server 形态：当前产品边界是状态显示与本地统计，不扩展成 MCP 服务。
- hooks 状态栏方案：hooks 不能解决 Codex TUI statusline 嵌入；仅在未来需要额外事件采集时单独评估。
- 日志上传 / 云端同步：本工具纯本地。

## 决策日志

| 日期 | 决策 | 依据 |
|---|---|---|
| 2026-04-22 | 起步只含 `clap` / `serde` / `anyhow` / `owo-colors` 四个依赖 | 最小化冷启动与编译时间 |
| 2026-04-22 | 二进制名 = crate name = `horologium` | 单一 brand |
| 2026-04-22 | Phase 3 TUI 用 ratatui 而非 dialoguer / inquire | 更大自由度，Powerline 预览需要 |
| 2026-04-22 | `status.rs` 内的 clap `#[derive(Args)]` 结构命名为 `StatusArgs` 而非 `Args` | 避免与 `clap::Args` trait 同名冲突 |
| 2026-04-22 | `references/` 目录存放 ccusage / ccstatusline 浅克隆，加入 `.gitignore` | Phase 2 开发需要对照其 JSONL 解析字段口径 |
| 2026-04-23 | git branch 走手写 `.git/HEAD` 解析，不引入 `git2` / `gix` / subprocess | 冷启动 + 最小依赖优先；`.git/HEAD` 格式稳定，<50 行即可 bash parity（含 worktree） |
| 2026-04-23 | Phase 1 "modified 计数" 从本期砍掉，延后到 Phase 3 TUI widget | 手写实现需要 git index parser，成本高于收益；TUI widget 阶段可以用更好的 UX 呈现 |
| 2026-04-23 | 核查 CC 2.1.118 changelog：statusline stdin schema 与调用约定均未变，Phase 1 无需迁移 | `/cost` + `/stats` 合并为 `/usage` 属于 TUI 命令整合，与状态栏输入源解耦 |
| 2026-04-23 | Phase 2 重定位：从"ccusage 的 Rust 重写"改为"外部 pipeline / 批处理 CLI" | 交互式查看已被 CC 2.1.118 的 `/usage` TUI 吸收；外部 CLI 的差异化在跨会话聚合、pipe-friendly 输出、CI 集成 |
| 2026-04-23 | 版本号策略：vX.0 = 新 Phase 完成，vX.Y = 小 bug/小增强 | 用户显式指定：v1.0 是 Phase 1 收尾的里程碑版本 |
| 2026-04-23 | v1.0 发布后立即跑 Claude + Codex 双审核；共识 3 MUST + 4 SHOULD + 2 NIT 合并为 v1.1 | Phase 1 承诺 bash parity，review 抓出 `90.5` 跨色档等真实 parity bug，不在用户轮班时积压 |
| 2026-04-23 | 百分比取整改用 `f64::round_ties_even`（Rust 1.77+）而非手写 banker's rounding | 标准库直达 IEEE 754 round-to-even，对齐 glibc `printf '%.0f'` 行为；顺带把 MSRV 升到 1.77 |
| 2026-04-23 | worktree config 查询走 `commondir` 指针解析；factor 出纯函数便于单测 | linked worktree 的 per-wt gitdir 不含 config；纯函数 `resolve_common_dir_from_content` 让单测不需要真实 worktree 布局 |
| 2026-04-23 | `tempfile` 加入 `[dev-dependencies]` 以便写 IO 测试；release 依赖不变 | v1.0 的 git.rs 测试全是字符串层级，IO pipeline 无证据；dev-dep 不影响冷启动 |
| 2026-04-23 | `--hyperlinks` 关闭时短路 `origin_web_url()` 与 `file://` URL 合成 | 冷启动热路径不必要的 IO（读 `.git/config`）在默认模式下是纯浪费 |
| 2026-04-23 | 切换 `~/.claude/statusline.sh` → `horologium status` 启动 2 周 dogfooding | Phase 1 出口条件；bash 原版备份到 `~/.backups/claude/` 便于回滚 |
| 2026-04-23 | 新增 `tests/parity/` snapshot harness（7 fixtures × 5 modes = 35 cases）作为 NIT C 落地 | 未来 refactor 的安全网；`--vs-bash` 同时发现 bash 两处 bug（`xargs basename` 拆空格；`git branch --show-current` 读进程 cwd 而非 JSON 的 workspace.current_dir）—— horologium 均已正确处理，记入 `known-diffs.md` |
| 2026-04-23 | Phase 2 范围重定位：从"外部 pipeline / 批处理差异化"改为"填补 Max 订阅历史统计空白" | 用户反馈：官方 `/usage` TUI 不覆盖历史/跨项目查询，Max 用户被堵死；原 roadmap 的"交互查看已被官方吸收"是错的 |
| 2026-04-23 | 定价表用 LiteLLM 快照 `include_str!` 嵌入 + Python regen script，不走运行时 fetch | 启动无网络依赖 + 二进制体积 +4 KB 可接受；发版时重跑 `scripts/gen-pricing.py` 更新 |
| 2026-04-23 | cache 1h write 价格硬编码（原记录："2× 5m"，**已于 2026-04-24 纠正，见本表末条**）；单测扫全表验证 | 原假设 LiteLLM 只给 5m 价，据此推断 1h = 2× 5m；事后核对 Anthropic 文档发现规则是 1h = 2× **base input** 而非 2× 5m |
| 2026-04-23 | 跨文件 message-id dedup：fold 里保 `HashMap<id, PerIdSummary>`，reduce 后才 bucket 到 `BTreeMap<date, Totals>` | Claude Code 不应该跨 jsonl 重复 id，但备份 / rsync 可能复制文件；二阶段 dedup 顺带让 `unknown_models` 计数与 records 数永远一致 |
| 2026-04-23 | Phase 2 `stat daily` MVP 发布，`session` / `blocks` 延后到 v2.x | `daily` 覆盖 Max 用户 90% 需求；`session` / `blocks` 是 ccusage 细分维度，Max 用户无刚需，不值得阻塞发版 |
| 2026-04-23 | v2.0.1 = codex gpt-5.5 / gpt-5 双审后的 5 项修复（F4 walker 注释 / F2 JSON 诊断走 stderr / F1 divergent-dup 检测 / F5 空 project 归一化 / S2 model-id 前缀归一化） | 83→90 tests；F3 浮点求和顺序（无用户反馈）和 S5 malformed 细分（观察粒度非 correctness）延后 |
| 2026-04-23 | MSRV 从 1.77 上调到 1.85 | 传递依赖 `getrandom v0.4.2` 需要 `edition2024` cargo feature（Rust 1.85 稳定），1.77 已无法解析 lockfile；顺带对齐 GitHub Actions CI 工作流的 toolchain pin |
| 2026-04-24 | 纠正 cache 1h 定价公式：`1h = 2 × base input`（而非 2× 5m），公式改为 `p.input_cost_per_token × 2 × 10⁶`；不变式测试翻转为 `1h == 2 × input` 扫全表 | Anthropic 官方 prompt-caching 文档明确：5m = 1.25× input，1h = 2× input，比例 1h/5m = 1.6 而不是 2。原规则让 v2.0.0 / v2.0.1 的 1h cache 部分高估 25%，按 dogfooding 14 天语料估算 TOTAL 被误报 ~6.7%；codex 独立审核点出，WebFetch 官方文档交叉验证 |
| 2026-04-24 | v2.0.2 发布：pricing 1h 公式纠正 + CLAUDE.md 陈旧口径刷新 + 新增 `docs/intro.md` 入门页 + README 样例按 v2.0.2 重跑 | dogfooding 期间账单误差是 correctness 问题，不值得留待 Phase 3 发版；按 vX.Y 策略立刻发 patch |
| 2026-05-03 | Phase 3 先做 TOML 配置 MVP 而不是完整 ratatui TUI | 当前真实需求是让状态栏布局/颜色/阈值可持久化；TUI 工作量较大且会推迟 Codex 兼容性分支，先用 `configure init/check/path` 收口配置能力 |
| 2026-07-12 | 纯结构重构（不动功能/CLI 接口）：项目定位明确为"跨 Agent CLI 工具"（Claude + Codex，后续 Gemini） | 用户反馈实现"太冗余太乱"；评审确认乱在 stat 模块与 CLI 分派，不在 status 内核。重构范围：(1) `Tier`/`CostMode` 直接 `derive(ValueEnum)`，删 `TierArg` + 两份 `CostModeArg` 包装层；(2) 抽 `CommonArgs`（source/since/until/project/json/root）flatten 进 `Daily`/`Session`/`Blocks`，删 3 个 `build_filters_from_*` 三胞胎 + 3 个 `emit_*_diagnostics_to_stderr` 三胞胎（统一到 `ReportDiagnostics` trait）；(3) 抽 `format::align_table` + `format_diagnostics_notes`，5 处手写表格渲染合一，`now` 也复用；(4) `now` 删自带 `CostModeArg`，复用 `windows::CostMode`。6412→6199 行（-213），137 tests + 50 parity snapshots 全绿，clippy 零警告。未碰 session dedup 语义分裂（correctness，登记后续项） |
| 2026-07-12 | 新建 `AGENTS.md` 作为主文档（pi harness），`CLAUDE.md` 同步为相同内容（CC harness） | 用户以 pi 为通用 agent 体系主；两份同名文档内容一致，避免 harness 间信息漂移；同时刷新 README 跨 agent 定位 + 修正 README 版本口径（v2.2.0→v2.1.0 对齐 Cargo.toml） |
| 2026-07-12 | session 聚合补 divergent 检测，correctness 修复 | `sessions` 之前用 `HashSet` 只判重不比对 payload，损坏日志（同 id 不同 token）被静默吞掉，而 `daily`/`blocks` 会报 `divergent_duplicates`。修法：`aggregate_one_session` 的 `HashSet<String>` 换成 `HashMap<String, SessionDupKey>`，碰撞时比对 timestamp/model/tokens，不同则计数；`SessionReport` 加 `divergent_duplicates` 字段，`ReportDiagnostics` impl 覆盖默认 0。保留 session 独立 per-file 路径（跨文件去重对 session 无意义），未强行走 `LocalAccumulator`。表格模式底部 + JSON 模式 stderr 均会报 note，与 `daily` 行为对齐。JSON 每行输出不变（divergent 是 Report 级别，走 stderr，与 `daily` NDJSON 先例一致）。139 tests（+2 回归测试）+ 50 parity 全绿。属可观察行为变化（损坏日志现在会多一行警告），按 vX.Y 策略该发 v2.1.1 |
| 2026-07-12 | 抽 `windows::cost_disclaimer` 统一 now/windows 文案 + 删 `Stat` hidden 子命令 + 默认 source 决策保留 | 三项后续收尾。(1) disclaimer：`now` 与 `windows` 各有一份 3-arm match 的 cost disclaimer 文案，语义不同（windows=累计 cost+EstLimit，now=剩余 cost）。抽 `windows::cost_disclaimer(mode, mult, DisclaimerScope)`，两命令复用，`DisclaimerScope::WindowCost`/`Remaining` 区分文案；行为逐字不变（parity 验证）。顺手删 windows.rs 死代码 `paths_root`。(2) Stat：hidden deprecated alias，thin wrapper 零逻辑重复。项目内无脚本依赖（仅 status.rs 一处引导文案 + README/intro 文档引用）。跨 agent 定位下 `stat` namespace 暗示 Claude Code 专属，与定位冲突，删除。属 breaking CLI change（外部 `horologium stat daily` 失效），建议 bump v2.2.0 发 release。(3) 默认 source：决策保留 per-command 默认（status=Claude 对齐 statusLine 主用例、stat 类=Codex 对齐配额监控主用例），不加 `configure` 全局 source（YAGNI，跨 agent 切换 `--src` 即可）。AGENTS.md「后续项」section 4 条全部结案。139 tests + 50 parity + clippy + fmt 全绿 |
| 2026-07-12 | 保留 experiment 分支 `experiment/codex-service-tier-estimates`（未合并，344 行） | 探索 Codex service tier 的 per-tier cost 估算（超越当前 std/agg 双模式）。未废弃，Phase 4 发布工程后回访是否合并。分支带 git description 标记，勿盲目删除 |
| 2026-07-12 | dogfooding walkthrough 发现定价表过期 + divergent duplicates 调研 | (1) 定价表过期：Codex 主模型 `gpt-5.6-sol`（1943 条）无定价，Claude 缺 `claude-opus-4-8`/`claude-fable-5`/thinking 变体。修复：regen `data/litellm-anthropic-pricing.json`（21→24 models）+ pricing.rs 加 `GPT_5_6_SOL`（同 GPT-5.5 价 $5/$30/$0.50）+ `claude_alias()` 通用化 `-thinking` 后缀/旧日期名/点号版本号映射 + `codex-unknown` 加入 silent。效果：Codex daily $0→$212.20，Claude daily $501.85→$714.11，unknown 从 9+ 降到 2 项（gemini-3-flash 9 条 + 1 条空 model，均非 Anthropic/Codex）。(2) Windows 路径：`aggregate_one_session` 的 project 提取加 `\\` 分割 fallback，`D:\\Scoop\\persist\\clash-verge-rev` → `clash-verge-rev`。(3) divergent duplicates 调研：1307 中跨文件重复 id 仅 104 个（82 payload 不同），抽样显示大部分是 Claude Code streaming 增量（同 model/ts，output 1 vs 90/211），非真损坏。当前 first-seen 策略可能保留 streaming 早期版低估 cost，warning 文案"log may be corrupted"对 streaming 误导。待产品决策（first-seen vs 取 max output），不擅自改 behavior。151 tests（+4 pricing alias）+ 50 parity + clippy + fmt 全绿 |
| 2026-07-12 | 采用 CC Switch-compatible 的 Claude streaming snapshot 去重口径 | 同类对照：(1) ccusage 用 `messageId:requestId`，真实 Claude 日志缺 `requestId` 时 `createUniqueHash` 返回 null，不去重，output=1/71 两行都会计入（轻微高估）；(2) CC Switch v3.13 在每个 JSONL 内按 `message.id` 聚合，优先有 `stop_reason` 的最终行，否则取更大的 `output_tokens`，并明确记录旧 gate 会系统性低估约 4.1%，92% 集中在 workflow/subagent。Horologium 采用后者：`Record` 新增 `stop_reason`；`read_selected_records` 在聚合前选择完整快照；兼容性 key 为 model + input + cache-read（cache creation 可仅在最终行出现），文件内/跨文件都取 stop_reason 优先/最大 output；仅不兼容 model/input/cache-read 才计 divergent 并 first-seen。真实效果：Claude output 1,314,974→1,839,634，cost $717.04→$729.34，divergent 1307→1（剩余为真同 id 不同 model/input 碰撞）。warning 改为 incompatible request metadata。154 tests（+3 streaming selection regression）+ 50 parity + clippy + fmt 全绿 |
| 2026-07-12 | v2.2.1 patch：定价/streaming correctness + 文档/版本收口 | v2.2.0 发布后 main 有 4 个修复 commit 但 Cargo 仍声明 2.2.0，形成“同版本两套代码”漂移。bump 2.2.1；同步 Cargo description/keywords 为跨 Agent；README/AGENTS 增当前版本；intro.md 从 Claude-only 更新为 Claude+Codex 并补齐 sessions/blocks/windows/now；纳入 LiteLLM 24-model snapshot、GPT-5.6 Sol、Claude aliases、Windows project path、CC Switch-compatible streaming selection、now.rs 8 tests。154 tests + 50 parity + clippy + fmt 全绿 |
| 2026-07-12 | Pi 作为首个外部 Adapter 试点，当前只做设计、不立即拆 Claude/Codex | Pi package 独立放在 `01.Pi-Packages/06.Horologium-Pi`，Rust 为主、薄 TypeScript bridge 接 Pi API；目标是 `/usage`、`/status`、footer 与运行时更新，同时保留 JSONL 对账。长期统一 CLI/多 Adapter 只记方向，待 Pi MVP 验证后再展开，见 `docs/pi-integration-design.md`。 |
| 2026-08-10 | 单 crate 拆为 workspace：`horologium-core`（lib）+ `horologium-cli`（bin） | 起因：用户感觉项目“重和乱”，提出按 harness 拆版本 / 按功能拆二进制两个方向。分析后否决两者（pricing 433 行 + aggregate 1651 行 + format 756 行是 harness 无关的，拆版本 = 复制三份且定价表必漂移；status 与 stat 唯一耦合点是 pricing::lookup，拆二进制收益≈0），但承认三点：(1) 仓库/crate/二进制是三个独立的拆分轴，不能打包讨论；(2) Pi 已决定独立仓库，“按 harness 拆”已部分发生，06.Horologium-Pi 需要复用 pricing/aggregate 的路径必须铺好；(3) statusline（基础设施，该冻结）与 stat（工具，持续演进）发布节奏被单二进制锁死是真实论点。结论：workspace 化是所有拆分方向的公共前置——core 抽成 lib 后，将来拆 bin 或供 Pi git-dependency 引用都无需再重构。纯搬移（git mv 保留历史）：core 7 文件 + data/ 入库，cli 保留 main/status/now/config/git/stat.rs；`format::align_table` 提 pub；record.rs 的 per-harness adapter 拆分留作后续。154 tests + 50 parity + clippy -D warnings + fmt 全绿，冷启动 ~3ms 不变，二进制 1.78MB |
| 2026-08-10 | dogfooding 观测期收尾：结论为持续使用 | 2026-04-23 切换 statusLine 起的 2 周观测期（至 2026-05-07）早已结束，实际持续使用至今 3.5 个月未回退；`~/.claude/settings.json` 仍指向 `horologium status --powerline`（refreshInterval 30s），bash 备份保留于 `~/.backups/claude/statusline.sh.bash-v1.20260423.bak` 供回滚。顺带修复 clippy 新 lint `cloned_ref_to_slice_refs`（windows.rs 测试代码 `&[p.clone()]` → `std::slice::from_ref(&p)`），154 tests + clippy -D warnings + fmt 全绿 |
