# DH BOT Windows 验收总纲

本文是 Windows 实机验收的唯一入口，自包含：新克隆仓库只读这一份就能跑完全流程。桌面层与进程层的细节步骤见 [`WINDOWS-REAL-MACHINE-TEST.md`](WINDOWS-REAL-MACHINE-TEST.md)，本文只在必须离线逐条执行的地方（PowerShell 取证块、进程快照命令）保留少量逐字重复，这是为“新克隆只读这一份”有意保留的冗余，不是待清理的重复。

## 0. 开跑前的版本确认

先钉住测试对象，再谈功能。上一版交接文档把快照钉在比实际代码落后若干提交的位置，结果是功能没有缺陷、但测的是旧构建，失败原因被记成功能问题。所以这一节必须先过。

本文对应的源码快照：

| 项 | 值 |
|---|---|
| 源码提交 | `d923e5d`（`main` 与 `origin/main` 同值） |
| 应用版本 | `3.0.0-beta.1` |
| SQLite schema | v14 |
| 仓库 | `DH-devmax/d-hbot`（私有） |

在仓库根目录确认三个新功能真的在这份源码里，缺任何一项说明拉错了提交：

```powershell
Test-Path tauri3\src\components\ContextMenu.tsx
Test-Path tauri3\src-tauri\src\http_body.rs
Select-String -Path tauri3\src-tauri\src\diagnostics.rs -Pattern 'defaultResponseLimitBytes' -Quiet
```

三条都必须为 `True`。测试报告首行记录 `git rev-parse HEAD` 的完整 40 位 SHA，不用短 SHA。

## 1. 产品边界

- 正式程序只使用 Rust/Tauri，旧 Go 版本只作归档参考。
- 生产版固定连接真实旺商聊 DevTools `127.0.0.1:9222`。
- Fixture 只存在于开发构建，不得进入生产 EXE、ZIP、安装包或前端入口。
- 私聊、红包、转账、下注、账单和结算不属于 DH BOT 3.0。
- AI 助手只在明确 `@DH`、`@ DH` 或旺商聊真实提及元数据时回复。
- 确定性机器规则不依赖 AI；AI 控制规则与 AI 助手相互独立。
- 新安装默认不启用群、规则、知识库、计划、预测或自动群名片。

## 2. 测试对象

1. 在 Windows 开发机从一个干净、确定的完整源码 SHA 构建。
2. 测试对象必须是准备上传云盘的同一份 production portable ZIP。
3. 不使用旧 beta、历史下载包、Fixture 包或 macOS 产物。
4. 首轮用 portable，第二轮用 NSIS 安装版；普通用户跑一次，管理员用户再跑一次。
5. 当前个人发行采用未签名 portable ZIP，`NotSigned` 记录为发行事实，不替换成自签名包。

必须记录：源码完整 SHA、ZIP SHA-256、`DH-BOT.exe` SHA-256、Windows 版本、旺商聊版本、WebView2 版本。

## 3. 三层验收边界

| 层 | 由谁执行 | 覆盖 |
|---|---|---|
| 本地构建层 | `pnpm` 脚本 | Rust、React、契约、生产隔离、NSIS、SHA-256 |
| 自动探针层 | `test-windows-real-machine.ps1`（523 行） | 真实 EXE、WebView2、9222 HTTP 探测、进程/窗口快照、子进程树、Win32 普通最小化、交互式退出、报告与诊断包落盘 |
| 人工桌面层 | 人 | 图标观感、DPI、右键菜单、剪贴板、真实群写操作、长稳 |

探针已经覆盖的项不要再手工重复，重点放在它测不到的地方：观感、剪贴板、真实群副作用。

### 已在 macOS 验证过的部分

协议与业务链已在 macOS 真实旺商聊双实例（主账号 `9222`、普通成员测试账号 `9223`）验证通过，详见 [`REAL-GROUP-TEST-20260731.md`](REAL-GROUP-TEST-20260731.md)。Windows 这一轮不需要为了「再确认一遍」重复这些破坏性写操作：

- AI 明确提及回复通过；普通文本静默通过。
- 临时知识库绑定、命中、解绑、删除通过；解绑后不复用旧答案。
- 重复 FAQ 内存缓存命中时没有重复模型调用。
- 机器规则：资金/验证码、超过 4 行、加权字符 100/200、广告、单图、图片频率、黑名单均通过。
- AI 控制规则：广告、辱骂、诈骗分类与撤回通过。
- 机器规则与 AI 规则对同一消息的撤回动作去重通过。
- 真实撤回走 `nim.recallMsg`，并用 `nim.getHistoryMsgs` 回读确认消息不存在。
- HTTP 撤回返回业务码 `1001` 时不记成功、不重复重试。
- 名片累计修改 5 次、60 秒权威名单对账、自动恢复与违规次数持久化通过。

