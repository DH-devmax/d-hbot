# DH BOT 真实业务测试运行手册

本文覆盖**跑在真实旺商聊上的测试**：真实群发送、撤回、禁言线形、@提及、发图、群名片改回、双实例、监听队列注入、AI 与知识库。
这些测试默认一条都不会执行，靠 67 个环境变量解锁。此前仓库里没有任何文档记录它们，跑门禁的人会看到“全绿”而完全不知道这批测试被跳过了。

常规门禁清单见 [ENGINEERING-STANDARDS.md](ENGINEERING-STANDARDS.md)，Windows 桌面层验收见 [WINDOWS-ACCEPTANCE.md](WINDOWS-ACCEPTANCE.md)。本文只管真实业务这一层。

## 1. 为什么默认跑不到

这批测试全部标了 `#[ignore = "..."]`（25 处，零处裸 `#[ignore]`）。静态忽略的含义是：

- **只设环境变量不会让它们执行**，`cargo test` 依然跳过。必须显式加 `-- --ignored`。
- 常规 `pnpm test:production` / `pnpm test:fixture` 的输出里，它们计入 `ignored`，不计入 `failed`。所以漏跑不会让门禁变红。

两个通道的忽略项数量不同：

| 通道 | 命令 | 忽略项 |
|---|---|---:|
| 生产 | `pnpm test:production` | 21 |
| Fixture | `pnpm test:fixture` | 25 |

差的 4 项是 AI 组：`tauri3/src-tauri/tests/macos_real_ai.rs` 顶部是 `#![cfg(all(target_os = "macos", feature = "fixture"))]`，**只在 fixture 通道存在**。

## 2. 前置条件

- **macOS**。三个真实测试文件都带 `#![cfg(target_os = "macos")]`，在 Windows 上根本不编译。Windows 那侧负责的是桌面层与进程层验收，不是这批。
- 本机旺商聊已登录，且带 `--remote-debugging-port=9222`。正常路径是在 DH BOT“设置”里点“启动 / 显示旺商聊”，由它带调试参数拉起；不要手工去 kill 旺商聊。确认方式：浏览器打开 `http://127.0.0.1:9222/json/list` 有返回。
- 双实例那 7 项还需要第二个实例在 9223，并且两个实例都在同一个目标群里。
- 写操作测试需要账号在目标群里有对应管理权限。
- AI 那 4 项需要本机 DH 配置里已保存 Provider，或用变量现配。

## 3. 命令形式

先只列不跑，确认自己将要执行哪些：

```sh
cd tauri3
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features \
  --test macos_real_protocol -- --ignored --list
```

按组执行：

```sh
# 协议组 13 项（生产通道）
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features \
  --test macos_real_protocol -- --ignored

# 双实例组 7 项（生产通道，需要 9222 + 9223）
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features \
  --test macos_real_dual_instance -- --ignored

# AI 组 4 项（只在 fixture 通道）
cargo test --manifest-path src-tauri/Cargo.toml --features fixture \
  --test macos_real_ai -- --ignored

# AI live 1 项（库内单测，fixture 通道）
cargo test --manifest-path src-tauri/Cargo.toml --features fixture \
  --lib -- --ignored ai::tests::live_openai_compatible_provider_accepts_dh_schema
```

单项精确执行，推荐写操作一次只放一项：

```sh
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features \
  --test macos_real_protocol -- --ignored --exact \
  sends_one_explicitly_configured_real_group_message
```

## 4. 只读探针（不写真实群）

这批可以先跑，用来确认连接和权限是否正常。

| 测试 | 需要的变量 |
|---|---|
| `probes_logged_in_macos_wangshangliao_without_writes` | 无，只要 9222 已登录 |
| `reports_real_listener_hook_state_without_writes` | 无 |
| `reports_real_group_mute_wire_shape_without_writes` | 无 |
| `reports_real_nim_recall_methods_without_writes` | 无 |
| `reports_configured_roster_card_names_without_writes` | `DH_REAL_READ_GROUP_ID` |
| `reports_one_member_identity_shape_without_writes` | `DH_REAL_READ_GROUP_ID` `DH_REAL_READ_USER_ID` |
| `finds_one_explicitly_marked_recent_group_message_without_writes` | `DH_REAL_HISTORY_GROUP_ID` `DH_REAL_HISTORY_GROUP_NAME` `DH_REAL_HISTORY_MARKER` |
| `reports_recent_real_messages_matching_marker` | `DH_REAL_DUAL_HISTORY_MARKER` |
| `reports_recent_real_messages_matching_server_ids` | `DH_REAL_DUAL_HISTORY_IDS` |
| `verifies_two_real_instances_share_the_configured_group` | 9222 + 9223 均已登录 |
| `probes_saved_ai_model_catalog_without_group_writes` | `DH_REAL_AI_MODELS_CONFIRM=PROBE_SAVED_MODELS` `DH_REAL_AI_ACCOUNT_ID` `DH_REAL_AI_EXPECT_MODEL` |
| `tests_saved_ai_provider_without_wangshangliao_or_group_writes` | `DH_REAL_AI_CONFIRM=TEST_SAVED_PROVIDER` 等，见第 6 节 |

