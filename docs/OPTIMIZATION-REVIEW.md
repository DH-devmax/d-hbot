# DH BOT 代码与实机问题优化审查

审查日期：2026-08-06
对照基线：`d739a70cb4446a9eb37bde1a0c3b62a5165c0c6b`
范围：Windows 生产构建、旺商聊连接、消息与规则、AI、预测、SQLite、报告链。

## 本轮已修复

| 优先级 | 现象 | 根因 | 修复与验收点 |
| --- | --- | --- | --- |
| P0 | npm Windows 发布命令找不到 `pwsh` | 发布机只有 Windows PowerShell 5.1 | 全部 Windows npm script 攉为 `powershell.exe -NoProfile -ExecutionPolicy Bypass`；三个脚本通过 PowerShell AST 解析 |
| P0 | 安装/卸载时可能结束正在运行的便携版 | 安装版和 portable 都使用 `DH-BOT.exe`，进程名无法区分 | portable 攉为 `DH-BOT-Portable.exe`；生产扫描器和实机脚本同时接受并校验两个明确名称 |
| P0 | 真实改名返回业务码 1016 | 唯一测试名称超过旺商聊 20 字符限制 | command 与 outbox 派发前统一 trim、非空和 20 字符校验，错误在协议调用前返回 |
| P0 | DH 自己改名会被当成成员违规 | 成员回调无法区分内部写入和外部改名 | schema v12 增加 2 分钟、精确名称、单次消费的预期回调表；手工和自动改名均先登记 |
| P0 | `rename_count` 永远无法从成员事件命中 | 规则只在消息 worker 内运行 | `MemberUpdated` 真变更先持久计数，再以 `member_updated` 输入评估机器规则并记录独立审计 |
| P0 | 成员事件命中旧 `recall` 动作会提交空消息 ID | 默认规则动作来自消息规则模板 | 成员事件保留命中证据，但跳过不适用的撤回动作；锁定名片恢复继续使用专用 rename outbox |
| P0 | 同群 AI 慢请求阻塞后续机器规则 | AI 回复在按群消息 worker 内同步等待 | 每群独立顺序 AI 队列；消息 worker 只完成确定性规则和入队，跨群/AI 规则共享 2 个并发许可 |
| P0 | Provider 失败但降级提示入队成功，AI run 被记成功 | 模型执行状态和 fallback 发送状态共用一个布尔值 | Provider 错误立即将 run 终态设为 `failed`；fallback 作为独立 effect 记录 |
| P1 | AI 异常最多阻塞 65 秒 | 单连接 60 秒、总预算 65 秒 | 单连接 15 秒、主备总预算 20 秒、Responses 路由探测 5 秒 |
| P1 | 预测 AI 润色可能漏掉趋势 | 纯文本结果只检查非空 | 缓存写入和读取都校验彩种、期号、结果、更新时间、趋势、方向和参考度；不完整时使用确定性模板 |
| P1 | 预测不同彩种互相串行 | 全局请求 mutex；base cache 锁跨网络 await | 最多 2 个只读请求并发、同彩种 single-flight、base cache 网络调用不持锁、调用方超时不再被强制放大到 20 秒 |
| P1 | 9222 重载恢复超过 5 秒 | 未就绪、监听安装和身份读取均固定睡眠 5 秒 | 攉为 250/500/1000/2000ms 有界退避，会话成功后复位 |
| P1 | 报告 PID、哈希和状态口径不一致 | 多个步骤分别采样并手工拼接 | 最终阶段一次重查进程与 9222/9223，统一生成 `PASS/FAIL/SKIPPED`、六个规定文件和诊断 ZIP |

## 已执行的自动验证

- `cargo check`：通过。
- `cargo test --no-default-features`：202 PASS，0 FAIL，1 个需要外部临时凭据的 live test 跳过。
- `pnpm test`：58 PASS，0 FAIL。
- PowerShell AST：`package-windows.ps1`、`verify-windows-production.ps1`、`test-windows-real-machine.ps1` 通过。
- Node 语法与 `package.json` 解析：通过。

## 仍需完成的发布门禁

| 优先级 | 项目 | 当前风险 | 所需证据 |
| --- | --- | --- | --- |
| P0 | 新源码生产构建 | 当前验证尚未产生本分支最终 EXE/ZIP/setup 哈希 | 本机 release + NSIS + portable，深度扫描，`SHA256SUMS.txt` 与 `HASHES.txt` 一致 |
| P0 | 安装/卸载进程隔离 | 已从命名上消除已知冲突，仍需验证 NSIS 行为 | portable 运行时安装/卸载；确认仅提示或处理安装版，不结束 portable 与旺商聊 |
| P0 | 真实旺商聊回归 | 自动测试不等同于真实协议回执 | 9222/9223 页面、账号、NIM、能力矩阵和协议指纹的只读记录；写入前单独汇报 |
| P0 | 改名与规则真实回调 | Fixture 已覆盖状态机，真实回调字段可能随客户端版本变化 | 20 字符内改名、回读、恢复；内部回调不计数，外部改名计数 1/5 次 |
| P0 | AI 真实 Provider | 旧 API 凭据已暴露过，不应继续作为发布证据 | 轮换凭据后验证成功、超时、429、5xx、非法 JSON、同群顺序和后续消息不阻塞 |
| P1 | AI 队列崩溃恢复 | AI 回复队列当前在内存中；进程在确定性处理完成后崩溃可能丢失待处理回复 | 后续将 AI job 持久化，或增加 queued AI run 的启动恢复；RC 前做进程中断测试 |
| P1 | Windows UI/DPI | 单元测试不覆盖系统缩放和 Explorer 托盘重建 | 100/125/150% DPI、任务栏、托盘、最小化、关闭提示、Explorer 重启截图与进程证据 |
| P1 | 睡眠和长稳 | 快速回归不覆盖网络与系统生命周期 | 睡眠恢复、旺商聊重启、DH 重启、NIM 503、持续观察和队列计数 |
| P2 | 报告完整业务证据 | 自动报告只自动收集本机进程、端口和产物；群操作证据仍来自业务审计 | 将消息、动作、公告、计划和恢复 ID 导出并并入最终报告 |

## 后续代码优化建议

1. 将 `AiReplyJob` 持久化为 SQLite outbox，状态使用 `queued/processing/succeeded/failed`，启动时恢复 `processing`，以消除进程崩溃窗口。
2. 将机器规则的适用事件类型写入规则模型。保存规则时阻止“成员事件 + recall”等无意义组合，减少运行时跳过分支。
3. 将连接退避、Provider 预算、读取并发和写入间隔集中到强类型运行配置，避免文档、测试和常量漂移。
4. 为 Windows 打包脚本增加 Pester 或最小 PowerShell 集成测试，使用伪 PE/临时目录验证命名、清单、哈希和 ZIP 内容。
5. 报告 JSON 增加稳定 schema 版本和证据引用字段，使 Markdown、日志、哈希和诊断 ZIP 都可由 JSON 重建。
6. 为生产端到端测试增加故障注入代理，覆盖 429、5xx、超时、连接重置和截断 JSON，而不依赖真实服务偶发失败。
7. 对群列表、成员列表和预测读取增加可观测 single-flight 指标：等待者数量、共享请求 ID、缓存年龄和绕过原因。
8. 将历史状态文档中的测试数量与 timeout 由脚本生成，避免发布后继续显示旧的 60/65 秒和 schema v11。

## 当前结论

代码级阻断项已完成修复并通过本地自动测试。发布结论仍为“仅限内部 beta”，直到本分支重新生成生产产物并完成 Windows 安装边界、双实例只读探测和经确认的真实群回归。
