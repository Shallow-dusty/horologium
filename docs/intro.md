# Horologium 是什么

一句话：**Claude Code 生态里，把状态栏和用量分析合并到一个 Rust 二进制的工具。**

`Horologium` 是拉丁语"时钟"，对应 Claude Code 的两个关键计时窗口：5 小时配额窗口 和 7 天配额窗口。

---

## 它解决的两个真实痛点

### 痛点 1：状态栏太慢

Claude Code 的 statusLine 是每条消息都会触发的热路径。社区主流方案是 `ccstatusline`（TypeScript / Node + Ink TUI），`npx` 冷启动 **0.8–1.5 秒**，每发一句话就卡一下。

Horologium 的 `status` 子命令用 Rust 写，**冷启动 <1 毫秒**——比 Node 方案快 1000 倍以上，比手写 bash 脚本（~35 ms）也快 35 倍。

### 痛点 2：Max 订阅看不到历史用量

Claude Code 2.1.118 的 `/usage` TUI 只显示**当前会话**的消耗。Max 订阅用户问不了这三件事：

- 我这周花了多少？
- 哪个项目最烧钱？
- 今天和昨天对比呢？

Horologium 的 `stat daily` 子命令直接读本地的 `~/.claude/projects/**/*.jsonl`（Claude Code 自己写的会话日志），按日聚合、按项目过滤，给出完整的历史账单。**单二进制、零网络、零 Node 依赖。**

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

### `horologium stat daily`

扫描本地所有会话日志，按本地时区日历日聚合，输出：

- 每日 records / 5 类 token（input / output / cache-5m / cache-1h / cache-read） / cost
- TOTAL 汇总行
- 未识别模型 / 受损行 / 重复 ID 异常的诊断

**过滤器**：`--since` / `--until` / `--project <子串>` / `--root <路径>` / `--json`（NDJSON 输出给 jq 用）

**性能**：本机 665 文件 / 517 MB / 14 天历史 → **58 毫秒**（rayon 8 核 7.4× 并行）

---

## 它的技术承诺

| 承诺 | 体现 |
|---|---|
| **冷启动 <20 ms** | 状态栏实测 <1 ms，全功能模式也在亚毫秒 |
| **单二进制** | 1 MB 左右，无运行时依赖，不装 Node/Python |
| **定价表内嵌** | 21 个 Claude 模型的价格快照 4 KB 打进二进制，启动不联网 |
| **零 Node 依赖** | 替代 ccusage / ccstatusline 的 Node 工具链 |
| **跨文件 message-id 去重** | 备份或 rsync 副本不会让同一条消息被重复计费 |

---

## 适合谁

- **Claude Code 的 Max 订阅用户**：想知道过去几天/几周花了多少
- **想要更快状态栏的人**：从 bash statusline 或 ccstatusline 迁移
- **把 AI 账单纳入 CI 观测的团队**：`--json` 管道能直接 scrape 到 Prometheus / Grafana

---

## 它不做什么

- **不做 MCP server 形态**：ccusage 有 `@ccusage/mcp`，Horologium 不做（MCP 不是热路径，Node 的成本可以接受）
- **不上云 / 不上传日志**：纯本地
- **不适配非 Claude Code 的 CLI**（codex / amp / opencode）：保持单一 scope

---

## 接下来

- 想直接跑起来 → 看 [README.md 的快速开始](../README.md#快速开始)
- 想看完整命令参考 → `horologium status --help` / `horologium stat daily --help`
- 想知道为什么这样设计 / 下一步做什么 → [`docs/roadmap.md`](roadmap.md)
