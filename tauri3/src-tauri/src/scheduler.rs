use chrono::{DateTime, Local, LocalResult, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::models::GroupSchedule;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleDecision {
    pub should_mute: bool,
    pub last_action: String,
    pub next_action: String,
    pub next_at: DateTime<Utc>,
    pub run_key: String,
}

pub fn evaluate(
    schedule: &GroupSchedule,
    group_id: i64,
    now: DateTime<Local>,
) -> AppResult<ScheduleDecision> {
    let now_utc = now.with_timezone(&Utc);
    evaluate_utc(schedule, group_id, now_utc)
}

pub fn evaluate_utc(
    schedule: &GroupSchedule,
    group_id: i64,
    now_utc: DateTime<Utc>,
) -> AppResult<ScheduleDecision> {
    let open = parse_time(&schedule.open_time)?;
    let close = parse_time(&schedule.close_time)?;
    if open == close {
        return Err(AppError::new("schedule_time", "开群时间和关群时间不能相同"));
    }
    let timezone = schedule.timezone.parse::<Tz>().ok();
    let current_local = timezone
        .map(|zone| now_utc.with_timezone(&zone).naive_local())
        .unwrap_or_else(|| now_utc.with_timezone(&Local).naive_local());
    let current = current_local.time();
    let should_mute = if open < close {
        current < open || current >= close
    } else {
        current >= close && current < open
    };
    let today = current_local.date();
    let yesterday = today
        .pred_opt()
        .ok_or_else(|| AppError::new("schedule_time", "日期超出计划范围"))?;
    let tomorrow = today
        .succ_opt()
        .ok_or_else(|| AppError::new("schedule_time", "日期超出计划范围"))?;
    let boundaries = [
        (yesterday.and_time(open), "open", yesterday, open),
        (yesterday.and_time(close), "close", yesterday, close),
        (today.and_time(open), "open", today, open),
        (today.and_time(close), "close", today, close),
        (tomorrow.and_time(open), "open", tomorrow, open),
        (tomorrow.and_time(close), "close", tomorrow, close),
    ];
    let (_, last_action, last_date, _) = boundaries
        .iter()
        .filter(|(boundary, _, _, _)| *boundary <= current_local)
        .max_by_key(|(boundary, _, _, _)| *boundary)
        .copied()
        .ok_or_else(|| AppError::new("schedule_time", "未找到最近计划边界"))?;
    let (_, next_action, next_date, next_time) = boundaries
        .iter()
        .filter(|(boundary, _, _, _)| *boundary > current_local)
        .min_by_key(|(boundary, _, _, _)| *boundary)
        .copied()
        .ok_or_else(|| AppError::new("schedule_time", "未找到下一计划边界"))?;
    let local_next_utc = if let Some(zone) = timezone {
        resolve_tz_boundary(zone, next_date.and_time(next_time))?
    } else {
        resolve_local_boundary(next_date.and_time(next_time))?
    };
    let run_date = last_date.format("%Y-%m-%d");
    Ok(ScheduleDecision {
        should_mute,
        last_action: last_action.into(),
        next_action: next_action.into(),
        next_at: local_next_utc,
        run_key: format!("{}:{}:{}:{}", schedule.id, group_id, run_date, last_action),
    })
}

fn resolve_tz_boundary(zone: Tz, value: NaiveDateTime) -> AppResult<DateTime<Utc>> {
    resolve_boundary(value, |candidate| zone.from_local_datetime(candidate))
        .map(|datetime| datetime.with_timezone(&Utc))
}

fn resolve_local_boundary(value: NaiveDateTime) -> AppResult<DateTime<Utc>> {
    resolve_boundary(value, |candidate| Local.from_local_datetime(candidate))
        .map(|datetime| datetime.with_timezone(&Utc))
}

fn resolve_boundary<T: TimeZone>(
    value: NaiveDateTime,
    resolve: impl Fn(&NaiveDateTime) -> LocalResult<DateTime<T>>,
) -> AppResult<DateTime<T>> {
    let mut candidate = value;
    for _ in 0..=180 {
        match resolve(&candidate) {
            LocalResult::Single(datetime) => return Ok(datetime),
            LocalResult::Ambiguous(first, second) => return Ok(first.min(second)),
            LocalResult::None => candidate += chrono::Duration::minutes(1),
        }
    }
    Err(AppError::new(
        "schedule_timezone",
        "下一次计划时间无有效时区映射",
    ))
}

fn parse_time(value: &str) -> AppResult<NaiveTime> {
    NaiveTime::parse_from_str(value.trim(), "%H:%M")
        .map_err(|_| AppError::new("schedule_time", format!("时间 {value} 必须使用 HH:MM 格式")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    fn schedule(open: &str, close: &str) -> GroupSchedule {
        GroupSchedule {
            id: 1,
            account_id: "a".into(),
            name: "每日".into(),
            enabled: true,
            open_time: open.into(),
            close_time: close.into(),
            timezone: "local".into(),
            group_ids: vec![7],
        }
    }
    fn local_at(hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(2026, 7, 20)
                    .unwrap()
                    .and_hms_opt(hour, minute, 0)
                    .unwrap(),
            )
            .earliest()
            .unwrap()
    }
    #[test]
    fn daytime_schedule_closes_outside_window() {
        assert!(
            evaluate(&schedule("08:00", "22:00"), 7, local_at(23, 0))
                .unwrap()
                .should_mute
        );
        assert!(
            !evaluate(&schedule("08:00", "22:00"), 7, local_at(12, 0))
                .unwrap()
                .should_mute
        );
    }
    #[test]
    fn cross_midnight_schedule_stays_open_after_midnight() {
        assert!(
            !evaluate(&schedule("20:00", "02:00"), 7, local_at(1, 0))
                .unwrap()
                .should_mute
        );
        assert!(
            evaluate(&schedule("20:00", "02:00"), 7, local_at(12, 0))
                .unwrap()
                .should_mute
        );
    }
    #[test]
    fn equal_times_are_invalid() {
        assert!(evaluate(&schedule("08:00", "08:00"), 7, local_at(9, 0)).is_err());
    }

    #[test]
    fn named_timezone_controls_daily_window() {
        let mut value = schedule("08:00", "22:00");
        value.timezone = "Asia/Shanghai".into();
        let utc = Utc.with_ymd_and_hms(2026, 7, 20, 0, 30, 0).unwrap();
        assert!(!evaluate_utc(&value, 7, utc).unwrap().should_mute);
    }

    #[test]
    fn cross_midnight_run_key_uses_the_actual_last_boundary_date() {
        let mut value = schedule("20:00", "02:00");
        value.timezone = "Asia/Shanghai".into();
        let after_midnight = Utc.with_ymd_and_hms(2026, 7, 20, 17, 0, 0).unwrap();
        let decision = evaluate_utc(&value, 7, after_midnight).unwrap();
        assert!(!decision.should_mute);
        assert_eq!(decision.last_action, "open");
        assert_eq!(decision.run_key, "1:7:2026-07-20:open");
        assert_eq!(decision.next_action, "close");
        assert_eq!(
            decision.next_at,
            Utc.with_ymd_and_hms(2026, 7, 20, 18, 0, 0).unwrap()
        );
    }

    #[test]
    fn spring_forward_moves_a_missing_boundary_to_first_valid_minute() {
        let mut value = schedule("01:00", "02:30");
        value.timezone = "America/New_York".into();
        let before_gap = Utc.with_ymd_and_hms(2026, 3, 8, 6, 30, 0).unwrap();
        let decision = evaluate_utc(&value, 7, before_gap).unwrap();
        assert!(!decision.should_mute);
        assert_eq!(decision.next_action, "close");
        assert_eq!(
            decision.next_at,
            Utc.with_ymd_and_hms(2026, 3, 8, 7, 0, 0).unwrap()
        );
    }

    #[test]
    fn fall_back_chooses_the_earliest_ambiguous_boundary() {
        let mut value = schedule("00:30", "01:30");
        value.timezone = "America/New_York".into();
        let before_first_close = Utc.with_ymd_and_hms(2026, 11, 1, 5, 0, 0).unwrap();
        let decision = evaluate_utc(&value, 7, before_first_close).unwrap();
        assert_eq!(
            decision.next_at,
            Utc.with_ymd_and_hms(2026, 11, 1, 5, 30, 0).unwrap()
        );
    }
}
