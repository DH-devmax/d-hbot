# Rust AI 与群聊验收：阶段结果

测试群：专用测试群（真实名称已移除）。实例 A 为 8787，实例 B 为 8788。

本文为历史快照，后续见 [9 月 17 日验收](../2026-09-17/RESULTS.md)。

## 结论

真实 Rust 双向文本收发、服务端发送确认、对端解码入库、接收 ACK 本地刷出、重启后去重已通过。**完整 AI 闭环尚未验收通过**：提供的模型服务在带认证的 `/v1/models` 请求中返回 HTTP 401，尚无有效模型配置。未执行真实提及回复、连续上下文、知识命中或 AI 回复后的重连去重测试。

两实例已恢复运行，账号会话保留。仅为已验证的当前账号记录收发能力；所有群 AI 开关仍关闭，未执行处罚或跨群发送。没有创建临时知识条目。

## 修复

- 群发送使用 SDK 的 `8/2` 路由；私信发送仍为 `7/1`。
- 实时通知 `4/1、4/2、4/10、4/11` 解开通知 ID 和内部协议头后再解析消息，拒绝嵌套通知。
- 成功发送确认按路由和请求序号关联，接受仅含时间与服务端消息 ID 的成功包；如回传客户端 ID，仍校验其一致性。
- 应用加密文本通过原生解码器离线互操作测试；业务群、发送者与 NIM 标识通过当前账号的群和成员映射绑定。
- 接收先落库、再 ACK；处理或保存失败关闭当前世代连接，从上次同步边界恢复。
- 发送前持久化唯一客户端 ID 与 nonce；结果未知不自动重发。本地确认保存失败按未知结果处理。
- 收发能力绑定账号世代，切换账号后不继承上一账号的已验证状态。
- 接收事件接入现有持久化处理队列；本人回显不进入 AI。会话页面区分处理状态、AI 状态、发送状态和接收 ACK。

## 真实用例

每条消息都以 `DH BOT 测试` 开头，且没有提及机器人。

| 用例 | 方向 | 服务端发送结果 | 对端记录 | 说明 |
| --- | --- | --- | --- | --- |
| 首次 T01 尝试 | B → 群 | rejected | 无 | 修复群发送路由前被拒绝 |
| T01 | B → A | unknown | 收到、解码、ACK flushed | 旧确认解析器未接受精简确认包；未自动重发，重同步后恢复接收 |
| T02 | B → A | unknown | 实时收到、解码、ACK flushed | 确认包 HTTP 无关，NIM code=200，属性仅 7 和 12；未自动重发 |
| T03 | A → B | sent | 实时收到、解码、ACK flushed | 修复确认解析器后通过 |
| T04 | B → A | sent | 实时收到、解码、ACK flushed | 修复确认解析器后通过 |

两实例重启后，T01–T04 每实例每条仅一条账本记录；入站处理次数均为 1，本人回显为 ignored。AI 调用数均为 0。此结果证明普通消息静默和接收去重，**不替代启用 AI 后的未提及静默测试，也不证明 AI 回复去重**。

`flushed` 仅表示接收确认已写入套接字，不表示远端用户已读。`sent` 仅表示服务端确认；T03/T04 的对端收到由另一实例的独立记录证明。旧 unknown 记录保留，不篡改为 sent。

脱敏证据：[live-results.json](live-results.json)。本地完整备份与研究证据在 `/tmp/dh-ai-acceptance-20260916`，不打包业务密钥、会话或原始群消息。

## 验证命令与结果

- `cargo test --manifest-path crates/dh-protocol/Cargo.toml --quiet`：38 个单元测试和 1 个集成测试通过，退出 0。
- `cargo test --manifest-path crates/dh-server/Cargo.toml --quiet`：24 项通过，退出 0。
- `cargo test --manifest-path crates/dh-core/Cargo.toml --quiet`：218 项通过，1 项真实模型测试忽略，退出 0。
- 前端测试：95 项通过；Web 构建通过。
- `node tools/verify_nim_header.mjs`：固定哈希 SDK 的头、属性、登录、群路由和嵌套通知证据通过，退出 0。
- `cargo run --quiet --manifest-path crates/dh-protocol/Cargo.toml --example message_vector` 生成合成向量，再运行 `tools/verify_message_envelope.py`：原生解码器对 Rust 生成消息的认证、LZ4、路由绑定通过。
- `dh-server probe-send DATA TEST_GROUP TEXT`：T03、T04 的字面输出均为 `probe_send status=sent; provider acceptance is not peer receipt`，退出 0。
- 管理员认证后，会话历史接口返回 4 条测试消息，状态为 processed / ignored，ACK 为 flushed；退出接口返回 204。

临时检查脚本曾错误地将退出接口的空响应解析成 JSON；已修正脚本并确认 204，这是检查脚本问题。

## 待完成

1. 保存有效模型服务、密钥和模型名，通过实际模型调用。
2. 仅启用 A 的测试群 AI，验证未提及静默、明确提及回复、连续上下文、当前群知识命中和资料缺口。
3. 核对 AI 请求、生成、原群发送确认及 B 对端接收证据。
4. 再次重连、重启，验证真实 AI 回复没有重复发送；恢复测试开关和临时知识。

已有数据库在线备份和 6 个主要源文件的哈希基线。`baseline-six-files.patch` 只覆盖这 6 个文件，不能当成整个脏工作区的完整回退。未回滚业务数据库，测试审计记录保留。
