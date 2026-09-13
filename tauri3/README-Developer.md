# DH BOT Developer Build

开发版有两个严格区分的运行环境：真实旺商聊用于 Contract v2 与专有写能力校准，Fixture 用于本地协议、成员、消息和自动化回归。ZCG 基线能力由运行时只读探测逐项开放；旺商聊专有写能力只有在真实状态恢复、脱敏和回放证据完整后才进入生产能力表。

## 启动

```text
pnpm install --frozen-lockfile
pnpm dev:fixture
```

`pnpm dev:fixture` 只启动 Fixture 环境：

- 页面服务：`127.0.0.1:51300`
- DevTools：`127.0.0.1:9233`
- 数据目录：`%APPDATA%\DH\fixture`

Windows 校准使用内部开发包 `DH-BOT-Dev.exe`，在设置中选择：

- 运行环境：`真实旺商聊`
- DevTools：`127.0.0.1:9222`

不要在真实校准时启动 `DH-Fixture.exe`，也不要使用 `9233` 或 `51300`。开发版首次启动默认选择真实环境；运行模式保存在 `%APPDATA%\DH\developer\runtime-mode`。

## 测试与构建

```text
pnpm test:fixture
pnpm verify:docs
pnpm tauri:build:developer
pnpm package:windows:developer
```

开发构建产物为内部 ZIP，包含 `DH-BOT-Dev.exe`、`DH-Fixture.exe` 和校验文件；不上传到公开发布目录。

生产构建由 GitHub Actions 的 Windows runner 使用 `pnpm tauri:build:production` 和 `pnpm package:windows:production`；Windows 开发机仍可按同样命令复现。当前个人发行产物未签名，随 portable ZIP 一起发布 `dist/production/SHA256SUMS.txt`；产物不携带开发测试模块、9233 端口或 Fixture 资源。
生产构建会显式将 `--no-default-features` 传给 Cargo，并在编译前删除曾暂存的 `DH-Fixture` 资源和开发二进制。`pnpm test:production-isolation` 验证扫描器会拦截 Fixture 命令、9233/51300 端口、开发数据目录和环境变量；Windows 打包还会用 7-Zip 展开 NSIS 安装器，并复查解压后的 portable ZIP。

开发版的运行模式文件位于 `%APPDATA%\DH\developer\runtime-mode`。Fixture 数据位于 `%APPDATA%\DH\fixture`；开发版真实模式和生产版使用 `%APPDATA%\DH\3.0` 与真实旺商聊 `9222`。

## 旺商聊契约采集

真实校准按以下顺序执行，并且只在明确授权的测试群操作：

1. 只读验证群列表、成员列表和分页。
2. 发送测试消息并撤回。
3. 临时禁言测试成员并立即解禁。
4. 临时修改测试成员名片并恢复原名。
5. 保存原公告，新增或修改测试公告，再恢复原公告。
6. 开启全员禁言并立即解除全禁。
7. 每项保存请求、双层回执、回调和状态回读，确认原状态已经恢复。
8. 生成脱敏 Contract v2，通过回放后更新生产能力校准表并重新构建生产版。

Fixture 只能验证代码逻辑，不产生真实旺商聊写操作证据。生产能力按项开放：ZCG 基线能力以启动时路由、Electron IPC、双层回执和 NIM 只读探测为准；旺商聊专有写能力在没有真实回读证据时显示“待首次手工验证”，不会被 Fixture 直接标为可用。

开发版“调试 → 真实 9222 能力校准”会把原始轨迹写入 `%APPDATA%\DH\developer\contracts\raw`，其中记录旺商聊文件版本、页面标题与地址、主脚本 SHA-256、IPC/NIM 请求、双层响应、回调和写后回读。原始文件只保留在本机，不提交、不打包。开发版只在 `real`、`127.0.0.1:9222` 且 NIM 已就绪时允许开始采集；Fixture、9233 和 51300 会被后端拒绝。

提交前使用 `node scripts/sanitize-contract-capture.mjs INPUT OUTPUT` 生成确定性脱敏副本。脚本检测到 API Key、Authorization、Cookie、密码、Token 或私钥时会终止；输出中的账号、群、成员、NIM、消息标识、公告和测试文本会按首次出现顺序替换为稳定占位值。随后执行 `pnpm verify:calibration-capture -- contracts/TRACE.sanitized.json`；只有成功、回读和恢复证据完整的旺商聊专有能力会输出 `supported`，移出成员仍只开放人工确认入口。

能力状态首先由启动时的只读运行时探测决定：旺商聊页面、Electron IPC、ZCG 基线路由、双层回执结构和 NIM 方法一致时，未知脚本 SHA 也可逐项开放对应能力。应用版本和主脚本 SHA-256 继续保存为诊断指纹；路由缺失、响应结构变化或解码失败才会把能力降为 `Unavailable`。旺商聊专有公告等写能力仍需 Contract v2 的真实回读证据，未完成首验时只开放人工验证入口。
