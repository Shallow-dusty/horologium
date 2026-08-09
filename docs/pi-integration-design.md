# Pi Integration Design（讨论稿）

> 状态：架构商讨中，尚未实现。当前只讨论 Pi，不展开其他 Harness 的拆分。

## 当前目标

在 Pi 内提供接近 Claude Code 的原生使用体验：

- `/usage`：查看用量信息
- `/status`：查看 Horologium/Pi 接入状态
- footer status segment：常驻、简洁的聚合状态
- Pi 运行期间及时更新，而不是每次查询才全量扫描历史
- 保留从 Pi session JSONL 重建/对账的能力

## 项目边界

Pi 集成作为独立 Pi package，计划放在：

```text
~/03.AI-Zenith/05.Agent-Axiom/01.Pi-Packages/03.Horologium-Pi/
```

它将是独立 Git 仓库，不放进 Horologium 仓库，也不通过复制或符号链接维护双份源码。
开发期由 Pi 直接安装本地 package 路径。

实现以 Rust 为主，但 Pi Extension API 只能加载 TypeScript/JavaScript，因此保留一层很薄的
TypeScript bridge，负责生命周期事件、slash command 和 UI；解析、去重与统计尽量放在 Rust。

## 与 Horologium 的关系

- **Horologium**：继续作为统一用户 CLI 和全量聚合入口。
- **Horologium-Pi**：负责理解 Pi 的 session/runtime 语义，并向 Horologium 提供稳定接口。
- Horologium 不应长期直接耦合 Pi JSONL schema；最终通过版本化接口读取 Pi Adapter 的规范化结果。
- 当前 Claude Code/Codex 实现先保持不动。Pi 作为未来可拆分 Adapter 架构的第一个试点，验证后再讨论迁移已有实现。

## 已确认的 Pi 数据能力

- 默认 session 根：`~/.pi/agent/sessions/`
- session 为 JSONL v3，header 提供稳定的 `id`、`cwd`、`timestamp`
- assistant message 提供 `provider`、`model`、`usage`、`usage.cost.total` 和 `stopReason`
- entry 的 `id` 可用于稳定去重
- session 是树结构：实际成本统计应包含所有已经发生的 assistant 请求；当前上下文只看活跃 branch
- 当前 WSL 有真实 Pi 数据；Windows Pi 暂无 session，Windows 支持留到 Pi WSL MVP 后

## 建议的首版用户体验

### `/usage`

面向“我用了多少”，而不是复制 Pi 已有的 `/session`：

1. 当前 Pi session：tokens、cost、context
2. 今日 Pi 用量
3. 最近 7 日 Pi 用量
4. 可选的 Horologium 全量汇总（Claude/Codex），通过统一 CLI 接口读取
5. 数据更新时间与是否完成对账

首版优先可滚动页面；稳定后再考虑 tab、筛选和完整 TUI。

### `/status`

面向“接入是否正常”：

- Pi package / Rust helper / Horologium 版本
- 当前 session 是否已同步
- 最后一次运行时采集和历史对账时间
- session root、文件数、解析错误、协议兼容性
- footer 是否启用

### Footer

不替换 Pi 默认 footer，只用 `ctx.ui.setStatus()` 增加 Horologium 独有的信息，例如：

```text
Horo · Pi today $0.42 · synced ✓
```

Pi 默认 footer 已显示当前模型、session token/cost/context；避免重复显示。

## 建议的数据策略

采用双轨，而不是二选一：

1. **运行时更新**：Pi extension 在合适的生命周期事件后同步本次新增 assistant usage。
2. **JSONL 对账**：启动、恢复或手动 sync 时扫描 session 文件，补回崩溃、未安装扩展期间及旧历史数据。

原始 Pi JSONL 是事实来源；运行时采集是低延迟加速层。两条路径使用相同稳定事件 ID，必须可幂等去重。

具体是直接提交规范化事件、同步当前 session 文件，还是先写本地 journal，尚未定稿。

## 暂缓的长期方向

以下只保留为备忘，本轮不展开设计：

- 一个公开 `horologium` CLI 统筹全部 Harness
- Claude/Codex/Pi/OpenCode/ZCode 各自成为可拆分 Adapter
- 不同 Harness 可按能力采用 extension、hook、watcher 或增量扫描
- 计划矩阵：Codex（Windows/WSL）、Claude Code（Windows/WSL）、Pi（Windows/WSL）、OpenCode（WSL）、ZCode（Windows）
- WSL 使用 Linux binary，但数据环境需与普通 Linux 区分
- 不采用 Rust 动态库 ABI；需要拆分时优先版本化 JSON/NDJSON 进程协议

## 下一步只讨论的四件事

1. `/usage` 首版究竟显示哪些区块，默认是 Pi-only 还是同时显示全量汇总。
2. `/status` 是普通文本、modal，还是二者组合。
3. Pi 运行时更新选择哪个生命周期事件，以及如何与 JSONL entry ID 对齐。
4. Rust helper 与现有 `horologium` 的最小接口：谁负责存储、谁负责聚合、失败时如何回退。

这四项确定后，再创建 `03.Horologium-Pi` 并实现 MVP。
