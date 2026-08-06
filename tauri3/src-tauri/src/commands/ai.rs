//! AI 设置、Provider 端点与连通性测试。

use tauri::State;

use crate::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AiAutomationInput {
    account_id: String,
    group_id: i64,
    enabled: bool,
    reply: bool,
    tasks: bool,
    recall: bool,
    mute: bool,
    remove: bool,
    manual_takeover: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AiProviderEndpointInput {
    id: i64,
    account_id: String,
    name: String,
    base_url: String,
    webhook_url: String,
    #[serde(default)]
    api_backend: String,
    model: String,
    #[serde(default = "default_reasoning_effort")]
    reasoning_effort: String,
    priority: i64,
    enabled: bool,
    api_key: Option<String>,
}

#[tauri::command]
pub(crate) async fn get_ai_settings(state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    let keys = [
        "ai.base_url",
        "ai.webhook_url",
        "ai.api_backend",
        "ai.model",
    ];
    let mut values = serde_json::Map::new();
    for key in keys {
        values.insert(
            key.trim_start_matches("ai.").replace('.', "_"),
            serde_json::Value::String(
                state
                    .database_executor
                    .get_setting(key.into())
                    .await?
                    .unwrap_or_default(),
            ),
        );
    }
    values.insert(
        "api_key_configured".into(),
        serde_json::Value::Bool(
            !state
                .secrets
                .load()?
                .get("ai.api_key")
                .map(String::is_empty)
                .unwrap_or(true),
        ),
    );
    Ok(serde_json::Value::Object(values))
}

#[tauri::command]
pub(crate) async fn get_ai_automation_settings(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
) -> AppResult<serde_json::Value> {
    let group = state
        .database_executor
        .list_groups(Some(account_id.clone()))
        .await?
        .into_iter()
        .find(|group| group.group_id == group_id)
        .ok_or_else(|| AppError::new("group_missing", "没有找到所选群"))?;
    let permissions = state
        .database_executor
        .group_ai_permissions(account_id.clone(), group_id)
        .await?;
    let legacy_tasks = state
        .database_executor
        .get_setting(format!("ai.permission.tasks.{account_id}.{group_id}"))
        .await?
        .map(|value| value == "true")
        .unwrap_or(true);
    let legacy_recall = state
        .database_executor
        .get_setting(format!("ai.permission.recall.{account_id}.{group_id}"))
        .await?
        .map(|value| value == "true")
        .unwrap_or(false);
    let legacy_mute = state
        .database_executor
        .get_setting(format!("ai.permission.mute.{account_id}.{group_id}"))
        .await?
        .map(|value| value == "true")
        .unwrap_or(false);
    let legacy_remove = state
        .database_executor
        .get_setting(format!("ai.permission.remove.{account_id}.{group_id}"))
        .await?
        .map(|value| value == "true")
        .unwrap_or(false);
    Ok(serde_json::json!({
        "enabled": group.enabled,
        "reply": permissions.as_ref().map(|value| value.reply).unwrap_or(group.ai_enabled),
        "tasks": permissions.as_ref().map(|value| value.tasks).unwrap_or(legacy_tasks),
        "recall": permissions.as_ref().map(|value| value.recall).unwrap_or(legacy_recall),
        "mute": permissions.as_ref().map(|value| value.mute).unwrap_or(legacy_mute),
        "remove": permissions.as_ref().map(|value| value.remove).unwrap_or(legacy_remove),
        "manualTakeover": group.manual_takeover
    }))
}

#[tauri::command]
pub(crate) async fn save_ai_automation_settings(
    state: State<'_, AppState>,
    settings: AiAutomationInput,
) -> AppResult<()> {
    require_manager(&state, settings.group_id).await?;
    let group = state
        .database_executor
        .list_groups(Some(settings.account_id.clone()))
        .await?
        .into_iter()
        .find(|group| group.group_id == settings.group_id)
        .ok_or_else(|| AppError::new("group_missing", "没有找到所选群"))?;
    state
        .database_executor
        .set_group_features(
            settings.account_id.clone(),
            settings.group_id,
            settings.enabled,
            settings.reply,
            group.moderation_enabled,
            settings.manual_takeover,
        )
        .await?;
    state
        .database_executor
        .save_group_ai_permissions(GroupAiPermissions {
            account_id: settings.account_id,
            group_id: settings.group_id,
            reply: settings.reply,
            tasks: settings.tasks,
            recall: settings.recall,
            mute: settings.mute,
            remove: settings.remove,
        })
        .await?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn save_ai_settings(
    state: State<'_, AppState>,
    base_url: String,
    webhook_url: String,
    api_backend: Option<String>,
    model: String,
    api_key: Option<String>,
) -> AppResult<()> {
    if !base_url.trim().is_empty()
        && !base_url.starts_with("https://")
        && !base_url.starts_with("http://127.0.0.1")
        && !base_url.starts_with("http://localhost")
    {
        return Err(AppError::new(
            "url_policy",
            "远程 AI 地址必须使用 HTTPS，本机服务只能使用回环 HTTP",
        ));
    }
    state
        .database_executor
        .set_setting("ai.base_url".into(), base_url.trim().into(), false)
        .await?;
    state
        .database_executor
        .set_setting("ai.webhook_url".into(), webhook_url.trim().into(), false)
        .await?;
    state
        .database_executor
        .set_setting(
            "ai.api_backend".into(),
            ai::normalize_api_backend(api_backend.as_deref().unwrap_or("chat_completions"))?,
            false,
        )
        .await?;
    state
        .database_executor
        .set_setting(
            "ai.model".into(),
            if model.trim().is_empty() {
                "deepseek-v4-pro"
            } else {
                model.trim()
            }
            .into(),
            false,
        )
        .await?;
    if let Some(api_key) = api_key.filter(|key| !key.trim().is_empty()) {
        let mut values = state.secrets.load()?;
        values.insert("ai.api_key".into(), api_key);
        state.secrets.save(&values)?;
    }
    Ok(())
}

fn validate_ai_endpoint_url(value: &str) -> AppResult<()> {
    if value.trim().is_empty()
        || value.starts_with("https://")
        || value.starts_with("http://127.0.0.1")
        || value.starts_with("http://localhost")
    {
        Ok(())
    } else {
        Err(AppError::new(
            "url_policy",
            "远程 AI 地址必须使用 HTTPS，本机服务只能使用回环 HTTP",
        ))
    }
}

#[tauri::command]
pub(crate) async fn list_ai_provider_endpoints(
    state: State<'_, AppState>,
    account_id: String,
) -> AppResult<Vec<AiProviderEndpoint>> {
    state
        .database_executor
        .ensure_ai_provider_endpoints(account_id.clone())
        .await?;
    let secrets = state.secrets.load()?;
    let mut endpoints = state
        .database_executor
        .list_ai_provider_endpoints(account_id)
        .await?;
    for endpoint in &mut endpoints {
        endpoint.api_key_configured = secrets
            .get(&endpoint.secret_ref)
            .is_some_and(|value| !value.trim().is_empty());
    }
    Ok(endpoints)
}

#[tauri::command]
pub(crate) async fn save_ai_provider_endpoint(
    state: State<'_, AppState>,
    input: AiProviderEndpointInput,
) -> AppResult<i64> {
    if input.account_id.trim().is_empty() || input.name.trim().is_empty() {
        return Err(AppError::new("ai_endpoint_input", "连接名称和账号不能为空"));
    }
    validate_ai_endpoint_url(&input.base_url)?;
    validate_ai_endpoint_url(&input.webhook_url)?;
    if input.base_url.trim().is_empty() && input.webhook_url.trim().is_empty() {
        return Err(AppError::new(
            "ai_endpoint_input",
            "请填写 Base URL 或 Webhook URL",
        ));
    }
    let api_backend = ai::normalize_api_backend(&input.api_backend)?;
    state
        .database_executor
        .ensure_ai_provider_endpoints(input.account_id.clone())
        .await?;
    let existing = state
        .database_executor
        .list_ai_provider_endpoints(input.account_id.clone())
        .await?
        .into_iter()
        .find(|endpoint| endpoint.id == input.id);
    let now = Utc::now();
    let secret_ref = existing
        .as_ref()
        .map(|endpoint| endpoint.secret_ref.clone())
        .unwrap_or_else(|| format!("ai.provider.{}.{}", input.account_id, uuid::Uuid::new_v4()));
    let endpoint = AiProviderEndpoint {
        id: input.id,
        account_id: input.account_id,
        name: input.name.trim().to_string(),
        base_url: input.base_url.trim().to_string(),
        webhook_url: input.webhook_url.trim().to_string(),
        api_backend,
        model: if input.model.trim().is_empty() {
            "deepseek-v4-pro".into()
        } else {
            input.model.trim().to_string()
        },
        reasoning_effort: ai::normalize_reasoning_effort(&input.reasoning_effort)?,
        secret_ref: secret_ref.clone(),
        priority: input.priority.max(0),
        enabled: input.enabled,
        api_key_configured: existing
            .as_ref()
            .is_some_and(|endpoint| endpoint.api_key_configured),
        health_status: "unchecked".into(),
        failure_count: 0,
        cooldown_until: None,
        last_error: String::new(),
        last_checked_at: None,
        created_at: existing
            .as_ref()
            .map(|endpoint| endpoint.created_at)
            .unwrap_or(now),
        updated_at: now,
    };
    let id = state
        .database_executor
        .save_ai_provider_endpoint(endpoint)
        .await?;
    if let Some(api_key) = input.api_key.filter(|value| !value.trim().is_empty()) {
        let mut values = state.secrets.load()?;
        values.insert(secret_ref, api_key.trim().to_string());
        state.secrets.save(&values)?;
    }
    Ok(id)
}

#[tauri::command]
pub(crate) async fn delete_ai_provider_endpoint(
    state: State<'_, AppState>,
    account_id: String,
    endpoint_id: i64,
) -> AppResult<()> {
    let endpoints = state
        .database_executor
        .list_ai_provider_endpoints(account_id.clone())
        .await?;
    if endpoints.len() <= 1 {
        return Err(AppError::new("ai_endpoint_delete", "至少保留一个 AI 连接"));
    }
    let secret_ref = endpoints
        .iter()
        .find(|endpoint| endpoint.id == endpoint_id)
        .map(|endpoint| endpoint.secret_ref.clone());
    state
        .database_executor
        .delete_ai_provider_endpoint(account_id, endpoint_id)
        .await?;
    if let Some(secret_ref) = secret_ref.filter(|value| value != "ai.api_key") {
        let mut values = state.secrets.load()?;
        values.remove(&secret_ref);
        state.secrets.save(&values)?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn test_ai_provider_endpoint(
    state: State<'_, AppState>,
    account_id: String,
    endpoint_id: i64,
    message: String,
) -> AppResult<ai::AiTestResult> {
    let (_, config) = ai_endpoint_configs(&state, &account_id, Some(endpoint_id))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::new("ai_endpoint_missing", "没有找到已启用的 AI 连接"))?;
    let result = state
        .ai_pool
        .provider(config)?
        .test(&ai::AiRequest::testing(message, Vec::new()))
        .await;
    let _ = state
        .database_executor
        .update_ai_provider_health(
            account_id,
            endpoint_id,
            result.is_ok(),
            result
                .as_ref()
                .err()
                .map(|error| error.message.clone())
                .unwrap_or_default(),
        )
        .await;
    result
}

#[tauri::command]
pub(crate) async fn test_ai(
    state: State<'_, AppState>,
    account_id: Option<String>,
    message: String,
    recent_context: Vec<ai::AiContextMessage>,
    include_built_in_knowledge: Option<bool>,
) -> AppResult<ai::AiTestResult> {
    let pairs = if let Some(account_id) = account_id.filter(|value| !value.trim().is_empty()) {
        ai_endpoint_configs(&state, &account_id, None).await?
    } else {
        let settings = get_ai_settings(state.clone()).await?;
        let values = settings.as_object().cloned().unwrap_or_default();
        vec![(
            AiProviderEndpoint {
                id: 0,
                account_id: "local-test".into(),
                name: "当前连接".into(),
                base_url: String::new(),
                webhook_url: String::new(),
                api_backend: values
                    .get("api_backend")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("chat_completions")
                    .into(),
                model: String::new(),
                reasoning_effort: "low".into(),
                secret_ref: "ai.api_key".into(),
                priority: 0,
                enabled: true,
                api_key_configured: false,
                health_status: "unchecked".into(),
                failure_count: 0,
                cooldown_until: None,
                last_error: String::new(),
                last_checked_at: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            ai::AiConfig {
                base_url: values
                    .get("base_url")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .into(),
                webhook_url: values
                    .get("webhook_url")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .into(),
                api_backend: values
                    .get("api_backend")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("chat_completions")
                    .into(),
                model: values
                    .get("model")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("deepseek-v4-pro")
                    .into(),
                reasoning_effort: "low".into(),
                api_key: state
                    .secrets
                    .load()?
                    .get("ai.api_key")
                    .cloned()
                    .unwrap_or_default(),
                timeout: ai::AI_PROVIDER_TIMEOUT,
            },
        )]
    };
    let model = pairs
        .first()
        .map(|(_, config)| config.model.clone())
        .unwrap_or_else(|| "deepseek-v4-pro".into());
    let provider = state
        .ai_pool
        .chain(pairs.into_iter().map(|(_, config)| config).collect())?;
    let request = ai::AiRequest::testing_with_knowledge(
        message,
        recent_context,
        include_built_in_knowledge.unwrap_or(false),
    );
    let started = std::time::Instant::now();
    let decision = ai::AiProvider::decide(provider.as_ref(), &request).await?;
    Ok(ai::AiTestResult {
        decision,
        model,
        elapsed_ms: started.elapsed().as_millis(),
        knowledge_source: if request.knowledge.is_empty() {
            "空上下文".into()
        } else {
            "DH 默认群规与 FAQ".into()
        },
    })
}
