use std::sync::Arc;

use crate::database::DatabaseExecutor;
use crate::diagnostics::Logger;
use crate::error::AppError;
use crate::gateway::RuntimeGateway;
use crate::models::EffectOutboxRequest;
use crate::shutdown::ShutdownSignal;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

struct BridgeState {
    gateway: Arc<dyn RuntimeGateway>,
    database: DatabaseExecutor,
}

pub fn spawn(
    gateway: Arc<dyn RuntimeGateway>,
    database: DatabaseExecutor,
    shutdown: Arc<ShutdownSignal>,
    logger: Logger,
) -> tauri::async_runtime::JoinHandle<()> {
    let state = Arc::new(BridgeState { gateway, database });
    tauri::async_runtime::spawn(async move {
        let router = Router::new()
            .route("/ping", get(ping))
            .route("/v1/group/get-group-list", post(list_groups))
            .route("/v1/group/get-group-members", post(list_members))
            .route("/v1/plugins/send-msg", post(send_text))
            .route("/v1/group/message-rollback", post(recall))
            .route("/v1/group/set-member-mute", post(mute))
            .route("/v1/group/member-mute-cancel", post(unmute))
            .route("/v1/group/set-member-nickname", post(rename))
            .route("/v1/group/remove-group-member", post(remove_member))
            .route("/v1/group/set-group-mute", post(set_group_mute))
            .with_state(state);
        let listener = loop {
            let bind = tokio::net::TcpListener::bind("127.0.0.1:51235");
            match tokio::select! {
                _ = shutdown.cancelled() => return,
                result = bind => result,
            } {
                Ok(listener) => break listener,
                Err(error) => {
                    logger.write("WARN", &format!("本地诊断桥监听失败，5 秒后重试：{error}"));
                    tokio::select! {
                        _ = shutdown.cancelled() => return,
                        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                    }
                }
            }
        };
        if let Err(error) = axum::serve(listener, router)
            .with_graceful_shutdown(async move { shutdown.cancelled().await })
            .await
        {
            logger.write("ERROR", &format!("本地诊断桥退出：{error}"));
        }
    })
}

type BridgeResponse = Result<Json<Value>, (StatusCode, Json<Value>)>;

// -- read routes (direct gateway) --------------------------------------------

async fn ping(State(state): State<Arc<BridgeState>>) -> BridgeResponse {
    let snapshot = state.gateway.diagnose().await;
    Ok(success(
        json!({"bridge":"rust","status":snapshot.status,"detail":snapshot.detail,"nimAccount":snapshot.nim_account}),
    ))
}

