# DH BOT 工程规范

## Rust 与协议

- Tauri command 使用结构化请求和 `AppResult<T>`，业务层不接受任意路由、JavaScript 或 SQL。
- 外部调用顺序为：参数校验、账号/群校验、权限校验、运行时能力探测、协议调用、回读验证、回执归档、审计。
- ZCG 基线能力按路由签名和双层 envelope 开放；文件版本与脚本哈希只参与诊断指纹。权限或业务失败不得误判为协议失效，路由缺失、结构变化和解码失败才降为 `Unavailable`。
- 人工与自动执行权限分开判断。`manualAllowed=true` 只开放管理员确认操作；后台 outbox 还必须满足 `automaticAllowed=true`。
- transport 与业务 `code/errno` 分层解析；成功、失败和未知结果均保存脱敏回执。
- `DatabaseExecutor` 独占 SQLite 连接。Tokio、CDP、AI 和网络请求不占用数据库线程。
- 入站消息、outbox、任务提醒和计划执行均使用稳定去重键。
- API Key、Cookie、Token、Authorization、登录数据和原始账号信息不得写入日志或契约。
- 业务应用必须通过 `BusinessAppRegistry` 注册和路由，不得在消息 worker 中增加应用专用文本分支。
- 应用数据先强类型解析和确定性校验，AI 只处理规范化数据并只输出文字；上游地址、认证和原始响应不进入 AI 请求。
- AI Provider 必须由 `AiProviderPool` 创建，业务代码不得按消息新建 HTTP Client。连接、单连接生成和总预算分别为 2 秒、60 秒和 65 秒，主备采用顺序切换而非并发竞速。
- AI 缓存只能保存无动作、无任务的 FAQ 文字。缓存键必须包含账号、群、问题、知识版本、人格版本、模型和 Provider 配置；上下文依赖回答不得缓存。
- 机器规则不得调用 AI Provider；机器规则与 AI 控制规则使用独立群开关、独立评估记录和独立审计来源。AI 控制规则一次请求统一判断全部启用类别。
- v2 规则不使用角色豁免和规则冷却。连续消息逐条处理；协议防频繁由账号级顺序队列的至少 500ms 写间隔承担，不得把协议节流重新实现成规则冷却。
- 规则优先级只接受 `low/medium/high`。同一消息仅执行最高等级规则产生的动作，同等级冲突按固定动作顺序合并，禁言采用较长时长。
- 撤回其他成员消息必须先以服务器消息 ID 调用 `nim.getHistoryMsgs` 精确定位，再调用 `nim.recallMsg` 并验证通知。业务码 `1001` 是永久失败，不得退避重试或宣称成功。
- 开奖适配器必须使用响应中的真实时间。缺少时间、期号、完整三位结果或凭据时返回不可用，不得使用本机当前时间伪造新鲜度。

## 日志与维护支持

- 所有进程日志通过 `diagnostics::Logger` 写入 JSONL；不得绕过该入口写入密钥、Cookie、Token、Authorization、密码、登录资料、原始消息、原始协议响应或用户目录。
- 每条日志必须带 UTC/本地可读时间、会话 ID、递增序号、级别和经过 `redact` 的文字。错误与崩溃日志在写入后调用 `sync_data`，避免正常退出前丢失关键故障线索。
- 日志按日期与 8 MiB 分段，默认最多保留 30 天和 64 MiB；清理逻辑只能匹配 `dh-*.jsonl` 或旧 `dh-*.log`，不得触碰数据库、秘密文件、旺商聊目录或支持包。
- 支持包只能由强类型 `export_support_bundle` 生成。它必须使用临时文件和原子替换，使用互不覆盖的文件名，内含 `manifest.json`、`SHA256SUMS.txt`、脱敏状态、匿名化审计和脱敏日志；不得包含数据库、秘密、Cookie、Token、Electron Profile、原始群消息、PDB、源码或 Fixture 数据。支持包最多保留 20 个、30 天和 128 MiB。维护人员必须用 `tools/verify_support_bundle.py` 在不解压、不执行包内文件的前提下核对路径、清单、大小和哈希。
- 对日志或支持包的任何新增字段，必须补充脱敏和内容边界测试。默认按“可交给维护人员”标准设计，不假设接收者可以看到真实账号、群或成员信息。

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

所有测试和开发构建在开发机执行，私有源码仓库不运行 GitHub Actions。ZCG 基线协议更新需完成路由、参数和 envelope 测试；旺商聊专有写能力仍需采集、脱敏、回放 Contract v2，并通过首次手工回读。

生产发布只在受控 Windows 开发机执行：固定经过本地验收的源码 commit SHA，完成 MSVC/NSIS/portable 构建、生产包深度扫描和真实桌面验收。生产包使用 `--no-default-features`，Fixture 不参与构建或发行。

Windows 实机桌面验收使用 `tauri3/scripts/test-windows-real-machine.ps1`，详细步骤见 [WINDOWS-REAL-MACHINE-TEST.md](WINDOWS-REAL-MACHINE-TEST.md)。该探针只存在于私有源码仓库，不复制到 NSIS、portable 或公开发行仓库。

上传云盘前还必须确认：源码工作树干净、记录完整 commit SHA、应用版本与文件名一致、生产隔离扫描通过、Windows 实机报告通过、`SHA256SUMS.txt` 与待上传文件一致。已有文件使用新版本号，不覆盖旧包。

## 代码评审清单

1. 是否破坏账号、群、成员身份边界。
2. 是否存在重复副作用或不确定回执自动重试。
3. 是否在数据库线程执行网络工作。
4. 是否泄露密钥、会话、原始协议或开发路径。
5. 是否覆盖成功、部分失败、全部失败和未知结果。
6. 是否保持生产/开发编译和数据隔离。
