# Horologium 是什么

一句话：**跨 Agent CLI（当前 Claude Code + Codex），把状态显示和本地用量分析合并到一个 Rust 二进制的工具。**

当前发布版本为 `v2.2.1`；开发线为 `v2.3.0`，已加入 `heatmap`。Pi 集成作为独立的 `Horologium-Pi` WSL MVP 已落地，主仓库只维护 core 与设计/路线图。

`Horologium` 是拉丁语“时钟”，对应 Agent 订阅常见的 5 小时与 7 天配额窗口。

---

## 它解决的两个真实痛点

### 痛点 1：状态栏太慢

Claude Code 的 statusLine 是每条消息都会触发的热路径。社区主流方案是 `ccstatusline`（TypeScript / Node + Ink TUI），`npx` 冷启动 **0.8–1.5 秒**，每发一句话就卡一下。

Horologium 的 `status` 子命令用 Rust 写，**冷启动 <1 毫秒**——比 Node 方案快 1000 倍以上，比手写 bash 脚本（~35 ms）也快 35 倍。

### 痛点 2：跨会话历史与配额看不清

Agent CLI 自带界面通常聚焦当前会话或当前窗口，难以回答跨日期、跨项目的问题：

- 我这周花了多少？
- 哪个项目最烧钱？
- 今天和昨天对比呢？

Horologium 直接读取本地会话日志：Claude 默认 `~/.claude/projects/**/*.jsonl`，Codex 默认 `~/.codex/sessions/**/*.jsonl`。可按日、会话、5h 块或服务端限额窗口汇总。**单二进制、零网络、零 Node 依赖。**

---

## 它包含什么

### `horologium status`

从 stdin 接 Claude Code 推来的 JSON（模型名 / 工作目录 / 上下文用量 / 当前会话成本 / 5h 7d 配额），打印一行状态栏。

四种渲染模式可自由组合：

| 模式 | 用途 |
|---|---|
| **默认 plain** | 2 空格分隔，纯 ANSI 颜色。最轻量 |
| `--powerline` | U+E0B0 三角箭头 + 256 色背景块（需 Nerd Font） |
| `--multiline` | 输出两行：身份（model / dir / branch）+ 用量（ctx% / cost / 5h / 7d） |
| `--hyperlinks` | dir 和 branch 段加 OSC 8 超链接，点击跳本地目录 / GitHub |

颜色阈值：`<70%` 绿 / `70–89%` 黄 / `≥90%` 红。

### `horologium daily`

扫描本地所有会话日志，按本地时区日历日聚合，输出：

- 每日 records / 5 类 token（input / output / cache-5m / cache-1h / cache-read） / cost
- TOTAL 汇总行
- 未识别模型 / 受损行 / 重复 ID 异常的诊断

**过滤器**：`--since` / `--until` / `--project <子串>` / `--root <路径>` / `--json`（NDJSON 输出给 jq 用）

其他报表命令：

- `sessions`：按会话汇总，可用 `--sort-cost`
- `blocks`：按本地时区固定 5h 块汇总
- `windows [5h|7d]`：从 Codex `rate_limits` 反推窗口成本与限额
- `now`：零输入显示当前 5h + 7d 的 used% / reset / 剩余 USD
- `heatmap`：GitHub 风格 year/month/week/day 活跃热力图，可按 cost 或 tokens 着色

**性能**：本机 665 文件 / 517 MB / 14 天历史 → **58 毫秒**（rayon 8 核 7.4× 并行）

### `horologium configure`

管理 `~/.config/horologium/config.toml`。当前是 TOML 配置 MVP：

- `configure init` 生成默认配置
- `configure check` 校验阈值、重复 segment、空 segment 列表
- `configure path` 打印配置路径
- `configure codex-statusline` 输出 Codex CLI 原生 `[tui].status_line` 推荐配置
- `status` 自动读取配置；CLI flag 可临时覆盖渲染开关

### `--source codex`

Codex 兼容 MVP 已落地：

- `status --source codex` 读取 Codex session JSONL，渲染最新 model / cwd / context / cost / rate limit
- `daily --source codex` 聚合 `~/.codex/sessions/**/*.jsonl`
- `sessions --source codex` 按 rollout 文件聚合
- `blocks --source codex` 复用 5h block 视图
- 成本列按 OpenAI API-equivalent USD 估算，token 口径来自 Codex `token_count.last_token_usage`

Codex CLI 自身的 TUI 状态栏使用官方 `/statusline` / `[tui].status_line`。它当前只支持
内置状态项，不支持 Claude Code 的 external command statusLine 协议；因此 Horologium
不会承诺把自定义行嵌入 Codex 底部状态栏。

因此 Codex 方向只保留三类主线能力：

- 官方显示配置辅助：`configure codex-statusline` 输出推荐的 Codex 原生状态栏配置。
- 外部状态显示：`status --source codex` 解析 session JSONL，再用 Horologium 自己的渲染器输出。
- 数据统计：`daily/sessions/blocks --source codex` 聚合 Codex 本地日志。

MCP 不属于这个目标；hooks 也不作为状态栏方案。后续只有在需要补充事件采集证据时，才单独评估 hooks。

---

## Pi 集成

Pi 版本位于独立仓库/本地包路径 `03.AI-Zenith/05.Agent-Axiom/01.Pi-Packages/06.Horologium-Pi`，提供 `/stats`、`/usage`、`/status`、`/statusline` 和 footer。Rust helper 复用本仓库的 `horologium-core`；Pi package 的资源稳定性修复、缓存策略和验证记录见其 `docs/repair-resource-stability.md`。

## 它的技术承诺

| 承诺 | 体现 |
|---|---|
| **冷启动 <20 ms** | 状态栏实测 <1 ms，全功能模式也在亚毫秒 |
| **单二进制** | 1 MB 左右，无运行时依赖，不装 Node/Python |
| **定价表内嵌** | 24 个 Claude 模型的 LiteLLM 快照 + OpenAI/Codex 内置费率打进二进制，启动不联网 |
| **零 Node 依赖** | 替代 ccusage / ccstatusline 的 Node 工具链 |
| **跨文件 message-id 去重** | 备份或 rsync 副本不会让同一条消息被重复计费 |

---

## 适合谁

- **Claude Code / Codex 用户**：想知道过去几天、会话或窗口用了多少
- **Claude Code 状态栏用户**：从 bash statusline 或 ccstatusline 迁移到亚毫秒热路径
- **把 AI 账单纳入 CI 观测的团队**：`--json` 管道能直接 scrape 到 Prometheus / Grafana

---

## 它不做什么

- **不做 Codex TUI 注入**：官方开放外部 statusline 接口前，不承诺嵌入 Codex 底部状态栏
- **不做 MCP server 形态**：当前目标是本地状态显示和数据统计，不扩展成 MCP 服务
- **不把 hooks 当作状态栏方案**：hooks 只可能作为后续事件采集入口，当前不进入主线
- **不上云 / 不上传日志**：纯本地
- **Gemini / Amp / OpenCode 尚未适配**：当前多源支持覆盖 Claude Code 与 Codex

---

## 接下来

- 想直接跑起来 → 看 [README.md 的快速开始](../README.md#快速开始)
- 想看完整命令参考 → `horologium status --help` / `horologium daily --help` / `horologium configure --help`
- 想知道为什么这样设计 / 下一步做什么 → [`docs/roadmap.md`](roadmap.md)
