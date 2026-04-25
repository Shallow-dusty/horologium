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

## Phase 2 — `stat`（用量解析） 🟡 MVP `daily` 完成

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
| 子命令：`stat session` / `stat blocks`（5h 窗口）| ⏳ 延后到 v2.x（Max 用户非刚需） |
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
- TOML / YAML 配置覆盖（MVP 不需要，见有用户抱怨再加）
- 成本模式切换（官方定价一种；ccusage 的 hybrid 非 Max 刚需）

## Phase 3 — `configure`（TUI 配置器）

**目标**：可视化配置状态栏布局，零 bash/jq 手写。

| 里程碑 | 状态 |
|---|---|
| ratatui 骨架 + crossterm 事件循环 | ⏳ |
| Widget 列表：model / cwd / branch / ctx% / cost / 5h / 7d / git-status | ⏳ |
| 拖拽/上下移动排序 | ⏳ |
| 颜色选择器（16/256/truecolor） | ⏳ |
| 实时预览 | ⏳ |
| 保存到 `~/.config/horologium/config.toml` | ⏳ |
| 一键写入 `~/.claude/settings.json` | ⏳ |

**风险**：ratatui 的开发量约是 Ink 的 2-3 倍。Phase 2 做完再评估是否有必要做 Phase 3 —— 也可能结论是"TOML 手改 + live reload 足够"。

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

| 里程碑 | 状态 |
|---|---|
| `status --source codex`：适配 Codex CLI stdin schema | ⏳ |
| `status --source gemini`：适配 Gemini CLI stdin schema | ⏳ |
| `stat daily --source codex`：扫描 Codex 日志目录 | ⏳ |
| `stat daily --source gemini`：扫描 Gemini CLI 日志目录 | ⏳ |
| 多源混合聚合（跨 CLI 的统一 daily 视图） | ⏳ |

**优先级**：低于 Phase 2-4，待核心功能稳定后再启动。

**设计原则**：核心渲染 / 聚合逻辑复用，只有输入解析和日志路径按 source 分流。

## Phase 6 — Claude Code Plugin 壳

**目标**：为纯 Claude Code 用户提供一键安装体验（plugin marketplace 分发）。

| 里程碑 | 状态 |
|---|---|
| `plugin.json` 声明 statusLine 指向二进制 | ⏳ |
| `/stat` slash command 包装 `horologium stat daily` | ⏳ |
| 配置切换 skill（powerline/multiline/hyperlinks） | ⏳ |

**优先级**：最低，Phase 5 完成后再评估。Plugin 壳是薄包装，不维护双版本。

## 非目标（明确不做）

- MCP server 形态：ccusage 有 `@ccusage/mcp`，Horologium 不做。MCP 不是热路径，走 Node 的成本可以接受。
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
