# DH BOT Windows 生产实机验收

## 为什么要分层测试

DH BOT 生产包由 Windows 开发机本地构建。构建脚本能证明指定源码可以通过 MSVC、扫描和打包，但仍需要真实旺商聊账号与桌面环境验证任务栏、托盘、窗口焦点和登录状态。当前源码使用 schema v14、`commands/` 域命令、`runtime/` 模块和 QueueKernel 副作用队列。正式验收分三层：

1. **本地构建层**：Rust、React、契约、生产隔离、NSIS 和 SHA-256。
2. **Windows 自动探针层**：真实 EXE、WebView2、9222、旺商聊进程参数、普通最小化和退出残留。
3. **人工桌面层**：图标观感、任务栏点击、X 关闭提示、账号登录状态复用。

Fixture 不参与这套测试。测试对象必须是 Windows 开发机生成并准备上传云盘的同一份未签名生产包，不使用历史 artifact 或开发 Fixture 包。

## 推荐环境

- Windows 10 22H2 或 Windows 11 23H2/24H2。
- 普通 Windows 用户运行一次，管理员用户再运行一次安装与维护流程。
- 旺商聊使用当前生产版本并登录测试账号。
- 首轮使用 portable，第二轮使用 NSIS 安装版。
- 关闭系统或安全软件中的全局 HTTP 代理后再做一次；开启代理后再做一次，确认 DH BOT 访问回环地址时不走代理。

## 运行自动探针

把以下两个文件放在同一台 Windows 电脑：

- Windows 本地构建的 `DH-BOT-*-windows-x64-portable.zip`
- 私有源码中的 `tauri3/scripts/test-windows-real-machine.ps1`

在 PowerShell 中执行：

```powershell
Set-ExecutionPolicy -Scope Process Bypass
.\test-windows-real-machine.ps1 `
  -Artifact .\DH-BOT-3.0.0-beta.1-windows-x64-portable.zip `
  -ExpectedSha256 PORTABLE_EXE_SHA256 `
  -SourceSha $(git rev-parse HEAD) `
  -OutputDirectory "$env:USERPROFILE\Desktop\DH-BOT-Test-$(Get-Date -Format yyyyMMdd-HHmmss)" `
  -VerifyInteractiveExit
```

`ExpectedSha256` 填同次本地构建的 `SHA256SUMS.txt` 中 `DH-BOT.exe` 对应值；portable 内的 `DH-BOT-Portable.exe` 是它的同字节副本，脚本会在解压后校验该文件。`SourceSha` 固定本轮源码完整 SHA，避免报告引用旧基线。当前个人发行的签名状态预期为 `NotSigned`，脚本将其记录为信息而不是失败。

脚本自动完成：

- 解压到独立临时目录，并验证 portable 文件边界。
- 校验 `DH-BOT-Portable.exe` 的 SHA-256，并记录 Authenticode 状态。
- 检查 WebView2 Runtime。
- 记录启动前是否已有 DH BOT 残留。
- 启动真实 `DH-BOT.exe`，等待主窗口。
- 用 Win32 窗口消息执行普通最小化，确认窗口仍处于可恢复的任务栏最小化状态。
- 关闭系统代理访问 `127.0.0.1:9222/json/version` 与 `/json/list`。
- 检查旺商聊进程是否带 `--remote-debugging-port=9222`。
- 等待操作员点击 X 并选择“退出 DH BOT”，检查主进程和 WebView2 子进程是否清理。
- 确认 DH BOT 退出时没有顺带结束旺商聊。

报告默认写到桌面的 `DH-BOT-Test-日期时间`（也可以用 `-OutputDirectory` 显式指定）：

```text
DH-BOT-Windows-Real-Machine.json
DH-BOT-Windows-Real-Machine.md
TEST-REPORT.md
TEST-RESULTS.json
AUTOMATED-TESTS.log
PROCESS-AND-PORTS.txt
HASHES.txt
FAILURE-CONTEXT.txt
DH-BOT-Diagnostic-时间戳.zip
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

### 右键菜单与剪贴板

这一节验证 macOS 上无法验证的部分：WKWebView 不暴露 CDP，开发机上驱动不了真实窗口的右键，所以以下各项在合并时状态为**未验证**，必须在 Windows 真机 WebView2 下逐条确认。

1. 在窗口空白处右键，确认**没有**出现 WebView2 默认菜单（返回 / 前进 / 重新加载 / 另存为 / 打印 / 检查），也没有弹出 DH 自定义菜单。
2. 按住 `Shift` 再右键，确认生产包里**依然没有**原生菜单。该透传分支只存在于开发通道，生产构建应已消除；若此处弹出“检查”，说明装的不是生产包。
3. 在“设置”页的 AI API Key 输入框右键，确认菜单只有“粘贴”和“全选”，**没有**“复制”和“剪切”。
4. 在任意普通输入框输入文字并选中，右键点“复制”，再右键点“粘贴”，确认文字被写回输入框且界面状态随之更新（不是只改了显示）。
   - 若“粘贴”是灰的，说明 WebView2 未放行 `navigator.clipboard.readText`。这是已知的降级路径而非崩溃：记录下来，并确认灰色项旁提示了 `Ctrl+V`，同时确认键盘 `Ctrl+V` 本身可用。
5. 在消息台的一行消息上右键，点“复制整行”，粘进 Excel，确认自动分列且列顺序与界面一致；复选框列和操作按钮列不应产生空列。
6. 在成员表某一行右键，点一个需要二次确认的操作（如“移出”），确认弹出的是页面原本的确认框，并可正常取消。
7. 把窗口拖到屏幕最右侧和最底部各一次，在靠边的行上右键，确认菜单向内翻转、完整可见，没有被窗口边缘截断。
8. 菜单打开后滚动列表，确认菜单关闭；再打开菜单后按 `↓` `↑` 可移动高亮，`Esc` 关闭并把焦点还给原来的元素。

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
- 未签名状态与当前个人发行策略一致，SHA-256 与待上传清单一致。
- 云盘上传后重新下载的 ZIP 与本地原文件 SHA-256 一致。
