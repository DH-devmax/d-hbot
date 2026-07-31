# DH BOT 协作开发要求

## 协作范围

- 协作者只加入私有源码仓库 `DH-devmax/d-hbot`。
- `DH-devmax/d-hbot-releases` 不授予源码写入权限，继续作为下载说明和历史索引。
- 旧账号仓库不参与当前开发线路。
- 协作者不得接触真实旺商聊账号、Cookie、Token、个人访问令牌、AI 密钥、云盘凭据或原始协议采集目录。
- Fixture、测试数据库和测试端口只用于本地开发，不进入生产包和云盘。

## 建议权限

第二 GitHub 账号使用独立账号登录，通过 Pull Request 协作。默认授予私有源码仓库 `Write` 权限；需要管理仓库设置时再临时提升为 `Maintain`。不直接授予 `Admin`，不共享个人访问令牌。

协作者的分支命名使用 `codex/<topic>` 或 `<username>/<topic>`，禁止直接向 `main` 推送。合并前至少完成一次审阅，并确认改动没有混入 Fixture、测试数据、密钥或生产发布文件。

## 每次提交前检查

```text
git status --short
git diff --cached
git remote get-url --push origin
gh auth status --active
git config user.name
git config user.email
```

目标必须是 `DH-devmax/d-hbot`，活动账号必须是当前协作账号，提交范围必须与任务一致。禁止使用 `--no-verify` 绕过仓库钩子。

## 本地验证门槛

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

涉及 Windows 发布时，还必须在受控 Windows 机器执行 MSVC 生产构建、生产包扫描和 `scripts/test-windows-real-machine.ps1`。未通过真实旺商聊连接、登录复用、任务栏/托盘、退出残留和 DPI 检查前，不得标记为 RC 或正式版。

## 功能要求

- 生产版固定连接 `127.0.0.1:9222`，不保留 Fixture 入口。
- AI 只响应明确 `@DH` 或旺商聊提及元数据。
- 默认规则、知识库、业务应用、计划和自动群名片保持停用，管理员明确启用后才运行。
- 群公告、批量公告、全员禁言和解除全禁逐群校验权限、能力状态、回执和审计。
- 旺商聊版本变化后重新运行只读能力探测；路由签名、响应结构和 NIM 方法一致时继续开放 ZCG 基线能力，结构变化的单项能力显示“当前不可用”。
- 所有外部副作用必须具备稳定去重键、回执归档和失败/未知状态。

## 生产发布边界

- 构建、扫描、打包和发布全部在本地完成，GitHub Actions 保持停用。
- 发行包只包含生产 EXE、portable ZIP、手册、默认规则和 `SHA256SUMS.txt`。
- 不得发布 Fixture、开发 EXE、`9233/51300`、源码、PDB、Source Map、测试数据库、PFX、私钥、密码、Token、Cookie 或原始协议。
- 云盘上传后必须重新下载并用 SHA-256 回读校验。

## 协作交付格式

每个 PR 或本地交付说明应包含：改动摘要、涉及页面/命令/数据库表、测试命令及结果、是否影响真实旺商聊写操作、生产隔离扫描结果和回滚方式。ZCG 基线路由变化时更新协议测试；旺商聊专有写能力变化时先更新脱敏 Contract v2，并保持对应能力为“待首次手工验证”。
