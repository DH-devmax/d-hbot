use std::collections::HashMap;
use std::time::Duration;

use chrono::Utc;
use regex::Regex;
use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::models::{PredictionResult, PredictionSnapshot};

pub fn is_prediction_request(text: &str) -> bool {
    crate::ai::is_mentioned(text) && text.contains("预测")
}

pub fn analyze(snapshot: &PredictionSnapshot) -> PredictionResult {
    let mut history = snapshot.history.clone();
    history.truncate(30);
    let mut counts: HashMap<i64, i64> = HashMap::new();
    let mut omissions = HashMap::new();
    for row in &history {
        for value in row.iter().filter(|value| **value >= 0 && **value <= 99) {
            *counts.entry(*value).or_default() += 1;
        }
    }
    for value in 0..=27 {
        omissions.insert(
            value,
            history
                .iter()
                .position(|row| row.contains(&value))
                .unwrap_or(history.len()) as i64,
        );
    }
    let mut values = (0..=27).collect::<Vec<i64>>();
    values.sort_by(|left, right| {
        let left_score = counts.get(left).copied().unwrap_or(0) * 2
            + omissions.get(left).copied().unwrap_or(0).min(10);
        let right_score = counts.get(right).copied().unwrap_or(0) * 2
            + omissions.get(right).copied().unwrap_or(0).min(10);
        right_score.cmp(&left_score).then(left.cmp(right))
    });
    let candidates = if history.is_empty() {
        Vec::new()
    } else {
        values.into_iter().take(3).collect()
    };
    let confidence = match history.len() {
        0..=4 => 0.2,
        5..=9 => 0.35,
        10..=24 => 0.55,
        _ => 0.7,
    };
    let trend = if history.is_empty() {
        "样本不足，暂不判断冷热".into()
    } else {
        format!("根据近 {} 期频率和遗漏计算候选方向", history.len())
    };
    PredictionResult {
        game: snapshot.game.clone(),
        period: snapshot.period.clone(),
        latest_result: snapshot.result.clone(),
        trend,
        candidates,
        confidence,
        updated_at: snapshot.updated_at,
        freshness: snapshot.freshness.clone(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub id: &'static str,
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    #[serde(skip_serializing)]
    endpoint: &'static str,
}

pub const GAMES: &[Game] = &[
    Game {
        id: "pc28",
        name: "PC28",
        aliases: &["pc蛋蛋", "PC蛋蛋"],
        endpoint: "http://www.pceggs.com/play/pc28.aspx",
    },
    Game {
        id: "jnd28",
        name: "加拿大28",
        aliases: &["加拿大"],
        endpoint: "http://www.ok1116.com/play/jnd28/",
    },
    Game {
        id: "bj28",
        name: "北京28",
        aliases: &["北京"],
        endpoint: "https://hao123wc.obs.ap-southeast-1.myhuaweicloud.com/zcs/duoduo_2.txt",
    },
    Game {
        id: "bit",
        name: "比特",
        aliases: &["比特彩"],
        endpoint: "",
    },
];

pub async fn fetch(text: &str) -> AppResult<Result<PredictionResult, String>> {
    let normalized = text.to_lowercase();
    let game = GAMES.iter().find(|game| {
        normalized.contains(&game.name.to_lowercase())
            || normalized.contains(game.id)
            || game
                .aliases
                .iter()
                .any(|alias| normalized.contains(&alias.to_lowercase()))
    });
    let Some(game) = game else {
        return Ok(Err(format!(
            "请在“预测”后写出彩种名称，例如：@DH 预测 {}",
            GAMES
                .iter()
                .map(|game| game.name)
                .collect::<Vec<_>>()
                .join("、")
        )));
    };
    if game.endpoint.is_empty() {
        return Err(AppError::new(
            "prediction_unavailable",
            "当前彩种暂未发现可用数据",
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|error| AppError::new("prediction_client", error.to_string()))?;
    let response = client.get(game.endpoint).send().await.map_err(|error| {
        AppError::new("prediction_request", format!("读取预测数据失败：{error}")).retryable()
    })?;
    if !response.status().is_success() {
        return Err(AppError::new(
            "prediction_http",
            format!("预测数据暂不可用（HTTP {}）", response.status()),
        )
        .retryable());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| AppError::new("prediction_response", error.to_string()))?;
    Ok(Ok(analyze(&parse_snapshot(game.name, &bytes))))
}

pub fn format_reply(result: &PredictionResult) -> String {
    format!("{} 第{}期\n最新结果：{}\n更新时间：{}\n趋势摘要：{}\n候选方向：{}\n参考度：{}，仅作信息参考。", result.game, result.period, join(&result.latest_result, " + "), result.updated_at.with_timezone(&chrono::Local).format("%m-%d %H:%M"), result.trend, if result.candidates.is_empty() { "暂无".into() } else { join(&result.candidates, "、") }, confidence_text(result.confidence))
}

fn parse_snapshot(game: &str, body: &[u8]) -> PredictionSnapshot {
    let text = String::from_utf8_lossy(body);
    let pattern = Regex::new(r"(?im)(?:期号|期数|period|issue|expect)[^0-9]{0,12}([0-9]{2,20})[^0-9]{0,20}([0-9]{1,3})[+,:，、 ]+([0-9]{1,3})[+,:，、 ]+([0-9]{1,3})").unwrap();
    let mut period = String::new();
    let mut result = Vec::new();
    let mut history = Vec::new();
    for capture in pattern.captures_iter(&text) {
        let row = (2..=4)
            .filter_map(|index| capture.get(index)?.as_str().parse::<i64>().ok())
            .collect::<Vec<_>>();
        if row.len() != 3 {
            continue;
        }
        if period.is_empty() {
            period = capture
                .get(1)
                .map(|value| value.as_str().to_string())
                .unwrap_or_default();
            result = row.clone();
        }
        history.push(row);
    }
    if period.is_empty() {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
            period = json_text(&value, &["period", "issue", "expect", "期号", "期数"]);
            result = numbers(&json_text(
                &value,
                &["result", "numbers", "开奖号码", "openCode"],
            ));
            if !result.is_empty() {
                history.push(result.clone());
            }
        }
    }
    PredictionSnapshot {
        game: game.into(),
        period: if period.is_empty() {
            "待更新".into()
        } else {
            period
        },
        result,
        updated_at: Utc::now(),
        history,
        freshness: "刚刚".into(),
    }
}

fn json_text(value: &serde_json::Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(value) = value.get(*key) {
            if let Some(text) = value.as_str() {
                return text.into();
            }
            if !value.is_null() {
                return value.to_string().trim_matches('"').into();
            }
        }
    }
    String::new()
}
fn numbers(value: &str) -> Vec<i64> {
    Regex::new(r"[0-9]{1,3}")
        .unwrap()
        .find_iter(value)
        .filter_map(|item| item.as_str().parse().ok())
        .take(3)
        .collect()
}
fn join(values: &[i64], separator: &str) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(separator)
}
fn confidence_text(value: f64) -> &'static str {
    if value >= 0.65 {
        "中高"
    } else if value >= 0.5 {
        "中"
    } else {
        "低"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    #[test]
    fn prediction_requires_explicit_mention() {
        assert!(is_prediction_request("@DH 预测 PC28"));
        assert!(!is_prediction_request("预测 PC28"));
    }
    #[test]
    fn analysis_is_deterministic() {
        let snapshot = PredictionSnapshot {
            game: "PC28".into(),
            period: "1".into(),
            result: vec![1, 2, 3],
            updated_at: Utc::now(),
            history: vec![vec![1, 2, 3], vec![1, 4, 5], vec![2, 6, 7]],
            freshness: "fresh".into(),
        };
        assert_eq!(analyze(&snapshot).candidates, analyze(&snapshot).candidates);
    }
    #[test]
    fn fixture_parser_hides_source_shape() {
        let snapshot = parse_snapshot(
            "PC28",
            "期号 20260720001 1+2+3 期号 20260720000 4+5+6".as_bytes(),
        );
        assert_eq!(snapshot.period, "20260720001");
        assert_eq!(snapshot.history.len(), 2);
        assert!(!format_reply(&analyze(&snapshot)).contains("http"));
    }
}