## 5. 会写真实群的测试

每项都要一个确认令牌，值必须逐字相等。令牌的作用就是防止顺手跑全量时误写真实群。

| 测试 | 确认令牌 | 影响 |
|---|---|---|
| `sends_one_explicitly_configured_real_group_message` | 无令牌，靠 `DH_REAL_SEND_*` 齐全 | 向真实群发一条消息 |
| `recalls_one_explicitly_configured_self_message` | 无令牌，靠 `DH_REAL_RECALL_*` 齐全 | 撤回自己的一条消息 |
| `recalls_one_explicitly_configured_other_members_test_message` | `DH_REAL_RECALL_CONFIRM=RECALL_OTHER_MEMBER` | 撤回他人消息，需管理权限 |
| `closes_and_reopens_two_real_groups_one_minute_apart` | `DH_REAL_SCHEDULE_CONFIRM=CLOSE_AND_REOPEN` | 全员禁言再解除，两个群 |
| `injects_one_explicit_message_into_the_real_listener_queue` | `DH_REAL_INJECT_CONFIRM=INJECT_FOR_RUNNING_RUNTIME` | 需要 DH BOT 运行中 |
| `acknowledges_one_unmapped_listener_event_without_external_writes` | `DH_REAL_UNMAPPED_INJECT_CONFIRM=ACK_UNMAPPED_FOR_RUNNING_RUNTIME` | 需要 DH BOT 运行中 |
| `sends_marked_messages_from_the_secondary_instance` | `DH_REAL_DUAL_SEND_CONFIRM=SEND_FROM_SECONDARY` | 副实例发消息 |
| `sends_real_mention_from_the_secondary_instance` | `DH_REAL_DUAL_MENTION_CONFIRM=SEND_MENTION_FROM_SECONDARY` | 副实例发 @提及 |
| `sends_tiny_image_from_the_secondary_instance` | `DH_REAL_DUAL_IMAGE_CONFIRM=SEND_IMAGE_FROM_SECONDARY` | 副实例发图，需启用图片机器规则 |
| `secondary_card_change_is_restored_by_primary_dh` | `DH_REAL_DUAL_CARD_CONFIRM=CHANGE_AND_RESTORE_OWN_CARD` | 改自己名片再由主 DH 恢复 |
| `configures_saved_ai_provider_from_environment` | `DH_REAL_AI_CONFIGURE_CONFIRM=CONFIGURE_SAVED_PROVIDER` | 写本机 DH 的 AI 配置 |
| `manages_temporary_real_group_knowledge` | `DH_REAL_KNOWLEDGE_CONFIRM=MANAGE_TEMPORARY_KNOWLEDGE` | 写真实知识库 |

## 6. 变量全表

源码中实际读取的共 67 个。

**只读探针（不写真实群）**

- `DH_REAL_READ_GROUP_ID`
- `DH_REAL_READ_USER_ID`
- `DH_REAL_HISTORY_GROUP_ID`
- `DH_REAL_HISTORY_GROUP_NAME`
- `DH_REAL_HISTORY_MARKER`

**真实群发送**

- `DH_REAL_SEND_GROUP_ID`
- `DH_REAL_SEND_GROUP_NAME`
- `DH_REAL_SEND_MEMBER_COUNT`
- `DH_REAL_SEND_TEXT`

**撤回**

- `DH_REAL_RECALL_CONFIRM`
- `DH_REAL_RECALL_GROUP_ID`
- `DH_REAL_RECALL_GROUP_NAME`
- `DH_REAL_RECALL_MESSAGE_ID`
- `DH_REAL_RECALL_SENDER_USER_ID`
- `DH_REAL_RECALL_TEXT_MARKER`

**计划任务（关群/开群）**

- `DH_REAL_SCHEDULE_CONFIRM`
- `DH_REAL_SCHEDULE_GROUP_NAMES`

**监听队列注入**

