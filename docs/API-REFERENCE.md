# DH BOT Tauri 命令参考

本文记录生产版前端可以调用的强类型 Tauri command。唯一代码事实来源是
`tauri3/src-tauri/src/lib.rs` 中的 `dh_handlers!`；前端不得绕过这些命令访问 SQLite、
密钥、任意协议路由、任意 JavaScript 或远程 DevTools。

## 运行、诊断与窗口

- `health`、`database_status`、`diagnose`：读取构建渠道、数据库和连接状态。
- `export_support_bundle`：生成本地脱敏诊断 ZIP，不自动上传。
- `get_close_behavior`、`reset_close_behavior`、`resolve_close_action`：管理右上角关闭行为。
- `get_gateway_capabilities`：读取逐项能力状态、来源、人工/自动许可和原因。

## 群、成员、消息与人工群控

- 群与成员：`list_groups`、`list_cached_groups`、`list_members`、`local_members`。
- 消息：`send_text`、`send_text_batch`、`query_messages`、`recent_messages`、
  `recall_message`。
- 单项成员动作：`mute_member`、`unmute_member`、`rename_member`、`remove_member`。
- 群级动作：`set_group_mute`、`get_group_mute_state`、`execute_group_batch`。
- 公告：`set_group_announcement`、`get_group_announcement`、`list_group_announcements`、
  `update_group_announcement`、`delete_group_announcement`。
- 管理上下文与批量成员动作：`get_group_management_context`、`execute_member_batch`。

所有写命令在 Rust 后端重新校验账号、群、成员、管理权限和能力状态。批量命令只接受固定
枚举动作，逐项返回成功、失败或待人工确认；前端不能传入协议 URL。

## AI、自动化与群名片

- 群 AI：`get_ai_automation_settings`、`save_ai_automation_settings`、
  `get_ai_settings`、`save_ai_settings`。
- Provider：`list_ai_provider_endpoints`、`save_ai_provider_endpoint`、
  `delete_ai_provider_endpoint`、`test_ai_provider_endpoint`。
- 离群测试：`test_ai`。测试结果不会进入群协议层。
- 群功能：`set_group_features`、`save_group_welcome`。
- 名片：`get_card_settings`、`save_card_settings`、`preview_card_names`、
  `apply_card_names`、`list_card_rename_jobs`、`retry_card_rename_jobs`。

API Key 在 Windows 通过 DPAPI 保存；命令响应、日志、审计和支持包不返回明文密钥。

## 规则、知识、任务与摘要

- 规则：`list_rules`、`set_group_rule_features`、`search_rule_members`、`save_rule`、
  `delete_rule`、`export_rules`、`import_rules`。
- 知识库：`list_knowledge_bases`、`create_knowledge_base`、`update_knowledge_base`、
  `clone_knowledge_base`、`delete_knowledge_base`、`list_knowledge_documents`、
  `save_knowledge_document`、`delete_knowledge_document`、`bind_knowledge_base`、
  `list_knowledge_bindings`。
- 任务：`list_tasks`、`save_task`、`delete_task`。
- 计划：`list_schedules`、`save_schedule`、`delete_schedule`、`list_schedule_runs`。
- 摘要：`list_daily_summaries`、`get_summary_settings`、`save_summary_settings`、
  `generate_daily_summary`。
- 审计：`list_audit`、`query_audit`、`export_audit`。

## 业务应用

- `list_business_apps`、`set_business_app_enabled`、`get_business_app_health`、
  `list_business_app_runs`、`test_business_app`。

预测是当前唯一内置业务应用，默认停用。业务应用不接受前端自定义数据源路由；缺少真实时间、
期号、完整结果或凭据时返回不可用，本地测试不发送群消息。

## 旺商聊桌面维护

- 定位与启动：`locate_wangshangliao`、`get_wang_startup_settings`、
  `save_wang_startup_settings`、`take_wang_startup_status`、`start_wangshangliao`、
  `focus_wangshangliao`、`inspect_wangshangliao`。
- 固定登录分区：`get_wang_maintenance_result`、`get_wang_profile_status`、
  `apply_wang_profile_patch`、`restore_wang_profile_patch`。

生产版端点编译期固定为 `127.0.0.1:9222`。需要修改 Program Files 时，同一 EXE 只通过
`--maintenance` UAC 模式执行结构白名单、备份、原子替换、验证和回滚。

## 事件通道

前端订阅以下只含不可变结果的事件：`wangshangliao-status`、`connection-status`、
`connection-error`、`gateway-capabilities`、`sync-progress`、`message-received`、
`task-progress`、`schedule-updated`、`automation-paused`。后台网络和协议 worker 不直接修改
React 页面状态；页面收到事件后按账号重新读取对应数据。

## 开发版边界

Fixture 和真实协议采集命令仅在 `fixture` feature 的内部开发构建注册。生产构建使用
`--no-default-features`，生产命令表、前端资源、EXE、NSIS 和 portable ZIP 均不得出现
Fixture 命令、`9233/51300`、开发数据目录或原始协议轨迹。
