# Horologium

Claude Code 状态栏与用量解析的统一 Rust CLI。

## 项目定位

- 替代 `~/.claude/statusline.sh`（bash）+ `ccusage`（Node）的组合
- 重视冷启动性能（状态栏每条消息触发）
- 单二进制分发，零 Node 依赖

## 当前阶段

**Phase 1 v1.1 已完成，dogfooding 启动**（2026-04-23）。`horologium status` 支持：
- 默认 plain 模式：bash statusline.sh 的 parity 替代（含 git branch）
- `--powerline`：U+E0B0 箭头分隔 + 256 色块
- `--multiline`：身份行 + 用量行分开
- `--hyperlinks`：dir / branch 段 OSC 8 可点击

全功能模式冷启动 <1 ms（bash 35 ms → **45× 提速**）。109 个单元测试（含 worktree / IO 集成测试）。release 依赖为 clap / serde / serde_json / anyhow / owo-colors / chrono / rayon；`tempfile` 仅作 dev-dependency。git 走手写 `.git/HEAD` + `commondir` 解析（worktree-aware），OSC 8 / 256 色 / percent-encoding 全手写 ANSI。

v1.1 修了 v1.0 审出来的 7 项：bash banker's rounding parity、worktree origin_web_url、ssh:// 多变体归一化、URL percent-encoding、hyperlinks 关闭时 IO 短路、IO 级测试覆盖、README 文档口径。详见 `docs/roadmap.md` 决策日志。

2026-04-23 起：`~/.claude/settings.json` 的 statusLine 已指向 `horologium status`（bash 原版备份于 `~/.backups/claude/`）；`tests/parity/` 下新增 snapshot harness（10 fixtures × 5 modes = 50 cases），`run.sh --vs-bash` 附带反向发现 bash 两处 bug，已记于 `tests/parity/known-diffs.md`。

2026-04-23：**Phase 2 `stat daily` MVP 完成**。填补 Max 订阅历史统计空白（官方 `/usage` TUI 仅覆盖当前会话）。跨会话 / 按日聚合，支持 `--since / --until / --project / --json / --root`，按 `message.id` 跨文件 dedup。定价表用 LiteLLM 快照嵌入 + `scripts/gen-pricing.py` 发版时 regen。本机 665 文件 / 517 MB / 14 天历史在 8 核上 ~60 ms 出结果。

2026-04-25：**Phase 2 v2.1.0 完成**。新增 `stat session`（按会话聚合，session 级过滤，`--sort-cost`）和 `stat blocks`（5h 固定窗口聚合，对齐 rate limit 节奏）。Codex 审核后修正 session 过滤语义（session 级而非 record 级）。109 个单元测试。

下一步：Phase 3 `configure` 或 Phase 4 发布工程（cargo-dist / 多平台产物）。

## 目录结构

```
src/
├── main.rs       # clap 分派
├── status.rs     # Phase 1: 状态栏渲染
├── git.rs        # .git/HEAD + origin URL 手写解析
├── stat/         # Phase 2: JSONL 用量分析
│   ├── mod.rs        # CLI + dispatch
│   ├── walker.rs     # 递归扫 .jsonl
│   ├── record.rs     # JSONL 行 → Record
│   ├── pricing.rs    # 嵌入 LiteLLM 快照 + cost 算法
│   ├── aggregate.rs  # rayon fold + 跨文件 id dedup + 日桶
│   └── format.rs     # table + NDJSON
└── config.rs     # Phase 3: TUI 配置器（未建）
data/
└── litellm-anthropic-pricing.json  # 定价源（gen-pricing.py regen）
scripts/
└── gen-pricing.py                  # 从 LiteLLM full JSON 生成 slim 快照
docs/
└── roadmap.md    # 四阶段路线图 + 决策日志
tests/parity/
├── run.sh        # snapshot harness（check / --update / --vs-bash）
├── fixtures/     # 10 个 JSON 输入
├── snapshots/    # 50 个 .snap（fixture × 5 render modes）
├── known-diffs.md
└── README.md
```

## 约定

- **每加一个子命令** = 一个独立 module（不往 main.rs 塞逻辑）
- **schema 变更追踪**：Claude Code stdin JSON 的字段演进（如 `utilization` → `used_percentage`）用 serde `#[serde(alias = "...")]` 兼容，不丢旧字段
- **零警告**：`cargo clippy -- -D warnings` 必须过
- **测试**：纯函数必测（如 `fmt_countdown`），IO 相关用 fixture

## 参考资料

- Claude Code statusline 官方文档: https://code.claude.com/docs/en/statusline
- ccusage 源码（TypeScript 对照）: https://github.com/ryoppippi/ccusage
- ccstatusline 源码（TypeScript 对照）: https://github.com/sirmalloc/ccstatusline

## 未完事项备忘

- 已发布版本：v1.0.0 / v1.1.0（Phase 1）与 v2.0.0 / v2.0.1 / v2.0.2（Phase 2 `stat daily` MVP + pricing patch）均已 push 至 `origin/main` 并建 GitHub Release
- 2 周 dogfooding 观测期进行中（起始 2026-04-23），观察 `horologium status` 作为 statusLine 的稳定性；有问题随时回退到 `~/.backups/claude/statusline.sh.bash-v1.20260423.bak`
- `stat session` / `stat blocks` 已完成（v2.1.0）；Phase 3 `configure` 未启动
