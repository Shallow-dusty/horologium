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

全功能模式冷启动 <1 ms（bash 35 ms → **45× 提速**）。43 个单元测试（含 7 个 worktree / IO 集成测试）。`tempfile` 仅作 dev-dependency；release 二进制依赖不变（clap / serde / serde_json / anyhow / owo-colors）。git 走手写 `.git/HEAD` + `commondir` 解析（worktree-aware），OSC 8 / 256 色 / percent-encoding 全手写 ANSI。

v1.1 修了 v1.0 审出来的 7 项：bash banker's rounding parity、worktree origin_web_url、ssh:// 多变体归一化、URL percent-encoding、hyperlinks 关闭时 IO 短路、IO 级测试覆盖、README 文档口径。详见 `docs/roadmap.md` 决策日志。

2026-04-23 起：`~/.claude/settings.json` 的 statusLine 已指向 `horologium status`（bash 原版备份于 `~/.backups/claude/`）；`tests/parity/` 下新增 snapshot harness（7 fixtures × 5 modes = 35 cases），`run.sh --vs-bash` 附带反向发现 bash 两处 bug，已记于 `tests/parity/known-diffs.md`。

下一步：2 周 dogfooding 窗口期内观测无退化，随后 Phase 2 `stat`（外部 pipeline / 批处理 CLI，非 ccusage 翻译）。

## 目录结构

```
src/
├── main.rs       # clap 分派
├── status.rs     # Phase 1: 状态栏渲染
├── git.rs        # .git/HEAD + origin URL 手写解析
├── stat.rs       # Phase 2: JSONL 用量分析（未建）
└── config.rs     # Phase 3: TUI 配置器（未建）
docs/
└── roadmap.md    # 四阶段路线图 + 决策日志
tests/parity/
├── run.sh        # snapshot harness（check / --update / --vs-bash）
├── fixtures/     # 7 个 JSON 输入
├── snapshots/    # 35 个 .snap（fixture × 5 render modes）
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

- v1.0.0 / v1.1.0 已 push 至 `origin/main` 且建 GitHub Release（2026-04-23）
- 2 周 dogfooding 观测期进行中（起始 2026-04-23），观察 `horologium status` 作为 statusLine 的稳定性；有问题随时回退到 `~/.backups/claude/statusline.sh.bash-v1.20260423.bak`
- Phase 2 `stat`、Phase 3 `configure` 未启动