Windows 独占的责任是 macOS 覆盖不到的部分：任务栏、托盘、登录分区、进程退出、DPI，加上本文第 7、8、9 节这三项本次新增且在 macOS 上无法验证的功能。第 12 节的真实群冒烟仍要在 Windows 跑一遍，目的是确认同一套协议在 Windows 构建上同样成立，而不是重新验证规则逻辑本身。

## 4. 本地构建门禁

用 `tauri3/package.json` 里的真实脚本名，不手抄命令。下列命令按 Windows 验收顺序内联保留，便于离线逐条执行；命令清单本身的唯一权威是 [`ENGINEERING-STANDARDS.md`](ENGINEERING-STANDARDS.md) 的“测试门禁”一节，两处不一致时以该节为准：

```powershell
cd tauri3
pnpm test                          # vitest
pnpm test:contract-sanitizer       # Contract v2
pnpm test:production               # Rust 生产通道
pnpm test:fixture                  # Rust Fixture 通道
pnpm test:production-isolation     # 生产隔离场景
pnpm verify:docs                   # 文档链接 + 仓库脱敏
pnpm tauri:build:production        # 边界校验 + 清理 + 生产打包
pnpm package:windows:production    # NSIS + portable + SHA256SUMS
pnpm verify:windows:production     # 产物深度扫描
```

`d923e5d` 上的实测基线，偏离要先查原因再继续：

| 门禁 | 实测 |
|---|---:|
| Rust 生产 | 225 项通过 |
| Rust Fixture | 231 项通过 |
| vitest | 20 文件 / 77 项通过 |
| Contract v2 | 34 项通过 |
| 生产隔离 | 10 场景通过 |
| Clippy 两通道 | 0 警告（`-D warnings`） |

两个通道各有 1 项需要外部密钥的 live 测试按设计忽略，不算失败。

## 5. 生产包边界

解压 portable ZIP，确认没有 `DH-Fixture.exe`、Fixture 字样入口、`9233`、`51300`、测试数据库、PDB、Source Map、源码或开发命令。再核对 EXE 与 ZIP 的 SHA-256。

前端产物要单独搜一遍。这三个符号依赖 Vite 的字面量 `import.meta.env.X` 才能被消除，一旦写法被改成动态取值，开发分支会残留在生产包里：

```powershell
Select-String -Path dist\assets\*.js -Pattern 'VITE_DH_FIXTURE','isDeveloperChannel','shiftKey'
```

预期零命中。其中 `shiftKey` 尤其重要：它是开发通道 `Shift + 右键` 透传原生菜单的判定，生产包里整个分支应当已被消除。

## 6. 桌面层与进程层

完整步骤见 [`WINDOWS-REAL-MACHINE-TEST.md`](WINDOWS-REAL-MACHINE-TEST.md)。要点：

- 任务栏与托盘均显示白色圆角底 DH 图标。
- 最小化按钮只缩到任务栏，不直接进托盘；点击当前活动任务栏图标遵循普通 Windows 最小化行为。
- 只有点右上角 X 才弹「挂到托盘 / 退出 DH BOT」。
- 托盘可显示、暂停/恢复自动化、退出；Explorer 重启后托盘图标恢复。
- 退出后 5 秒内没有 `DH-BOT.exe` 及其 WebView2 子进程残留，且不连带结束旺商聊。
- 验证 100%、125%、150% DPI 和窄窗口布局。
- 系统代理开、关各跑一次，回环请求都不应走代理。

## 7. 右键菜单与剪贴板

这一节在 macOS 上无法验证：WKWebView 不暴露 CDP，开发机驱动不了真实窗口的右键。合并时状态为**未验证**，必须在 Windows 真机 WebView2 下逐条确认。

菜单的设计前提是「不新增能力」：数据行和列表项的菜单项直接镜像行内已有按钮，读取其 `title` 作为文案、执行时调用该按钮。所以业务逻辑、二次确认和 `disabled` 规则应当与点按钮完全一致。

1. 窗口空白处右键：**没有** WebView2 默认菜单（返回 / 前进 / 重新加载 / 另存为 / 打印 / 检查），也不弹 DH 自定义菜单。
2. 按住 `Shift` 再右键：生产包里**依然没有**原生菜单。若弹出「检查」，说明装的不是生产包，回到第 5 节重查。
3. 设置页 AI API Key 输入框右键：只有「粘贴」和「全选」，**没有**「复制」和「剪切」。
4. 普通输入框选中文字 → 右键「复制」→ 右键「粘贴」：文字写回输入框，且界面状态随之更新（不是只改显示）。
   - 「粘贴」置灰说明 WebView2 未放行 `navigator.clipboard.readText`。这是已知降级路径而非崩溃：记录下来，确认灰色项旁提示了 `Ctrl+V`，并确认 `Ctrl+V` 本身可用。
