# DH BOT 3.0 协议与回执契约

本文描述 DH BOT 与旺商聊之间实际依赖的边界。ZCG 只作为协议行为参考，DH BOT 不启动
`xplugin.exe`、不连接 `51234`、不读取 ZCG 登录链，也不携带下注、账单或结算功能。

## 1. 调用链

```text
DH BOT Rust Gateway
  -> http://127.0.0.1:9222/json/list (选择唯一旺商聊页面)
  -> CDP / Electron xclient IPC
  -> 旺商聊页面的 NIM 方法或 HTTP 业务路由
  -> transport envelope + business envelope
  -> GatewayReceipt
  -> SQLite outbox/action/audit
```

生产端点固定为回环地址。DH 不接受前端传入任意 URL 或任意路由；每种操作由强类型
网关方法构造请求。9222 页面不可访问、页面不是旺商聊、NIM 未初始化和其他程序占用端口
必须分别报告，不能统一显示为“协议不支持”。

## 2. ZCG 基线能力

运行时只读探测会检查路由签名、IPC、NIM 方法和双层响应结构。结构一致时，即使旺商聊
主脚本 SHA 不同，也可以开放对应的 ZCG 基线能力：

| DH 能力 | 基线路由/方法 | 写后验证 |
|---|---|---|
| 群列表 | `/v1/group/get-group-list` | 群 ID 和名称可解析 |
| 成员列表 | `/v1/group/get-group-members` | 名单数量、游标和完整性 |
| 成员详情 | `/v1/group/get-group-member-info` | userId/nimId 和角色 |
| 成员禁言 | `/v1/group/set-member-mute` | 回读禁言截止时间 |
| 成员解禁 | `/v1/group/member-mute-cancel` | 回读为可发言 |
| 修改名片 | `/v1/group/set-member-nickname` 或 NIM 更新方法 | 回读新名片 |
| 移出成员 | `/v1/group/remove-group-member` | 回读完整名单 |
| 撤回消息 | NIM `getHistoryMsgs` + `recallMsg` 优先 | 撤回通知 |
| 全群禁言 | `/v1/group/set-group-mute`，`MUTE_MEMBER` | 回读群发言状态 |
| 解除全禁 | `/v1/group/set-group-mute`，`MUTE_NO` | 回读群发言状态 |
| 发送文本 | `/v1/plugins/encode-msg` + NIM `sendCustomMsg` | 稳定消息 ID/出站回显 |

HTTP 撤回路由返回业务码 `1001`（禁止调用）时是永久失败，不进入退避重试；其他成员
消息必须先使用 NIM 历史消息定位，再调用 NIM 撤回。权限失败不能被误判为协议失效。

## 3. 旺商聊专有能力

群公告不是 ZCG 基线能力。旺商聊公告链使用公告列表、添加/编辑路由（`notice-list`、
`add-notice`、`notice-opt`）和 NIM 广播：

1. 读取当前公告和作者/noticeId。
2. 新公告走 `add-notice`；明确选择旧公告编辑时才走 `notice-opt`。
3. 回读内容和公告消息 ID，确认与提交内容一致。
4. 只有首次手工回读成功后，才开放批量或自动公告。

公告历史由本地群页按公告 ID、时间、作者和来源保存展示；旺商聊更新后若字段结构变化，
只读仍可保留，写入降为 `ManualVerification`。

## 4. 能力状态

```text
Supported          已通过结构探测和必要回读，可按权限执行
ManualVerification 可人工确认，后台自动动作关闭
Unavailable        当前页面/路由/NIM 结构无法使用
Unsupported        当前产品明确不开放
```

每项能力还保存来源（ZCG 契约、旺商聊 Electron、NIM 运行时或人工回执）、原因、检查时间、
协议指纹和自动/人工许可。版本号和脚本 SHA 只用于追踪升级，不单独锁死全部能力。

## 5. 回执结构和错误分类

`GatewayReceipt` 至少包含 request ID、操作、状态、transport code、业务 `code/errno/msg`、
消息/对象 ID、是否需要人工确认和脱敏错误。错误按以下类别归档：

| 类型 | 例子 | 处理 |
|---|---|---|
| Transport | 连接失败、超时、HTTP 5xx | 有界退避；未知副作用不自动重试 |
| NimNotReady | 登录后会话尚未初始化 | 保持等待，重新探测 |
| Permission | 当前账号不是管理员 | 逐项失败并提示权限 |
| NotFound | 群、成员或消息不存在 | 永久失败，记录对象 ID 类型 |
| Business | 双层 envelope 的业务错误 | 按错误码分类；1001 撤回永久失败 |
| Unsupported | 路由或方法不存在 | 降级对应能力 |
| Decode | 字段类型/结构变化 | 保留原始诊断摘要，不保存原响应 |
| Unknown | 请求已发但回执不确定 | 人工确认，不自动补发 |

所有写调用遵守账号级顺序队列，间隔至少 500ms。只读群列表缓存 10 秒、成员名单缓存
30 秒；写后验证绕过缓存。相同刷新请求 single-flight 合并，避免频繁调用旺商聊。

## 6. Contract v2 采集和审查

### 两层证据边界

`tauri3/contracts/wangshangliao_capabilities.json` 是按旺商聊版本和脚本指纹归档的人工校准表，
用于记录历史写操作证据；其中某项写能力为 `unverified` 不代表 ZCG 基线不可用，也不会单独
锁住所有群管功能。当前运行时先执行 `ZcgLegacyProfileV1` 的只读路由、Electron IPC、双层
envelope 和 NIM 方法探测，再按能力逐项决定 `Supported`、`ManualVerification` 或
`Unavailable`。公告、公告编辑和其他旺商聊专有写能力仍必须有真实写入后的回读证据。

真实采集只在开发版和本机忽略目录进行，仓库只提交确定性脱敏结果：

```text
tauri3/contracts/raw/        原始轨迹，仅本机
tauri3/contracts/*.json      脱敏 Contract v2，可回放
```

每条轨迹包括应用版本、页面信息、主脚本 SHA、请求、transport/business 双层响应、回调、
规范化回执和预期最终状态。脱敏器会替换账号、群、用户、NIM、消息、公告 ID、URL、会话、
Token、Cookie、Authorization、Base64 协议体和路径；不允许原始密钥进入 Git。

新增或更新旺商聊专有能力必须满足：两组脱敏测试群完整操作、可恢复操作的前后基线、回读一致、
错误场景和人工复核。未满足时只显示 `ManualVerification`，不以 Fixture 结果冒充真实能力。

校验命令和字段要求见 [`tauri3/contracts/README.md`](../tauri3/contracts/README.md)。

## 7. 安全边界

- 不把账号密码、Cookie、Token、API Key、Authorization 或原始协议响应写入日志、数据库或
  Contract。
- 不开放任意协议路由、任意 JavaScript 或远程 DevTools 地址。
- 高影响动作执行前再次检查账号管理权限、群归属、成员身份和能力状态。
- 生产包扫描不得包含 Fixture、`9233/51300`、ZCG 二进制、原始契约、测试数据库、源码、
  PDB、Source Map 或开发命令。
