use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::models::{Group, Member, Message, PredictionResult};
use crate::prediction::{self, PredictionFreshness, PredictionSource};

pub const PREDICTION_APP_ID: &str = "prediction";
pub const PREDICTION_APP_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BusinessAppManifest {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub version: &'static str,
    pub trigger_hint: &'static str,
    pub scope: &'static str,
}

#[derive(Debug, Clone)]
pub struct BusinessAppContext<'a> {
    pub account_id: &'a str,
    pub group: &'a Group,
    pub member: &'a Member,
    pub message: &'a Message,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BusinessAppHealth {
    pub app_id: String,
    pub status: String,
    pub detail: String,
    pub checked_at: DateTime<Utc>,
    pub games: Vec<BusinessAppGameHealth>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BusinessAppGameHealth {
    pub id: String,
    pub name: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct BusinessAppResult {
    pub app_id: String,
    pub status: String,
    pub freshness: String,
    pub fallback_reply: String,
    pub narration: Option<Value>,
}

#[async_trait]
pub trait BusinessApp: Send + Sync {
    fn manifest(&self) -> BusinessAppManifest;
    fn matches(&self, message: &str) -> bool;
    async fn health(&self, now: DateTime<Utc>) -> AppResult<BusinessAppHealth>;
    async fn run(&self, context: &BusinessAppContext<'_>) -> AppResult<BusinessAppResult>;
}

#[derive(Clone)]
pub struct BusinessAppRegistry {
    apps: Vec<Arc<dyn BusinessApp>>,
}

impl BusinessAppRegistry {
    pub fn new(prediction_source: Arc<dyn PredictionSource>) -> Self {
        Self {
            apps: vec![Arc::new(PredictionApp {
                source: prediction_source,
            })],
        }
    }

    pub fn find_for_message(&self, message: &str) -> Option<Arc<dyn BusinessApp>> {
        self.apps.iter().find(|app| app.matches(message)).cloned()
    }

    pub fn by_id(&self, app_id: &str) -> Option<Arc<dyn BusinessApp>> {
        self.apps
            .iter()
            .find(|app| app.manifest().id == app_id)
            .cloned()
    }
}

struct PredictionApp {
    source: Arc<dyn PredictionSource>,
}

impl PredictionApp {
    fn unavailable(error: impl Into<String>) -> BusinessAppResult {
        let detail = error.into();
        BusinessAppResult {
            app_id: PREDICTION_APP_ID.into(),
            status: "unavailable".into(),
            freshness: "missing".into(),
            fallback_reply: format!("预测数据暂时不可用：{detail}"),
            narration: None,
        }
    }

    fn stale(snapshot: &crate::models::PredictionSnapshot) -> BusinessAppResult {
        BusinessAppResult {
            app_id: PREDICTION_APP_ID.into(),
            status: "stale".into(),
            freshness: "stale".into(),
            fallback_reply: format!(
                "{} 第{}期\n最新结果：{}\n更新时间：{}\n数据状态：数据已过期，当前仅展示最后一次已核验结果。",
                snapshot.game,
                snapshot.period,
                snapshot
                    .result
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" + "),
                snapshot.updated_at.with_timezone(&chrono::Local).format("%m-%d %H:%M"),
            ),
            narration: None,
        }
    }

    fn fresh(result: PredictionResult) -> BusinessAppResult {
        let fallback_reply = prediction::format_reply(&result);
        let narration = serde_json::json!({
            "game": result.game,
            "period": result.period,
            "latestResult": result.latest_result,
            "updatedAt": result.updated_at.to_rfc3339(),
            "trend": result.trend,
            "candidates": result.candidates,
            "confidence": result.confidence,
            "freshness": result.freshness,
        });
        BusinessAppResult {
            app_id: PREDICTION_APP_ID.into(),
            status: "succeeded".into(),
            freshness: "fresh".into(),
            fallback_reply,
            narration: Some(narration),
        }
    }
}

#[async_trait]
impl BusinessApp for PredictionApp {
    fn manifest(&self) -> BusinessAppManifest {
        BusinessAppManifest {
            id: PREDICTION_APP_ID,
            name: "预测",
            description: "读取已校准结果并生成统计参考",
            version: PREDICTION_APP_VERSION,
            trigger_hint: "@DH 预测 彩种名称",
            scope: "所有已启用管理且开启 AI 回复的群",
        }
    }

    fn matches(&self, message: &str) -> bool {
        let command = crate::ai::message_without_mention(message);
        command
            .trim_start_matches(|character: char| {
                character.is_whitespace() || matches!(character, ':' | '：' | ',' | '，')
            })
            .starts_with("预测")
    }

    async fn health(&self, now: DateTime<Utc>) -> AppResult<BusinessAppHealth> {
        let games = self.source.list_games().await?;
        let mut health = Vec::with_capacity(games.len());
        for game in games {
            let item = match self.source.load(&game).await {
                Ok(result) => match result.status {
                    PredictionFreshness::Fresh => BusinessAppGameHealth {
                        id: game.id.into(),
                        name: game.name.into(),
                        status: "ready".into(),
                        detail: "结果与历史数据可用".into(),
                    },
                    PredictionFreshness::Stale => BusinessAppGameHealth {
                        id: game.id.into(),
                        name: game.name.into(),
                        status: "stale".into(),
                        detail: "仅检测到过期结果".into(),
                    },
                    PredictionFreshness::Missing => BusinessAppGameHealth {
                        id: game.id.into(),
                        name: game.name.into(),
                        status: "unavailable".into(),
                        detail: "未检测到可用结果".into(),
                    },
                },
                Err(error) => BusinessAppGameHealth {
                    id: game.id.into(),
                    name: game.name.into(),
                    status: "unavailable".into(),
                    detail: if error.code == "prediction_http" {
                        "数据源返回异常，请稍后重试".into()
                    } else {
                        "数据源连接失败，请稍后重试".into()
                    },
                },
            };
            health.push(item);
        }
        let ready = health.iter().filter(|item| item.status == "ready").count();
        let stale = health.iter().filter(|item| item.status == "stale").count();
        let status = if ready > 0 {
            "ready"
        } else if stale > 0 {
            "stale"
        } else {
            "unavailable"
        };
        Ok(BusinessAppHealth {
            app_id: PREDICTION_APP_ID.into(),
            status: status.into(),
            detail: if ready > 0 {
                format!("{ready} 个彩种数据可用")
            } else {
                "当前没有通过校验的数据源".into()
            },
            checked_at: now,
            games: health,
        })
    }

    async fn run(&self, context: &BusinessAppContext<'_>) -> AppResult<BusinessAppResult> {
        if context.account_id != context.group.account_id
            || context.group.group_id != context.member.group_id
            || context.group.group_id != context.message.group_id
        {
            return Err(AppError::new(
                "business_app_context",
                "业务应用上下文中的账号或群信息不一致",
            ));
        }
        let _evaluated_at = context.now;
        let text = context.message.text.as_str();
        let result = match prediction::fetch_status(text, self.source.as_ref()).await {
            Ok(result) => result,
            Err(_) => return Ok(Self::unavailable("数据源暂时不可用，请稍后重试")),
        };
        let status = match result {
            Err(prompt) => BusinessAppResult {
                app_id: PREDICTION_APP_ID.into(),
                status: "needs_input".into(),
                freshness: "missing".into(),
                fallback_reply: prompt,
                narration: None,
            },
            Ok(source_result) => match source_result.status {
                PredictionFreshness::Fresh => match source_result.snapshot {
                    Some(snapshot) => Self::fresh(prediction::analyze(&snapshot)),
                    None => Self::unavailable("当前彩种暂未发现可用数据"),
                },
                PredictionFreshness::Stale => match source_result.snapshot {
                    Some(snapshot) => Self::stale(&snapshot),
                    None => Self::unavailable("当前彩种暂未发现可用数据"),
                },
                PredictionFreshness::Missing => Self::unavailable("当前彩种暂未发现可用数据"),
            },
        };
        Ok(status)
    }
}

pub fn prediction_narration_prompt(data: &Value) -> String {
    format!(
        "你正在为 DH BOT 的预测业务应用整理已校验数据。仅依据下方规范化数据生成自然、简洁中文。保留彩种、期号、最新结果、更新时间、趋势、候选方向与参考度；明确这是统计参考，不承诺结果。只返回 reply，不创建 actions 或 tasks。不得提及接口、供应方、认证字段、原始响应或内部实现。\n\n规范化数据：{}",
        data
    )
}

pub fn app_not_available(app_id: &str) -> AppError {
    AppError::new("business_app_missing", format!("业务应用 {app_id} 未注册"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PredictionSnapshot;
    use crate::prediction::FixturePredictionSource;
    use chrono::Duration;

    #[tokio::test]
    async fn prediction_app_returns_stale_without_narration() {
        let source = FixturePredictionSource::new([PredictionSnapshot {
            game: "PC28".into(),
            period: "1".into(),
            result: vec![1, 2, 3],
            updated_at: Utc::now() - Duration::minutes(20),
            history: vec![vec![1, 2, 3]],
            freshness: "fresh".into(),
        }]);
        let app = PredictionApp {
            source: Arc::new(source),
        };
        let health = app.health(Utc::now()).await.unwrap();
        assert_eq!(health.games[0].status, "stale");
    }

    #[test]
    fn narration_prompt_hides_sources() {
        let prompt = prediction_narration_prompt(&serde_json::json!({"game":"PC28","period":"1"}));
        assert!(prompt.contains("PC28"));
        assert!(!prompt.contains("http"));
    }

    #[test]
    fn prediction_app_only_matches_a_prediction_command() {
        let source = FixturePredictionSource::new([]);
        let app = PredictionApp {
            source: Arc::new(source),
        };
        assert!(app.matches("@DH 预测 PC28"));
        assert!(app.matches("@ DH：预测"));
        assert!(!app.matches("@DH 请问能不能预测"));
        assert!(!app.matches("普通聊天预测"));
    }
}