5. 消息台某行右键「复制整行」→ 粘进 Excel：自动分列，列顺序与界面一致，复选框列和操作按钮列不产生空列。
6. 成员表某行右键，点一个需要二次确认的操作（如「移出」）：弹出的是页面原本的确认框，可正常取消。
7. 窗口拖到屏幕最右侧和最底部各一次，在靠边的行上右键：菜单向内翻转、完整可见，未被窗口边缘截断。
8. 菜单打开后滚动列表：菜单关闭。再打开后 `↓` `↑` 移动高亮，`Esc` 关闭并把焦点还给原元素。
9. 表头行右键：不弹菜单。

## 8. 诊断包与 memoryCaps

「调试 → 生成诊断包」（命令 `export_support_bundle`）在 `%APPDATA%\DH\3.0\support-bundles` 生成 `DH-BOT-support-YYYYMMDD-HHMMSS.zip`。解压后查 `diagnostic.json` 的 `memoryCaps`，七个键必须全部存在且与下表一致：

| 键 | 期望值 | 含义 |
|---|---:|---|
| `maxMemberEventCache` | 500 | 条 |
| `maxReportedSet` | 500 | 条 |
| `maxMemberCacheGroups` | 100 | 群 |
| `maxTrackedWorkItems` | 200 | 条 |
| `maxGatewayBatch` | 100 | 条 |
| `aiProviderTimeoutSeconds` | 15 | 秒 |
| `defaultResponseLimitBytes` | 20971520 | 字节（20 MiB） |

```powershell
Expand-Archive -Path .\DH-BOT-support-*.zip -DestinationPath .\bundle -Force
(Get-Content .\bundle\diagnostic.json -Raw | ConvertFrom-Json).memoryCaps
```

除 `aiProviderTimeoutSeconds` 是时间上限外，其余都是计数或字节数；键名带单位的以键名为准。这些值直接读编译期常量，不是文档里抄的字面量，所以对不上就是装错了版本，不是文档过期。

同时确认包内不含 `dh.db`、`secrets.dat`、旺商聊分区、Cookie、Token、API Key、原始协议或原始群消息，并用包内 `SHA256SUMS.txt` 对照 `manifest.json`。维护端可在仓库根目录跑只读校验：

```sh
python3 tools/verify_support_bundle.py /path/to/DH-BOT-support-YYYYMMDD-HHMMSS.zip
```

## 9. 公网响应体限长读取

远端响应统一走 `http_body::read_capped_body`，上限在读取过程中生效而不是读完再判断，超限即中断，避免先把整个响应收进内存。AI 与预测两条路径共用这一入口。

- 正向：预测页拉一次加拿大28（BCLC 官方年度归档，是当前公开源里最大的一份）。应当正常返回，证明 20 MiB 不至于太紧。
- 反向：无可用数据源时健康状态必须明确显示数据源不可用，不生成候选方向，不用本机时间伪装开奖时间。

## 10. 旺商聊启动与 9222

- 旺商聊未运行：DH BOT 自动启动带 `--remote-debugging-port=9222` 的旺商聊并置前。
- 旺商聊已运行但没有 9222：先询问再精确重启，不处理其他程序。
- `/json/list` 暂时异常而 `/json` 可用时应自动回退；三个入口均异常时界面显示具体错误。
- 结束旺商聊主进程后，DH BOT 在下一轮检查恢复等待或重新启动，不永久停留在旧错误。

```powershell
curl.exe --noproxy "*" -sS -i http://127.0.0.1:9222/json/version
curl.exe --noproxy "*" -sS -i http://127.0.0.1:9222/json/list
netstat -ano | findstr :9222
Get-CimInstance Win32_Process |
  Where-Object { $_.CommandLine -match 'remote-debugging-port=9222' } |
  Select-Object ProcessId, Name, ExecutablePath, CommandLine
```

## 11. 登录状态复用

1. 在 DH BOT 启动的旺商聊中完成一次密码登录，等 NIM 就绪。
2. 退出 DH BOT，确认旺商聊仍运行；再退出旺商聊。
3. 重新打开 DH BOT，确认复用 `dh-primary` 分区并进入原账号，不再要求密码。
4. 重启 Windows 后重复一次。
5. 旺商聊升级后检查「登录分区维护」状态；需要重新应用时只恢复启动脚本，不删除 Cookie、Token、Local Storage 或旺商聊账号数据。

