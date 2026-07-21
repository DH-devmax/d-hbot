# DH BOT 3.0 beta.1 状态

当前 Rust + Tauri 重构的有效完成度约 **96%**。Beta.1.1 核心收口、生产/Fixture 编译隔离、主要界面与自动化测试已落地；剩余百分比只由 Windows MSVC 产物、真实旺商聊契约校准和实机 RC 验收解锁。

## 已完成

- schema v5 幂等迁移：`actions.receipt_json`、`effect_outbox.receipt_json`、有序 `gateway_inbox`、统一副作 outbox、AI/摘要运行状态、知识分块、任务提醒和动作去重。
- `DatabaseExecutor` 独占 `Database::open` 交付的唯一 SQLite 连接，使用 `dh-sqlite` 线程、256 有界队列、FIFO 排空和 `Shutdown` 回执。Tauri/runtime 生产路径不再持有可直接调用的同步数据库。
- 连接、消息、名片、规则、提醒、摘要、计划和诊断桥 worker 统一监督；退出最多等待 5 秒，未确认副作归档为 `unknown`，再排空 SQLite。
- 消息链路固定为“批量持久化 → 连续 ACK → 按群串行派发”；重复、乱序、重试、重启恢复、解码失败和未知回执可追踪。
- 成员身份合并、完整/部分名单、入群/离群/资料变更、陌生发言发现、黑名单回群、注销/封禁状态和群名片重试已持久化。
- 规则冷却、时间窗、图片边界、角色/成员豁免、多规则贡献者、语义阈值、固定动作顺序和成功后冷却已接通。
- AI 仅由明确 `@DH` 或提及元数据触发；总开关、回复/任务/撤回/禁言/移出权限、最近上下文、知识分块和 `ai_runs` 已接通。
- 任务提醒持久 claim、每日摘要、电脑时区、跨午夜/DST 开关群计划和计划历史已完成。
- Contract v2 包含版本/主脚本哈希、请求、双层响应、回调、标准化状态和 `Supported/Unverified/Unsupported` 能力校准；原始轨迹目录被 Git 忽略，仓库只接受确定性脱敏契约。
- 生产版固定真实旺商聊 `127.0.0.1:9222`；Fixture 只在 `fixture` feature 与内部开发包出现，使用独立数据目录和端口。
- Windows 进程身份含 PID、规范路径、创建时间、文件版本和 SHA-256；关闭前三重复核。固定分区补丁使用结构白名单、manifest、原子替换、验证和回滚，并支持同一 EXE 的 UAC 维护模式。
- 托盘单击/双击恢复、动态暂停/恢复文案、最小化通知、关闭选择记忆和真正退出已接通。`tray-icon 0.24.1` 的 Windows 实现原生处理 `TaskbarCreated`。
- React 页面、事件刷新、离线缓存、批量部分失败、关闭对话框和开发 Fixture Playwright 工作流已覆盖。
- 当前 Tauri 界面已重新截图，并生成 11 页 A4 横向中文图解 PDF。
- Go 2.7 代码归档到 `go-2.7-final` 标签；当前 Rust 分支已移除 Go 构建入口和源码。

## 当前验证

- `cargo test --no-default-features`：93 项（含 schema v1/v4/v5、损坏库、并发 claim、执行器排空和未知副作）。
- `cargo test --features fixture`：94 项 + 1 项 CDP 集成测试，包含 1000 条突发和 101 条分批。
- 两套 `cargo clippy --all-targets -- -D warnings`通过。
- `pnpm test`：7 个文件、15 项 RTL/Vitest 通过。
- `pnpm test:e2e:fixture`：开发 Fixture 核心流程通过。
- Contract v2 脱敏器、生产/开发构建边界、前端生产扫描、Rust release 构建和两套文档目录扫描通过。
- `DH-Manual-ZH.pdf`：11 页，全页重新渲染为 PNG 并通过联系表视觉检查。

## Beta.2 / RC 必须由外部环境证明的门禁

- Windows MSVC CI 编译、NSIS 和 portable ZIP 生成、WebView2 离线包、Windows 产物深度解包扫描。
- Windows 10 22H2 与 Windows 11 23H2/24H2 的标准用户/管理员、安装/升级/卸载/portable、UAC、托盘、休眠恢复和 100%/125%/150% DPI。
- 基于真实旺商聊文件版本与主脚本 SHA-256 的脱敏 Contract v2 采集与回放。未校准版本继续保持 `Unverified` 并关闭自动写操作。
- 16 人“大海兼职群”先做只读同步，再做测试消息撤回、短时禁言/立即解禁、临时改名/自动恢复；移出成员仍只在 Fixture 验证。
- 正式标签发布的 Authenticode 签名和 `signtool verify`。未配置证书的构建仅作内部 beta。

生产新安装不创建测试群、成员、规则、知识库、计划或自动化开关；Fixture 仅存在于内部开发包。
