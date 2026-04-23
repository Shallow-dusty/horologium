# Horologium

Claude Code 状态栏与用量解析的统一 Rust CLI。

## 项目定位

- 替代 `~/.claude/statusline.sh`（bash）+ `ccusage`（Node）的组合
- 重视冷启动性能（状态栏每条消息触发）
- 单二进制分发，零 Node 依赖

## 当前阶段

**Phase 1 v1.1 已完成**（2026-04-23）。`horologium status` 支持：
- 默认 plain 模式：bash statusline.sh 的 parity 替代（含 git branch）
- `--powerline`：U+E0B0 箭头分隔 + 256 色块
- `--multiline`：身份行 + 用量行分开
- `--hyperlinks`：dir / branch 段 OSC 8 可点击

全功能模式冷启动 <1 ms（bash 35 ms → **45× 提速**）。43 个单元测试（含 7 个 worktree / IO 集成测试）。`tempfile` 仅作 dev-dependency；release 二进制依赖不变（clap / serde / serde_json / anyhow / owo-colors）。git 走手写 `.git/HEAD` + `commondir` 解析（worktree-aware），OSC 8 / 256 色 / percent-encoding 全手写 ANSI。

v1.1 修了 v1.0 审出来的 7 项：bash banker's rounding parity、worktree origin_web_url、ssh:// 多变体归一化、URL percent-encoding、hyperlinks 关闭时 IO 短路、IO 级测试覆盖、README 文档口径。详见 `docs/roadmap.md` 决策日志。

下一步：Phase 2 `stat`（外部 pipeline / 批处理 CLI，非 ccusage 翻译）。

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

- 远程仓库 `git@github.com:Shallow-dusty/horologium.git` 已建；本地 v1.0.0 tag 与 Phase 1 收尾的 commits 尚未 push（默认不自动 push，需用户指令）
- 启用 Phase 1 v1.0 后，实际使用 ≥ 2 周以验证"替代 statusline.sh 无退化"出口条件
- Phase 2 `stat`、Phase 3 `configure` 未启动
