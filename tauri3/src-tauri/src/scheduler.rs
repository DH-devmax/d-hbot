use chrono::{DateTime, Local, NaiveTime, TimeZone, Utc};
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
    let (last_action, next_action, next_date, next_time) = if should_mute {
        let next_date = if current < open {
            current_local.date()
        } else {
            current_local.date().succ_opt().unwrap()
        };
        ("close", "open", next_date, open)
    } else {
        let next_date = if current < close {
            current_local.date()
        } else {
            current_local.date().succ_opt().unwrap()
        };
        ("open", "close", next_date, close)
    };
    let local_next_utc = if let Some(zone) = timezone {
        zone.from_local_datetime(&next_date.and_time(next_time))
            .earliest()
            .ok_or_else(|| AppError::new("schedule_timezone", "下一次计划时间在当前时区不存在"))?
            .with_timezone(&Utc)
    } else {
        Local
            .from_local_datetime(&next_date.and_time(next_time))
            .earliest()
            .ok_or_else(|| AppError::new("schedule_timezone", "下一次计划时间在当前时区不存在"))?
            .with_timezone(&Utc)
    };
    let run_date = current_local.format("%Y-%m-%d");
    Ok(ScheduleDecision {
        should_mute,
        last_action: last_action.into(),
        next_action: next_action.into(),
        next_at: local_next_utc,
        run_key: format!("{}:{}:{}:{}", schedule.id, group_id, run_date, last_action),
    })
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
}
