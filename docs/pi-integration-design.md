# Pi Integration Design

> 状态：**设计已定稿，WSL MVP 已实现（2026-08-24）**。本文保留设计决策与边界；实现位于独立仓库 `03.AI-Zenith/05.Agent-Axiom/01.Pi-Packages/06.Horologium-Pi`，不复制进 Horologium 主仓库。

## 当前状态

Pi 是 Horologium 的首个外部 Adapter 试点。当前 WSL MVP 已提供：

- `/stats`：年/月/周/日热力图、窗口汇总、模型排行与交互 explorer
- `/usage`：当前 Pi session 的 tokens、cost、requests、model、cwd
- `/status`：helper、session root、文件数、记录数、解析错误与缓存诊断
- `/statusline`：footer 字段、参数与颜色控制面板
- footer：Pi 原生两行信息 + Horologium 统计字段
- Rust helper：复用 `horologium-core` 的 pricing/聚合能力，TS bridge 负责 Pi 生命周期和 UI

2026-08-24 已完成 P0 资源稳定性修复：footer 不再每 20 秒启动历史扫描；扫描缓存使用 per-file 增量索引、跨进程非阻塞锁、原子写入和失败退避。修复细节见 Pi 包的 `docs/repair-resource-stability.md`。

## 当前目标

在 Pi 内提供接近 Claude Code 的原生使用体验：

- 保持 `/usage`、`/status` 和 footer 的低干扰体验
- 运行期间及时更新，而不是把全历史扫描放进输入关键路径
- 保留从 Pi session JSONL 重建、去重和对账的能力
- 让跨 Harness 的全量视图继续由 Horologium CLI 负责

## 项目边界

Pi 集成是独立 Pi package，开发路径为：

```text
~/03.AI-Zenith/05.Agent-Axiom/01.Pi-Packages/06.Horologium-Pi/
```

Pi 的 Extension API 只能加载 TypeScript/JavaScript，因此采用薄 TypeScript bridge：

- 生命周期事件、slash command、footer 和卡片 UI：TypeScript
- JSONL 解析、去重、缓存和统计：Rust helper
- helper 通过一次性 JSON/JSONL 风格 stdout 协议通信，不采用动态库 ABI
- Horologium 主仓库继续作为统一 CLI 和 `horologium-core` 依赖源

长期目标是版本化 Adapter 协议；当前 package 在开发环境通过本地 path 依赖复用 core，尚未承诺稳定的跨仓库发布接口。

## 与 Horologium 的关系

- **Horologium**：Claude Code/Codex 的统一 CLI、全量报表和 harness 无关 core
- **Horologium-Pi**：理解 Pi session/runtime 语义，提供 Pi 原生交互面
- Pi 不把 JSONL schema 直接耦合进 Horologium CLI；未来若需要统一汇总，应通过版本化 JSON/NDJSON 接口
- Claude Code/Codex 现有输入解析保持不动，Pi 作为独立 Adapter 演进

## 已确认的 Pi 数据能力

- 默认 session 根：`~/.pi/agent/sessions/`
- session JSONL v3 header 提供稳定的 `id`、`cwd`、`timestamp`
- assistant message 提供 `provider`、`model`、`usage`、`usage.cost.total` 和 `stopReason`
- entry `id` 可用于主会话去重
- session 是树结构；成本统计包含已发生的所有 assistant 请求，而不是只看活跃 branch
- 当前以 WSL 数据为 MVP 目标；Windows Pi 暂无可用 session corpus，Windows 支持仍待现场验证

## 已实现的用户体验

### `/usage`（Pi-only）

文本卡展示当前 session 的 tokens、cost、requests、主模型和 cwd，不混入 Claude/Codex 全局汇总。

### `/status`

文本卡展示 helper 版本、session root、文件/记录数、malformed 数量、cache hit/stale、扫描耗时和复用/重解析文件数。

### `/stats`

支持静态快照和交互 explorer：

- `y/m/w/d` 切换粒度
- `c/t` 切换 cost/tokens
- `s` 切换模型排行口径
- 左右键切换窗口，`0` 回到当前，`r` 刷新，Enter 固定卡片，Esc 退出
- 热力图在 TS renderer 的 `render(width)` 中绘制，避免预渲染字符串被 TUI 换行破坏

### `/statusline` 与 footer

`/statusline` 控制字段开关、参数和颜色。footer 通过 `setFooter()` 保留 Pi 原生信息并增加 Horologium 行；rate/requests 每 20 秒从 `ctx.sessionManager.getEntries()` 内存计算，cost 窗口由慢速历史缓存刷新。

现场止血开关：

```bash
HOROLOGIUM_PI_DISABLE_FOOTER=1
```

设置后不创建 footer 后台 poller，但 `/stats`、`/usage`、`/status` 仍可手动调用。

## 数据与资源策略

### 运行时快路径

- TTFT/Decode 使用 extension 事件打点，成功的 `message_end` 才提交样本
- footer rate 使用 `output / end-to-end duration`，不重复累加 reasoning
- footer 不再调用 `helper today`，避免 20 秒周期触发全历史扫描

### JSONL 对账与缓存

- helper 启动、`/stats`、`/usage`、`/status` 时读取原始 JSONL
- cache schema v4 保存 per-file `(path, size, mtime, records, malformed)` 索引
- 未变化文件复用，新增/修改文件单独重解析，最后按全局 entry key 去重
- cache 写入采用 temp + `fsync` + rename
- 多 Pi/helper 进程使用 `fs2` 非阻塞锁；锁竞争者立即返回 stale cache，不排队等待
- helper 失败时 footer 保留最近一次成功值，并显示 `stale`/`unsynced`

### 资源验证基线

当前约 584 个 JSONL 文件的真实 corpus：

- 增量更新约 0.35–0.42 秒，只重解析 1 个变化文件
- 精确 cache hit 的扫描阶段约 86–89 ms
- 5 helper 并发压力测试最多 1 个 parser
- 10 分钟、5 helper、20 秒周期回归通过，结束后无 helper 残留

## 暂缓的长期方向

- 一个公开 `horologium` CLI 统筹全部 Harness
- Claude/Codex/Pi/OpenCode/ZCode 可拆分 Adapter 的稳定发布协议
- Pi Windows 支持与 Windows host 资源回归
- 增量运行时状态文件：当前 TTFT/Decode 已有 state，跨 session 的完整运行时汇总仍可后续扩展
- 多源混合聚合（跨 CLI 的统一 daily 视图）
- Pi 专用独立 UI（可复用 core heatmap 纯函数）

## 已收敛的四个决策（2026-08-10）

1. **`/usage` = Pi-only**：当前 session + Pi 侧窗口信息，不混 Claude/Codex 全量汇总。
2. **`/status` = 文本卡**：`appendEntry` + `registerEntryRenderer`，不用 modal；`/usage` 同形态。
3. **运行时更新挂 assistant 生命周期事件**：TTFT/Decode 使用事件流；footer rate/requests 直接读取当前 session 内存。
4. **helper = 独立子进程 + JSON 协议**：core 负责聚合，cache 负责对账；helper 失败时 UI 降级而不拖垮 Pi。

## 相关文件

- Pi package README：`03.AI-Zenith/05.Agent-Axiom/01.Pi-Packages/06.Horologium-Pi/README.md`
- 资源稳定性修复：Pi package `docs/repair-resource-stability.md`
- 主仓库路线图：`docs/roadmap.md`
