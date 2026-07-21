use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::diagnostics::Logger;
use crate::error::AppError;
use crate::gateway::RuntimeGateway;
use crate::models::MemberRef;

pub fn spawn(
    gateway: Arc<dyn RuntimeGateway>,
    shutdown: Arc<Notify>,
    logger: Logger,
) -> tauri::async_runtime::JoinHandle<()> {
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
            .with_state(gateway);
        let listener = loop {
            let bind = tokio::net::TcpListener::bind("127.0.0.1:51235");
            match tokio::select! {
                _ = shutdown.notified() => return,
                result = bind => result,
            } {
                Ok(listener) => break listener,
                Err(error) => {
                    logger.write("WARN", &format!("本地诊断桥监听失败，5 秒后重试：{error}"));
                    tokio::select! {
                        _ = shutdown.notified() => return,
                        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                    }
                }
            }
        };
        if let Err(error) = axum::serve(listener, router)
            .with_graceful_shutdown(async move { shutdown.notified().await })
            .await
        {
            logger.write("ERROR", &format!("本地诊断桥退出：{error}"));
        }
    })
}

type BridgeResponse = Result<Json<Value>, (StatusCode, Json<Value>)>;

async fn ping(State(gateway): State<Arc<dyn RuntimeGateway>>) -> BridgeResponse {
    let snapshot = gateway.diagnose().await;
    Ok(success(
        json!({"bridge":"rust","status":snapshot.status,"detail":snapshot.detail,"nimAccount":snapshot.nim_account}),
    ))
}

async fn list_groups(State(gateway): State<Arc<dyn RuntimeGateway>>) -> BridgeResponse {
    gateway
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
    State(gateway): State<Arc<dyn RuntimeGateway>>,
    Json(input): Json<GroupID>,
) -> BridgeResponse {
    gateway
        .list_members(input.group_id)
        .await
        .map(|value| success(json!(value)))
        .map_err(failure)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendText {
    group_id: i64,
    text: String,
}
async fn send_text(
    State(gateway): State<Arc<dyn RuntimeGateway>>,
    Json(input): Json<SendText>,
) -> BridgeResponse {
    gateway
        .send_text(input.group_id, &input.text)
        .await
        .map(|receipt| success(json!(receipt)))
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
    State(gateway): State<Arc<dyn RuntimeGateway>>,
    Json(input): Json<Recall>,
) -> BridgeResponse {
    gateway
        .recall(input.group_id, input.user_id, &input.message_id)
        .await
        .map(|receipt| success(json!(receipt)))
        .map_err(failure)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mute {
    group_id: i64,
    user_id: i64,
    duration_seconds: i64,
}
async fn mute(
    State(gateway): State<Arc<dyn RuntimeGateway>>,
    Json(input): Json<Mute>,
) -> BridgeResponse {
    gateway
        .mute(input.group_id, input.user_id, input.duration_seconds)
        .await
        .map(|receipt| success(json!(receipt)))
        .map_err(failure)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemberID {
    group_id: i64,
    user_id: i64,
}
async fn unmute(
    State(gateway): State<Arc<dyn RuntimeGateway>>,
    Json(input): Json<MemberID>,
) -> BridgeResponse {
    gateway
        .unmute(input.group_id, input.user_id)
        .await
        .map(|receipt| success(json!(receipt)))
        .map_err(failure)
}
async fn remove_member(
    State(gateway): State<Arc<dyn RuntimeGateway>>,
    Json(input): Json<MemberID>,
) -> BridgeResponse {
    gateway
        .remove_member(input.group_id, input.user_id)
        .await
        .map(|receipt| success(json!(receipt)))
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
    State(gateway): State<Arc<dyn RuntimeGateway>>,
    Json(input): Json<Rename>,
) -> BridgeResponse {
    gateway
        .rename(
            input.group_id,
            &MemberRef {
                user_id: input.user_id,
                nim_id: input.nim_id,
            },
            &input.nickname,
        )
        .await
        .map(|receipt| success(json!(receipt)))
        .map_err(failure)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupMute {
    group_id: i64,
    muted: bool,
}
async fn set_group_mute(
    State(gateway): State<Arc<dyn RuntimeGateway>>,
    Json(input): Json<GroupMute>,
) -> BridgeResponse {
    gateway
        .set_group_mute(input.group_id, input.muted)
        .await
        .map(|receipt| success(json!(receipt)))
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
