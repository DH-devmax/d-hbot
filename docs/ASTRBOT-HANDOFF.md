# AstrBot 交接

## 仓库边界

- `DH-devmax/d-hbot`：公开的历史工作台、Rust 协议及业务适配参考。
- `DH-devmax/AstrBot`：后续私用开发 fork；上游为 `AstrBotDevs/AstrBot`。
- 两者使用独立 checkout、数据目录和凭据，不删除仍运行的 DH 数据。

## 目标分工（尚未实现）

Telegram 使用 AstrBot 自带适配。旺商聊需要平台适配插件和经过认证的 Rust 桥，
不必在 Python 中重写业务协议。AstrBot 管理模型、人格、会话与知识检索；Rust 保留登录、
NIM、权限校验、群管动作和审计。现有 Web 管理 Cookie 接口不等于已完成插件桥协议。

桥需实现事件订阅、断点续传、持久化去重、发送回执与状态查询。身份键必须包含平台、账号、群；
模型不得生成群或成员标识。发送结果未知先核对回执，不自动重试。
群管动作继续受管理员权限和确定性规则约束，插件不开放任意协议路由。

## 迁移顺序

1. 固定 fork 基线，在独立分支开发；保留上游 remote。
2. 定义版本化桥契约与夹具，先打通单账号、单群接收和文本回复。
3. 验证明确提及、本人回显过滤、重连恢复、重复事件、未知回执与账号隔离。
4. 迁移知识原始文档并重新索引，按群绑定；不复制旧向量、密钥或全部聊天历史。
5. 切换群时关闭 DH AI，确保只有一个回复方，保留人工接管。
6. 单独验收 Telegram 与旺商聊，默认不跨平台共享记忆；通过后再扩大范围。

回退：停用新插件并确认停止消费，再恢复原群的 DH AI 配置，避免双重回复。
本次仓库交接不切换真实账号，不声称插件已经上线。

## 原适配参考

- `crates/dh-protocol/src/nim_client.rs`：连接、回执关联与管理动作。
- `crates/dh-server/src/rust_gateway.rs`：成员权限、群管写入与回读。
- `crates/dh-server/src/conversations.rs`：收发持久化与会话账本。
- `tauri3/src-tauri/src/ai_pipeline.rs`：提及触发、上下文和队列。
- `tauri3/src-tauri/src/command_support.rs`：共享权限与结果归档。

参考：[AstrBot](https://github.com/AstrBotDevs/AstrBot)、
[Telegram](https://docs.astrbot.app/platform/telegram.html)、
[平台适配](https://github.com/AstrBotDevs/AstrBot/wiki/zh-dev-plugin-platform-adapter)。
