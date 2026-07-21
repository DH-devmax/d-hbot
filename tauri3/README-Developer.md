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

开发版的运行模式文件位于 `%APPDATA%\DH\developer\runtime-mode`，Fixture 数据位于 `%APPDATA%\DH\fixture`；生产版固定使用 `%APPDATA%\DH\3.0` 和真实旺商聊 `9222`。
