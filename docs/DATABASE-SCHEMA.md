# DH BOT 3.0 数据模型

当前数据库版本为 **schema v13**。数据库只保存 DH BOT 的运行状态；旺商聊的 Cookie、
Token、Local Storage 和登录分区由旺商聊自己管理，不进入 `dh.db`。

首次启动默认创建空运行状态：规则模板全部停用，内置知识库启用但不绑定群，预测应用和
计划停用，AI Provider 地址与密钥留空，模型默认 `deepseek-v4-pro`。账号级默认内容由
`defaults.content.version.2.<accountId>` 幂等标记控制，不覆盖管理员已有配置。

## 路径和启动

| 渠道 | 数据库 | 密钥 | 说明 |
|---|---|---|---|
| Production | `%APPDATA%\\DH\\3.0\\dh.db` | `secrets.dat` | 真实旺商聊 |
| Developer | `%APPDATA%\\DH\\fixture\\dh.db` | 独立开发密钥 | Fixture，不与生产串库 |

打开数据库时执行 WAL、外键约束和 `PRAGMA quick_check`。检查失败时保留原文件并进入
恢复页，不覆盖数据库。迁移前生成快照；迁移使用事务，失败时保留快照和错误信息。
旧 Go 数据不迁移到 3.0；旧文件只归档到 `legacy-backups` 供人工回看。

## 核心表

| 表 | 作用 | 关键身份/唯一约束 |
|---|---|---|
| `accounts` | 旺商聊账号发现信息 | `id` |
| `groups` | 群配置、AI/机器规则开关和人工接管 | `account_id + group_id` |
| `members` | 成员当前/原始/管理名片、角色和生命周期 | `account_id + group_id + user_id` |
| `member_identity_aliases` | NIM 临时身份到稳定 userId 的合并 | `account_id + group_id + nim_id` |
| `messages` | 群消息、处理状态、提及和来源 | `account_id + group_id + server_message_id` |
| `gateway_inbox` | 有序桥接事件、ACK 和恢复队列 | `account_id + event_id`；桥接序号另有唯一约束 |
| `effect_outbox` | 所有外部副作用的持久队列 | `account_id + dedupe_key`（非空时） |
| `actions` | 动作意图、执行结果和脱敏回执 | 由 dedupe key/消息/规则关联 |
| `audit_events` | 可读的来源、事件、级别和详情 | 自增 ID，按时间查询 |

## 规则和知识

| 表 | 作用 |
|---|---|
| `rules` | 规则定义；`rule_type` 为 `machine` 或 `ai`，`scope` 为 `global` 或 `selected` |
| `rule_groups` | 一条规则绑定多个群 |
| `rule_whitelist_members` | 稳定 userId 白名单，不保存逗号拼接文本 |
| `rule_evaluations` | 检测、置信度、观察/自动决定和原因 |
| `rule_actions` | 撤回、禁言、移出等固定动作顺序 |
| `rule_runtime_state` | 旧时间窗/计数兼容状态；v2 规则不使用规则冷却 |
| `knowledge_bases` | 多群知识库、内置/只读标志 |
| `knowledge_documents` | TXT/Markdown/FAQ 内容和内容哈希 |
| `knowledge_chunks` | 稳定分块和中文检索字段 |
| `knowledge_base_groups` | 一个库绑定多个群、一个群绑定多个库 |

## AI、任务和业务应用

| 表 | 作用 |
|---|---|
| `group_ai_permissions` | 回复、任务、撤回、禁言、移出分别授权 |
| `ai_provider_endpoints` | Provider 优先级、模型、Responses 思考深度、健康状态和密钥引用；不保存明文密钥。`xhight` 输入会规范为 `xhigh`，默认思考深度为 `low` |
| `ai_runs` | 同一消息/决策的幂等和重试状态 |
| `tasks` | 负责人、创建人、截止时间、提醒 claim 和结果 |
| `daily_summaries` / `summary_runs` | 按账号、群、本地日期生成的私密摘要 |
| `group_schedules` / `group_schedule_groups` / `schedule_runs` | 每日开群/关群、时区、跨午夜和执行历史 |
| `business_apps` / `business_app_runs` | 业务应用注册、启停、健康和运行记录 |
| `card_rename_jobs` | 名片改名、验证、欢迎和失败重试 |
| `gateway_capability_verifications` | 协议指纹、能力来源、状态、证据哈希和脱敏错误 |
| `app_settings` | 非敏感应用设置；敏感值只保存引用或由 DPAPI 保护 |

## 状态和一致性规则

- 所有外部动作先入 `effect_outbox`，成功回执再更新 `actions` 和审计；未知回执不当作成功。
- `messages.processing_state`、`gateway_inbox.state`、outbox `state` 和任务提醒状态均可在
  重启后恢复。领取使用 claim 时间和稳定去重键，避免重复发送。
- 成员离群只在权威完整名单确认后标记 `present=false`，不删除后缀、黑名单、锁定名片和历史。
- 文档按内容哈希去重；知识库绑定或文档更新会让检索版本改变，旧缓存自然失效。
- 账号切换时前端清空旧账号内存状态；查询始终带 `account_id`，不得跨账号复用群、成员或知识。
- 所有时间字段按 UTC 存储；计划额外保存用户电脑时区和本地运行日期，DST/跨午夜使用同一
  时区计算器。

## 维护和回滚

维护人员只通过应用的调试页查看完整性和生成诊断包，不直接复制 `dh.db`。诊断包只含
匿名化审计、状态摘要、脱敏日志和校验清单。升级前的数据库快照保留在本机备份目录；
出现 `quick_check` 或迁移失败时，先停止写入、保留错误现场，再使用最近快照恢复，不能
用空库覆盖原数据。
