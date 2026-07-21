# DH BOT 2.7.0

DH BOT 是使用 Go 和原生 Win32 构建的旺商聊 AI 群管理工具。主程序不含
Electron、WebView、易语言或额外运行时，界面使用 Win32/GDI 与 Common
Controls，品牌图标来自 `https://auth.daha6.cloud/icon.svg`。

## 产品范围

- 多群同步，只有管理员明确启用的群会处理消息。
- 群身份使用 `groupId`；成员优先使用 `userId`，详情缺失时使用 `nimId`，昵称只用于显示。
- AI 问答、群规、FAQ、TXT/Markdown 知识库、群摘要和任务提醒。
- 关键词、精确、前缀、正则、长度、行数、图片次数、改名次数、黑名单和
  AI 语义分类规则。
- 撤回、禁言、解禁、恢复名片、移出、黑名单和全员禁言的强类型协议接口。
- 人工接管、全局暂停、观察模式、冷却、优先级、角色豁免和成员白名单。
- SQLite 消息幂等、动作记录和完整审计。
- 全局内置规则默认只撤回，自定义规则可另行选择动作。
- 原生响应式窗口、`1180x760` 最小尺寸和 Windows 通知区后台驻留。
- 本机时区每日摘要，只写入总览仪表盘并保存在本地 SQLite。
- 原生成员复选框多选、批量恢复原名称和账号状态清理。
- 只有当前旺商聊账号在目标群中是群主或管理员时才开放群管理操作。
- 消息、规则、知识、任务和审计使用群名称下拉多选，不要求手填群 ID。
- 入群通知触发 `@「群名片」` 欢迎；普通成员同步不发送欢迎。
- 群名片支持整群预览、一键规范、在线入群自动修改、停机新增补改、持久重试和覆盖率提示。
- 有名称成员生成两字简称，缺少名称的成员使用 `DH群员0001` 形式的固定四位编号。
- 每日群发言计划按 Windows 当前时区自动开群和关群，支持跨午夜、启动补齐和失败退避重试。
- `@DH 预测 彩种名称` 使用 EXE 内置适配器先完成近期窗口、热冷、遗漏和候选统计，再由 AI 整理为自然回复；上游地址和原始字段不进入群消息。

## 运行结构

正常启动链只有 `DH-BOT.exe`。程序内部启动回环桥，默认连接：

- 旺商聊 DevTools：`http://127.0.0.1:9222`
- DH 内嵌桥：`http://127.0.0.1:51235`

`DHBridge.exe` 位于发行包 `tools/diagnostic`，仅用于独立诊断。桥只接受 DH
群管所需的固定路由，并限制为本机回环访问。

## 数据

- 数据库：`%APPDATA%\DH\dh.db`
- AI 密钥：`%APPDATA%\DH\secrets.dat`，Windows DPAPI 加密
- 消息保留 30 天
- 审计保留 180 天

升级时只读取旧 `state.json` 的连接地址和默认群 ID，写入 SQLite 后移除旧文件。

## 构建

项目要求 Go 1.23。macOS/Linux 交叉构建：

```sh
make test
make build
make bridge
make smoke-tool
make package
```

Windows PowerShell：

```powershell
.\build.ps1
```

发行产物：

- `dist/DH-BOT-2.7.0-windows-x64.exe`：单文件主程序，可直接分发。
- `dist/DH-BOT-2.7.0-windows-x64.zip`：主程序、手册、图解 PDF、规则模板、诊断工具和调试说明。
- DH BOT 2.7.0 支持自动定位和启动旺商聊；已有未开启 9222 的旺商聊会先弹窗确认，再按精确路径重启。
- 设置页和侧栏新增“调试”，可逐项检查 DevTools 页面、DH 内嵌桥、协议会话/NIM 和旺商聊进程，并给出处理建议。
- NIM 尚未初始化现在返回 503 和明确提示，不再伪装成普通 HTTP 500。
- AI 群聊回复只由明确的 `@DH` 触发；普通问句、单独出现 `DH` 或旧唤醒词不会触发。人格与业务边界见 `AGENTS.md`。
- 默认规则和内置知识库在旺商聊连接前即可查看；默认知识库仍需绑定群后才参与 AI 检索。
- `DH-BOT.exe` 和内部 Windows 子进程均按无控制台窗口方式启动；可选的 `Start-DH.vbs` 也不会保留 CMD 黑窗口。
- 知识库支持多库多群绑定；未绑定群的知识库不会注入 AI。
- 第二次双击会唤起已经运行的 DH，不会创建第二个实例。
- 数据库迁移前会在 `%APPDATA%\\DH\\backups` 保留最近三份快照。
- Windows Explorer 重启后，DH BOT 会自动恢复白底通知区图标。

## 测试

```sh
go test ./...
go test -race ./internal/moderation ./internal/store ./internal/appcore
go vet ./...
GOOS=windows GOARCH=amd64 CGO_ENABLED=0 go build ./cmd/dh
```

测试覆盖规则边界、动作合并、角色豁免、冷却、图片时间窗、改名锁定、AI
响应校验、Webhook 超时、知识检索、消息幂等、SQLite 迁移、任务提醒、每日
摘要、群计划跨午夜与重试、预测统计、群名片编号边界、任务重启续跑、旧库迁移和旺商聊 HTTP/NIM 协议请求格式。

## 目录

- `cmd/dh`：DH 原生 Windows 主程序
- `cmd/dh-bridge`：独立诊断桥
- `cmd/dh-smoke`：Windows UI 冒烟工具
- `internal/appcore`：群同步、消息流水线、AI 和自动动作
- `internal/moderation`：规则引擎与 JSON 导入导出
- `internal/protocol`：旺商聊强类型网关
- `internal/cdpbridge`：Electron CDP/NIM 内嵌桥
- `internal/store`：纯 Go SQLite 数据层
- `docs/DH使用手册.md`：大白话使用手册

## Rust + Tauri 3.0 alpha

`tauri3/` is the clean-data Rust backend and Tauri UI. It deliberately does
not read the Go 2.7 database: first run uses `%APPDATA%\\DH\\3.0\\dh.db`,
archives legacy files, and starts with zero groups, members, rules, knowledge
bases, tasks and audits. The alpha branch is the future migration track;
`main` remains the Go 2.7 stable application until protocol and Windows
acceptance are complete.