## 12. 真实群冒烟

每一步保存消息 ID、动作 ID、协议回执和回读结果。写操作走账号级顺序队列，调用间隔至少 500ms，这是协议保护而不是规则冷却，慢是预期的。

1. 只读同步群列表、成员名单、角色与能力。
2. 发送唯一标记的普通测试消息。
3. 从普通成员账号发送唯一标记的「请提供验证码」消息。
4. 确认机器规则命中，且只产生一个撤回动作。
5. 确认路由为 `nim.recallMsg`，`nim.getHistoryMsgs` 回读不再包含该服务器消息 ID。
6. 短时禁言后立即解禁，回读成员状态。
7. 临时修改名片，确认自动恢复；成员资料回调在当前旺商聊版本不稳定，生产逻辑靠 60 秒完整名单对账兜底，等满一轮再判失败。
8. 新增测试公告、读取公告历史、编辑、置顶/取消置顶，再恢复原公告。发布新公告走 `add-notice`，编辑旧公告走带明确公告 ID 的 `notice-opt`，两条路径不互相猜测。
9. 计划任务用一分钟窗口验证全群禁言/解除，结束后恢复允许发言。
10. 移出成员只在 Fixture 验证，**不在真实群执行**。

批量结果三态的判读：`succeeded` 已写动作与审计；`failed` 可修正后重试失败群；`unknown` 先到旺商聊核对，不自动重试。

调试页的能力状态由运行时只读探测决定，不再按旺商聊脚本 SHA 统一锁定，所以同一份生产包在不同机器上可能显示不同能力，这是设计而不是故障。全员禁言在启动时检查 ZCG 路由、IPC、NIM 方法和状态回读，全部通过才显示「已检测可用」，否则显示「待首次手工验证」或「当前不可用」及具体原因。某项写能力为 `unverified` 不等于 ZCG 基线不可用；判定规则见 [`PROTOCOL-CONTRACT.md`](PROTOCOL-CONTRACT.md) 第 2 节。

## 13. AI 与知识库

### 超时与预算基线

下面几个值直接来自 `src-tauri/src/ai.rs`，不是从旧文档抄的。上一版交接文档同一节里写了两个互相矛盾的数字（单次 60 秒 / 单连接 15 秒），实际代码只有一套：

| 项 | 值 | 来源 |
|---|---:|---|
| 单连接生成上限 | 15 秒 | `AI_PROVIDER_TIMEOUT` |
| 主备总预算 | 20 秒 | `AI_TOTAL_BUDGET` |
| 最多尝试连接数 | 2 | 达到 2 次或超总预算即停 |
| 连接超时 | 2 秒 | `connect_timeout` |
| FAQ 问答缓存 | 10 分钟 | 知识版本变化即失效 |

用户在设置页填的单次超时若超过 15 秒会被收敛回 15 秒，这是设计行为，不是配置没生效。历史报告里的 60 秒 / 65 秒是当时那一轮的配置记录，不代表当前代码。

思考深度使用 `xhigh`；设置页填 `xhight` 会被规范为 `xhigh`（`ai.rs` `normalize_reasoning_effort`），只接受 `low`、`medium`、`high`、`xhigh` 四个值，其余直接报错。API 后端为 Responses。Provider 名称与模型在报告里用 `MODEL_UNDER_TEST` 占位，API Key 由 Windows 端在设置页录入，不从任何文档获取。

### 冒烟步骤

- 在 Windows 设置页录入测试 Provider，不在日志或报告写密钥。
- `/v1/models` 确认模型存在，再执行最小 Responses 调用。
- 普通文本保持静默；`@DH` 与旺商聊真实提及当前账号都收到回复。
- 临时知识库绑定当前群，用唯一事实提问；解绑后不再引用该答案。
- 重复 FAQ 验证缓存命中（知识版本未变时走 10 分钟内存缓存，模型调用 0 次）。摘要、预测、任务、动作和依赖最近上下文的回答不使用问答缓存。
- 审计包含 Provider、模型、知识片段数量、缓存状态、模型耗时和总耗时，但不含完整提示词与密钥。
- AI 上游偶尔返回解释文字而非分类 JSON；此时应记录失败、零动作并继续处理后续消息，不执行猜测动作。

## 14. 长时间稳定性

建议连续运行 8 至 24 小时，覆盖：

