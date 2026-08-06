# DH BOT 3.0

DH BOT 是使用 Rust、Tauri v2 和 React 构建的 Windows 旺商聊 AI 群管理工作台。

当前源码技术快照：应用版本 `3.0.0-beta.1`，SQLite schema v13，生产端点
`127.0.0.1:9222`，默认 AI 模型 `deepseek-v4-pro`。

3.0 代码位于 [`tauri3/`](tauri3/)，Rust/Tauri 是当前唯一活动架构。正式程序不包含 Go 运行时、Go sidecar 或 Go 构建入口。旧 Go 2.7 实现只作为只读架构参考，可通过 `go-2.7-final` 标签或 [`archive/go-2.7-final/`](archive/go-2.7-final/) 中的源码 ZIP 查阅，不参与任何生产构建。

## 产品范围

- 多群组与成员同步，以 `groupId` 识别群，以 `userId` / `nimId` 识别成员。
- 确定性群管规则、群名片、黑名单、知识库、AI 回复、任务、每日摘要和定时开关群。
- AI 群回复只由明确 `@DH` 或旺商聊提及元数据触发。
- SQLite schema v13，有序 inbox、幂等 outbox、动作回执归档、重启恢复和完整审计。
- Windows 托盘、单实例、旺商聊 DevTools 启动、固定登录分区和 UAC 维护流程。

## 本地构建与测试

私有源码仓库不运行 GitHub Actions。生产、Fixture 和界面测试均在开发机完成；生产版固定连接 `127.0.0.1:9222`，编译时移除所有 Fixture 入口：

```sh
cd tauri3
pnpm install --frozen-lockfile
pnpm test:contract-sanitizer
pnpm test:production-isolation
pnpm test:production
pnpm test:ui
pnpm tauri:build:production
```

内部开发版才包含 Fixture，使用独立端口和数据目录：

```sh
cd tauri3
pnpm test:fixture
pnpm test:e2e:fixture
pnpm tauri:build:developer
```

提交前还需执行 `pnpm verify:docs`，核对版本、schema、文档链接、凭据和真实身份脱敏。

Windows 开发机在提交生产发布前还需执行生产打包、深度扫描和实机探针。完整顺序见 [`docs/ENGINEERING-STANDARDS.md`](docs/ENGINEERING-STANDARDS.md)。

## 目录

- `tauri3/src-tauri/`：Rust 核心、CDP/NIM 网关、SQLite、规则、AI 和 Windows 桌面能力。
- `tauri3/src/`：React + TypeScript 运营界面。
- `tauri3/contracts/`：版本化、脱敏后的旺商聊 Contract v2。
- `package/DH-BOT-Default-Rules.json`：DH BOT 默认群管规则模板（兼容 ZCG 规则行为）。
- `docs/DH使用手册.md`：中文使用手册。
- `docs/DH-Manual-ZH.pdf`：当前 Tauri 界面图解手册。
- `docs/DIAGNOSTICS-AND-SUPPORT.md`：日志、诊断包、隐私边界与用户上报说明。
- `tools/verify_support_bundle.py`：维护人员只读校验诊断 ZIP 的路径、清单、大小和 SHA-256。
- `docs/ARCHITECTURE.md`：React、Tauri、Rust、SQLite 与 CDP/NIM 数据流。
- `docs/TECHNICAL-DESIGN.md`：模块地图、消息流水线、执行器、outbox、AI 和发布门禁。
- `docs/API-REFERENCE.md`：生产 Tauri commands、事件通道、权限和开发命令隔离边界。
- `docs/DATABASE-SCHEMA.md`：schema v13 表、身份约束、状态恢复和备份边界。
- `docs/PROTOCOL-CONTRACT.md`：ZCG 基线、旺商聊专有协议、能力探测和回执错误分类。
- `docs/FEATURE-CATALOG.md`：功能入口、命令、默认开关、权限和能力状态。
- `docs/ENGINEERING-STANDARDS.md`：协议、数据库、前端交互和测试规范。
- `docs/ISOLATION.md`：生产/开发、端口、数据、凭据和发布仓库隔离。
- `docs/WINDOWS-TEST-HANDOFF.md`：Windows 协作者可直接执行的当前上下文、测试顺序和结果模板。
- `docs/WINDOWS-CODEX-PROMPT.md`：可直接交给 Windows Codex 的验收提示词。
- `tools/verify_docs.py`：核对版本、schema 与 Markdown 相对链接。
- `tools/verify_repository_redaction.py`：扫描 Git 跟踪内容和归档 ZIP 的凭据与真实身份残留。
- `archive/go-2.7-final/`：旧 Go 2.7 只读源码 ZIP、说明和 SHA-256；当前工作树不保留 Go 源码。

## 数据与发布

- 生产数据：`%APPDATA%\DH\3.0`
- 开发 Fixture 数据：`%APPDATA%\DH\fixture`
- 核心源码仓库保持私有，GitHub Actions 停用；生产构建与测试只在受控 Windows 开发机执行。
- 个人发行采用未签名便携 ZIP，随包提供 `SHA256SUMS.txt`，通过管理员公布的云盘链接分发。
- [`DH-devmax/d-hbot-releases`](https://github.com/DH-devmax/d-hbot-releases) 仅作为可选下载说明或历史索引，不构建程序、不保存源码和 Fixture。
- Windows 首次启动可能显示“未知发布者”或 SmartScreen 提示，这是当前未签名个人发行的预期状态。

本地发布步骤见 [`docs/LOCAL-RELEASE.md`](docs/LOCAL-RELEASE.md)，仓库边界见 [`docs/RELEASE-ARCHITECTURE.md`](docs/RELEASE-ARCHITECTURE.md)。

详细进度和验收边界见 [`tauri3/STATUS.md`](tauri3/STATUS.md)。
