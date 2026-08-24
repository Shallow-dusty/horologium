# Current Status

> 快照日期：2026-08-24

## Git 状态

### Horologium 主仓库

- 当前分支：`main`
- 工作区：clean
- `origin/main`：落后本地（包含 heatmap 与本轮文档整理提交），尚未 push
- 当前已发布版本：`v2.2.1`
- 本地开发线：`v2.3.0`
- 本轮整理提交：`ab2be48`（heatmap）、`f9d915b`/`461d413`/`74c60b6`（文档与状态快照）
- `experiment/codex-service-tier-estimates` 保留，不删除、不合并
- `.pi/` 已加入 `.gitignore`，Pi loop runtime 文件不再污染 Git 状态

### Horologium-Pi 独立仓库

- 路径：`03.AI-Zenith/05.Agent-Axiom/01.Pi-Packages/06.Horologium-Pi`
- 当前分支：`main`
- 工作区：clean
- 资源修复提交：`5c33764 fix(resource): prevent footer scan amplification`
- 最新提交：`8c21386 docs: sync installation and repair status`
- 当前仓库未配置 remote；它作为 Pi 独立本地 package 使用
- `~/.pi/agent/settings.json` 已登记该本地 package 路径
- Release helper 已构建：`helper/target/release/horologium-pi-helper`
- 已运行的 Pi 进程需要 `/reload` 或重启，才会加载新 extension/helper

## 功能状态

| 能力 | 状态 |
|---|---|
| Claude Code `status` | 已发布，持续 dogfooding |
| Claude/Codex daily/sessions/blocks/windows/now | 已完成 |
| `heatmap` year/month/week/day | v2.3 本地开发线已完成，待发布工程 |
| TOML configure | MVP 已完成 |
| Pi `/stats` / `/usage` / `/status` / `/statusline` / footer | WSL MVP 已完成 |
| Pi 资源稳定性修复 | 已完成并通过压力回归 |
| Pi Windows 支持 | 待真实 session corpus 与宿主环境验证 |
| 跨 Harness 稳定汇总协议 | 尚未实现，仍是长期方向 |

## Pi 资源修复验收

修复内容：

- footer 使用不可重入、可停止的 serialized poller
- 失败按 30s/60s/120s…退避，保留旧数据并标记 stale/unsynced
- 当前 session 的 rate/requests 走内存 fast path，不再每 20 秒扫描历史
- 同进程相同 helper 查询 Promise 合并
- Rust cache v4：per-file 增量索引、非阻塞跨进程锁、10s 扫描预算、temp+fsync+rename
- `HOROLOGIUM_PI_DISABLE_FOOTER=1` 现场止血开关

真实 corpus 基线（约 584 个 JSONL 文件）：

- 增量变化扫描约 0.35–0.42s，通常只解析 1 个文件
- 精确 cache hit 的扫描阶段约 86–89ms
- 5 helper 并发时每批最多 1 个 parser
- 10 分钟 / 5 helper / 20s 周期压力测试通过，退出后无 helper 残留

## 验证命令

### 主仓库

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

### Horologium-Pi

```bash
npm run typecheck
npm run test:ttft
npm run test:stability
npm run build:helper
npm run test:pressure
npm run harness
cargo test --manifest-path helper/Cargo.toml
cargo clippy --manifest-path helper/Cargo.toml --all-targets -- -D warnings
```

完整资源修复设计、证据和宿主环境边界见 Pi 仓库的 `docs/repair-resource-stability.md`。

## 下一步

1. 为主仓库 v2.3.0 heatmap 走正式 release 工程：版本/tag、CI、多平台产物。
2. 为 Horologium-Pi 决定 remote/release 分发方式，避免长期只依赖本地 path package。
3. 在实际 Pi 多会话与 Windows/Tabby 环境中完成宿主资源和输入延迟复测。
4. 需要跨 Harness 汇总时，再定义版本化 JSON/NDJSON Adapter 协议。
