# DH BOT 3.0 架构

## 分层

| 层 | 主要代码 | 职责 |
|---|---|---|
| React | `tauri3/src` | 页面、表单、批量选择、逐项结果和离线状态 |
| Tauri command | `tauri3/src-tauri/src/lib.rs` | 强类型参数校验、权限检查、服务编排 |
| Rust service | `runtime.rs`、`moderation.rs`、`ai.rs`、`business_apps.rs` | 消息、规则、AI、业务应用、任务、计划和副作用 |
| 数据 | `database.rs`、`repository.rs` | `DatabaseExecutor` 单线程事务、幂等和审计 |
| 协议 | `gateway.rs` | DevTools/CDP、旺商聊 HTTP 路由与 NIM 调用 |

前端不直接访问 SQLite、密钥或任意协议路由。所有写操作都经过固定枚举 command，再进入权限、能力和参数校验。

## 入站消息

```mermaid
flowchart LR
  A["旺商聊 CDP / NIM"] --> B["RuntimeGateway 有序批次"]
  B --> C["SQLite inbox + message 事务"]
  C --> D["确认连续 bridgeSeq"]
  C --> E["按群串行派发"]
  E --> F["规则 / AI / 任务"]
  F --> G["effect_outbox"]
  G --> H["协议动作"]
  H --> I["回执 + action + audit"]
```

消息以账号、群和服务器消息 ID 去重。副作用使用稳定键去重；回执不确定时标记 `unknown`，进入人工核对流程。

## 批量群控

```mermaid
flowchart LR
  A["当前搜索结果选择群"] --> B["execute_group_batch"]
  B --> C["群 ID 去重和公告长度校验"]
  C --> D["逐群管理权限检查"]
  D --> E["能力状态检查"]
  E --> F["公告 / 全员禁言 / 解除全禁"]
  F --> G["逐群回执、动作和审计"]
  G --> H["成功 / 失败 / 待人工确认"]
```

批量调用按选择顺序执行，单群失败不会阻断后续群。公告使用同一份文本；AI 优化只修改草稿。

## AI、规则与人工操作

- AI 只由明确 `@DH` 或旺商聊提及元数据触发。
- 确定性规则与 AI 自动化独立，默认模板停用，默认动作仅撤回。
- 人工群控必须由使用者点击并确认，不受 AI 开关影响。
- 所有高影响动作在执行前重新检查当前账号、群归属、角色和能力状态。

## 业务应用与预测

```mermaid
flowchart LR
  A["明确 @DH 预测"] --> B["群、AI 回复权限与人工接管检查"]
  B --> C["BusinessAppRegistry 意图路由"]
  C --> D["PredictionApp 拉取并校验数据"]
  D --> E["去重、时间/期号校验与确定性统计"]
  E --> F["AI 仅润色规范化结果"]
  F --> G["文字 effect_outbox"]
  G --> H["business_app_runs 运行记录"]
```

- `BusinessAppRegistry` 是唯一应用入口；运行时不再使用预测专用文本分支。
- `PredictionApp` 默认停用，账号级开启后仅适用于已启用管理、AI 自动化和 `reply` 权限的群。
- AI 只接收规范化统计，不接收数据源地址、认证字段或原始响应；其动作和任务输出在该应用中被忽略。
- 过期、缺期或数据异常时不给候选方向；AI 超时或非法输出时使用固定应用模板。
- `business_app_runs` 以稳定运行键去重，记录数据新鲜度、AI 使用状态、耗时和错误。本地测试不发群消息、不创建任务、不执行群管动作。

## 连接与能力校准

生产版固定连接 `127.0.0.1:9222`。`GatewayCapabilities` 使用 `Supported / Unverified / Unsupported`，并绑定旺商聊文件版本和主脚本 SHA-256。未知版本允许读取，自动写能力保持待校准状态。
