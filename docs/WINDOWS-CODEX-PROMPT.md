# Windows Codex 验收提示词

你正在 Windows 真实桌面环境验收 DH BOT 3.0。先完整阅读同目录的：

1. DH-BOT-WINDOWS-CONTEXT.md
2. WINDOWS-REAL-MACHINE-TEST.md
3. REAL-GROUP-TEST-20260731.md
4. DIAGNOSTICS-AND-SUPPORT.md

执行约束：

- 只测试本目录中准备发行的 production portable ZIP，先记录源码 SHA、ZIP SHA-256 和 EXE SHA-256。
- 不使用 Fixture、历史 beta 或开发构建。
- 不记录或回传 API Key、Cookie、Token、Authorization、账号密码或旺商聊数据库。
- 先跑 test-windows-real-machine.ps1，再做人工桌面、9222、登录复用和真实群冒烟。
- 真实群写操作前保存原状态，结束后恢复禁言、名片、公告、置顶和全群发言状态。
- 不在真实群测试移出成员。
- 每个步骤输出 PASS/FAIL、实际证据、时间、PID、消息 ID/动作 ID和恢复结果。
- 错误时保留日志和支持包，不只描述 UI 提示。
- 不修改源码；发现问题时先给出最小复现、日志证据、推定模块和建议修复，不直接提交。

完成后按 DH-BOT-WINDOWS-CONTEXT.md 的“结果回填格式”生成：

- DH-BOT-Windows-Real-Machine.md
- DH-BOT-Windows-Real-Machine.json
- FAILURE-CONTEXT.txt（只有失败时生成）

开始前先确认：Windows 版本、旺商聊版本、WebView2 版本、测试包路径和三个 SHA 值。
