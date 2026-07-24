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
- 生产构建：只在受控 Windows 开发机执行，使用 `--no-default-features`，构建后进行生产隔离扫描和实机验收。
- 个人发行：上传未签名 portable ZIP 与 `SHA256SUMS.txt` 到管理员控制的云盘。
- 公开发行仓库：`DH-devmax/d-hbot-releases` 只保留下载说明或历史索引，Actions 停用，不保存源码、Fixture、证书或构建凭据。
- 旧仓库 `sh492773746/d-hbot` 与 `sh492773746/d-hbot-releases` 仅作为历史归档，当前 Actions 不再依赖。
- 云盘目录只包含 portable ZIP、可选安装包、手册、规则模板和 `SHA256SUMS.txt`，不上传 PFX、密码、Token、源码、PDB 或 Source Map。

## 账号与 Actions 隔离

- 当前源码 `origin` 应指向 `git@github.com:DH-devmax/d-hbot.git`。
- 当前生产文件分发目标为管理员控制的云盘；GitHub 发行仓库只作可选索引。
- 旧账号仓库不接受新线路的提交或发布投递。
- `.githooks/pre-commit` 与 `.githooks/pre-push` 会核对远端所有者、活动 `gh` 账号和 Git 作者；账号不一致时停止。

两个当前仓库均停用 Actions。云盘文件、校验清单和实机报告不得包含私有源码、Token、证书、原始协议或真实群数据。

## 日志边界

日志和崩溃报告必须过滤 API Key、Webhook Token、Cookie、Authorization、账号会话与原始协议响应。真实群名、成员和消息不得进入仓库 fixture；测试数据统一使用占位身份。
