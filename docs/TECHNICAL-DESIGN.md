# DH BOT 3.0 技术设计

本文是面向开发者和维护人员的实现说明。产品入口和操作方法见
[`DH使用手册.md`](DH使用手册.md)，功能清单见
[`FEATURE-CATALOG.md`](FEATURE-CATALOG.md)，生产命令清单见
[`API-REFERENCE.md`](API-REFERENCE.md)，本文件重点说明模块边界、并发模型、失败处理和测试方式。

## 1. 运行时边界

DH BOT 3.0 是 Rust + Tauri v2 + React/TypeScript 应用。生产程序只连接本机旺商聊
DevTools `http://127.0.0.1:9222`，Fixture 只能通过编译期 `fixture` feature 进入开发包。

| 构建渠道 | Cargo | 数据目录 | DevTools | 允许内容 |
|---|---|---|---|---|
| Production | `--no-default-features` | `%APPDATA%\\DH\\3.0` | `9222` | 真实旺商聊、生产命令 |
| Developer | `--features fixture` | `%APPDATA%\\DH\\fixture` | `9233` | Fixture、开发校准、自动化测试 |

生产版不会根据环境变量、旧运行模式文件或命令参数切换到 Fixture。前端的开发入口、
Fixture 命令和 Fixture 资源也在生产构建时移除。旧 Go 2.7 代码仅在
[`archive/go-2.7-final/`](../archive/go-2.7-final/) 作为只读参考。

## 2. 模块地图

```text
tauri3/src/                         React 页面和 typed client
    | invoke() / 事件
tauri3/src-tauri/src/lib.rs         AppState、共享校验辅助、dh_handlers! 注册表
    |-- commands/                   Tauri command 按域分文件（14 个）
    |     ai · audit · bizapps · cards · groups · knowledge · messaging
    |     moderation · rules · summaries · system · tasks · wang
    |     developer（仅 fixture feature）
    |-- runtime/                    BackendRuntime；mod.rs 保存字段与消息流水线
    |     mod.rs                    消息 worker、Dispatcher(outbox)、连接与事件循环
    |     summary · activity · card · roster · narration  各 worker 循环
    |-- runtime_work.rs             RuntimeWorkTracker · DispatchStats · RuntimeCoordination
    |-- queue_kernel.rs             QueueKernel — ExpiryGuard / OrderKeyLock / LaneConcurrencyGate
    |-- gateway.rs                  RuntimeGateway、CDP、Electron IPC、NIM、成员缓存
    |-- bridge.rs                   axum 127.0.0.1:51235；BridgeState{gateway, database}
    |-- database.rs                 schema v14、迁移、DatabaseExecutor(256 FIFO)
    |-- repository.rs               事务查询、分页、claim 和审计写入
    |-- moderation.rs               机器规则与 AI 控制规则
    |-- cardnames.rs                建议名片、改名验证、欢迎队列
    |-- knowledge.rs / ai.rs        文档检索、AiProviderPool、AI 决策校验
    |-- business_apps.rs            BusinessAppRegistry
    |-- http_body.rs                共享限长响应体读取；ai.rs 与 prediction.rs 经此读远端响应
    |-- prediction.rs               PublicLotterySource — BCLC/CWL/pc28.help
    |-- scheduler.rs                摘要、任务提醒、每日开关群计划
    |-- diagnostics.rs              JSONL 日志轮转和支持包 ZIP
    |-- secrets.rs / platform.rs    DPAPI、进程身份、托盘和 Windows 维护
    |-- fixture.rs                  仅 fixture feature 的内存/浏览器测试网关
SQLite WAL (dh-sqlite thread)       唯一持久化连接；schema v14
旺商聊 9222                          CDP → Electron xclient IPC → NIM
axum :51235                         本地桥；入队 effect_outbox / 查诊断状态
```

前端不能直接读 SQLite、密钥或 DevTools。所有写入命令必须使用固定的 Rust 请求类型，
不能传任意协议路由、JavaScript 或 SQL。

## 3. 入站消息和事件

RuntimeGateway 提供有序的 `GatewayBatch`。每条记录有 `bridgeSeq`、稳定事件 ID、
捕获时间和消息/成员/连接事件类型。

