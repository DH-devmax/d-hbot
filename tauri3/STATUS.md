# DH BOT 3.0 beta.1 状态

当前 Rust + Tauri 重构的代码闭环约 **99%**，考虑尚未获得的 Windows 实机、Authenticode 和真实旺商聊契约证据，有效完成度约 **97%**。Beta.1.1 核心收口、生产/Fixture 编译隔离、业务页面与自动化测试已落地；剩余百分比由 Windows 发布闭环、真实旺商聊契约校准和实机 RC 验收解锁。

## 已完成

- schema v6 幂等迁移：`actions.receipt_json`、`effect_outbox.receipt_json`、有序 `gateway_inbox`、统一副作 outbox、AI/摘要运行状态、知识分块、任务提醒、动作去重以及账号隔离的业务应用/运行记录。
- `DatabaseExecutor` 独占 `Database::open` 交付的唯一 SQLite 连接，使用 `dh-sqlite` 线程、256 有界队列、FIFO 排空和 `Shutdown` 回执。Tauri/runtime 生产路径不再持有可直接调用的同步数据库。
- 连接、消息、名片、规则、提醒、摘要、计划和诊断桥 worker 统一监督；退出最多等待 5 秒，未确认副作归档为 `unknown`，再排空 SQLite。
- 消息链路固定为“批量持久化 → 连续 ACK → 按群串行派发”；重复、乱序、重试、重启恢复、解码失败和未知回执可追踪。
- ACK 失败重试会重新确认旺商聊源队列，源 ACK 与本地状态由单一 SQLite 事务收口；更低序号未确认时禁止越过处理后续事件。
- 成员身份合并、完整/部分名单、入群/离群/资料变更、陌生发言发现、黑名单回群、注销/封禁状态和群名片重试已持久化。
- 规则冷却、时间窗、图片边界、角色/成员豁免、多规则贡献者、语义阈值、固定动作顺序和成功后冷却已接通。
- AI 仅由明确 `@DH` 或提及元数据触发；总开关、回复/任务/撤回/禁言/移出权限、最近上下文、知识分块和 `ai_runs` 已接通。
- 预测已从消息硬编码分支迁入 `BusinessAppRegistry`；默认停用，严格要求 AI `reply` 权限，完成数据新鲜度检查、确定性统计、AI 纯文字润色、模板回退与运行去重。
- 任务提醒持久 claim、每日摘要、电脑时区、跨午夜/DST 开关群计划和计划历史已完成。
- Contract v2 包含版本/主脚本哈希、请求、双层响应、回调、标准化状态和 `Supported/Unverified/Unsupported` 能力校准；原始轨迹目录被 Git 忽略，仓库只接受确定性脱敏契约。
- 生产 `CdpGateway` 使用独立能力注册表；未知版本的发送、撤回、禁言、改名、移出和全群发言均为 `Unverified`，真实写入口统一阻断，调试页逐项显示校准状态。Fixture 校准表不会编入生产包。
- 生产版固定真实旺商聊 `127.0.0.1:9222`；Fixture 只在 `fixture` feature 与内部开发包出现，使用独立数据目录和端口。
- Windows 进程身份含 PID、规范路径、创建时间、文件版本和 SHA-256；关闭前三重复核。固定分区补丁使用结构白名单、manifest、原子替换、验证和回滚，并支持同一 EXE 的 UAC 维护模式。
- 旺商聊路径优先读取用户设置，其次检索注册表、常见安装目录和开始菜单；自动启动默认开启。已运行但没有 9222 时只对精确主进程弹出确认，Electron 子进程被排除；UAC 维护成功后自动续跑启动。
- 自动启动 worker 每 10 秒监督 DevTools；旺商聊主进程退出后自动重启，主进程仍在但没有 DevTools 时只提示确认重启，不会自动结束现有进程。
- 启动状态持久在 `AppState`，前端注册监听后主动领取最近状态并按事件 ID 去重，避免旺商聊快速启动时丢失确认或结果。启动失败使用中文原生错误框，不再直接 panic。
- 9222 只接受带旺商聊标识的 DevTools 页面；其他程序占用端口时不结束任何进程。自动与人工启动共用互斥锁，确认重启遇到 UAC 时保留原进程，维护完成后自动续跑。
- 托盘单击/双击恢复、动态暂停/恢复文案、关闭选择记忆和真正退出已接通；普通最小化保留在 Windows 任务栏，只有右上角关闭才进入托盘确认。`tray-icon 0.24.1` 的 Windows 实现原生处理 `TaskbarCreated`。
- React 页面、事件刷新、离线缓存、批量部分失败、关闭对话框和开发 Fixture Playwright 工作流已覆盖。
- 消息台的群、关键词、类型、处理状态和游标均由 SQLite 查询，页面按 30 条真实分页，不再一次读取 500/1000 条后在前端裁切。
- 成员批量禁言、解禁、移出、加入/移出黑名单统一返回逐项结果；注销状态参与失效成员清理，单个成员失败不会中断后续成员。
- 群组页已增加批量公告、批量全员禁言和批量解除全禁。选择范围固定为当前搜索结果，后端按群校验权限与能力，逐群归档回执和审计；失败群可重试，未知回执进入人工核对。
- 开发 Fixture 默认提供两个隔离测试群，支持同一公告跨群写入与全群发言状态维护。生产版仍不包含 Fixture 入口、端口或测试数据。
- 规则编辑器已接通全部 matcher、窗口、次数、语义阈值、角色豁免和成员白名单；知识库及文档启停、审计成员/事件筛选与过滤后导出已接通。
- 运行时可注入时钟、事件、AI、语义分类和预测数据源；真实浏览器 headless 测试完整经过 CDP、NIM、Runtime、SQLite、outbox、动作及审计。
- 当前 Tauri 界面已重新截图，并生成 11 页 A4 横向中文图解 PDF。
- Go 2.7 代码归档到 `go-2.7-final` 标签；当前 Rust 分支已移除 Go 构建入口和源码。

