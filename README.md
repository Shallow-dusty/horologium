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
3. **`horologium configure`** — 交互式 TUI 配置器（替代 ccstatusline 主业）。

## 当前状态

- Phase 1 `status`：**v1.0 已完成**。全功能模式冷启动 <1 ms（比 bash 35 ms 快 35×+）。
- Phase 2 `stat`：未实现。
- Phase 3 `configure`：未实现。

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
| 颜色 | `owo-colors`（零依赖 ANSI） |
| TUI（后续）| `ratatui` + `crossterm` |
| JSONL 并行读取（后续）| `rayon` |
| 错误处理 | `anyhow` |

编译设定启用 `lto = "thin"` + `codegen-units = 1` + `strip = "symbols"`，追求最小冷启动开销。

## 开发

```bash
cargo run -- status < test-fixtures/sample.json    # 本地跑
cargo test                                         # 跑单测
cargo clippy -- -D warnings                        # 静态检查
cargo fmt --check                                  # 格式
```

## 与原版的关系

- 不 fork 不绑定，schema 对齐 Claude Code 官方 `statusLine` stdin JSON 规范
- `stat` 子命令会兼容 ccusage 的 JSONL 路径约定（`~/.claude/projects/*.jsonl`）
- 不依赖 ccusage/ccstatusline 任何运行时

## License

MIT
