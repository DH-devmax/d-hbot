//! 任务、活动与群计划。

use tauri::State;

use crate::*;

#[tauri::command]
pub(crate) async fn list_tasks(
    state: State<'_, AppState>,
    account_id: String,
    group_id: Option<i64>,
) -> AppResult<Vec<TaskItem>> {
    state
        .database_executor
        .list_tasks(account_id, group_id)
        .await
}

#[tauri::command]
pub(crate) async fn save_task(state: State<'_, AppState>, task: TaskItem) -> AppResult<i64> {
    require_account(&state, &task.account_id).await?;
    require_manager(&state, task.group_id).await?;
    let id = state.database_executor.save_task(task).await?;
    state.runtime_coordination.notify();
    Ok(id)
}

#[tauri::command]
pub(crate) async fn delete_task(
    state: State<'_, AppState>,
    account_id: String,
    task_id: i64,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    state
        .database_executor
        .delete_task(account_id, task_id)
        .await
}

#[tauri::command]
pub(crate) async fn list_activities(
    state: State<'_, AppState>,
    account_id: String,
    include_deleted: Option<bool>,
) -> AppResult<Vec<Activity>> {
    state
        .database_executor
        .list_activities(account_id, include_deleted.unwrap_or(false))
        .await
}

fn normalize_activity(mut activity: Activity) -> AppResult<Activity> {
    activity.name = activity.name.trim().to_string();
    activity.content = activity.content.trim().to_string();
    activity.ai_instructions = activity.ai_instructions.trim().to_string();
    if activity.timezone.trim().is_empty() || activity.timezone == "local" {
        activity.timezone = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into());
    }
    activity.group_ids.sort_unstable();
    activity.group_ids.dedup();
    activity.weekdays.sort_unstable();
    activity.weekdays.dedup();
    activity.send_times = activity
        .send_times
        .into_iter()
        .map(|value| value.trim().to_string())
        .collect();
    activity.send_times.sort();
    activity.send_times.dedup();
    activities::validate(&activity)?;
    activity.next_run_at = if activity.enabled {
        activities::next_occurrence(&activity, Utc::now() - chrono::Duration::seconds(1))?
    } else {
        None
    };
    Ok(activity)
}

#[tauri::command]
pub(crate) async fn save_activity(state: State<'_, AppState>, activity: Activity) -> AppResult<i64> {
    let activity = normalize_activity(activity)?;
    require_account(&state, &activity.account_id).await?;
    for group_id in &activity.group_ids {
        require_manager(&state, *group_id).await?;
    }
    let id = state.database_executor.save_activity(activity).await?;
    state.runtime_coordination.notify();
    Ok(id)
}

#[tauri::command]
pub(crate) async fn delete_activity(
    state: State<'_, AppState>,
    account_id: String,
    activity_id: i64,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    let activity = state
        .database_executor
        .activity(account_id.clone(), activity_id)
        .await?
        .ok_or_else(|| AppError::new("activity_missing", "活动不存在或已被删除"))?;
    for group_id in activity.group_ids {
        require_manager(&state, group_id).await?;
    }
    state
        .database_executor
        .delete_activity(account_id, activity_id)
        .await?;
    state.runtime_coordination.notify();
    Ok(())
}

#[tauri::command]
pub(crate) async fn list_activity_runs(
    state: State<'_, AppState>,
    account_id: String,
    activity_id: Option<i64>,
    limit: Option<usize>,
) -> AppResult<Vec<ActivityRun>> {
    state
        .database_executor
        .list_activity_runs(account_id, activity_id, limit.unwrap_or(100))
        .await
}

