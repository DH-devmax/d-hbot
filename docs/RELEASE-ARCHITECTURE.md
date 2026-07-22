# DH BOT 发布仓库边界

## 仓库职责

- `DH-devmax/d-hbot`：当前私有源码仓库，保存 Rust/Tauri 源码、脱敏协议契约、内部 Fixture、测试与签名流程。
- `DH-devmax/d-hbot-releases`：当前公开发行仓库，只保存 README、发布校验 workflow、签名后的 Windows 产物、手册和 SHA-256。
- `sh492773746/d-hbot` 与 `sh492773746/d-hbot-releases`：旧账号归档，当前 workflow 不写入。

公开发行仓库不接收 Rust、TypeScript、Go、PDB、Source Map、Fixture、旺商聊原始协议采集、账号数据或构建凭据。

## Actions 链路

1. 私有仓库的 `tauri-windows.yml` 运行生产测试、隔离扫描和 Windows 构建。
2. `v3.*` 标签要求 Authenticode 证书并验证所有 PE 签名。
3. 私有 workflow 使用 `DH_RELEASE_DEPLOY_KEY` 将发行文件写入公开仓库的 `incoming/<tag>/`。
4. Deploy Key 仅对公开发行仓库拥有写权限，不具备读取私有源码仓库或其他仓库的权限。
5. 公开 workflow 校验标签、文件扩展名、清单与 `SHA256SUMS.txt`。
6. 校验通过后使用公开仓库自己的 `GITHUB_TOKEN` 创建 Release，并清理工作树中的临时投递目录。

## 所需 Secrets

- `DH_RELEASE_DEPLOY_KEY`：仓库级 Deploy Key 私钥，仅用于向公开发行仓库投递产物。
- `DH_SIGN_PFX_B64`：Authenticode PFX 的 Base64 内容。
- `DH_SIGN_PASSWORD`：PFX 密码。

Secrets 只配置在私有源码仓库。公开发行仓库不保存私有源码访问令牌和签名证书。

## 新账号配置状态

当前活动线路：

| 项目 | 状态 |
| --- | --- |
| 私有源码 | `DH-devmax/d-hbot` |
| 公开发行 | `DH-devmax/d-hbot-releases` |
| `DH_RELEASE_DEPLOY_KEY` | 已配置到新私有仓库；公钥只挂在新公开发行仓库 |
| `DH_SIGN_PFX_B64` | 待配置真实 Authenticode PFX |
| `DH_SIGN_PASSWORD` | 与 PFX 同时配置 |
| 旧账号仓库 | 仅归档，不参与新 Actions |

配置签名 Secrets 时，在本地受控终端执行，不把 PFX、密码或解码文件放入仓库：

```sh
base64 < signing-certificate.pfx | tr -d '\\n' | gh secret set DH_SIGN_PFX_B64 --repo DH-devmax/d-hbot
printf '%s' "$PFX_PASSWORD" | gh secret set DH_SIGN_PASSWORD --repo DH-devmax/d-hbot
gh secret list --repo DH-devmax/d-hbot
```

正式 `v3.*` 标签会强制要求这两个 Secrets；未配置时，手动分支构建仍可运行测试和生成内部未签名产物，标签发布会在签名准备阶段停止。

## 发布命令

确认版本字段与标签一致后创建标签：

```sh
git tag -s v3.0.0 -m "DH BOT 3.0.0"
git push origin v3.0.0
```

标签构建通过后，公开 Release 由两段 Actions 自动完成。未配置 Authenticode 证书时，标签发布会停止在签名准备阶段，普通手动构建仍可作为内部验证产物。
