# DH BOT Windows 本地发行步骤

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

进入 `tauri3` 后执行：

```text
pnpm install --frozen-lockfile
pnpm test:contract-sanitizer
pnpm test:production-isolation
pnpm test:production
pnpm test:fixture
pnpm test:ui
pnpm test:e2e:fixture
cargo clippy --no-default-features --all-targets -- -D warnings
```

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

打包脚本不会读取 PFX 或执行代码签名。`SHA256SUMS.txt` 同时记录主程序、安装包、文档、规则模板和 portable ZIP。

## 4. 实机验收

```powershell
.\scripts\test-windows-real-machine.ps1 `
  -Artifact .\dist\DH-BOT-VERSION-windows-x64-portable.zip `
  -ExpectedSha256 EXE_SHA256 `
  -VerifyInteractiveExit
```

`EXE_SHA256` 取自 `dist\production\SHA256SUMS.txt`。签名状态显示 `NotSigned` 属于当前个人发行的预期状态；仍需完成 9222、任务栏、托盘、退出残留和旺商聊登录复用检查。

## 5. 上传云盘

最少上传：

1. portable ZIP。
2. `SHA256SUMS.txt`。
3. PDF 使用手册。

上传后重新下载一次，用 `Get-FileHash -Algorithm SHA256` 比对云盘文件。分享说明应写明版本、SHA-256、Windows 首次启动可能出现的未知发布者提示，以及下载链接的更新日期。

自签名根证书、PFX 和密码不随包发布，也不要求用户安装根证书。