#[tauri::command]
pub(crate) async fn preview_activity_text(
    state: State<'_, AppState>,
    activity: Activity,
    group_id: i64,
) -> AppResult<ActivityPreview> {
    let activity = normalize_activity(activity)?;
    require_account(&state, &activity.account_id).await?;
    require_manager(&state, group_id).await?;
    if !activity.group_ids.contains(&group_id) {
        return Err(AppError::new("activity_group", "预览群不在活动适用范围内"));
    }
    if !activity.ai_optimize {
        return Ok(ActivityPreview {
            text: activity.content,
            source: "fixed".into(),
        });
    }
    let group_name = state
        .database_executor
        .list_groups(Some(activity.account_id.clone()))
        .await?
        .into_iter()
        .find(|group| group.group_id == group_id)
        .map(|group| group.name)
        .unwrap_or_else(|| "当前群".into());
    let recent_texts = state
        .database_executor
        .activity_context(activity.account_id.clone(), group_id, 5)
        .await
        .unwrap_or_default()
        .1
        .into_iter()
        .map(|run| run.text)
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>();
    let result = async {
        let provider = state.ai_pool.chain(
            ai_endpoint_configs(&state, &activity.account_id, None)
                .await?
                .into_iter()
                .map(|(_, config)| config)
                .collect(),
        )?;
        let request = ai::AiRequest {
            version: "1",
            event_id: uuid::Uuid::new_v4().to_string(),
            persona: ai::PERSONA,
            group_id,
            group_name: group_name.clone(),
            member_id: 0,
            member_name: "活动预览".into(),
            member_role: "admin".into(),
            message_id: uuid::Uuid::new_v4().to_string(),
            message: activities::ai_prompt(&activity, &group_name, &recent_texts),
            recent_context: Vec::new(),
            knowledge: Vec::new(),
        };
        let decision = ai::AiProvider::decide(provider.as_ref(), &request).await?;
        activities::validate_generated_text(&activity.content, &decision.reply, &recent_texts)
    }
    .await;
    Ok(match result {
        Ok(text) => ActivityPreview {
            text,
            source: "ai".into(),
        },
        Err(_) => ActivityPreview {
            text: activity.content,
            source: "ai-fallback".into(),
        },
    })
}

#[tauri::command]
pub(crate) async fn publish_activity_now(
    state: State<'_, AppState>,
    account_id: String,
    activity_id: i64,
) -> AppResult<usize> {
    require_account(&state, &account_id).await?;
    let activity = state
        .database_executor
        .activity(account_id, activity_id)
        .await?
        .filter(|activity| activity.deleted_at.is_none())
        .ok_or_else(|| AppError::new("activity_missing", "活动不存在或已被删除"))?;
    activities::validate(&activity)?;
    for group_id in &activity.group_ids {
        require_manager(&state, *group_id).await?;
    }
    let created = state
        .database_executor
        .create_activity_runs_now(activity, Utc::now(), uuid::Uuid::new_v4().to_string())
        .await?;
    state.runtime_coordination.notify();
    Ok(created)
}

#[tauri::command]
pub(crate) async fn list_schedules(
    state: State<'_, AppState>,
    account_id: String,
) -> AppResult<Vec<GroupSchedule>> {
    state.database_executor.list_schedules(account_id).await
}

#[tauri::command]
pub(crate) async fn save_schedule(state: State<'_, AppState>, mut schedule: GroupSchedule) -> AppResult<i64> {
    if schedule.name.trim().is_empty() || schedule.group_ids.is_empty() {
        return Err(AppError::new(
            "schedule_invalid",
            "计划名称和使用群不能为空",
        ));
    }
    require_account(&state, &schedule.account_id).await?;
    for group_id in &schedule.group_ids {
        require_manager(&state, *group_id).await?;
    }
    if schedule.timezone.trim().is_empty() || schedule.timezone == "local" {
        schedule.timezone = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into());
    }
    let _ = scheduler::evaluate_utc(&schedule, schedule.group_ids[0], Utc::now())?;
    let id = state.database_executor.save_schedule(schedule).await?;
    state.runtime_coordination.notify();
    Ok(id)
}

#[tauri::command]
pub(crate) async fn delete_schedule(
    state: State<'_, AppState>,
    account_id: String,
    schedule_id: i64,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    state
        .database_executor
        .delete_schedule(account_id, schedule_id)
        .await
}

#[tauri::command]
pub(crate) async fn list_schedule_runs(
    state: State<'_, AppState>,
    account_id: String,
    schedule_id: Option<i64>,
    limit: Option<usize>,
) -> AppResult<Vec<ScheduleRun>> {
    state
        .database_executor
        .list_schedule_runs(account_id, schedule_id, limit.unwrap_or(100))
        .await
}