```text
install_listener
    -> read_batch(100)
    -> SQLite 事务：gateway_inbox + messages/diagnostic
    -> 只 ACK 已持久化的连续 bridgeSeq 前缀
    -> 按 groupId 串行派发
    -> 规则 / AI / 任务 / 摘要 / 名片
```

实现约束：

- 使用 `(accountId, bridgeSession, bridgeSeq)` 和稳定事件 ID 去重；消息再使用
  `(accountId, groupId, serverMessageId)` 唯一约束。
- 乱序消息按服务器消息 ID 和消息时间处理，不跳过更低序号的 ACK。
- 出站回显、本人消息、P2P 和非群会话不进入群管规则；能识别群但解码失败的消息
  以“其他”落库并写诊断。
- ACK、处理和副作用是三个状态。重启恢复 pending/retry，未知协议回执进入人工确认，
  不因为超时重复执行高影响动作。
- 每群一个串行处理队列；网络和 AI 延迟不会阻塞下一批入站读取。
- 消息监听、批次读取和 ACK 使用独立 CDP 会话；群列表、成员名单和人工协议调用使用
  业务 CDP 会话。旺商聊成员接口即使发生慢响应，也不会占用消息队列的请求锁或阻塞 ACK。

## 4. SQLite 执行器和事务

`Database::open` 只执行目录准备、WAL、`quick_check`、schema 初始化/迁移和快照；启动后
唯一连接交给 `DatabaseExecutor`。执行器由名为 `dh-sqlite` 的 OS 线程持有，使用容量 256
的 FIFO 队列：

```text
Tokio/Tauri command --(Run job)--> 256 项有界队列 --> dh-sqlite --> rusqlite Connection
                                              \--> result oneshot
```

- 网络请求、CDP、AI 和等待重试不能在数据库线程执行。
- ingest、ACK、outbox claim、回执归档和审计需要同一事务时必须在一个 job 内完成。
- 退出先停止领取新任务，再排空关键写入，发送 `Shutdown`，等待线程结束；最多等待
  5 秒，仍不确定的副作用写为 `unknown`。
- 数据库迁移使用事务和启动前快照，字段补齐按 `PRAGMA table_info` 幂等执行。
- 所有数据库时间存 UTC；计划额外保存 Windows 时区和本地日期键。
- 消息处理所需的群、成员、最近消息和启用规则由一个 `message_processing_context` job
  一次返回，避免单条消息在 actor 队列中产生四次往返。

数据库字段边界和唯一约束见 [`DATABASE-SCHEMA.md`](DATABASE-SCHEMA.md)。

## 5. 规则、AI 和副作用

### 机器规则

机器规则只使用确定性 matcher（关键词、长度、行数、图片次数、黑名单、名片变更等），
命中后直接生成 `ActionIntent`，不调用模型。机器规则和 AI 控制规则有独立的群开关、
评估记录和审计来源。规则优先级只使用低/中/高；v2 的角色豁免和规则冷却字段保留
用于旧库兼容但不再参与判断。

### AI 控制规则与 AI 助手

AI 控制规则对同一消息合并一次语义分类请求，默认观察；撤回、禁言和移出权限分别检查。
AI 助手只有明确 `@DH` 或旺商聊提及元数据才触发，普通出现 `DH` 保持静默。

AI 输入只包含当前群、当前消息之前的上下文和已绑定知识片段。Provider 使用持久连接池：
连接 2 秒、单连接生成 15 秒、总预算 20 秒，主连接失败后按健康冷却顺序切换一个备用连接。较快响应会立即返回，上限只用于网络或模型异常时止损。Responses 输出上限为 1024 tokens，Chat Completions 兼容路径保持 512 tokens。
纯 FAQ 文字才允许进入 10 分钟内存缓存；动作、任务、预测、处罚、摘要和上下文依赖
回答不缓存。

### 统一 outbox 与 Dispatcher

回复、预测、欢迎、撤回、禁言、移出、改名、提醒和通知全部写入 `effect_outbox`（schema v14
新增 `priority / lane / order_key / correlation_id / origin / expires_at`），每项带稳定
`dedupeKey`。**Dispatcher** worker 每 tick 最多 claim 4 条，通过 **QueueKernel** 中间件链
后再执行协议调用：

