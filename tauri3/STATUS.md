# DH BOT 3.0 beta.1 状态

当前 Rust + Tauri 迁移线约完成 **84%**：Beta.1 核心正确性与主要运营页面已落地，生产版与开发 Fixture 版保持编译期隔离。

## 已完成

- 3.0 独立数据目录、快照、`quick_check`、DPAPI/开发密钥存储、日志脱敏。
- schema v3 幂等迁移：消息处理状态、成员生命周期字段、任务提醒字段、规则运行状态和动作去重键。
- 消息链路固定为持久化、源确认、按群串行处理；重复消息、失败重试、重启恢复和解码失败消息均可追踪。
- 成员名单基线、完整名单差异、入群/离群/陌生发言发现、黑名单回群处理和群名片锁定基准。
- 规则冷却、时间窗、图片与名片计数、角色/成员豁免、动作顺序和真实消息/规则关联。
- 知识库 CRUD、复制、文档删除、多群绑定和只读保护；任务提醒持久化 claim；每日摘要和开关群计划执行记录。
- 计划使用电脑时区、跨午夜和 DST 计算；审计查询、JSON/CSV 导出；消息查询、批量发送、规则 JSON 导入导出。
- 前端拆分为总览、群组与成员、消息台、规则、知识与 AI、任务与计划、审计、设置；每页独立加载/错误/空状态。
- 生产版固定真实旺商聊 `127.0.0.1:9222`；开发 Fixture 版使用独立 `DH\\fixture`、`9233` 和独立 Tauri 配置。
- Fixture 仅在 `fixture` Cargo feature 中编译，生产前端、生产资源和生产二进制不含测试入口。
- Windows 进程识别、前台恢复、托盘单击/双击、固定登录分区维护接口和可选 Authenticode 签名脚本已建立。

## 验证结果

- `cargo test --no-default-features`：39 项通过。
- `cargo test --features fixture`：39 项通过，含真实 CDP Fixture 契约测试。
- 两套 `cargo clippy -- -D warnings` 通过。
- `pnpm test`：2 项通过；生产/开发 Vite 构建通过。
- 生产前端和生产二进制已完成 Fixture 标记扫描。

## Beta.2 / RC 待办

- Windows MSVC 原生构建、NSIS/portable 实机包、离线 WebView2、UAC 维护烟测和 Authenticode 证书验收。
- 旺商聊真实版本设备契约回放、脚本升级覆盖、固定登录分区迁移和 Windows 10/11/DPI/休眠验收。
- React Testing Library/Playwright 全流程测试、1000 条突发消息、真实 16 人测试群只读与可回滚操作验收。
- Go/Rust 冻结协议 fixture 对照、最新版箭头截图 PDF 手册、生产发布清单和最终 SHA-256。

生产版默认不创建测试群、规则、知识库、任务或审计数据；Fixture 仅作为内部开发包和自动化测试工具。
