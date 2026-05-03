# Horologium

> Claude Code 的统一状态栏与用量解析工具，用 Rust 写。单二进制、冷启动毫秒级。

`Horologium` 是拉丁语"时钟/计时器"，对应 5h 与 7d 配额窗口两台走针不停的钟。

## 为什么做这个

Claude Code 生态里状态栏和用量分析各有一把刀：

- **ccusage**（TypeScript / Node）—— 解析 `~/.claude/projects/*.jsonl`，做 daily / session / blocks 成本报表
- **ccstatusline**（TypeScript / Node + Ink TUI）—— 交互式配置状态栏，提供多种 widget

两者都是 `npx` 跑的 Node 程序，**冷启动 0.8–1.5s**。状态栏这种每条消息都会被触发的热路径，Node 冷启动是硬成本；Claude Code v2.1.80+ 又已经把 `rate_limits.five_hour` / `seven_day` 字段直接推进 stdin JSON，服务端权威数据也不再需要本地日志估算。

Horologium 做三件事：

1. **`horologium status`** — 从 stdin 读 Claude Code JSON，打印状态栏。目标冷启动 < 20 ms。
2. **`horologium stat`** — 解析 JSONL 日志，出用量/成本报表（替代 ccusage 主业）。
3. **`horologium configure`** — 管理状态栏 TOML 配置；TUI 配置器留作后续。

## 当前状态

- Phase 1 `status`：**v1.1 已完成 + dogfooding 中**。全功能模式冷启动 <1 ms（比 bash 35 ms 快 35×+）。
- Phase 2 `stat`：**v2.1.0 已完成**。`daily` / `session` / `blocks` 覆盖历史、会话和 5h 窗口聚合。
- Phase 3 `configure`：**TOML 配置 MVP 已实现**。支持生成/校验配置、调整渲染开关、segment 顺序/隐藏、Powerline 颜色和阈值。
- Phase 5 `--source codex`：**Codex 兼容 MVP 已实现**。`status` / `stat daily` / `stat session` / `stat blocks` 均可读 Codex session JSONL。

路线图详见 [`docs/roadmap.md`](docs/roadmap.md)。

## 快速开始

```bash
cd ~/08.Rust-Inscription/01.Horologium
cargo build --release
```

配置到 Claude Code `~/.claude/settings.json`：

```json
{
  "statusLine": {
    "type": "command",
    "command": "~/08.Rust-Inscription/01.Horologium/target/release/horologium status",
    "refreshInterval": 30
  }
}
```

或者装到 PATH：

```bash
cargo install --path .
# 然后 command 改成 "horologium status"
```

## 状态栏输出示例

默认 plain 模式：

```
Opus 4.7  01.Horologium  main  15%  $1.23  5h:75%⏳2h14m  7d:92%⏳3d5h
                                                ↑ 黄          ↑ 红
```

颜色阈值：`<70%` 绿 / `70-89%` 黄 / `>=90%` 红。
倒计时格式：`Xd Yh` / `Xh Ym` / `Ym`，到点显示 `reset`。

## 渲染开关

`horologium status` 支持三个正交的渲染 flag，可自由组合：

| flag | 效果 |
|---|---|
| `--powerline` | 每段带背景色块 + U+E0B0 三角箭头分隔（需 Powerline/Nerd Font） |
| `--multiline` | 输出两行：第一行身份（model / dir / branch），第二行用量（ctx% / cost / 5h / 7d） |
| `--hyperlinks` | dir 段和 branch 段包一层 OSC 8 超链接——dir 跳 `file://...`，branch 跳 git origin 的 web URL。现代终端（iTerm2 / WezTerm / Kitty / Alacritty 等）渲染为可点击；部分旧终端会把转义字节直接显示出来，因此默认关闭 |

示例：`horologium status --powerline --multiline --hyperlinks`

## 数据源

默认数据源是 Claude Code。Codex 兼容通过 `--source codex` 开启：

```bash
horologium status --source codex < ~/.codex/sessions/2026/05/03/rollout-xxx.jsonl
horologium stat daily --source codex
horologium stat session --source codex --sort-cost
horologium stat blocks --source codex
```

默认日志路径：

```text
claude  ~/.claude/projects
codex   ~/.codex/sessions
```

Codex `status` 会折叠 session JSONL 中最新的 `turn_context` 与 `token_count` 事件，
复用同一套 segment、Powerline、多行、超链接和 TOML 配置。Codex 成本列使用
OpenAI API-equivalent USD 估算；Codex 订阅里的 credit 消耗可按 OpenAI 官方 Codex
token-based rate card 对照。

Codex CLI 的 TUI 状态栏请使用官方原生 `/statusline` 或 `[tui].status_line`。Codex
当前只支持内置状态项，不支持 Claude Code 那种 external command statusLine 协议，
所以 `horologium status --source codex` 适合调试/外部渲染 session JSONL，不会被
Codex TUI 自动调用。

## `configure`

默认配置文件路径：`~/.config/horologium/config.toml`。

```bash
horologium configure path        # 打印配置路径
horologium configure init        # 生成默认配置
horologium configure init --force
horologium configure check       # 校验当前配置
horologium configure codex-statusline  # 输出推荐的 ~/.codex/config.toml 片段
```