```text
claim(4/tick)
  └─ QueueKernel::run_chain(item, account_id)
       1. ExpiryGuard       — expires_at 超时 → ChainOutcome::Skip
       2. OrderKeyLock      — order_key 非空时获取互斥锁，保证同 key 串行
       3. LaneConcurrencyGate — 同 lane 最多 4 并发，返回 OwnedSemaphorePermit
       → ChainOutcome::Proceed{order_guard, lane_permit}
  └─ 协议调用（持有 guard + permit 直到完成）
  └─ 回执 → actions + audit_events
  └─ DispatchStats 计数器原子更新
```

**DispatchStats** 原子计数器（`dispatched / proceeded / skipped / rejected / chain_micros`）
通过 `RuntimeCoordination.dispatch_stats` 在整个运行时共享，不需要锁；快照由
`DispatchStatsSnapshot` 序列化后进入诊断包。

`DatabaseExecutor::claim_due_task_reminders` 仅由 `repository.rs` 测试使用，运行时路径
通过 `finish_task_reminder` / `mark_task_reminder_unknown` 完成提醒生命周期。

固定执行顺序：

```text
权限/群归属 → capability → 参数校验 → 协议调用 → 回读验证 → 回执 → 审计
```

回读失败或 transport 结果不确定时标记”待人工确认”，不自动补发。账号级外部写队列
保持至少 500ms 间隔；这是协议保护，不是规则冷却。

### 运行任务与并发模型

`RuntimeWorkTracker` 以账号、群和任务类型的哈希通道聚合运行任务。快照只含标签、状态、
进度、后续数量和脱敏错误；事件最多每 100ms 发出一次。可测任务报告步骤百分比，不可测
网络任务使用不定进度。成功项保留 5 秒，失败和未知项由前端关闭只读抽屉时确认清除。

并发边界固定为：每群消息有序、每群 AI 回复有序、AI/预测网络请求最多 2 个并发、
账号协议写入串行且间隔至少 500ms。数据库继续使用单连接 actor，不对 SQLite 启用并行写。
空闲 worker 等待共享通知并以 30 秒超时兜底；启动时从持久队列恢复显示数量，但使用
`max(已观察数量, 数据库积压)` 消除启动扫描和 worker 同时 claim 的重复计数竞态。

## 6. 业务应用和预测

预测只能从 `BusinessAppRegistry` 路由，不能在消息 worker 中增加
`text.contains("预测")` 分支。`PredictionApp` 执行：

```text
数据源 -> 强类型解析 -> 期号/时间校验 -> 历史去重排序
      -> 频率/遗漏/冷热统计 -> 固定中文模板 -> 可选 AI 润色
```

数据源地址、Token 和原始 JSON 不进入 AI 或群回复。缺少真实开奖时间、凭据、期号或
完整结果时保持不可用；不使用本机当前时间伪造新鲜度。预测应用默认停用，且必须有
当前群 AI `reply` 权限。应用运行以稳定键写入 `business_app_runs`，本地测试不发送群消息。

## 7. 能力探测和权限

能力状态为 `Supported`、`ManualVerification`、`Unavailable`、`Unsupported`，并记录
来源 `ZcgContract`、`WangElectron`、`NimRuntime` 或 `ManualReceipt`。旺商聊版本和主脚本
SHA 仅用于诊断指纹；路由、IPC、NIM 方法和双层响应结构才决定 ZCG 基线能力是否开放。

公告是旺商聊专有能力：只读探测通过后允许管理员首次手工发布，发布后回读公告内容和
消息 ID，确认一致才开放批量/自动流程。其他高影响动作始终再次检查管理权限和能力状态。
协议细节和回执字段见 [`PROTOCOL-CONTRACT.md`](PROTOCOL-CONTRACT.md)。

## 8. 测试和发布门禁

```sh
cd tauri3
pnpm install --frozen-lockfile
pnpm test:contract-sanitizer
pnpm test:production-isolation
pnpm test:production
pnpm test:fixture
pnpm test:ui
pnpm test:e2e:fixture
cargo clippy --manifest-path src-tauri/Cargo.toml --no-default-features --all-targets -- -D warnings
cargo clippy --manifest-path src-tauri/Cargo.toml --features fixture --all-targets -- -D warnings
pnpm verify:docs
```

