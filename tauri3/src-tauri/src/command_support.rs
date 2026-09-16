pub(crate) async fn ai_endpoint_configs(
    state: &AppState,
    account_id: &str,
    endpoint_id: Option<i64>,
) -> AppResult<Vec<(AiProviderEndpoint, ai::AiConfig)>> {
    state
        .database_executor
        .ensure_ai_provider_endpoints(account_id.to_string())
        .await?;
    let secrets = state.secrets.load()?;
    let endpoints = state
        .database_executor
        .list_ai_provider_endpoints(account_id.to_string())
        .await?;
    Ok(endpoints
        .into_iter()
        .filter(|endpoint| endpoint.enabled && endpoint_id.is_none_or(|id| id == endpoint.id))
        .map(|endpoint| {
            let config = ai::AiConfig {
                base_url: endpoint.base_url.clone(),
                webhook_url: endpoint.webhook_url.clone(),
                api_backend: endpoint.api_backend.clone(),
                model: endpoint.model.clone(),
                reasoning_effort: endpoint.reasoning_effort.clone(),
                api_key: secrets
                    .get(&endpoint.secret_ref)
                    .cloned()
                    .unwrap_or_default(),
                timeout: ai::AI_PROVIDER_TIMEOUT,
            };
            (endpoint, config)
        })
        .collect())
}

pub(crate) fn validate_group_batch_input(
    input: GroupBatchInput,
) -> AppResult<(GroupBatchAction, Vec<i64>, String)> {
    let mut group_ids = Vec::with_capacity(input.group_ids.len());
    for group_id in input.group_ids {
        if group_id <= 0 {
            return Err(AppError::new("invalid_group_id", "群 ID 必须为正数"));
        }
        if !group_ids.contains(&group_id) {
            group_ids.push(group_id);
        }
    }
    if group_ids.is_empty() {
        return Err(AppError::new("groups_empty", "请至少选择一个群"));
    }

    let text = input.text.unwrap_or_default().trim().to_string();
    if matches!(input.action, GroupBatchAction::Announcement) {
        if text.is_empty() {
            return Err(AppError::new("announcement_empty", "群公告内容不能为空"));
        }
        if text.chars().count() > 1000 {
            return Err(AppError::new(
                "announcement_too_long",
                "群公告最多 1000 个字符",
            ));
        }
    }
    Ok((input.action, group_ids, text))
}

pub(crate) fn normalize_and_validate_rule(rule: &mut ModerationRule) -> AppResult<()> {
    if rule.mode == "auto" {
        rule.mode = "automatic".into();
    }
    if !matches!(rule.rule_type.as_str(), "machine" | "ai")
        || !matches!(rule.scope.as_str(), "global" | "selected")
        || !matches!(rule.priority_level.as_str(), "low" | "medium" | "high")
    {
        return Err(AppError::new(
            "rule_validation",
            "规则类型、范围或优先级无效",
        ));
    }
    if !matches!(rule.mode.as_str(), "observe" | "automatic") {
        return Err(AppError::new(
            "rule_validation",
            "规则执行模式必须为观察或自动",
        ));
    }
    let valid_matcher = match rule.rule_type.as_str() {
        "ai" => rule.matcher == "semantic",
        _ => matches!(
            rule.matcher.as_str(),
            "contains"
                | "exact"
                | "prefix"
                | "regex"
                | "length"
                | "lines"
                | "image_count"
                | "blacklist"
                | "rename_count"
        ),
    };
    if !valid_matcher {
        return Err(AppError::new(
            "rule_matcher",
            "机器规则与 AI 控制规则的匹配方式不能混用",
        ));
    }
    rule.name = rule.name.trim().to_string();
    if rule.name.is_empty() || rule.name.chars().count() > 100 {
        return Err(AppError::new(
            "rule_name",
            "规则名称不能为空且不能超过 100 个字符",
        ));
    }
    if matches!(
        rule.matcher.as_str(),
        "contains" | "exact" | "prefix" | "regex" | "semantic"
    ) && rule.pattern.trim().is_empty()
    {
        return Err(AppError::new("rule_pattern", "规则匹配内容不能为空"));
    }
    if rule.matcher == "regex" {
        regex::Regex::new(&rule.pattern)
            .map_err(|error| AppError::new("rule_regex", format!("正则表达式格式有误：{error}")))?;
    }
    if rule.rule_type == "ai" && !(0.0..=1.0).contains(&rule.semantic_threshold) {
        return Err(AppError::new(
            "rule_threshold",
            "AI 置信度必须在 0 到 1 之间",
        ));
    }
    let mut seen_groups = std::collections::HashSet::new();
    rule.group_ids
        .retain(|group_id| *group_id > 0 && seen_groups.insert(*group_id));
    if rule.scope == "global" {
        rule.group_ids.clear();
    } else if rule.group_ids.is_empty() {
        return Err(AppError::new("rule_groups", "指定群规则至少选择一个群"));
    }
    let mut seen_members = std::collections::HashSet::new();
    rule.whitelist_user_ids
        .retain(|user_id| *user_id > 0 && seen_members.insert(*user_id));
    for action in &rule.actions {
        if !matches!(
            action.kind.as_str(),
            "recall" | "mute" | "remove" | "blacklist" | "notify" | "reply"
        ) {
            return Err(AppError::new("rule_action", "规则包含不支持的动作"));
        }
        if action.kind == "mute" && action.duration_seconds <= 0 {
            return Err(AppError::new("rule_action", "禁言时长必须大于 0 秒"));
        }
    }
    rule.cooldown_seconds = 0;
    rule.exempt_roles.clear();
    rule.exempt_user_ids = rule.whitelist_user_ids.clone();
    Ok(())
}

