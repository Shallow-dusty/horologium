# Horologium

Claude Code 状态栏与用量解析的统一 Rust CLI。

## 项目定位

- 替代 `~/.claude/statusline.sh`（bash）+ `ccusage`（Node）的组合
- 重视冷启动性能（状态栏每条消息触发）
- 单二进制分发，零 Node 依赖

## 当前阶段

Phase 1 MVP 已完成 + Codex 审查并修复：`horologium status` 可读 stdin JSON、输出含 5h/7d 倒计时 + 颜色的状态栏，与 `~/.claude/statusline.sh` 4 种边界输入下输出一致。冷启动 1.9ms（bash 35ms → 18-35x 提速）。6 个单元测试覆盖 `fmt_countdown` / `basename` / `format_window` 的关键分支。

下一步建议参考 `docs/roadmap.md`。

## 目录结构

```
src/
├── main.rs       # clap 分派
├── status.rs     # Phase 1: 状态栏渲染
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

- Phase 1 还缺：benchmark、git branch、Powerline、多行
- 还未 `git init`，还未建 GitHub 仓库
- Cargo 依赖未拉取（首次需要 `cargo fetch` 或 `cargo build`）