生产构建必须使用 `--no-default-features`，扫描中不能出现 Fixture、`9233/51300`、
测试数据库、源码、PDB、Source Map 或开发命令。最终生产 EXE 只能在受控 Windows MSVC
开发机生成并做真实旺商聊启动、托盘、登录复用、DPI 和退出残留验收。仓库与发行边界见
[`ISOLATION.md`](ISOLATION.md) 和 [`RELEASE-ARCHITECTURE.md`](RELEASE-ARCHITECTURE.md)。

## 9. 内存边界与本地桥

### 内存边界

所有运行时内存集合都有编译期上限，禁止无限增长：

| 常量 | 值 | 文件 | 作用 |
|---|---|---|---|
| `MAX_GATEWAY_BATCH` | 100 | `gateway.rs` | 单次 CDP 轮询最大事件数 |
| `MAX_MEMBER_CACHE_GROUPS` | 100 | `gateway.rs` | 成员名单 LRU 缓存上限；超出按 `checked_at` 驱逐最旧群 |
| `MAX_MEMBER_EVENT_CACHE` | 500 | `gateway.rs` | 单次成员事件合并上限 |
| `MAX_REPORTED_SET` | 500 | `runtime/mod.rs` | 已报告运行项去重集合上限 |
| `MAX_TRACKED_ITEMS` | 200 | `runtime_work.rs` | `RuntimeWorkTracker` 活跃项上限 |
| `AI_PROVIDER_TIMEOUT` | 15 s | `ai.rs` | 单 AI 连接生成上限（另有 2s 连接超时、20s 总预算） |
| `DEFAULT_RESPONSE_LIMIT` | 20 MiB | `http_body.rs` | 单个远端 HTTP 响应体上限；按 chunk 累加，超限立即返回并断开连接 |

`list_members()` 在写入缓存前检查 `cache.len() >= MAX_MEMBER_CACHE_GROUPS`，驱逐最旧群后再插入，避免因群数量增长导致内存无限累积。

响应体上限必须在读取过程中生效：先 `bytes().await` 或 `.json()` 再检查长度，等于已经把整个响应收进内存，上限形同虚设。`http_body::read_capped_body` 因此按 chunk 累加，一超限立刻返回 `CappedReadError` 并丢弃 response（连接随之关闭）。该模块只决定"读多少"，不决定"怎么报错"——错误码由各调用方映射到自己域内，避免某一域的错误码泄漏到别处。

诊断包的 `memoryCaps` 字段输出以上全部七项供线上核对，键名依次为 `maxMemberEventCache`、`maxReportedSet`、`maxMemberCacheGroups`、`maxTrackedWorkItems`、`maxGatewayBatch`、`aiProviderTimeoutSeconds`、`defaultResponseLimitBytes`。

这些值直接引用常量本身，不重抄字面量：重抄等于同一个值维护两处，改了常量忘了改诊断包时线上会读到旧值而测试仍绿。`memory_caps_report_every_compile_time_constant` 逐字段断言 JSON 与常量相等，因此漏改会在测试期暴露。除键名自带单位的两项（秒、字节）外其余均为计数；`aiProviderTimeoutSeconds` 是时间上限而非内存上限，为保持字段名稳定仍归在 `memoryCaps` 下。

### axum 本地桥

`bridge.rs` 在 `127.0.0.1:51235` 启动轻量 axum 服务（仅限本机回环），让外部工具可以向 outbox 入队副作用或查询诊断状态，而不需要 Tauri IPC：

```rust
struct BridgeState {
    gateway:  Arc<dyn RuntimeGateway + Send + Sync>,
    database: DatabaseExecutor,
}

POST /enqueue  → database.enqueue_effect(EffectOutboxRequest) → EnqueuedEffect
GET  /status   → gateway.diagnose() → DiagnosticSnapshot
```

`BridgeState` 与 `AppState` 共享同一个 `DatabaseExecutor` 和 `RuntimeGateway` 实例，写入的副作用由 Dispatcher 统一消费，不存在第二条写路径。