## 当前验证

- `cargo test --no-default-features`：127 项（含 schema v1/v4/v6、损坏库原文件保护、ACK 事务回滚、序号阻塞、并发 claim、执行器排空、回执脱敏、数据库消息筛选分页、业务应用默认值与去重、仅预测命令命中、端口占用识别、生产能力校准、自动启动和关闭偏好校验）。
- `cargo test --features fixture`：128 项 + 2 项真实浏览器 CDP 集成测试，包含双群公告、双群全员禁言/解除、1000 条突发、101 条分批、业务应用注册与保守回退、完整 Runtime 副作用链和版本/脚本哈希校准。
- 两套 `cargo clippy --all-targets -- -D warnings` 通过。
- `pnpm test`：14 个文件、36 项 RTL/Vitest 通过，包含业务应用卡片、数据源健康检查、离群测试、批量公告当前搜索结果全选、部分失败保留、未知回执人工确认、确认重启后等待 UAC 并续跑、规则完整字段、知识启停、审计筛选和消息游标分页回归测试。
- `pnpm test:e2e:fixture`：开发 Fixture 核心流程通过，覆盖成员搜索、消息、规则、知识绑定、任务、计划、审计和调试页的实际 IPC 调用。
- Contract v2 采集组装与脱敏器 9 项测试及自检，可在多 DevTools 页面中唯一选择旺商聊；生产/开发构建边界、9 场景生产隔离扫描、前端生产扫描、Rust release 构建和两套文档目录扫描通过。
- `DH-Manual-ZH.pdf`：11 页，全页重新渲染为 PNG 并通过联系表视觉检查。
- 本机 macOS 未安装 `x86_64-pc-windows-msvc` 所需的 MSVC/C 头文件，也未安装 MinGW；Windows GUI 交叉检查因此停在 `ring` 工具链准备阶段，需由 Windows MSVC/GitHub Runner 生成新产物后完成实机烟测。
- macOS Tauri 桌面包已连接真实旺商聊 2.6.3 的 `127.0.0.1:9222`；能识别登录路由和 `nim-not-ready`，本地 Rust 诊断桥正常返回。关闭窗口的“取消 / 挂到托盘 / 退出”确认框与后台进程存活已通过 Computer Use 实测。
- 使用 `cargo-xwin`、Windows CRT/SDK 和 MSVC Rust target 完成生产及 Fixture 全目标静态编译检查与 release PE 链接；过程中修正了 `windows-sys 0.59` 的 DPAPI blob 与 `LocalFree` 绑定。生产主程序已确认为 `IMAGE_SUBSYSTEM_WINDOWS_GUI`，发布扫描器会拦截会显示 CMD 的 CUI 构建。
- 已用当前代码生成本地未签名 `DH-BOT-3.0.0-beta.1-windows-x64-portable-unsigned-rust-final.zip`，解压后生产隔离扫描和包内 SHA-256 校验通过；PE 为 `97d2ca4075ff08573cfcba823ad660925406375b0b026fe830f38ff472b28880`，ZIP 为 `5610866f9fa594a110b1a7ed0011fe501e428f59f3e8a33b176696bd5afb4d4c`，PDF 为 `46522c116eab3be6bdd73b36742b1530c411f810e5d9ab5129885a1972f1007a`。它只用于内部 beta，不替代 NSIS、Authenticode 和 Windows 实机验收。
- Go 2.7 旧架构已生成可重复校验的 `archive/go-2.7-final/DH-BOT-go-2.7-final-source.zip`；当前分支不存在 Go 源码或 Go 构建入口。
- 构建归属已调整为“源码仓库只做本地测试、公开发行仓库唯一生产构建”。源码仓库不再保留生产或 Fixture workflow；正式发布由发行仓库手动输入完整源码 SHA 和标签，强制签名后创建公开 Release。

## Beta.2 / RC 必须由外部环境证明的门禁

- 发行仓库 Windows MSVC 生产编译、NSIS 和 portable ZIP 生成、WebView2 离线包、Windows 产物深度解包扫描。
- Windows 10 22H2 与 Windows 11 23H2/24H2 的标准用户/管理员、安装/升级/卸载/portable、UAC、托盘、休眠恢复和 100%/125%/150% DPI。
- 基于真实旺商聊文件版本与主脚本 SHA-256 的脱敏 Contract v2 采集与回放。2.7.7 群公告已校准；双群全员禁言/解除全禁及其他待验证写能力继续保持 `Unverified`。
- 16 人“大海兼职群”先做只读同步，再做测试消息撤回、短时禁言/立即解禁、临时改名/自动恢复；移出成员仍只在 Fixture 验证。
- 发行仓库正式生产构建的 Authenticode 签名和 `signtool verify`。未配置证书时阻止公开发布。

生产新安装不创建测试群、成员、规则、知识库、计划或自动化开关；Fixture 仅存在于内部开发包。
