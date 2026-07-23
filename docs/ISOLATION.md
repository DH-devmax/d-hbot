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

- 私有源码：`DH-devmax/d-hbot`，保存 Rust/Tauri、脱敏契约、Fixture 和本地测试工具；Actions 停用，不生成构建产物。
- 公开发行：`DH-devmax/d-hbot-releases`，唯一运行生产 Actions，只保存 README、生产 workflow 和签名产物。
- 旧仓库 `sh492773746/d-hbot` 与 `sh492773746/d-hbot-releases` 仅作为历史归档，当前 Actions 不再依赖。
- 发行 Actions 使用 `DH_SOURCE_READ_TOKEN` 只读检出私有源码的精确 commit SHA，不接受分支或可变引用。
- `DH_SOURCE_READ_TOKEN`、`DH_SIGN_PFX_B64` 和 `DH_SIGN_PASSWORD` 只配置在公开发行仓库 Secrets。
- 公开 Release 只包含 EXE/ZIP、手册、规则模板和 `SHA256SUMS.txt`。

## 账号与 Actions 隔离

- 当前源码 `origin` 应指向 `git@github.com:DH-devmax/d-hbot.git`。
- 当前发布目标固定为 `DH-devmax/d-hbot-releases`。
- Fine-grained 源码 Token 只拥有 `DH-devmax/d-hbot` 的 Metadata/Contents 读取权限，不能写源码或访问其他仓库。
- 旧账号仓库不接受新线路的提交或发布投递。
- `.githooks/pre-commit` 与 `.githooks/pre-push` 会核对远端所有者、活动 `gh` 账号和 Git 作者；账号不一致时停止。

发行 workflow 只允许手动触发，不接受 PR、push、fork 或源码仓库事件。公开 Runner 的日志、缓存和 Release 不得包含私有源码、Token、证书、原始协议或真实群数据。

## 日志边界

日志和崩溃报告必须过滤 API Key、Webhook Token、Cookie、Authorization、账号会话与原始协议响应。真实群名、成员和消息不得进入仓库 fixture；测试数据统一使用占位身份。