async fn list_groups(State(state): State<Arc<BridgeState>>) -> BridgeResponse {
    state
        .gateway
        .list_groups()
        .await
        .map(|value| success(json!(value)))
        .map_err(failure)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupID {
    group_id: i64,
}
async fn list_members(
    State(state): State<Arc<BridgeState>>,
    Json(input): Json<GroupID>,
) -> BridgeResponse {
    state
        .gateway
        .list_members(input.group_id)
        .await
        .map(|value| success(json!(value)))
        .map_err(failure)
}

// -- helpers for write routes ------------------------------------------------

async fn get_account_id(state: &BridgeState) -> Result<String, (StatusCode, Json<Value>)> {
    state
        .gateway
        .session_identity()
        .await
        .map(|(_, id)| id)
        .map_err(failure)
}

fn enqueue_request(
    account_id: String,
    group_id: i64,
    effect_type: &str,
    payload: Value,
) -> EffectOutboxRequest {
    EffectOutboxRequest {
        account_id,
        group_id,
        effect_type: effect_type.into(),
        payload_json: payload.to_string(),
        dedupe_key: Uuid::new_v4().to_string(),
    }
}

// -- write routes (enqueued via outbox) --------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendText {
    group_id: i64,
    text: String,
}
async fn send_text(
    State(state): State<Arc<BridgeState>>,
    Json(input): Json<SendText>,
) -> BridgeResponse {
    let account_id = get_account_id(&state).await?;
    state
        .database
        .enqueue_effect(enqueue_request(
            account_id,
            input.group_id,
            "send_text",
            json!({"text": input.text, "purpose": "bridge"}),
        ))
        .await
        .map(|enqueued| success(json!(enqueued)))
        .map_err(failure)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Recall {
    group_id: i64,
    user_id: i64,
    message_id: String,
}
async fn recall(
    State(state): State<Arc<BridgeState>>,
    Json(input): Json<Recall>,
) -> BridgeResponse {
    let account_id = get_account_id(&state).await?;
    state
        .database
        .enqueue_effect(enqueue_request(
            account_id,
            input.group_id,
            "recall",
            json!({"userId": input.user_id, "serverMessageId": input.message_id}),
        ))
        .await
        .map(|enqueued| success(json!(enqueued)))
        .map_err(failure)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mute {
    group_id: i64,
    user_id: i64,
    duration_seconds: i64,
}
async fn mute(State(state): State<Arc<BridgeState>>, Json(input): Json<Mute>) -> BridgeResponse {
    let account_id = get_account_id(&state).await?;
    state
        .database
        .enqueue_effect(enqueue_request(
            account_id,
            input.group_id,
            "mute",
            json!({"userId": input.user_id, "durationSeconds": input.duration_seconds}),
        ))
        .await
        .map(|enqueued| success(json!(enqueued)))
        .map_err(failure)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemberID {
    group_id: i64,
    user_id: i64,
}
async fn unmute(
    State(state): State<Arc<BridgeState>>,
    Json(input): Json<MemberID>,
) -> BridgeResponse {
    let account_id = get_account_id(&state).await?;
    state
        .database
        .enqueue_effect(enqueue_request(
            account_id,
            input.group_id,
            "unmute",
            json!({"userId": input.user_id}),
        ))
        .await
        .map(|enqueued| success(json!(enqueued)))
        .map_err(failure)
}
async fn remove_member(
    State(state): State<Arc<BridgeState>>,
    Json(input): Json<MemberID>,
) -> BridgeResponse {
    let account_id = get_account_id(&state).await?;
    state
        .database
        .enqueue_effect(enqueue_request(
            account_id,
            input.group_id,
            "remove",
            json!({"userId": input.user_id}),
        ))
        .await
        .map(|enqueued| success(json!(enqueued)))
        .map_err(failure)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rename {
    group_id: i64,
    user_id: Option<i64>,
    nim_id: Option<String>,
    nickname: String,
}
async fn rename(
    State(state): State<Arc<BridgeState>>,
    Json(input): Json<Rename>,
) -> BridgeResponse {
    let account_id = get_account_id(&state).await?;
    state
        .database
        .enqueue_effect(enqueue_request(
            account_id,
            input.group_id,
            "rename",
            json!({
                "userId": input.user_id.unwrap_or(0),
                "nimId": input.nim_id,
                "nickname": input.nickname,
            }),
        ))
        .await
        .map(|enqueued| success(json!(enqueued)))
        .map_err(failure)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupMute {
    group_id: i64,
    muted: bool,
}
async fn set_group_mute(
    State(state): State<Arc<BridgeState>>,
    Json(input): Json<GroupMute>,
) -> BridgeResponse {
    let account_id = get_account_id(&state).await?;
    state
        .database
        .enqueue_effect(enqueue_request(
            account_id,
            input.group_id,
            "group_mute",
            json!({"muted": input.muted}),
        ))
        .await
        .map(|enqueued| success(json!(enqueued)))
        .map_err(failure)
}

fn success(data: Value) -> Json<Value> {
    Json(json!({"code":0,"errno":0,"msg":"OK","data":data}))
}
fn failure(error: AppError) -> (StatusCode, Json<Value>) {
    let status = if error.code == "nim_not_ready" {
        StatusCode::SERVICE_UNAVAILABLE
    } else if error.retryable {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::BAD_REQUEST
    };
    (
        status,
        Json(
            json!({"code":status.as_u16(),"errno":1,"msg":error.message,"retryable":error.retryable}),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nim_not_ready_maps_to_503() {
        assert_eq!(
            failure(AppError::new("nim_not_ready", "wait")).0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
