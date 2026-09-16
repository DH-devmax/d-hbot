# DH BOT 3.0 当前状态

应用版本 `3.0.0-beta.1`，SQLite schema v14。以下描述当前实现范围，不以历史测试数量或完成度百分比代替实际验收。

## 桌面与 Web

- 桌面版使用 Rust/Tauri 和官方客户端网关，Production 与 Fixture 构建隔离。
- Web 使用 `dh-server`，复用 82 个业务命令的校验、权限与审计处理；具有独立管理员会话、页面事件和诊断下载。
- `serve-rust` 已实现业务登录、设备验证码分支、续期、NIM 认证和心跳，以及群列表、成员分页和部分 HTTP 群管接口。
- 纯 Rust 编解码、同步确认、收发、AI 回复、公告、成员禁言/解禁和在线撤回已有真实双账号验证。
- 全群禁言、移除/拉黑、定时任务、离线撤回与长期运行仍待验收；群自动执行开关已恢复关闭。
- 最新结果见 [阶段验收](../docs/acceptance/2026-09-17/RESULTS.md)。后续私用开发转向 [AstrBot](../docs/ASTRBOT-HANDOFF.md)，插件尚未实现。

## 文档入口

- [Web 配置、登录和接口范围](../docs/RUST-WEB-LOGIN.md)
- [协议契约与研究证据](../docs/PROTOCOL-CONTRACT.md)
- [Windows 验收](../docs/WINDOWS-ACCEPTANCE.md)
- [工程检查与发布要求](../docs/ENGINEERING-STANDARDS.md)

旧 Go 2.7 归档已从工作目录移除，可在 Git 历史中查阅。现有生产数据、数据库迁移和脱敏协议向量继续保留。
