# DH BOT 隔离规范

## 构建隔离

| 项目 | 生产版 | 开发版 |
|---|---|---|
| Rust feature | `--no-default-features` | `--features fixture` |
| 产品 | `DH-BOT.exe` | `DH-BOT-Dev.exe` + `DH-Fixture.exe` |
| DevTools | `127.0.0.1:9222` | Fixture `127.0.0.1:9233` |
| 页面服务 | 无 | `127.0.0.1:51300` |
| 数据 | `%APPDATA%\DH\3.0` | `%APPDATA%\DH\fixture` |
| 测试入口 | 编译时移除 | 显示开发测试环境 |

生产构建忽略 Fixture 环境变量和旧运行模式文件。生产扫描覆盖前端、Rust 二进制、NSIS 与 ZIP，阻止 Fixture 命令、端口、测试数据和开发路径进入发行包。

## 数据与凭据

- DH 数据库与旺商聊 Electron 登录分区分开；DH 不保存旺商聊密码。
- AI 密钥在 Windows 使用 DPAPI，数据库与日志只保存引用或配置状态。
- 原始协议采集保存在忽略目录 `tauri3/contracts/raw/`，仓库只接受确定性脱敏 Contract v2。
- Fixture 不读取旺商聊目录、真实凭据或生产数据库。

## 仓库与发布

- 私有源码：`sh492773746/d-hbot`，保存 Rust/Tauri、脱敏契约、Fixture 和测试。
- 公开发行：`sh492773746/d-hbot-releases`，只保存 README、校验 Actions 和签名产物。
- 私有 Actions 使用只对公开发行仓库有写权限的 `DH_RELEASE_DEPLOY_KEY`。
- `DH_SIGN_PFX_B64` 和 `DH_SIGN_PASSWORD` 只配置在私有仓库 Secrets。
- 公开 Release 只包含 EXE/ZIP、手册、规则模板和 `SHA256SUMS.txt`。

## 日志边界

日志和崩溃报告必须过滤 API Key、Webhook Token、Cookie、Authorization、账号会话与原始协议响应。真实群名、成员和消息不得进入仓库 fixture；测试数据统一使用占位身份。