`status` 会自动读取这个 TOML。命令行 flag 仍然优先：例如配置里 `powerline = false` 时，
`horologium status --powerline` 会临时打开 Powerline。测试或脚本可以用
`HOROLOGIUM_CONFIG=/path/to/config.toml` 指向另一份配置。

配置支持：

- `[render]`：`powerline` / `multiline` / `hyperlinks`
- `[[segments]]`：按数组顺序渲染；删除某段即可隐藏；`bg` / `fg` / `row` 可覆盖 Powerline 色块和多行布局
- `[thresholds]`：调整 rate limit 的绿/黄/红阈值
- `configure codex-statusline`：输出 Codex CLI 原生 `[tui].status_line` 推荐配置

## `stat daily`

按日聚合 `~/.claude/projects/**/*.jsonl` 里的用量，打印 table 或 NDJSON。
弥补 Claude Code `/usage` TUI 的"仅看当前会话"盲区 —— 适合跨会话累计、
按项目看花销、CI/脚本集成。

```bash
horologium stat daily                          # 全部历史
horologium stat daily --since 2026-04-01       # 限定时间
horologium stat daily --since 2026-04-01 --until 2026-04-23
horologium stat daily --project Horologium     # cwd 子串过滤
horologium stat daily --json                   # NDJSON，pipe 到 jq
horologium stat daily --root /other/path       # 指向非默认 projects 目录
horologium stat daily --source codex           # 统计 ~/.codex/sessions
```

输出示例：

```
Day         Records    Input   Cache-5m   Cache-1h   Cache-Read     Output     Cost
-----------------------------------------------------------------------------------
2026-04-22      817   90,301    280,358  4,776,995  114,655,764  1,117,634  $133.46
2026-04-23    1,622   53,500  1,096,975  3,873,737  214,520,212  1,108,817  $179.66
-----------------------------------------------------------------------------------
TOTAL         2,439  143,801  1,377,333  8,650,732  329,175,976  2,226,451  $313.12
```

定价表嵌在二进制里。Claude 模型来自
`data/litellm-anthropic-pricing.json`（源头是 LiteLLM 的
`model_prices_and_context_window.json`，重新生成：`scripts/gen-pricing.py`）。
OpenAI / Codex 模型内置 GPT-5.5、GPT-5.4、GPT-5.4 mini、GPT-5.3-Codex、
GPT-5.2 的当前公开费率。

未识别的模型（例如 Claude Code 内部用的 `<synthetic>` 标签）token 照算、
cost 计 0，并在底部列出 warning。按 `message.id` 跨文件去重，消息不会被
重复计费。665 文件 / 517 MB 的语料在 8 核上 ~60 ms 扫完。

## 与 `statusline.sh` 的 parity 承诺

Horologium 对标 `~/.claude/statusline.sh` 的行为，在下列条件下保证 branch-by-branch 一致：

- 百分比显示：context % 截断小数（`89.9 → 89`），rate limit 百分比四舍五入（`89.7 → 90`）
- 缺字段默认：`context_window.used_percentage` / `cost.total_cost_usd` 缺失时均渲染为 0
- Rate limit 门控：`rate_limits.five_hour` 缺失时整块不输出；存在时同时输出 5h + 7d（后者缺失 fallback 为 `7d:0%` 无倒计时）
- `rate_limits` 字段兼容 `utilization` / `used_percentage` 两种历史命名（serde alias）

**已知细微差异（非 bug）**：

| 差异 | 说明 |
|---|---|
| 分隔符空白 | Rust 统一用 2 空格 `join`；bash 原版在 model→dir 间用 3 空格，其他 2 空格或 1 空格 |
| ANSI reset | owo-colors 用 `\e[39m`（default fg），bash 用 `\e[0m`（all reset），渲染效果相同 |
| Git branch 实现 | bash 用 `git branch --show-current` subprocess；Rust 手动解析 `.git/HEAD`（零依赖、含 worktree 支持），行为一致 |

## 技术栈

| 层 | 选型 |
|---|---|
| CLI 分派 | `clap` derive |
| JSON 解析 | `serde` + `serde_json` |
| TOML 配置 | `toml` |
| 颜色 | `owo-colors`（零依赖 ANSI） |
| 时间 / 日期 | `chrono`（local tz + `NaiveDate` 桶键） |
| JSONL 并行读取 | `rayon`（fold + reduce） |
| TUI（后续） | `ratatui` + `crossterm` |
| 错误处理 | `anyhow` |

编译设定启用 `lto = "thin"` + `codegen-units = 1` + `strip = "symbols"`，追求最小冷启动开销。

## 开发

```bash
cargo run -- status < test-fixtures/sample.json    # 本地跑
cargo test                                         # 跑单测
cargo clippy -- -D warnings                        # 静态检查
cargo fmt -- --check                               # 格式
```

## 与原版的关系

- 不 fork 不绑定，schema 对齐 Claude Code 官方 `statusLine` stdin JSON 规范
- `stat` 子命令会兼容 ccusage 的 JSONL 路径约定（`~/.claude/projects/*.jsonl`）
- Codex 兼容读取本地 `~/.codex/sessions/**/*.jsonl`，不依赖 Codex 私有运行时
- 不依赖 ccusage/ccstatusline 任何运行时

## License

MIT
