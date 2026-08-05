use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc,
};
use chrono_tz::Tz;

use crate::error::{AppError, AppResult};
use crate::models::Activity;

pub const MISSED_RUN_GRACE_MINUTES: i64 = 10;

pub fn validate(activity: &Activity) -> AppResult<()> {
    if activity.name.trim().is_empty() {
        return Err(AppError::new("activity_name", "活动名称不能为空"));
    }
    let content_length = activity.content.trim().chars().count();
    if content_length == 0 || content_length > 1000 {
        return Err(AppError::new(
            "activity_content",
            "活动文案需要控制在 1 到 1000 个字符",
        ));
    }
    if activity.group_ids.is_empty() {
        return Err(AppError::new("activity_groups", "请至少选择一个活动群"));
    }
    let start = parse_date(&activity.start_date)?;
    let end = parse_date(&activity.end_date)?;
    if end < start {
        return Err(AppError::new(
            "activity_dates",
            "活动结束日期不能早于开始日期",
        ));
    }
    parse_timezone(&activity.timezone)?;
    if activity.weekdays.is_empty()
        || activity
            .weekdays
            .iter()
            .any(|weekday| !(1..=7).contains(weekday))
    {
        return Err(AppError::new("activity_weekdays", "请至少选择一个有效星期"));
    }
    if activity.send_times.is_empty() {
        return Err(AppError::new("activity_times", "请至少添加一个发送时刻"));
    }
    for time in &activity.send_times {
        parse_time(time)?;
    }
    if activity.ai_instructions.chars().count() > 200 {
        return Err(AppError::new(
            "activity_ai_instructions",
            "AI 风格说明最多 200 个字符",
        ));
    }
    Ok(())
}

pub fn next_occurrence(
    activity: &Activity,
    after: DateTime<Utc>,
) -> AppResult<Option<DateTime<Utc>>> {
    validate(activity)?;
    if !activity.enabled || activity.deleted_at.is_some() {
        return Ok(None);
    }
    let timezone = parse_timezone(&activity.timezone)?;
    let start = parse_date(&activity.start_date)?;
    let end = parse_date(&activity.end_date)?;
    let local_after = after.with_timezone(&timezone);
    let mut date = start.max(local_after.date_naive());
    let mut times = activity
        .send_times
        .iter()
        .map(|value| parse_time(value))
        .collect::<AppResult<Vec<_>>>()?;
    times.sort_unstable();
    times.dedup();
    let weekdays = activity
        .weekdays
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    while date <= end {
        if weekdays.contains(&(date.weekday().number_from_monday() as u8)) {
            for time in &times {
                let local = NaiveDateTime::new(date, *time);
                let candidate = match timezone.from_local_datetime(&local) {
                    LocalResult::Single(value) => value.with_timezone(&Utc),
                    LocalResult::Ambiguous(left, right) => left.min(right).with_timezone(&Utc),
                    LocalResult::None => continue,
                };
                if candidate > after {
                    return Ok(Some(candidate));
                }
            }
        }
        date = date
            .checked_add_signed(Duration::days(1))
            .ok_or_else(|| AppError::new("activity_dates", "活动日期超出支持范围"))?;
    }
    Ok(None)
}

pub fn scheduled_run_key(activity_id: i64, group_id: i64, scheduled_for: DateTime<Utc>) -> String {
    format!(
        "activity:{activity_id}:{group_id}:{}",
        scheduled_for.timestamp_millis()
    )
}

pub fn ai_prompt(activity: &Activity, group_name: &str, recent_texts: &[String]) -> String {
    let recent = if recent_texts.is_empty() {
        "（暂无近期活动文案）".to_string()
    } else {
        recent_texts
            .iter()
            .take(3)
            .enumerate()
            .map(|(index, text)| format!("{}. {}", index + 1, text))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "@DH [内部活动文案优化请求]\n请为群“{group_name}”改写下面的活动文案。只输出最终可发送文本，不解释，不生成管理动作或任务。必须保留所有日期、时间、金额、数字、链接和事实，不新增优惠、承诺或规则；表达自然、活泼、简洁，并避免与近期文案重复。\n活动名称：{}\n管理员原文：{}\n风格要求：{}\n近期文案：\n{}",
        activity.name,
        activity.content,
        if activity.ai_instructions.trim().is_empty() {
            "友好、清楚、适合群聊"
        } else {
            activity.ai_instructions.trim()
        },
        recent
    )
}