pub(crate) fn audit_event_label(value: &str) -> &str {
    match value {
        "message_received" => "收到群消息",
        "message_processed" => "群消息处理完成",
        "message_ignored" => "群消息未进入处理",
        "member_event_fallback" => "成员事件转为名单对账",
        "rule_matched" => "规则命中",
        "machine_rule_evaluated" => "机器规则检测",
        "ai_rule_evaluated" => "AI 规则检测",
        "ai_rule_failed" => "AI 规则检测失败",
        "action_executed" => "执行群管动作",
        "effect_dispatched" => "协议动作执行结果",
        "ai_reply" => "AI 回复",
        "ai_reply_failed" => "AI 回复失败",
        "member_joined" => "成员入群",
        "member_left" => "成员离群",
        "member_updated" => "成员资料变更",
        "card_renamed" => "群名片修改",
        "card_rename_job" => "群名片任务执行结果",
        "locked_card_restore_queued" => "锁定名片恢复排队",
        "blacklisted_member_rejoined" => "黑名单成员重新入群",
        "schedule_open" | "schedule_open_group" => "定时开群",
        "schedule_close" | "schedule_close_group" => "定时关群",
        "daily_summary" => "生成每日摘要",
        "daily_summary_failed" => "每日摘要失败",
        "task_reminder_queued" => "任务提醒排队",
        "activity_queued" => "活动进入发布队列",
        "activity_published" => "活动发布成功",
        "activity_publish_failed" => "活动发布失败",
        "semantic_classifier_fallback" => "语义分类降级",
        "gateway_queue_overflow" => "消息队列溢出",
        "automation_paused" => "自动化已暂停",
        "人工更新群公告" => "人工发布新群公告",
        _ => value,
    }
}

pub(crate) fn localize_audit_details(raw: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return localize_plain_audit_details(raw);
    };
    render_localized_audit_value(&value, false)
}

pub(crate) fn localize_plain_audit_details(raw: &str) -> String {
    let localized = raw
        .replace("消息类型=text", "消息类型=文本")
        .replace("消息类型=image", "消息类型=图片")
        .replace("消息类型=card", "消息类型=名片")
        .replace("消息类型=notice", "消息类型=通知")
        .replace("消息类型=other", "消息类型=其他")
        .replace("收到 NIM 入群事件", "收到旺商聊成员入群事件")
        .replace("收到 NIM 离群事件", "收到旺商聊成员离群事件")
        .replace("收到 NIM 成员资料更新事件", "收到旺商聊成员资料更新事件");
    if localized.contains("消息类型=") && localized.contains("，序号=") {
        format!(
            "{}（旧版记录仅保存消息类型和队列序号）",
            localized.replace("，序号=", "，接收队列序号=")
        )
    } else {
        localized
    }
}

