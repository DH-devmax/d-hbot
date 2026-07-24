# DH BOT GitHub Actions 状态

## 当前结论

DH BOT 不使用 GitHub Actions 构建程序：

| 仓库 | 用途 | Actions |
|---|---|---|
| `DH-devmax/d-hbot` | 私有 Rust/Tauri 源码、Fixture、脱敏契约和本地工具 | 停用 |
| `DH-devmax/d-hbot-releases` | 可选下载说明与历史发行索引 | 停用 |
| `sh492773746/*` | 旧账号归档 | 不参与当前线路 |

测试、Windows MSVC 构建、NSIS/portable 打包、生产隔离扫描和真实桌面验收都在开发机本地完成。GitHub 不保存签名证书、源码读取 Token 或构建产物。

## 本地必检

在 `tauri3` 目录运行：

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

Windows 开发机随后运行：

```text
pnpm tauri:build:production
pnpm package:windows:production
pnpm verify:windows:production
```

最后使用 `scripts/test-windows-real-machine.ps1` 验收 portable ZIP。完整流程见 [LOCAL-RELEASE.md](LOCAL-RELEASE.md)。

## GitHub 检查

以下命令用于确认远端没有活动 workflow：

```text
gh workflow list --repo DH-devmax/d-hbot
gh workflow list --repo DH-devmax/d-hbot-releases
gh api repos/DH-devmax/d-hbot/actions/permissions
gh api repos/DH-devmax/d-hbot-releases/actions/permissions
```

两个权限接口都应返回 `enabled: false`，workflow 列表应为空。后续若采用公共信任代码签名或恢复 GitHub Release，需要先更新本规范和长期记忆，再建立新的独立发布方案。
