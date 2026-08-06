//! 业务应用注册、启停、健康与测试。

use tauri::State;

use crate::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BusinessAppTestInput {
    account_id: String,
    app_id: String,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BusinessAppTestResult {
    app_id: String,
    status: String,
    freshness: String,
    reply: String,
    ai_used: bool,
    error: String,
    elapsed_ms: u128,
}

#[tauri::command]
pub(crate) async fn list_business_apps(
    state: State<'_, AppState>,
    account_id: String,
) -> AppResult<Vec<BusinessAppRecord>> {
    state
        .database_executor
        .ensure_business_apps(account_id.clone())
        .await?;
    state.database_executor.list_business_apps(account_id).await
}

#[tauri::command]
pub(crate) async fn set_business_app_enabled(
    state: State<'_, AppState>,
    account_id: String,
    app_id: String,
    enabled: bool,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    if app_id != PREDICTION_APP_ID {
        return Err(business_apps::app_not_available(&app_id));
    }
    if enabled {
        let registry = BusinessAppRegistry::new(state.prediction_source.clone());
        let app = registry
            .by_id(&app_id)
            .ok_or_else(|| business_apps::app_not_available(&app_id))?;
        let health = app.health(Utc::now()).await?;
        state
            .database_executor
            .update_business_app_health(
                account_id.clone(),
                app_id.clone(),
                health.status.clone(),
                health.detail.clone(),
                health.checked_at,
            )
            .await?;
        if health.status != "ready" {
            state
                .database_executor
                .set_business_app_enabled(account_id, app_id, false)
                .await?;
            return Err(AppError::new(
                "business_app_unavailable",
                format!("预测应用尚未通过数据校验：{}", health.detail),
            ));
        }
    }
    state
        .database_executor
        .set_business_app_enabled(account_id, app_id, enabled)
        .await
}

#[tauri::command]
pub(crate) async fn get_business_app_health(
    state: State<'_, AppState>,
    account_id: String,
    app_id: String,
) -> AppResult<BusinessAppHealth> {
    require_account(&state, &account_id).await?;
    if app_id != PREDICTION_APP_ID {
        return Err(business_apps::app_not_available(&app_id));
    }
    let registry = BusinessAppRegistry::new(state.prediction_source.clone());
    let app = registry
        .by_id(&app_id)
        .ok_or_else(|| business_apps::app_not_available(&app_id))?;
    let health = app.health(Utc::now()).await?;
    state
        .database_executor
        .update_business_app_health(
            account_id.clone(),
            app_id.clone(),
            health.status.clone(),
            health.detail.clone(),
            health.checked_at,
        )
        .await?;
    if health.status != "ready" {
        state
            .database_executor
            .set_business_app_enabled(account_id, app_id, false)
            .await?;
    }
    Ok(health)
}

#[tauri::command]
pub(crate) async fn list_business_app_runs(
    state: State<'_, AppState>,
    account_id: String,
    app_id: String,
    limit: Option<usize>,
) -> AppResult<Vec<BusinessAppRun>> {
    state
        .database_executor
        .list_business_app_runs(account_id, app_id, limit.unwrap_or(50))
        .await
}

#[tauri::command]
pub(crate) async fn test_business_app(
    state: State<'_, AppState>,
    input: BusinessAppTestInput,
) -> AppResult<BusinessAppTestResult> {
    if input.app_id != PREDICTION_APP_ID {
        return Err(business_apps::app_not_available(&input.app_id));
    }
    if input.message.trim().is_empty() {
        return Err(AppError::new(
            "business_app_input",
            "请输入一条预测测试内容",
        ));
    }
    let started = std::time::Instant::now();
    let registry = BusinessAppRegistry::new(state.prediction_source.clone());
    let app = registry
        .by_id(&input.app_id)
        .ok_or_else(|| business_apps::app_not_available(&input.app_id))?;
    let now = Utc::now();
    let group = Group {
        account_id: input.account_id.clone(),
        group_id: 0,
        name: "本地业务应用测试".into(),
        owner_user_id: 1,
        enabled: true,
        ai_enabled: true,
        moderation_enabled: false,
        machine_rules_enabled: false,
        ai_rules_enabled: false,
        manual_takeover: false,
        welcome_message: String::new(),
        updated_at: now,
    };
    let member = Member {
        account_id: input.account_id.clone(),
        group_id: 0,
        user_id: 1,
        nim_id: String::new(),
        nickname: "本地测试用户".into(),
        card_name: "本地测试用户".into(),
        original_card_name: "本地测试用户".into(),
        managed_card_name: String::new(),
        card_suffix: String::new(),
        role: "admin".into(),
        account_state: "".into(),
        blacklisted: false,
        present: true,
        join_source: "local-test".into(),
        prompt_read: true,
        locked_card_name: String::new(),
        violation_count: 0,
        discovered_at: now,
        joined_at: Some(now),
        last_seen_at: now,
        updated_at: now,
    };
    let message = Message {
        id: 0,
        account_id: input.account_id.clone(),
        group_id: 0,
        server_message_id: "LOCAL-BUSINESS-APP-TEST".into(),
        sequence: 0,
        user_id: 1,
        sender_name: "本地测试用户".into(),
        kind: "text".into(),
        text: input.message,
        sent_at: now,
        received_at: now,
        processed_at: None,
        acknowledged_at: None,
        processing_state: "pending".into(),
        attempts: 0,
        next_attempt_at: None,
        last_error: String::new(),
        mentions_json: "[]".into(),
        source_kind: Some("local-test".into()),
        flow: Some("inbound".into()),
    };
    let outcome = app
        .run(&BusinessAppContext {
            account_id: &input.account_id,
            group: &group,
            member: &member,
            message: &message,
            now,
        })
        .await?;
    if outcome.app_id != input.app_id {
        return Err(AppError::new(
            "business_app_result",
            "业务应用返回了错误的应用标识",
        ));
    }
    let mut reply = outcome.fallback_reply.clone();
    let mut ai_used = false;
    let mut error = String::new();
    if let Some(data) = outcome.narration.as_ref() {
        let provider = state.ai_pool.chain(
            ai_endpoint_configs(&state, &input.account_id, None)
                .await?
                .into_iter()
                .map(|(_, config)| config)
                .collect(),
        )?;
        let request = ai::AiRequest {
            version: "1",
            event_id: uuid::Uuid::new_v4().to_string(),
            persona: ai::PERSONA,
            group_id: 0,
            group_name: group.name.clone(),
            member_id: 1,
            member_name: member.card_name.clone(),
            member_role: member.role.clone(),
            message_id: "LOCAL-BUSINESS-APP-TEST".into(),
            message: business_apps::prediction_narration_prompt(data),
            recent_context: Vec::new(),
            knowledge: Vec::new(),
        };
        match ai::AiProvider::decide(provider.as_ref(), &request).await {
            Ok(decision) if !decision.reply.trim().is_empty() => {
                reply = decision.reply.trim().into();
                ai_used = true;
            }
            Ok(_) => error = "AI 没有返回文字，已使用应用模板".into(),
            Err(reason) => error = format!("{}；已使用应用模板", reason.message),
        }
    }
    Ok(BusinessAppTestResult {
        app_id: outcome.app_id,
        status: if ai_used {
            "succeeded"
        } else {
            outcome.status.as_str()
        }
        .into(),
        freshness: outcome.freshness,
        reply,
        ai_used,
        error,
        elapsed_ms: started.elapsed().as_millis(),
    })
}
