# DH BOT 工程规范

## Rust 与协议

- Tauri command 使用结构化请求和 `AppResult<T>`，业务层不接受任意路由、JavaScript 或 SQL。
- 外部调用顺序为：参数校验、账号/群校验、权限校验、能力校验、协议调用、回执归档、审计。
- transport 与业务 `code/errno` 分层解析；成功、失败和未知结果均保存脱敏回执。
- `DatabaseExecutor` 独占 SQLite 连接。Tokio、CDP、AI 和网络请求不占用数据库线程。
- 入站消息、outbox、任务提醒和计划执行均使用稳定去重键。
- API Key、Cookie、Token、Authorization、登录数据和原始账号信息不得写入日志或契约。
- 业务应用必须通过 `BusinessAppRegistry` 注册和路由，不得在消息 worker 中增加应用专用文本分支。
- 应用数据先强类型解析和确定性校验，AI 只处理规范化数据并只输出文字；上游地址、认证和原始响应不进入 AI 请求。

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
pnpm install --frozen-lockfile
pnpm test:contract-sanitizer
pnpm test:production-isolation
cargo test --no-default-features
cargo test --features fixture
pnpm test:ui
pnpm test:e2e:fixture
cargo clippy --no-default-features --all-targets -- -D warnings
```

所有测试和开发构建在开发机执行，私有源码仓库不运行 GitHub Actions。真实写协议更新必须先采集、脱敏、回放 Contract v2，再更新能力注册表。

生产发布由 `DH-devmax/d-hbot-releases` 唯一执行：管理员手动输入经过本地验收的完整源码 commit SHA 与版本标签，Windows Runner 复跑生产安全门禁、MSVC/NSIS/portable 构建、Authenticode 和生产包深度扫描。发行 workflow 不运行 Fixture，也不接受可变源码引用。

Windows 实机桌面验收使用 `tauri3/scripts/test-windows-real-machine.ps1`，详细步骤见 [WINDOWS-REAL-MACHINE-TEST.md](WINDOWS-REAL-MACHINE-TEST.md)。该探针只存在于私有源码仓库，不复制到 NSIS、portable 或公开发行仓库。

触发生产发布前还必须确认：源码工作树干净、commit 已推送到 `DH-devmax/d-hbot`、当前 `gh` 账号为 `DH-devmax`、发行仓库三个 Secrets 已配置、公开标签与应用版本一致。生产 Release 已存在时使用新版本号，不覆盖旧资产。

## 代码评审清单

1. 是否破坏账号、群、成员身份边界。
2. 是否存在重复副作用或不确定回执自动重试。
3. 是否在数据库线程执行网络工作。
4. 是否泄露密钥、会话、原始协议或开发路径。
5. 是否覆盖成功、部分失败、全部失败和未知结果。
6. 是否保持生产/开发编译和数据隔离。
