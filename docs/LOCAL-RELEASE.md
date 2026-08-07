# DH BOT Windows 本地发行步骤

DH BOT 不使用 GitHub Actions 构建程序：

| 仓库 | 用途 | Actions |
|---|---|---|
| `DH-devmax/d-hbot` | 私有 Rust/Tauri 源码、Fixture、脱敏契约和本地工具 | 停用 |
| `DH-devmax/d-hbot-releases` | 可选下载说明与历史发行索引 | 停用 |
| `sh492773746/*` | 旧账号归档 | 不参与当前线路 |

测试、Windows MSVC 构建、NSIS/portable 打包、生产隔离扫描和真实桌面验收都在开发机本地完成。GitHub 不保存签名证书、源码读取 Token 或构建产物。

## 1. 固定源码

在源码仓库确认工作树、远端、账号和作者：

```text
git status --short
git rev-parse HEAD
git remote get-url --push origin
gh auth status --active
git config user.name
git config user.email
```

记录完整 40 位 commit SHA。发行使用该提交，不在构建过程中继续改代码。

## 2. 完成本地门禁

门禁清单见 [ENGINEERING-STANDARDS.md](ENGINEERING-STANDARDS.md) 的“测试门禁”一节，那里是唯一权威来源。全部通过后再进入下一步。

Fixture 只用于开发测试，不进入下一步生产构建。

## 3. 在 Windows 构建

使用已安装 Rust MSVC、Node.js、pnpm、Tauri 构建依赖、WebView2 和 7-Zip 的 Windows 开发机：

```powershell
pnpm tauri:build:production
pnpm package:windows:production
pnpm verify:windows:production
```

主要输出：

```text
tauri3\dist\production\DH-BOT.exe
tauri3\dist\production\DH-BOT-VERSION-windows-x64-setup.exe
tauri3\dist\production\SHA256SUMS.txt
tauri3\dist\DH-BOT-VERSION-windows-x64-portable.zip
```

打包脚本不会读取 PFX 或执行代码签名。`SHA256SUMS.txt` 同时记录主程序、安装包、文档、规则模板和 portable ZIP。当前个人发行不要求 Authenticode；未签名 portable ZIP 是预期发布形态。任何自签名测试证书、PFX、密码和密钥都不得进入包或云盘。

## 4. 实机验收

```powershell
.\scripts\test-windows-real-machine.ps1 `
  -Artifact .\dist\DH-BOT-VERSION-windows-x64-portable.zip `
  -ExpectedSha256 EXE_SHA256 `
  -VerifyInteractiveExit
```

`EXE_SHA256` 取自 `dist\production\SHA256SUMS.txt`。签名状态显示 `NotSigned` 属于当前个人发行的预期状态；仍需完成 9222、任务栏、托盘、退出残留和旺商聊登录复用检查。

在实机验收中额外进入“调试”连续生成两次诊断包，检查 ZIP 均可打开且路径不相同，包内 `manifest.json` 与 `SHA256SUMS.txt` 存在，且压缩包内没有 `dh.db`、`secrets.dat`、Cookie、Token、API Key、真实群名或原始消息。诊断包是用户自行发送给维护人员的排障材料，不随发行包上传。

## 5. 上传云盘

最少上传：

1. portable ZIP。
2. `SHA256SUMS.txt`。
3. PDF 使用手册。

上传后重新下载一次，用 `Get-FileHash -Algorithm SHA256` 比对云盘文件。分享说明应写明版本、SHA-256、Windows 首次启动可能出现的未知发布者提示，以及下载链接的更新日期。

自签名根证书、PFX 和密码不随包发布，也不要求用户安装根证书。

## 6. 远端 Actions 只读核对

以下命令用于确认远端没有活动 workflow：

```text
gh workflow list --repo DH-devmax/d-hbot
gh workflow list --repo DH-devmax/d-hbot-releases
gh api repos/DH-devmax/d-hbot/actions/permissions
gh api repos/DH-devmax/d-hbot-releases/actions/permissions
```

两个权限接口都应返回 `enabled: false`，workflow 列表应为空。后续若采用公共信任代码签名或恢复 GitHub Release，需要先更新本规范和长期记忆，再建立新的独立发布方案。

最近只读核对：2026-08-01，两个权限接口均返回 `enabled: false`，两个 workflow 列表均为空。