- 旺商聊页面重载、NIM 503、断线重连、旺商聊重启、Windows 睡眠/唤醒。
- AI 429、5xx、超时、非法 JSON 和备用 Provider 切换。
- 消息无丢失、无重复 AI 调用、无重复撤回。
- SQLite inbox/outbox 重试、审计、任务和计划在重启后恢复。

每次异常保留完整排障包，不只保存截图。

## 15. 失败时保留的证据

```powershell
Get-CimInstance Win32_Process |
  Where-Object { $_.Name -match 'DH-BOT|wangshangliao|msedgewebview2' } |
  Select-Object ProcessId, ParentProcessId, Name, ExecutablePath, CommandLine

netstat -ano | findstr :9222
Get-ChildItem "$env:APPDATA\DH\3.0\logs" -File |
  Sort-Object LastWriteTime -Descending |
  Select-Object -First 3 FullName, Length, LastWriteTime
```

日志已做密钥脱敏，发送前仍应检查不含账号密码、Cookie、Token 或 Authorization。

## 16. 安全与恢复要求

- 报告中不写 API Key、Cookie、Token、Authorization 或账号密码。
- 真实群写操作必须先保存原状态。
- 禁言测试后立即解禁；改名后恢复原名；公告后恢复原公告与置顶状态；全群禁言后恢复允许发言。
- 不在真实群执行移出成员。
- 回执不确定时停止自动重试，转人工确认。

## 17. 结果回填格式

```text
测试时间：
源码 SHA（完整 40 位）：
ZIP SHA-256：
EXE SHA-256：
Windows 版本：
旺商聊版本：
WebView2 版本：

[PASS/FAIL] 版本确认（三项存在性检查）：
[PASS/FAIL] 本地构建门禁（225/231/77/34/10）：
[PASS/FAIL] 生产隔离（含 dist 三符号零命中）：
[PASS/FAIL] 任务栏普通最小化：
[PASS/FAIL] X 关闭提示与托盘：
[PASS/FAIL] 退出后无残留：
[PASS/FAIL] DPI 100/125/150：
[PASS/FAIL] 右键菜单屏蔽默认菜单：
[PASS/FAIL] Shift+右键生产包无原生菜单：
[PASS/FAIL] API Key 框无复制/剪切：
[PASS/FAIL] 剪贴板复制/粘贴回写状态：
[PASS/FAIL] 复制整行进 Excel 分列：
[PASS/FAIL] 菜单镜像按钮与二次确认：
[PASS/FAIL] 菜单边缘翻转与键盘导航：
[PASS/FAIL] 诊断包 memoryCaps 七项：
[PASS/FAIL] 诊断包脱敏边界：
[PASS/FAIL] 限长读取正向（加拿大28 年度归档）：
[PASS/FAIL] 9222 启动与无代理读取：
[PASS/FAIL] 账号登录复用：
[PASS/FAIL] Windows 重启后登录复用：
[PASS/FAIL] 群/成员只读同步：
[PASS/FAIL] NIM 真实撤回与历史回读：
[PASS/FAIL] 禁言/解禁回读：
[PASS/FAIL] 改名/恢复回读：
[PASS/FAIL] 公告新增/编辑/置顶/历史回读：
[PASS/FAIL] 一分钟开关群计划：
[PASS/FAIL] @DH 与真实提及 AI 回复：
[PASS/FAIL] 知识库绑定/解绑：
[PASS/FAIL] FAQ 缓存：
[PASS/FAIL] 审计与日志脱敏：
[PASS/FAIL] 8-24 小时稳定性：

失败步骤：
预期：
实际：
错误时间：
相关 PID：
相关消息 ID/动作 ID：
排障包路径：
是否已恢复群状态：
```

探针自身还会在桌面 `DH-BOT-Test-日期时间` 目录写出 `DH-BOT-Windows-Real-Machine.json`、`DH-BOT-Windows-Real-Machine.md` 和 `TEST-REPORT.md`。把这些文件连同上面的回填表一起交回即可，不必发送账号、Cookie、Token 或旺商聊数据库。

## 18. 发布门槛

以下全部满足后可进入 3.0 RC：

- Windows 同一生产包完成自动探针与人工桌面验收。
- 9222、登录复用、任务栏/托盘、进程退出和 DPI 通过。
- 右键菜单在真机 WebView2 下确认屏蔽默认菜单，剪贴板行为已记录（放行或已确认降级路径）。
- 诊断包 `memoryCaps` 七项齐全且与常量一致。
- Windows 真实群的撤回、禁言/解禁、改名/恢复、公告回读通过。
- 日志与支持包完整且脱敏。
- 云盘上传后重新下载，ZIP SHA-256 与本地一致。
- 预测可保持默认停用，不阻塞核心版本发布。