- `DH_REAL_INJECT_CONFIRM`
- `DH_REAL_INJECT_GROUP_ID`
- `DH_REAL_INJECT_GROUP_NAME`
- `DH_REAL_INJECT_MESSAGE_ID`
- `DH_REAL_INJECT_SENDER_USER_ID`
- `DH_REAL_INJECT_TEXT`
- `DH_REAL_UNMAPPED_INJECT_CONFIRM`

**双实例连接**

- `DH_REAL_PRIMARY_DEVTOOLS_URL`
- `DH_REAL_SECONDARY_DEVTOOLS_URL`
- `DH_REAL_DUAL_GROUP_ID`
- `DH_REAL_DUAL_GROUP_NAME`

**双实例发送**

- `DH_REAL_DUAL_SEND_CONFIRM`
- `DH_REAL_DUAL_SEND_COUNT`
- `DH_REAL_DUAL_SEND_INTERVAL_MS`
- `DH_REAL_DUAL_SEND_TEXT`

**双实例提及**

- `DH_REAL_DUAL_MENTION_CONFIRM`
- `DH_REAL_DUAL_MENTION_ACCOUNT`
- `DH_REAL_DUAL_MENTION_NAME`
- `DH_REAL_DUAL_MENTION_TEXT`

**双实例发图**

- `DH_REAL_DUAL_IMAGE_CONFIRM`

**双实例名片改回**

- `DH_REAL_DUAL_CARD_CONFIRM`
- `DH_REAL_DUAL_CARD_MARKER`
- `DH_REAL_DUAL_CARD_ORIGINAL`
- `DH_REAL_DUAL_CARD_REPEAT`
- `DH_REAL_DUAL_CARD_RESTORE_WAIT_SECONDS`

**双实例历史只读**

- `DH_REAL_DUAL_HISTORY_MARKER`
- `DH_REAL_DUAL_HISTORY_IDS`

**AI 配置写入（仅 fixture 通道）**

- `DH_REAL_AI_ACCOUNT_ID`
- `DH_REAL_AI_API_KEY`
- `DH_REAL_AI_BACKEND`
- `DH_REAL_AI_BASE_URL`
- `DH_REAL_AI_CONFIGURE_CONFIRM`
- `DH_REAL_AI_MODEL`
- `DH_REAL_AI_NAME`

**AI 调用（仅 fixture 通道）**

- `DH_REAL_AI_CONFIRM`
- `DH_REAL_AI_OVERRIDE_BACKEND`
- `DH_REAL_AI_OVERRIDE_MODEL`
- `DH_REAL_AI_OVERRIDE_REASONING`
- `DH_REAL_AI_PROMPT`
- `DH_REAL_AI_MODELS_CONFIRM`
- `DH_REAL_AI_EXPECT_MODEL`

**知识库写入（仅 fixture 通道）**

- `DH_REAL_KNOWLEDGE_CONFIRM`
- `DH_REAL_KNOWLEDGE_ACTION`
- `DH_REAL_KNOWLEDGE_GROUP_ID`

**AI live（库内单测）**

- `DH_AI_LIVE_URL`
- `DH_AI_LIVE_KEY`
- `DH_AI_LIVE_BACKEND`
- `DH_AI_LIVE_MODEL`
- `DH_AI_LIVE_MESSAGE`
- `DH_AI_LIVE_REASONING`

## 7. 变量缺失时会发生什么

我把这 67 个变量逐个查过读取方式：

| 行为 | 数量 | 含义 |
|---|---:|---|
| 缺失即报错 | 32 | 测试硬失败，不会假装通过 |
| 有默认值 | 24 | 会用默认值继续，注意默认值是否真是你想要的 |
| 未判定 | 11 | 读取方式较绕，我没能静态断定，跑之前自己核一遍 |
| 静默返回 | 0 | 未发现“缺变量就提前 return 并报通过”的写法 |

也就是说：加了 `--ignored` 之后不会出现“看起来跑了其实没跑”的假绿。但**不加 `--ignored` 就是彻底没跑**，而那种情况门禁是绿的。

## 8. 安全边界

- 写操作一次只开一项，跑完确认真实群里的效果符合预期再开下一项。
- 撤回他人消息、全员禁言这两类会留下群成员可见痕迹，先在自己可控的测试群跑。
- `closes_and_reopens_two_real_groups_one_minute_apart` 会真的全员禁言，间隔一分钟后解除。中途打断可能把群留在禁言状态，需要手工解除。
- 名片测试会改自己的群名片，靠主 DH 恢复；跑完核对名片是否已还原。
- 这些变量含真实群号、成员 ID 和 AI 密钥，**不要写进仓库任何文件**。用当次 shell 的环境变量，或仓库外的文件。`.env` 之类也不要放进工作树。

