# DH BOT 3.0

DH BOT 是使用 Rust、Tauri v2 和 React 构建的 Windows 旺商聊 AI 群管理工作台。

3.0 代码位于 [`tauri3/`](tauri3/)，Rust/Tauri 是当前唯一活动架构。正式程序不包含 Go 运行时、Go sidecar 或 Go 构建入口。旧 Go 2.7 实现只作为只读架构参考，可通过 `go-2.7-final` 标签或 [`archive/go-2.7-final/`](archive/go-2.7-final/) 中的源码 ZIP 查阅，不参与任何生产构建。

## 产品范围

- 多群组与成员同步，以 `groupId` 识别群，以 `userId` / `nimId` 识别成员。
- 确定性群管规则、群名片、黑名单、知识库、AI 回复、任务、每日摘要和定时开关群。
- AI 群回复只由明确 `@DH` 或旺商聊提及元数据触发。
- SQLite schema v5，有序 inbox、幂等 outbox、动作回执归档、重启恢复和完整审计。
- Windows 托盘、单实例、旺商聊 DevTools 启动、固定登录分区和 UAC 维护流程。

## 构建通道

生产版固定连接 `127.0.0.1:9222`，编译时移除所有 Fixture 入口：

```sh
cd tauri3
pnpm install --frozen-lockfile
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

## 目录

- `tauri3/src-tauri/`：Rust 核心、CDP/NIM 网关、SQLite、规则、AI 和 Windows 桌面能力。
- `tauri3/src/`：React + TypeScript 运营界面。
- `tauri3/contracts/`：版本化、脱敏后的旺商聊 Contract v2。
- `package/DH-BOT-Default-Rules.json`：DH BOT 默认群管规则模板（兼容 ZCG 规则行为）。
- `docs/DH使用手册.md`：中文使用手册。
- `docs/DH-Manual-ZH.pdf`：当前 Tauri 界面图解手册。
- `docs/ARCHITECTURE.md`：React、Tauri、Rust、SQLite 与 CDP/NIM 数据流。
- `docs/FEATURE-CATALOG.md`：功能入口、命令、默认开关、权限和能力状态。
- `docs/ENGINEERING-STANDARDS.md`：协议、数据库、前端交互和测试规范。
- `docs/ISOLATION.md`：生产/开发、端口、数据、凭据和发布仓库隔离。
- `archive/go-2.7-final/`：旧 Go 2.7 只读源码 ZIP、说明和 SHA-256；当前工作树不保留 Go 源码。

## 数据与发布

- 生产数据：`%APPDATA%\DH\3.0`
- 开发 Fixture 数据：`%APPDATA%\DH\fixture`
- 核心源码仓库保持私有；公开下载统一发布到 [`d-hbot-releases`](https://github.com/sh492773746/d-hbot-releases)。
- 正式标签发布必须配置 Authenticode 证书。
- 未签名的 CI 产物只作内部 beta，同时生成 SHA-256 校验文件。

发布仓库边界、Deploy Key 和 Actions 投递流程见 [`docs/RELEASE-ARCHITECTURE.md`](docs/RELEASE-ARCHITECTURE.md)。

详细进度和验收边界见 [`tauri3/STATUS.md`](tauri3/STATUS.md)。
