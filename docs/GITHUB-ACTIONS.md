# DH BOT GitHub Actions 运维说明

## 当前线路

| 用途 | 仓库 | 可见性 | 内容 |
| --- | --- | --- | --- |
| 源码与构建 | `DH-devmax/d-hbot` | 私有 | Rust/Tauri、React、契约、Fixture、测试和 CI |
| 公开发行 | `DH-devmax/d-hbot-releases` | 公开 | EXE、ZIP、手册、规则模板和 SHA-256 |
| 历史归档 | `sh492773746/d-hbot`、`sh492773746/d-hbot-releases` | 旧线路 | 只读参考，不接收新投递 |

本地源码仓库：

```sh
git remote -v
# origin  -> git@github.com:DH-devmax/d-hbot.git
# legacy  -> https://github.com/sh492773746/d-hbot.git
```

## 账号校验

- 当前新线路账号：`DH-devmax`。
- 旧账号：`sh492773746`，仅在维护历史仓库时使用。
- 源码仓库已启用 `.githooks/pre-commit` 和 `.githooks/pre-push`。
- hook 会核对远端所有者、活动 `gh` 账号、Git 作者姓名和邮箱。
- 不使用 `--no-verify` 绕过检查。

提交或推送前：

```sh
gh auth status --active
git status --short
git diff --cached --check
```

## Secrets

只在私有源码仓库配置 Secrets：

| Secret | 用途 | 当前状态 |
| --- | --- | --- |
| `DH_RELEASE_DEPLOY_KEY` | 把构建产物投递到公开发行仓库 | 已配置；Deploy Key 只对发行仓库写入 |
| `DH_SIGN_PFX_B64` | Authenticode PFX 的 Base64 内容 | 待真实证书 |
| `DH_SIGN_PASSWORD` | PFX 密码 | 与真实证书一起配置 |

配置命令在受控终端执行，禁止把 PFX、密码、解码文件或 Secret 输出到日志：

```sh
base64 < signing-certificate.pfx | tr -d '\\n' | gh secret set DH_SIGN_PFX_B64 --repo DH-devmax/d-hbot
printf '%s' "$PFX_PASSWORD" | gh secret set DH_SIGN_PASSWORD --repo DH-devmax/d-hbot
gh secret list --repo DH-devmax/d-hbot
```

本机没有真实 Authenticode PFX 时，保持两个签名 Secret 为空。手动分支构建可生成内部未签名产物；`v3.*` 标签会在签名准备阶段检查并停止。

## Workflow

私有源码仓库：

- `tauri-windows.yml`：生产测试、Windows 构建、隔离扫描、签名和投递。
- `tauri-developer.yml`：内部 Fixture 构建，不进入公开发行仓库。

生产构建只允许在 `DH-devmax/d-hbot` 执行，并固定投递到 `DH-devmax/d-hbot-releases`。生产包不包含 Fixture、`9233/51300`、测试数据库或开发命令。

公开发行仓库的 `publish-staged-release.yml` 会校验：

1. `release.json` 来源必须是 `DH-devmax/d-hbot`。
2. 标签符合 `v3.x.y` 格式。
3. 文件名没有目录、路径穿越、源码、调试文件或 Fixture。
4. `SHA256SUMS.txt` 校验全部通过。
5. 校验后创建 GitHub Release，再清理 `incoming/<tag>`。

## 运行检查

完成 `workflow` scope 授权并推送源码后：

```sh
gh workflow list --repo DH-devmax/d-hbot
gh workflow run tauri-windows.yml --repo DH-devmax/d-hbot --ref codex/tauri-3-backend
gh run list --repo DH-devmax/d-hbot --limit 5
```

手动分支构建不会要求签名证书；正式标签前必须先确认三个 Secrets：

```sh
gh secret list --repo DH-devmax/d-hbot
```

Actions 额度、Windows Runner、PFX 证书和 Deploy Key 是四个独立条件。一个条件通过，不代表另外三个已经就绪。

## Windows 实机验收

Actions 成功只代表构建、自动测试和生产隔离通过。任务栏、托盘、旺商聊登录状态、9222 实际进程和退出残留使用 [WINDOWS-REAL-MACHINE-TEST.md](WINDOWS-REAL-MACHINE-TEST.md) 中的实机探针与人工清单验收。探针属于私有开发工具，不随公开生产包发布。
