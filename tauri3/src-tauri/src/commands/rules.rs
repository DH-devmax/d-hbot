//! 机器规则与 AI 规则的增删改查、导入导出。

use tauri::State;

use crate::*;

#[tauri::command]
pub(crate) async fn list_rules(
    state: State<'_, AppState>,
    account_id: String,
    group_id: Option<i64>,
) -> AppResult<Vec<ModerationRule>> {
    state
        .database_executor
        .list_rules(account_id, group_id)
        .await
}

#[tauri::command]
pub(crate) async fn set_group_rule_features(
    state: State<'_, AppState>,
    account_id: String,
    group_id: i64,
    machine_enabled: bool,
    ai_enabled: bool,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    require_manager(&state, group_id).await?;
    state
        .database_executor
        .set_group_rule_features(account_id, group_id, machine_enabled, ai_enabled)
        .await
}

#[tauri::command]
pub(crate) async fn search_rule_members(
    state: State<'_, AppState>,
    account_id: String,
    group_ids: Vec<i64>,
    keyword: String,
    cursor: Option<String>,
    limit: usize,
) -> AppResult<Page<Member>> {
    require_account(&state, &account_id).await?;
    let mut selected = Vec::new();
    let query = keyword.trim().to_lowercase();
    let start = cursor
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut seen = std::collections::HashSet::new();
    for group_id in group_ids.into_iter().filter(|value| *value > 0) {
        // 白名单搜索只读当前账号的本地成员索引，避免逐群网络权限校验阻塞编辑器。
        for member in state
            .database_executor
            .list_members(account_id.clone(), group_id)
            .await?
        {
            if !member.present || !seen.insert(member.user_id) {
                continue;
            }
            let haystack = format!(
                "{} {} {} {} {} {}",
                member.nickname,
                member.card_name,
                member.original_card_name,
                member.managed_card_name,
                member.user_id,
                member.nim_id
            )
            .to_lowercase();
            if query.is_empty() || haystack.contains(&query) {
                selected.push(member);
            }
        }
    }
    selected.sort_by(|left, right| {
        left.card_name
            .cmp(&right.card_name)
            .then(left.user_id.cmp(&right.user_id))
    });
    let page_limit = limit.clamp(1, 100);
    let items = selected
        .iter()
        .skip(start)
        .take(page_limit)
        .cloned()
        .collect::<Vec<_>>();
    let next = start + items.len();
    Ok(Page {
        items,
        next_cursor: (next < selected.len()).then(|| next.to_string()),
    })
}

#[tauri::command]
pub(crate) async fn save_rule(state: State<'_, AppState>, mut rule: ModerationRule) -> AppResult<i64> {
    require_account(&state, &rule.account_id).await?;
    normalize_and_validate_rule(&mut rule)?;
    for group_id in &rule.group_ids {
        require_manager(&state, *group_id).await?;
    }
    state.database_executor.save_rule(rule).await
}

#[tauri::command]
pub(crate) async fn delete_rule(
    state: State<'_, AppState>,
    account_id: String,
    rule_id: i64,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    state
        .database_executor
        .delete_rule(account_id, rule_id)
        .await
}

#[tauri::command]
pub(crate) async fn export_rules(state: State<'_, AppState>, account_id: String) -> AppResult<String> {
    let rules = state.database_executor.list_rules(account_id, None).await?;
    serde_json::to_string_pretty(&serde_json::json!({"version":2,"rules":rules}))
        .map_err(|error| AppError::new("rules_export", error.to_string()))
}

#[tauri::command]
pub(crate) async fn import_rules(
    state: State<'_, AppState>,
    account_id: String,
    json: String,
) -> AppResult<usize> {
    require_account(&state, &account_id).await?;
    #[derive(Deserialize)]
    struct RulesFile {
        version: i64,
        rules: Vec<ModerationRule>,
    }
    let document: RulesFile = serde_json::from_str(&json)
        .map_err(|error| AppError::new("rules_import", format!("规则 JSON 格式错误：{error}")))?;
    if !matches!(document.version, 1 | 2) {
        return Err(AppError::new(
            "rules_import_version",
            "当前仅支持规则文件 v1 或 v2",
        ));
    }
    let mut rules = Vec::with_capacity(document.rules.len());
    for mut rule in document.rules {
        rule.id = 0;
        rule.account_id = account_id.clone();
        if document.version == 1 {
            rule.rule_type = if rule.matcher == "semantic" {
                "ai"
            } else {
                "machine"
            }
            .into();
            rule.scope = if rule.group_id == 0 {
                "global"
            } else {
                "selected"
            }
            .into();
            rule.group_ids = (rule.group_id > 0)
                .then_some(rule.group_id)
                .into_iter()
                .collect();
            rule.priority_level = if rule.priority >= 200 {
                "high"
            } else if rule.priority < 100 {
                "low"
            } else {
                "medium"
            }
            .into();
            rule.whitelist_user_ids = rule.exempt_user_ids.clone();
        }
        normalize_and_validate_rule(&mut rule)
            .map_err(|error| AppError::new("rules_import", error.message))?;
        for group_id in &rule.group_ids {
            require_manager(&state, *group_id).await?;
        }
        rules.push(rule);
    }
    state
        .database_executor
        .import_rules(account_id, rules)
        .await
}
