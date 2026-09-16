use crate::{authorized, same_origin, ApiResult, Server};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use dh_core::{
    conversations::{ConversationKey, InboundMessage},
    error::{AppError, AppResult},
};
use dh_protocol::nim_message::{self, Event, Scene};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

pub async fn receive(state: &Server) -> AppResult<()> {
    let (account, epoch, message_key, packet) = {
        let Ok(mut guard) = state.protocol.try_lock() else {
            return Ok(());
        };
        let Some(session) = guard.as_mut() else {
            return Ok(());
        };
        let Some(account) = session.nim_account.clone() else {
            return Ok(());
        };
        if session.nim.is_none() {
            return Ok(());
        }
        let result = async {
            if !session.sync_started {
                let lookup = account.clone();
                let cursor = state
                    .service
                    .database
                    .execute(move |db| db.conversation_cursor(&lookup))
                    .await?;
                session
                    .nim
                    .as_mut()
                    .unwrap()
                    .start_sync(cursor)
                    .await
                    .map_err(|_| AppError::new("message_sync", "消息同步连接中断"))?;
                session.sync_started = true;
            }
            session
                .nim
                .as_mut()
                .unwrap()
                .poll(Duration::from_millis(100))
                .await
                .map_err(|_| AppError::new("message_receive", "消息接收连接中断"))
        }
        .await;
        match result {
            Ok(Some(packet)) => (
                account,
                session.epoch.load(std::sync::atomic::Ordering::SeqCst),
                session.message_key().ok(),
                packet,
            ),
            Ok(None) => return Ok(()),
            Err(error) => {
                session.nim = None;
                session.sync_started = false;
                return Err(error);
            }
        }
    };
    let processing = async {
    let service = packet.header.service;
    let command = packet.header.command;
    let event = nim_message::decode(&packet);
    let reason = if event.is_err() {
        "decode_failed"
    } else {
        "received"
    };
    let saved = account.clone();
    let body = packet.body.to_vec();
    state
        .service
        .database
        .execute(move |db| db.journal_nim_packet(&saved, service, command, &body, reason))
        .await?;
    match event {
        Ok(Event::Recalled { target, server_id }) => {
            let saved = account.clone();
            state.service.database.execute(move |db| db.mark_conversation_recalled(&saved, &target, &server_id, "notification")).await?;
        }
        Ok(Event::Messages(messages)) => {
            for message in messages {
                let outgoing = message.from == account;
                let target = if message.scene == Scene::Group || outgoing {
                    message.to.clone()
                } else {
                    message.from.clone()
                };
                let key = ConversationKey {
                    platform: "wangshangliao".into(),
                    account_id: account.clone(),
                    kind: if message.scene == Scene::Group {
                        "group"
                    } else {
                        "private"
                    }
                    .into(),
                    target,
                };
                let mut kind = if message.kind == 0 {
                    "text"
                } else if message.kind == 100 {
                    "encrypted"
                } else {
                    "unsupported"
                };
                let mut text = if kind == "text" {
                    message.body.clone()
                } else {
                    String::new()
                };
                let mut name = String::new();
                let mut sender_name = message.nickname.clone();
                let mut metadata = json!({"attachment":message.attachment,"idClient":message.client_id,"idServer":message.server_id,"from":message.from,"to":message.to,"time":message.time,"flow":if outgoing{"out"}else{"in"},"scene":if message.scene==Scene::Group{"team"}else{"p2p"},"service":service,"command":command});
                if message.kind == 100 {
                    if let Some(secret) = message_key.as_ref() {
                        if let Ok(decoded) = dh_protocol::message_envelope::open_authenticated(
                            **secret,
                            &message.attachment,
                        ) {
                            if message.scene == Scene::Group && decoded.session == 2 {
                                let from = decoded.from.as_ref().unwrap();
                                let to = decoded.to.as_ref().unwrap();
                                let group = i64::from(to.id);
                                let lookup = account.clone();
                                let mut route = state
                                    .service
                                    .database
                                    .execute(move |db| db.conversation_route(&lookup, group))
                                    .await?;
                                if route.as_deref() != Some(message.to.as_str()) {
                                    let _ = state.service.gateway.list_groups().await;
                                    let lookup = account.clone();
                                    route = state
                                        .service
                                        .database
                                        .execute(move |db| db.conversation_route(&lookup, group))
                                        .await?;
                                }
                                let lookup = account.clone();
                                let nim_sender = message.from.clone();
                                let mut sender = state
                                    .service
                                    .database
                                    .execute(move |db| {
                                        db.resolve_member_user_id(&lookup, group, &nim_sender)
                                    })
                                    .await?;
                                if sender != Some(i64::from(from.id)) {
                                    if let Ok(roster) =
                                        state.service.gateway.list_members(group).await
                                    {
                                        sender = roster
                                            .members
                                            .iter()
                                            .find(|m| m.nim_id == message.from)
                                            .map(|m| m.user_id);
                                        for member in roster.members {
                                            state.service.database.upsert_member(member).await?;
                                        }
                                    }
                                }
                                if route.as_deref() == Some(message.to.as_str())
                                    && sender == Some(i64::from(from.id))
                                {
                                    name = to.name.clone();
                                    sender_name = from.name.clone();
                                    let content = decoded.content.as_ref().or_else(|| {
                                        decoded.mentions.as_ref().and_then(|m| m.content.as_ref())
                                    });
                                    kind = if decoded.format == 0 && content.is_some() {
                                        "text"
                                    } else {
                                        "unsupported"
                                    };
                                    if kind == "text" {
                                        text = content.unwrap().data.clone();
                                    }
                                    metadata["type"] =
                                        json!(if kind == "text" { "text" } else { "custom" });
                                    metadata["decoded"] = json!({"from":{"id":from.id,"name":from.name},"to":{"id":to.id,"name":to.name},"msgSession":2,"msgFormat":decoded.format,"content":{"data":text},"mentions":decoded.mentions.as_ref().map(|m|m.people.iter().map(|p|json!({"uid":p.uid,"nick":p.nick,"start":p.start,"end":p.end})).collect::<Vec<_>>())});
                                }
                            }
                        }
                    }
                }
                let input = InboundMessage {
                    conversation: key.clone(),
                    conversation_name: name,
                    server_id: message.server_id.clone(),
                    sender: message.from.clone(),
                    sender_name,
                    sent_at: i64::try_from(message.time)
                        .map_err(|_| AppError::new("message_time", "消息时间无效"))?,
                    text,
                    content_kind: kind.into(),
                    outgoing,
                    raw_metadata: metadata.to_string(),
                };
                state
                    .service
                    .database
                    .execute(move |db| db.ingest_conversation(&input))
                    .await?;
                // ACK only the same account/epoch. Local persistence precedes socket flush.
                let mut guard = state.protocol.lock().await;
                if let Some(session) = guard.as_mut().filter(|s| {
                    s.nim_account.as_deref() == Some(&account)
                        && s.epoch.load(std::sync::atomic::Ordering::SeqCst) == epoch
                }) {
                    if let Some(nim) = session.nim.as_mut() {
                        if nim
                            .acknowledge(message.scene, std::slice::from_ref(&message.server_id))
                            .await
                            .is_err()
                        {
                            session.nim = None;
                            session.sync_started = false;
                            return Err(AppError::new("message_ack", "消息确认连接中断"));
                        }
                        state
                            .service
                            .database
                            .execute(move |db| db.mark_conversation_ack(&key, &message.server_id))
                            .await?;
                    }
                }
            }
        }
        Ok(Event::SyncDone(cursor)) => {
            let account = account.clone();
            state
                .service
                .database
                .execute(move |db| db.save_conversation_cursor(&account, cursor))
                .await?;
        }
        Ok(Event::Unhandled { .. }) => {}
        Err(_) => {
            return Err(AppError::new(
                "message_decode",
                "消息解析失败，已保留诊断记录并停止推进同步游标",
            ));
        }
    }
    Ok(())
    }.await;
    // A consumed packet must be replayed after any persistence or decoding failure.
    // Never advance the old socket's sync boundary after losing part of a batch.
    if processing.is_err() {
        let mut guard = state.protocol.lock().await;
        if let Some(session) = guard.as_mut().filter(|s| {
            s.nim_account.as_deref() == Some(&account)
                && s.epoch.load(std::sync::atomic::Ordering::SeqCst) == epoch
        }) {
            session.nim = None;
            session.sync_started = false;
        }
    }
    processing
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    id: Option<String>,
    before: Option<i64>,
}
async fn account(state: &Server, headers: &HeaderMap) -> Result<String, StatusCode> {
    if !authorized(state, headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    if !same_origin(state, headers) {
        return Err(StatusCode::FORBIDDEN);
    }
    state
        .protocol
        .lock()
        .await
        .as_ref()
        .and_then(|s| s.nim_account.clone())
        .ok_or(StatusCode::CONFLICT)
}
pub async fn list(State(state): State<Arc<Server>>, headers: HeaderMap) -> ApiResult {
    let account = account(&state, &headers).await?;
    let values = state
        .service
        .database
        .execute(move |db| db.list_conversations(&account))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({"conversations":values})))
}
pub async fn history(
    State(state): State<Arc<Server>>,
    headers: HeaderMap,
    Json(query): Json<Query>,
) -> ApiResult {
    let account = account(&state, &headers).await?;
    let id = query.id.ok_or(StatusCode::BAD_REQUEST)?;
    if uuid::Uuid::parse_str(&id).is_err() || query.before.unwrap_or(0) < 0 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let messages = state
        .service
        .database
        .execute(move |db| db.conversation_history(&account, &id, query.before.unwrap_or(0)))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({"messages":messages})))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    id: String,
    enabled: bool,
    manual_takeover: bool,
}
pub async fn policy(
    State(state): State<Arc<Server>>,
    headers: HeaderMap,
    Json(query): Json<Policy>,
) -> ApiResult {
    let account = account(&state, &headers).await?;
    if uuid::Uuid::parse_str(&query.id).is_err() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let lookup = account.clone();
    let verified = state
        .service
        .database
        .execute(move |db| db.get_setting(&format!("rust.message_verified.{lookup}")))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        == Some("true".into());
    if query.enabled && !verified {
        return Ok(Json(
            json!({"ok":false,"message":"消息收发验证完成后才能启用自动回复"}),
        ));
    }
    state
        .service
        .database
        .execute(move |db| {
            db.set_conversation_policy(&account, &query.id, query.enabled, query.manual_takeover)
        })
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    Ok(Json(Value::Bool(true)))
}
