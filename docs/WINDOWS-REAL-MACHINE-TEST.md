# DH BOT Windows 生产实机验收

## 为什么要分层测试

发行仓库的生产 Actions 能证明指定源码 SHA 可以在 Windows MSVC 下构建、签名、扫描和打包，但它不具备旺商聊账号，也不能替代真实桌面的任务栏、托盘、窗口焦点和登录状态测试。正式验收分三层：

1. **Actions 构建层**：Rust、React、契约、生产隔离、NSIS 和 SHA-256。
2. **Windows 自动探针层**：真实 EXE、WebView2、9222、旺商聊进程参数、普通最小化和退出残留。
3. **人工桌面层**：图标观感、任务栏点击、X 关闭提示、账号登录状态复用。

Fixture 不参与这套测试。测试对象必须是 `DH-devmax/d-hbot-releases` 公开 Release 中的已签名生产包，不使用源码仓库历史 artifact 或本地未签名包。

## 推荐环境

- Windows 10 22H2 或 Windows 11 23H2/24H2。
- 普通 Windows 用户运行一次，管理员用户再运行一次安装与维护流程。
- 旺商聊使用当前生产版本并登录测试账号。
- 首轮使用 portable，第二轮使用 NSIS 安装版。
- 关闭系统或安全软件中的全局 HTTP 代理后再做一次；开启代理后再做一次，确认 DH BOT 访问回环地址时不走代理。

## 运行自动探针

把以下两个文件放在同一台 Windows 电脑：

- 公开 Release 下载的 `DH-BOT-*-windows-x64-portable.zip`
- 私有源码中的 `tauri3/scripts/test-windows-real-machine.ps1`

在 PowerShell 中执行：

```powershell
Set-ExecutionPolicy -Scope Process Bypass
.\test-windows-real-machine.ps1 `
  -Artifact .\DH-BOT-3.0.0-beta.1-windows-x64-portable.zip `
  -ExpectedSha256 EXE_SHA256 `
  -VerifyInteractiveExit
```

`ExpectedSha256` 填同一公开 Release 的 `SHA256SUMS.txt` 中 `DH-BOT.exe` 对应值。公开生产包签名状态必须为 `Valid`；未签名包不进入这套正式验收。

脚本自动完成：

- 解压到独立临时目录，并验证 portable 文件边界。
- 校验 `DH-BOT.exe` 的 SHA-256 和 Authenticode 状态。
- 检查 WebView2 Runtime。
- 记录启动前是否已有 DH BOT 残留。
- 启动真实 `DH-BOT.exe`，等待主窗口。
- 用 Win32 窗口消息执行普通最小化，确认窗口仍处于可恢复的任务栏最小化状态。
- 关闭系统代理访问 `127.0.0.1:9222/json/version` 与 `/json/list`。
- 检查旺商聊进程是否带 `--remote-debugging-port=9222`。
- 等待操作员点击 X 并选择“退出 DH BOT”，检查主进程和 WebView2 子进程是否清理。
- 确认 DH BOT 退出时没有顺带结束旺商聊。

报告默认写到桌面的 `DH-BOT-Test-日期时间`：

```text
DH-BOT-Windows-Real-Machine.json
DH-BOT-Windows-Real-Machine.md
```

把这两个文件交给本地或云电脑 Codex CLI 即可复盘，不必发送账号、Cookie、Token 或旺商聊数据库。

## 人工桌面检查

### 任务栏与托盘

1. 启动 DH BOT，确认任务栏显示白色圆角底 DH 图标。
2. 点击窗口最小化按钮，确认只缩到任务栏，没有直接进入托盘。
3. 恢复窗口，再点击一次当前激活的任务栏图标，确认行为仍是普通最小化。
4. 点击右上角 X，确认出现“挂到托盘 / 退出 DH BOT”。
5. 选择挂到托盘，确认任务栏窗口消失，托盘图标存在且可恢复。
6. 托盘右键选择退出，确认 5 秒后任务管理器内没有 `DH-BOT.exe`。

### 旺商聊与 9222

1. 旺商聊未运行时打开 DH BOT，确认旺商聊自动启动并出现在前台。
2. 旺商聊已运行但没有 9222 时打开 DH BOT，确认先提示再重启，不直接结束进程。
3. 在 PowerShell 执行：

```powershell
curl.exe --noproxy "*" -sS -i http://127.0.0.1:9222/json/version
curl.exe --noproxy "*" -sS -i http://127.0.0.1:9222/json/list
netstat -ano | findstr :9222
Get-CimInstance Win32_Process |
  Where-Object { $_.CommandLine -match 'remote-debugging-port=9222' } |
  Select-Object ProcessId, Name, ExecutablePath, CommandLine
```

4. `/json/list` 暂时异常而 `/json` 可用时，DH BOT 应自动回退；三个入口均异常时界面应显示具体错误。
5. 结束旺商聊主进程，观察 DH BOT 是否在下一轮检查中恢复等待或重新启动，而不是永久停留在旧错误。

### 登录状态复用

1. 在 DH BOT 启动的旺商聊中完成一次密码登录，等待 NIM 显示就绪。
2. 退出 DH BOT，确认旺商聊仍运行。
3. 退出旺商聊，再重新打开 DH BOT。
4. 确认旺商聊复用 `dh-primary` 分区并进入原账号，不再次要求密码。
5. 重启 Windows 后重复一次。
6. 旺商聊升级后检查“登录分区维护”状态；需要重新应用时只恢复启动脚本，不清理旺商聊账号数据。

## 失败时保留的证据

不要只截错误提示。至少保留：

```powershell
Get-CimInstance Win32_Process |
  Where-Object { $_.Name -match 'DH-BOT|wangshangliao|msedgewebview2' } |
  Select-Object ProcessId, ParentProcessId, Name, ExecutablePath, CommandLine

netstat -ano | findstr :9222
Get-ChildItem "$env:APPDATA\DH\3.0\logs" -File |
  Sort-Object LastWriteTime -Descending |
  Select-Object -First 3 FullName, Length, LastWriteTime
```

日志已经做密钥脱敏；发送前仍应检查内容，不包含账号密码、Cookie、Token 或 Authorization。

## 通过标准

- 9222 的 DevTools JSON 至少一个入口返回旺商聊页面，DH BOT 内部检查不受系统代理影响。
- 正常最小化保留任务栏窗口；只有 X 关闭流程会进入托盘或退出。
- 选择退出后 DH BOT 主进程和其 WebView2 子进程在 5 秒内清理。
- DH BOT 退出不结束旺商聊。
- 登录状态在旺商聊重启及 Windows 重启后继续复用。
- 生产包中没有 Fixture、测试端口、测试数据库、开发命令或源码。
- 正式标签产物 Authenticode 状态为 `Valid`，SHA-256 与公开清单一致。
