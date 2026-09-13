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

- 源码：`DH-devmax/d-hbot`，保存 Rust/Tauri、脱敏契约、Fixture 和 CI；Actions 生成短期保留的测试与生产构建 artifact。
- 生产构建：由 GitHub Actions 的 Windows runner 执行，使用 `--no-default-features`，构建后进行生产隔离扫描；真实桌面验收在自托管 Windows runner 执行。
- 个人发行：上传未签名 portable ZIP 与 `SHA256SUMS.txt` 到管理员控制的云盘。
- 公开发行仓库：`DH-devmax/d-hbot-releases` 只保留下载说明或历史索引，不保存源码、Fixture、证书或构建凭据。
- 旧仓库 `sh492773746/d-hbot` 与 `sh492773746/d-hbot-releases` 仅作为历史归档，当前 Actions 不再依赖。
- 云盘目录只包含 portable ZIP、可选安装包、手册、规则模板和 `SHA256SUMS.txt`，不上传 PFX、密码、Token、源码、PDB 或 Source Map。

## 账号与 Actions 隔离

- 当前源码 `origin` 的所有者必须是 `DH-devmax/d-hbot`；HTTPS 与 SSH 传输均可，实际 URL 以 `git remote get-url --push origin` 为准。
- 当前生产文件分发目标为管理员控制的云盘；GitHub 发行仓库只作可选索引。
- 旧账号仓库不接受新线路的提交或发布投递。
- `.githooks/pre-commit` 与 `.githooks/pre-push` 会核对远端所有者、活动 `gh` 账号和 Git 作者；账号不一致时停止。

源码仓库 Actions 只读取公开代码并上传构建结果；云盘文件、校验清单和实机报告不得包含 Token、证书、原始协议或真实群数据。

提交前必须同时通过 `pnpm verify:docs`、Contract sanitizer 和生产隔离检查。仓库脱敏扫描会读取 Git 跟踪文件及归档 ZIP；真实测试群、长消息 ID、用户主目录、Bearer/JWT、私钥和常见 API/GitHub 密钥格式均视为失败。

## 日志边界

日志和崩溃报告必须过滤 API Key、Webhook Token、Cookie、Authorization、账号会话与原始协议响应。真实群名、成员和消息不得进入仓库 fixture；测试数据统一使用占位身份。

`调试 → 生成诊断包` 只在用户手工点击后于本机写入 `%APPDATA%\DH\3.0\support-bundles`。包内只允许脱敏连接状态、能力摘要、匿名化审计、脱敏近期日志、清单和校验和；禁止包含 `dh.db`、`secrets.dat`、旺商聊登录分区、Cookie、Token、API Key、原始消息、真实群名、成员名、原始协议或 Fixture 数据。诊断包以临时文件原子提交，同一秒连续生成自动编号，保留上限为 20 个、30 天和 128 MiB。生产包可使用诊断包功能，但不带 Fixture 端口、Fixture 命令或 Fixture 数据。
