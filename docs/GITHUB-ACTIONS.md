# DH BOT GitHub Actions 运维说明

## 当前线路

| 用途 | 仓库 | 可见性 | Actions |
| --- | --- | --- | --- |
| 私有源码 | `DH-devmax/d-hbot` | 私有 | 停用；测试和开发构建只在本地执行 |
| 生产构建与发行 | `DH-devmax/d-hbot-releases` | 公开 | 唯一生产 workflow，手动触发 |
| 历史归档 | `sh492773746/d-hbot`、`sh492773746/d-hbot-releases` | 旧线路 | 不接收当前线路的提交或发布 |

源码仓库历史 Actions 记录只表示旧流程，不是当前生产入口。

## 账号与提交检查

- 当前线路账号：`DH-devmax`。
- 源码 `origin`：`git@github.com:DH-devmax/d-hbot.git`。
- 旧账号 `sh492773746` 只用于历史仓库维护。
- 提交和推送必须经过 `.githooks/pre-commit` 与 `.githooks/pre-push`，不得使用 `--no-verify`。

每次提交和发布前执行：

```sh
gh auth status --active
git status --short
git diff --cached --check
git remote get-url --push origin
git config user.name
git config user.email
```

## 本地门禁

在 `tauri3/` 依次执行：

```sh
pnpm install --frozen-lockfile
pnpm test:contract-sanitizer
pnpm test:production-isolation
pnpm test:production
pnpm test:fixture
pnpm test:ui
pnpm test:e2e:fixture
cargo clippy --manifest-path src-tauri/Cargo.toml --no-default-features --all-targets -- -D warnings
```

Windows 开发机另外执行生产构建、`package:windows:production`、`verify:windows:production` 和 `test:windows:real`。本地门禁失败时不触发发行仓库 workflow。

## 生产 workflow

`DH-devmax/d-hbot-releases` 只保留 `build-production.yml`，通过 `workflow_dispatch` 接收：

- `source_commit`：`DH-devmax/d-hbot` 中完整的 40 位 commit SHA。
- `release_tag`：与应用版本一致的 `v3.x.y` 或预发布标签。

workflow 固定检出精确 SHA，运行生产门禁，构建 `--no-default-features`，签名所有 PE，扫描 NSIS/portable，并直接创建公开 Release。它不注册 PR、push 或 fork 触发，也不构建 Fixture。

## 发行仓库 Secrets

| Secret | 用途 |
| --- | --- |
| `DH_SOURCE_READ_TOKEN` | 只读检出私有源码仓库 |
| `DH_SIGN_PFX_B64` | Authenticode PFX Base64 |
| `DH_SIGN_PASSWORD` | PFX 密码 |

三个 Secrets 只配置在发行仓库。缺少任一 Secret 时生产 workflow 在检出或构建前停止，不生成公开资产。源码仓库不再使用 `DH_RELEASE_DEPLOY_KEY`。

## 运行与检查

```sh
gh workflow list --repo DH-devmax/d-hbot-releases
gh workflow run build-production.yml \
  --repo DH-devmax/d-hbot-releases \
  -f source_commit=40_HEX_SOURCE_COMMIT \
  -f release_tag=v3.0.0
gh run list --repo DH-devmax/d-hbot-releases --limit 5
```

生产通过后检查：

```sh
gh release view v3.0.0 --repo DH-devmax/d-hbot-releases
```

公开 Release 只包含已签名 EXE、安装包、portable ZIP、手册、默认规则、来源清单和 `SHA256SUMS.txt`。

## Windows 实机验收

Actions 证明 Windows MSVC 构建、自动测试、签名和生产隔离通过；任务栏、托盘、旺商聊登录状态、9222 和退出残留仍按 [WINDOWS-REAL-MACHINE-TEST.md](WINDOWS-REAL-MACHINE-TEST.md) 验收。实机对象必须从公开 Release 下载，开发 Fixture 和本地未签名包不作为正式验收对象。
