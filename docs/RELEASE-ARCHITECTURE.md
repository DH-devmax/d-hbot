# DH BOT 发布仓库边界

## 仓库职责

- `DH-devmax/d-hbot`：私有源码仓库，保存 Rust/Tauri、脱敏契约、Fixture、本地测试脚本和开发文档。仓库 Actions 停用，不生成任何构建产物。
- `DH-devmax/d-hbot-releases`：公开发行仓库，唯一负责 Windows 生产构建、签名、扫描和 GitHub Release。
- `sh492773746/d-hbot` 与 `sh492773746/d-hbot-releases`：旧账号归档，不接收提交、Secrets 或发布。

公开发行仓库不保存 Rust、TypeScript、Go、PDB、Source Map、Fixture、旺商聊原始协议、账号数据或构建凭据。Windows Runner 只在任务期间读取指定的私有源码提交，任务结束后由 GitHub 托管环境销毁工作目录。

## 生产 Actions 链路

1. 开发机完成生产、Fixture、界面和实机测试，并记录准备发布的 40 位源码 commit SHA。
2. 管理员在公开发行仓库手动运行 `build-production.yml`，输入 `source_commit` 和 `release_tag`。
3. workflow 先校验 SHA、标签、现有 Tag/Release 和三个必需 Secrets，再使用只读 Token 检出精确源码提交。
4. Windows Hosted Runner 执行生产边界、契约、Rust、React 和 Clippy 门禁，再用 `--no-default-features` 构建。
5. 主程序与 NSIS 安装包完成 Authenticode 签名和 `signtool verify`，随后执行 NSIS、portable 和生产隔离深度扫描。
6. workflow 生成来源清单与 `SHA256SUMS.txt`，使用发行仓库自己的 `GITHUB_TOKEN` 创建公开 Release。

生产 workflow 不接受 push、pull request、fork 或源码仓库事件触发，不使用 `incoming/<tag>` 中转，也不覆盖既有标签或 Release。

## Secrets

Secrets 只配置在 `DH-devmax/d-hbot-releases`：

| Secret | 最小权限与用途 |
| --- | --- |
| `DH_SOURCE_READ_TOKEN` | Fine-grained Token，只授予 `DH-devmax/d-hbot` 的 Metadata/Contents 只读权限 |
| `DH_SIGN_PFX_B64` | Authenticode PFX 的 Base64 内容 |
| `DH_SIGN_PASSWORD` | PFX 密码 |

源码仓库不保存以上 Secrets。旧 `DH_RELEASE_DEPLOY_KEY` 及发行仓库写入 Deploy Key 在迁移完成后删除。

配置 Secrets 时只在受控终端操作，禁止把 Token、PFX、密码或解码文件放入仓库、日志和命令历史。配置完成后仅用 `gh secret list` 核对名称与更新时间，不读取 Secret 内容。

## 手动发布

1. 确认本地工作树、源码远端、活动 GitHub 账号和本地测试报告。
2. 推送源码 commit，复制 `git rev-parse HEAD` 的完整 40 位 SHA。
3. 在 `DH-devmax/d-hbot-releases/actions` 选择 `Build DH BOT production`。
4. 输入完整 `source_commit` 和与 `tauri3/package.json` 一致的 `release_tag`。
5. workflow 全部通过后，从公开 Release 下载并按 `SHA256SUMS.txt` 做 Windows 实机验收。

缺少签名证书、Token、版本不一致、重复标签或任一生产门禁失败时，workflow 停止且不创建公开 Release。

## 回滚

- workflow 失败：修复源码或仓库配置后使用新的源码 SHA 重新运行；失败任务不会留下半成品 Release。
- Release 已创建但实机验收失败：将 Release 标记为预发布并保留证据，修复后使用新版本号发布，不覆盖原资产。
- 源码读取凭据异常：立即撤销 Fine-grained Token，重新创建最小权限 Token 并更新发行仓库 Secret。
