# Roadmap

按"先热路径、后冷路径"顺序迭代。每个 phase 有明确出口条件。

## Phase 1 — `status`（状态栏渲染） ✅ MVP

**目标**：替代 `~/.claude/statusline.sh` 的 bash 实现，冷启动 <20ms。

| 里程碑 | 状态 |
|---|---|
| stdin JSON 解析（model / workspace / context / cost / rate_limits）| ✅ |
| 5h / 7d 倒计时 + 百分比 + 三档颜色 | ✅ |
| 单测覆盖 `fmt_countdown` | ✅ |
| `cargo build --release` 产物可直接配到 settings.json | ✅ |
| 冷启动 benchmark（`hyperfine` 对比 bash 与 ccusage statusline） | ✅ 2026-04-22：Horologium 1.9 ms ± 0.5 / bash statusline.sh 35.2 ms ± 2.4，**18–35x 提速** |
| Codex 交叉审查 + parity 修复 | ✅ 2026-04-22：修 4 项（rate pct 四舍五入 / context 0 默认 / cost 0 默认 / rate 门控对齐）+ basename edge case，测试从 1 扩到 6 |
| git branch / modified 计数 | ⏳ |
| Powerline 分段渲染开关（`--powerline`）| ⏳ |
| 多行输出支持（`--multiline`）| ⏳ |
| OSC 8 clickable links（git 远端 / cwd） | ⏳ |

**出口条件**：日常用 Horologium 的 `status` 替代当前 bash 脚本 ≥ 2 周无退化。

## Phase 2 — `stat`（用量解析）

**目标**：替代 `npx ccusage daily/session/blocks` 的主要用法，解析速度 ≥ ccusage 5x。

| 里程碑 | 状态 |
|---|---|
| 发现并读取 `~/.claude/projects/*.jsonl` | ⏳ |
| 并行解析（`rayon`）| ⏳ |
| 计数：input / output / cache-read / cache-write tokens | ⏳ |
| 成本估算（对齐 Anthropic 定价表，单独 `pricing.rs`） | ⏳ |
| 子命令：`stat daily` / `stat session` / `stat blocks`（5h 窗口）| ⏳ |
| `--json` 输出 | ⏳ |
| 与 ccusage 黄金样本对照验证 | ⏳ |

**需要确定的设计**：
- 定价表如何更新？内嵌 + `--pricing-table` 覆盖 or 启动时拉取？倾向内嵌 + 每发版同步。
- Session 的定义？沿用 ccusage 的 5h 滚动窗口。

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

## 非目标（明确不做）

- MCP server 形态：ccusage 有 `@ccusage/mcp`，Horologium 不做。MCP 不是热路径，走 Node 的成本可以接受。
- 日志上传 / 云端同步：本工具纯本地。
- 非 Claude Code 的 CLI 适配（codex / amp / opencode）：保持单一 scope。

## 决策日志

| 日期 | 决策 | 依据 |
|---|---|---|
| 2026-04-22 | 起步只含 `clap` / `serde` / `anyhow` / `owo-colors` 四个依赖 | 最小化冷启动与编译时间 |
| 2026-04-22 | 二进制名 = crate name = `horologium` | 单一 brand |
| 2026-04-22 | Phase 3 TUI 用 ratatui 而非 dialoguer / inquire | 更大自由度，Powerline 预览需要 |
| 2026-04-22 | `status.rs` 内的 clap `#[derive(Args)]` 结构命名为 `StatusArgs` 而非 `Args` | 避免与 `clap::Args` trait 同名冲突 |
| 2026-04-22 | `references/` 目录存放 ccusage / ccstatusline 浅克隆，加入 `.gitignore` | Phase 2 开发需要对照其 JSONL 解析字段口径 |