pub(crate) fn render_localized_audit_value(value: &serde_json::Value, nested: bool) -> String {
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .map(|value| render_localized_audit_value(value, true))
            .collect::<Vec<_>>()
            .join("、"),
        serde_json::Value::Object(values) => values
            .iter()
            .filter(|(_, value)| !value.is_null() && value.as_str() != Some(""))
            .map(|(key, value)| {
                format!(
                    "{}：{}",
                    audit_detail_key_label(key),
                    render_localized_audit_value(value, true)
                )
            })
            .collect::<Vec<_>>()
            .join(if nested { "；" } else { "　" }),
        serde_json::Value::String(value) => audit_detail_value_label(value).to_string(),
        serde_json::Value::Bool(value) => if *value { "是" } else { "否" }.to_string(),
        serde_json::Value::Null => String::new(),
        value => value.to_string(),
    }
}

pub(crate) fn audit_detail_key_label(value: &str) -> &str {
    match value {
        "status" => "状态",
        "success" => "是否成功",
        "effect" => "自动动作",
        "error" => "错误",
        "receipt" => "协议回执",
        "route" => "协议路由",
        "transportCode" => "传输状态码",
        "transportErrno" => "传输错误码",
        "businessCode" => "业务状态码",
        "businessErrno" => "业务错误码",
        "businessMessage" => "业务说明",
        "requestId" => "请求编号",
        "messageId" => "本地消息编号",
        "session" => "会话",
        "verification" => "回读验证",
        "acknowledged" => "本次确认数量",
        "acknowledgedThrough" => "已确认至序号",
        "remaining" => "剩余数量",
        "dropped" => "丢弃数量",
        "retryable" => "可重试",
        "unknown" => "结果待确认",
        "groupId" => "群",
        "userId" => "成员",
        "durationSeconds" => "时长（秒）",
        "action" => "操作",
        "muted" => "全员禁言",
        "noticeId" => "公告编号",
        "content" => "内容",
        "actionKind" => "动作",
        "ruleId" => "规则编号",
        "matchedRuleIds" => "命中规则",
        "contributorRuleIds" => "动作来源规则",
        "automatic" => "自动执行",
        "elapsedMs" => "耗时（毫秒）",
        "scores" => "语义置信度",
        "confidence" => "置信度",
        "decision" => "执行决定",
        "mode" => "执行模式",
        "reason" => "原因",
        _ => value,
    }
}

pub(crate) fn audit_detail_value_label(value: &str) -> &str {
    match value {
        "incoming" => "收到",
        "outgoing" => "发出",
        "text" => "文本",
        "image" => "图片",
        "card" => "名片",
        "notice" => "通知",
        "other" => "其他",
        "pending" => "待处理",
        "queued" => "已入库并排队",
        "processing" => "处理中",
        "processed" => "已处理",
        "ignored" => "已忽略",
        "rules-and-ai-evaluated" => "规则与 AI 检查完成",
        "persisted-and-acknowledged" => "已保存并确认源队列",
        "roster-reconciliation" => "由 60 秒成员名单对账补齐",
        "succeeded" => "成功",
        "failed" => "失败",
        "unknown" => "待人工确认",
        "verified" => "已回读确认",
        "unsupported" => "未开放",
        "not-applicable" => "无需回读",
        "group_mute" => "全群发言控制",
        "automatic" => "自动执行",
        "observe" => "观察",
        "recall" => "撤回",
        "mute" => "禁言",
        "unmute" => "解除禁言",
        "remove" => "移出成员",
        "blacklist" => "加入黑名单",
        "notify" => "提示",
        "send_text" => "发送文字",
        "OK" => "正常",
        "true" => "是",
        "false" => "否",
        _ => value,
    }
}

pub(crate) fn redact_archived_receipt(mut receipt: GatewayReceipt) -> GatewayReceipt {
    receipt.route = redact(&receipt.route);
    receipt.status = redact(&receipt.status);
    receipt.business_message = redact(&receipt.business_message);
    receipt.request_id = redact(&receipt.request_id);
    receipt.message_id = redact(&receipt.message_id);
    receipt.session = redact(&receipt.session);
    receipt
}

