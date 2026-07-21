# DH BOT Developer Build

开发版用于本地协议、成员、消息和自动化回归，不接触真实旺商聊数据。

## 启动

```text
pnpm install
pnpm dev:fixture
```

开发版使用：

- 页面服务：`127.0.0.1:51300`
- DevTools：`127.0.0.1:9233`
- 数据目录：`%APPDATA%\DH\fixture`

## 测试与构建

```text
pnpm test:fixture
pnpm tauri:build:developer
pnpm package:windows:developer
```

开发构建产物为内部 ZIP，包含 `DH-BOT-Dev.exe`、`DH-Fixture.exe` 和校验文件；不上传到公开发布目录。

生产构建使用 `pnpm tauri:build:production` 和 `pnpm package:windows:production`，该产物不携带开发测试模块、9233 端口或 Fixture 资源。
生产构建会显式将 `--no-default-features` 传给 Cargo，并在编译前删除曾暂存的 `DH-Fixture` 资源和开发二进制。`pnpm test:production-isolation` 验证扫描器会拦截 Fixture 命令、9233/51300 端口、开发数据目录和环境变量；Windows 打包还会用 7-Zip 展开 NSIS 安装器，并复查解压后的 portable ZIP。

开发版的运行模式文件位于 `%APPDATA%\DH\developer\runtime-mode`，Fixture 数据位于 `%APPDATA%\DH\fixture`；生产版固定使用 `%APPDATA%\DH\3.0` 和真实旺商聊 `9222`。

## 旺商聊契约采集

真实契约原始轨迹只放在 `contracts/raw/`，其中记录旺商聊文件版本、页面标题与地址、主脚本 SHA-256、IPC/NIM 请求、双层响应和回调。该目录内容由 Git 忽略，不用于打包。

提交前使用 `node scripts/sanitize-contract-capture.mjs INPUT OUTPUT` 生成确定性脱敏副本。脚本检测到 API Key、Authorization、Cookie、密码、Token 或私钥时会终止；输出中的账号、群、成员、NIM 和消息标识会按首次出现顺序替换为稳定占位值。

能力状态只按旺商聊文件版本与主脚本 SHA-256 的精确组合校准。未知组合保留只读同步，写能力显示为 `Unverified`，完成 Contract v2 回放后再加入校准表。