pub fn validate_generated_text(
    original: &str,
    candidate: &str,
    recent_texts: &[String],
) -> AppResult<String> {
    let candidate = candidate.trim();
    if candidate.is_empty() || candidate.chars().count() > 1000 {
        return Err(AppError::new(
            "activity_ai_text",
            "AI 活动文案为空或超过 1000 字符",
        ));
    }
    let normalized = normalize_for_comparison(candidate);
    if recent_texts
        .iter()
        .any(|recent| normalize_for_comparison(recent) == normalized)
    {
        return Err(AppError::new(
            "activity_ai_duplicate",
            "AI 活动文案与近期发布重复",
        ));
    }
    for required in factual_tokens(original) {
        if !candidate.contains(&required) {
            return Err(AppError::new(
                "activity_ai_facts",
                format!("AI 活动文案遗漏关键事实：{required}"),
            ));
        }
    }
    Ok(candidate.to_string())
}

fn factual_tokens(value: &str) -> Vec<String> {
    let matcher = regex::Regex::new(r"https?://[^\s]+|(?:\d[\d:./-]*\d|\d)")
        .expect("activity factual token regex");
    let mut values = matcher
        .find_iter(value)
        .map(|matched| {
            matched
                .as_str()
                .trim_end_matches(|character: char| "，。！？、；;,.!?)]}".contains(character))
                .to_string()
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn normalize_for_comparison(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace() && !character.is_ascii_punctuation())
        .flat_map(char::to_lowercase)
        .collect()
}

fn parse_timezone(value: &str) -> AppResult<Tz> {
    value
        .trim()
        .parse::<Tz>()
        .map_err(|_| AppError::new("activity_timezone", "活动时区无效"))
}

fn parse_date(value: &str) -> AppResult<NaiveDate> {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
        .map_err(|_| AppError::new("activity_dates", "活动日期格式无效"))
}

fn parse_time(value: &str) -> AppResult<NaiveTime> {
    NaiveTime::parse_from_str(value.trim(), "%H:%M")
        .map_err(|_| AppError::new("activity_times", "发送时刻格式无效"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn activity() -> Activity {
        Activity {
            id: 7,
            account_id: "account".into(),
            name: "夏日活动".into(),
            content: "欢迎参加".into(),
            enabled: true,
            ai_optimize: false,
            ai_instructions: String::new(),
            timezone: "Asia/Shanghai".into(),
            start_date: "2026-08-10".into(),
            end_date: "2026-08-12".into(),
            weekdays: vec![1, 2, 3, 4, 5, 6, 7],
            send_times: vec!["09:00".into(), "18:30".into()],
            group_ids: vec![1],
            next_run_at: None,
            source_key: String::new(),
            deleted_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn computes_multiple_local_occurrences() {
        let value = activity();
        let after = Utc.with_ymd_and_hms(2026, 8, 10, 1, 30, 0).unwrap();
        assert_eq!(
            next_occurrence(&value, after).unwrap().unwrap(),
            Utc.with_ymd_and_hms(2026, 8, 10, 10, 30, 0).unwrap()
        );
    }

    #[test]
    fn respects_weekday_and_end_date() {
        let mut value = activity();
        value.weekdays = vec![3];
        let before = Utc.with_ymd_and_hms(2026, 8, 10, 0, 0, 0).unwrap();
        assert_eq!(
            next_occurrence(&value, before).unwrap().unwrap(),
            Utc.with_ymd_and_hms(2026, 8, 12, 1, 0, 0).unwrap()
        );
        let after = Utc.with_ymd_and_hms(2026, 8, 12, 11, 0, 0).unwrap();
        assert!(next_occurrence(&value, after).unwrap().is_none());
    }

    #[test]
    fn ai_text_must_preserve_links_and_numbers() {
        let original = "8 月 12 日 19:30 开始，详情 https://example.com/a";
        assert!(validate_generated_text(
            original,
            "8 月 12 日 19:30 准时开始，详情 https://example.com/a，欢迎参加！",
            &[]
        )
        .is_ok());
        assert!(validate_generated_text(original, "今晚开始，欢迎参加！", &[]).is_err());
    }

    #[test]
    fn ai_text_rejects_recent_duplicate() {
        assert!(validate_generated_text("欢迎参加", "欢迎参加", &[" 欢迎参加 ".into()]).is_err());
    }
}