pub(crate) fn manual_event_name(kind: &str) -> &'static str {
    match kind {
        "send_text" => "人工发送群消息",
        "recall" => "人工撤回消息",
        "mute" => "人工禁言成员",
        "unmute" => "人工解禁成员",
        "rename" => "人工修改群名片",
        "remove" => "人工移出成员",
        "blacklist" => "人工加入黑名单",
        "unblacklist" => "人工移出黑名单",
        "group_mute" => "人工设置全群发言",
        "announcement" => "人工发布新群公告",
        "announcement_update" => "人工编辑群公告",
        "announcement_delete" => "人工删除群公告",
        _ => "人工群管操作",
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn archive_manual_gateway_result(
    state: &State<'_, AppState>,
    account_id: &str,
    group_id: i64,
    user_id: i64,
    kind: &str,
    duration_seconds: i64,
    reason: &str,
    result: AppResult<GatewayReceipt>,
) -> AppResult<GatewayReceipt> {
    let (success, unknown, error_text, receipt) = match &result {
        Ok(receipt) => (
            receipt.status == "succeeded",
            receipt.status != "succeeded" && receipt.status != "failed",
            if receipt.status == "succeeded" { String::new() } else {
                "操作结果尚未确认，请刷新状态后核实，不要重复提交".into()
            },
            receipt.clone(),
        ),
        Err(error) => {
            let unknown = error.delivery_outcome_unknown();
            let mut receipt = GatewayReceipt::failed(kind, error);
            if unknown {
                receipt.status = "unknown".into();
            }
            (false, unknown, error.message.clone(), receipt)
        }
    };
    let error_text = redact(&error_text);
    let receipt = redact_archived_receipt(receipt);
    let receipt_json = serde_json::to_string(&receipt).unwrap_or_else(|_| "{}".into());
    let action_result = state
        .database_executor
        .record_action(ActionRecord {
            id: 0,
            account_id: account_id.into(),
            group_id,
            user_id,
            message_id: None,
            rule_id: None,
            kind: kind.into(),
            mode: "manual".into(),
            duration_seconds,
            reason: reason.into(),
            success,
            error: error_text.clone(),
            receipt_json: receipt_json.clone(),
            dedupe_key: format!("manual:{}", uuid::Uuid::new_v4()),
            created_at: Utc::now(),
        })
        .await;
    let audit_result = state
        .database_executor
        .record_audit(AuditEvent {
            id: 0,
            account_id: account_id.into(),
            group_id,
            user_id,
            actor: "当前管理员".into(),
            event: manual_event_name(kind).into(),
            level: if success {
                "info"
            } else if unknown {
                "warning"
            } else {
                "error"
            }
            .into(),
            details: serde_json::json!({
                "status": if success { "succeeded" } else if unknown { "unknown" } else { "failed" },
                "error": error_text,
                "receipt": receipt,
            })
            .to_string(),
            created_at: Utc::now(),
        })
        .await;
    if let Err(error) = action_result.and(audit_result.map(|_| 0)) {
        state
            .logger
            .write("ERROR", &format!("人工群管操作归档失败：{}", error.message));
        if result.is_ok() {
            return Err(AppError::new(
                "manual_action_archive",
                "操作已发送，但本地归档失败，请查看调试日志",
            ));
        }
    }
    result.and_then(confirmed_manual_receipt)
}

fn confirmed_manual_receipt(receipt: GatewayReceipt) -> AppResult<GatewayReceipt> {
    match receipt.status.as_str() {
        "succeeded" => Ok(receipt),
        "failed" => Err(AppError::new("action_failed", "操作未成功，请查看操作记录")),
        _ => Err(AppError::new("delivery_unknown", "操作结果尚未确认，请刷新状态后核实，不要重复提交")
            .with_gateway(crate::error::GatewayErrorMetadata::new(
                receipt.route, crate::error::GatewayErrorLayer::Delivery,
            ))),
    }
}

#[cfg(test)]
mod manual_receipt_tests {
    use super::*;
    #[test]
    fn uncertain_receipt_is_not_success_and_is_not_retryable() {
        let mut receipt = GatewayReceipt::succeeded("test");
        receipt.status = "unknown".into();
        let error = confirmed_manual_receipt(receipt).unwrap_err();
        assert!(error.delivery_outcome_unknown());
        assert!(!error.retryable);
        assert!(confirmed_manual_receipt(GatewayReceipt::succeeded("test")).is_ok());
    }
}

pub(crate) async fn require_group_member_for_send(
    state: &State<'_, AppState>,
    group_id: i64,
) -> AppResult<()> {
    if group_id <= 0 {
        return Err(AppError::new("group_invalid", "请选择有效的目标群"));
    }
    let (sender_id, _) = state.gateway.session_identity().await?;
    let roster = state.gateway.list_members(group_id).await?;
    if sender_id > 0 && roster.members.iter().any(|member| {
        member.user_id == sender_id && member.present
    }) {
        Ok(())
    } else {
        Err(AppError::new(
            "group_membership_required",
            "当前账号不在目标群中，请刷新群列表后重试",
        ))
    }
}

pub(crate) async fn require_manager(state: &State<'_, AppState>, group_id: i64) -> AppResult<()> {
    let (sender_id, _) = state.gateway.session_identity().await?;
    let roster = state.gateway.list_members(group_id).await?;
    if roster.members.iter().any(|member| {
        member.user_id == sender_id
            && member.user_id > 0
            && member.present
            && matches!(member.role.as_str(), "owner" | "admin")
    }) {
        Ok(())
    } else {
        Err(AppError::new(
            "management_required",
            "需要将账号权限设置为管理",
        ))
    }
}

pub(crate) async fn require_member_manager(
    state: &State<'_, AppState>,
    group_id: i64,
) -> AppResult<MemberRoster> {
    let (sender_id, _) = state.gateway.session_identity().await?;
    let roster = state.gateway.list_members(group_id).await?;
    if roster.members.iter().any(|member| {
        member.user_id == sender_id
            && member.user_id > 0
            && member.present
            && matches!(member.role.as_str(), "owner" | "admin")
    }) {
        Ok(roster)
    } else {
        Err(AppError::new(
            "management_required",
            "需要将账号权限设置为管理员",
        ))
    }
}

pub(crate) fn canonical_member(roster: &MemberRoster, user_id: i64) -> AppResult<Member> {
    let member = roster
        .members
        .iter()
        .find(|member| member.user_id == user_id && member.user_id > 0 && member.present)
        .cloned()
        .ok_or_else(|| AppError::new("member_identity", "目标成员不在当前群名单中"))?;
    if roster.synthetic_user_ids.contains(&member.user_id) {
        return Err(AppError::new(
            "member_identity_synthetic",
            "目标成员只有临时身份，不能执行群成员写操作",
        ));
    }
    if matches!(member.role.as_str(), "owner" | "admin") {
        return Err(AppError::new(
            "member_role_protected",
            "不能对群主或管理员执行成员写操作",
        ));
    }
    Ok(member)
}

pub(crate) fn canonical_member_ref(
    roster: &MemberRoster,
    requested: &MemberRef,
) -> AppResult<Member> {
    let requested_user_id = requested.user_id.filter(|value| *value > 0);
    let requested_nim_id = requested
        .nim_id
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    if requested_user_id.is_none() && requested_nim_id.is_none() {
        return Err(AppError::new("member_identity", "目标成员缺少有效身份"));
    }
    let matches = roster
        .members
        .iter()
        .filter(|member| {
            let user_matches = requested_user_id.is_none_or(|user_id| member.user_id == user_id);
            let nim_matches = requested_nim_id.is_none_or(|nim_id| member.nim_id == nim_id);
            user_matches && nim_matches
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(AppError::new(
            "member_identity",
            "目标成员身份无法唯一映射到当前名单",
        ));
    }
    canonical_member(roster, matches[0].user_id)
}

pub(crate) async fn require_account(
    state: &State<'_, AppState>,
    account_id: &str,
) -> AppResult<i64> {
    let (sender_id, current_account) = state.gateway.session_identity().await?;
    if current_account == account_id {
        Ok(sender_id)
    } else {
        Err(AppError::new(
            "account_mismatch",
            "当前登录账号与操作数据不一致，请刷新后重试",
        ))
    }
}
