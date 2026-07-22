# DH BOT 工程规范

## Rust 与协议

- Tauri command 使用结构化请求和 `AppResult<T>`，业务层不接受任意路由、JavaScript 或 SQL。
- 外部调用顺序为：参数校验、账号/群校验、权限校验、能力校验、协议调用、回执归档、审计。
- transport 与业务 `code/errno` 分层解析；成功、失败和未知结果均保存脱敏回执。
- `DatabaseExecutor` 独占 SQLite 连接。Tokio、CDP、AI 和网络请求不占用数据库线程。
- 入站消息、outbox、任务提醒和计划执行均使用稳定去重键。
- API Key、Cookie、Token、Authorization、登录数据和原始账号信息不得写入日志或契约。

## React 与交互

- 页面通过 typed client 调用 command，不直接访问数据文件。
- 每页区分 `idle/loading/empty/ready/error/offlineCached`；局部错误不覆盖全站。
- 批量选择必须写明范围。群组页“全选”只处理当前搜索结果。
- 高影响动作必须有明确确认、进行中状态、防重复点击和逐项结果。
- 所有命令按钮使用 `data-help` 大白话说明；图标按钮同时提供可访问名称。
- 按钮高度、6px 以内圆角、焦点、禁用和窄窗口换行遵循统一视觉令牌。
- 100%/125%/150% DPI 下不得出现遮挡、横向内容丢失或按钮文字溢出。

## 测试门禁

```text
pnpm test:ui
pnpm test:e2e:fixture
cargo test --no-default-features
cargo test --features fixture
pnpm test:production-isolation
```

生产发布还需 Windows MSVC、NSIS、portable、Authenticode 和生产包深度扫描。真实写协议更新必须先采集、脱敏、回放 Contract v2，再更新能力注册表。

## 代码评审清单

1. 是否破坏账号、群、成员身份边界。
2. 是否存在重复副作用或不确定回执自动重试。
3. 是否在数据库线程执行网络工作。
4. 是否泄露密钥、会话、原始协议或开发路径。
5. 是否覆盖成功、部分失败、全部失败和未知结果。
6. 是否保持生产/开发编译和数据隔离。
