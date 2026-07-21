#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use dh_bot_lib::fixture::{
    FixtureFaults, FixtureGateway, FixtureSnapshot, FIXTURE_ACCOUNT, FIXTURE_GROUP,
};
use dh_bot_lib::gateway::{GroupGateway, RuntimeGateway};
use dh_bot_lib::models::MemberRef;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;

#[derive(Clone)]
struct HostState {
    gateway: Arc<RwLock<FixtureGateway>>,
    http_port: u16,
    devtools_port: u16,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageInput {
    version: Option<u8>,
    group_id: Option<i64>,
    user_id: Option<i64>,
    text: String,
    sequence: Option<u64>,
    server_message_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemberEventInput {
    version: Option<u8>,
    group_id: Option<i64>,
    user_id: i64,
    nim_id: Option<String>,
    name: Option<String>,
    role: Option<String>,
}

async fn snapshot(State(state): State<HostState>) -> Json<FixtureSnapshot> {
    let gateway = state.gateway.read().await.clone();
    Json(gateway.snapshot().await)
}

async fn actions(State(state): State<HostState>) -> Json<Vec<dh_bot_lib::fixture::FixtureAction>> {
    let gateway = state.gateway.read().await.clone();
    Json(gateway.snapshot().await.actions)
}

async fn reset(State(state): State<HostState>) -> Json<serde_json::Value> {
    *state.gateway.write().await = FixtureGateway::new_default();
    Json(serde_json::json!({"ok":true,"groupId":FIXTURE_GROUP}))
}

async fn emit_message(
    State(state): State<HostState>,
    Json(input): Json<MessageInput>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    validate_version(input.version)?;
    let gateway = state.gateway.read().await.clone();
    gateway
        .emit_message(
            input.group_id.unwrap_or(FIXTURE_GROUP),
            input.user_id.unwrap_or(10006),
            input.text.trim(),
            input.sequence,
            input.server_message_id,
        )
        .await
        .map(|sequence| Json(serde_json::json!({"ok":true,"sequence":sequence})))
        .map_err(|error| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"ok":false,"error":error.message})),
            )
        })
}

async fn member_joined(
    State(state): State<HostState>,
    Json(input): Json<MemberEventInput>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    validate_version(input.version)?;
    let gateway = state.gateway.read().await.clone();
    gateway
        .member_joined(
            input.group_id.unwrap_or(FIXTURE_GROUP),
            input.user_id,
            input.nim_id,
            input.name,
            input.role,
        )
        .await
        .map(|_| Json(serde_json::json!({"ok":true})))
        .map_err(|error| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"ok":false,"error":error.message})),
            )
        })
}

async fn member_left(
    State(state): State<HostState>,
    Json(input): Json<MemberEventInput>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    validate_version(input.version)?;
    let gateway = state.gateway.read().await.clone();
    gateway
        .member_left(input.group_id.unwrap_or(FIXTURE_GROUP), input.user_id)
        .await
        .map(|_| Json(serde_json::json!({"ok":true})))
        .map_err(|error| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"ok":false,"error":error.message})),
            )
        })
}

fn validate_version(
    version: Option<u8>,
) -> Result<(), (axum::http::StatusCode, Json<serde_json::Value>)> {
    if version.unwrap_or(1) == 1 {
        return Ok(());
    }
    Err((
        axum::http::StatusCode::BAD_REQUEST,
        Json(json!({"ok":false,"error":"Fixture 场景仅支持 JSON v1"})),
    ))
}

async fn faults(
    State(state): State<HostState>,
    Json(input): Json<FixtureFaults>,
) -> Json<serde_json::Value> {
    let gateway = state.gateway.read().await.clone();
    gateway.set_faults(input).await;
    Json(serde_json::json!({"ok":true}))
}

async fn index() -> axum::response::Html<&'static str> {
    axum::response::Html(INDEX)
}

async fn devtools_pages(
    State(state): State<HostState>,
) -> Result<Json<Value>, axum::http::StatusCode> {
    if !state
        .gateway
        .read()
        .await
        .snapshot()
        .await
        .faults
        .devtools_ready
    {
        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }
    let websocket = format!("127.0.0.1:{}/devtools/page/fixture", state.devtools_port);
    Ok(Json(json!([{
        "description":"DH Fixture DevTools target",
        "devtoolsFrontendUrl":format!("/devtools/inspector.html?ws={websocket}"),
        "id":"fixture",
        "title":"DH Fixture / 旺商聊",
        "type":"page",
        "url":format!("http://127.0.0.1:{}/", state.http_port),
        "webSocketDebuggerUrl":format!("ws://{websocket}")
    }])))
}

async fn devtools_version(
    State(state): State<HostState>,
) -> Result<Json<Value>, axum::http::StatusCode> {
    if !state
        .gateway
        .read()
        .await
        .snapshot()
        .await
        .faults
        .devtools_ready
    {
        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(Json(json!({
        "Browser":"DH Fixture/1.0",
        "Protocol-Version":"1.3",
        "User-Agent":"DH-Fixture",
        "webSocketDebuggerUrl":format!("ws://127.0.0.1:{}/devtools/page/fixture", state.devtools_port)
    })))
}

async fn devtools_socket(
    ws: WebSocketUpgrade,
    State(state): State<HostState>,
) -> axum::response::Response {
    ws.on_upgrade(move |socket| serve_devtools(socket, state))
}

async fn serve_devtools(mut socket: WebSocket, state: HostState) {
    while let Some(Ok(WsMessage::Text(text))) = socket.recv().await {
        let Ok(request) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let id = request.get("id").and_then(Value::as_u64).unwrap_or(0);
        let expression = request
            .pointer("/params/expression")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let gateway = state.gateway.read().await.clone();
        let value = evaluate_fixture(&gateway, expression).await.unwrap_or_else(|error| {
            if expression.contains("ipc.send(\"xclient\"") {
                json!({"transportCode": if error.retryable {504} else {403}, "errno":1, "error":error.message})
            } else {
                json!({"ok":false,"errorMessage":error.message})
            }
        });
        let response = json!({"id":id,"result":{"result":{"type":"object","value":value}}});
        if socket
            .send(WsMessage::Text(response.to_string().into()))
            .await
            .is_err()
        {
            break;
        }
    }
}

async fn evaluate_fixture(
    gateway: &FixtureGateway,
    expression: &str,
) -> dh_bot_lib::error::AppResult<Value> {
    if expression.contains("nimAccount") {
        let snapshot = gateway.snapshot().await;
        return Ok(json!({"nimAccount": if snapshot.faults.nim_ready {FIXTURE_ACCOUNT} else {""}}));
    }
    if expression.contains("ipc.send(\"xclient\"") {
        let input = extract_input(expression)?;
        if input.get("type").and_then(Value::as_str) == Some("encode") {
            return Ok(json!({"transportCode":200,"errno":0,"response":"fixture-encoded-message"}));
        }
        let route = input
            .get("route")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let payload = input.get("payload").cloned().unwrap_or_else(|| json!({}));
        let response = gateway.wire_route(route, payload).await?;
        return Ok(json!({"transportCode":200,"errno":0,"response":response.to_string()}));
    }
    if expression.contains("getTeamMembers") {
        return Ok(gateway.wire_nim_members(FIXTURE_GROUP).await);
    }
    if expression.contains("updateNickInTeam") {
        let input = extract_input(expression)?;
        let account = input
            .get("account")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let nickname = input
            .get("nick")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let user_id = gateway
            .snapshot()
            .await
            .members
            .into_iter()
            .find(|member| member.nim_id == account)
            .map(|member| member.user_id)
            .unwrap_or(0);
        gateway
            .rename(
                FIXTURE_GROUP,
                &MemberRef {
                    user_id: Some(user_id),
                    nim_id: Some(account.into()),
                },
                nickname,
            )
            .await?;
        return Ok(json!({"ok":true}));
    }
    if expression.contains("sendCustomMsg") {
        let input = extract_input(expression)?;
        let delivery = gateway
            .send_text(
                FIXTURE_GROUP,
                input
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .await?;
        return Ok(json!({"ok":true,"idServer":delivery,"idClient":delivery}));
    }
    if expression.contains("state.queue.slice") {
        return gateway.poll_messages().await;
    }
    if let Some(sequence) = expression
        .split("Number(item.seq)>")
        .nth(1)
        .and_then(|value| {
            value
                .split(|character: char| !character.is_ascii_digit())
                .next()
        })
        .and_then(|value| value.parse::<u64>().ok())
    {
        return gateway.acknowledge_messages(sequence).await;
    }
    if expression.contains("__dhBridgeMessages") || expression.contains("const nim=window.nim") {
        return gateway.install_message_listener().await;
    }
    Ok(json!({"ok":true}))
}

fn extract_input(expression: &str) -> dh_bot_lib::error::AppResult<Value> {
    let value = expression.split("const input=").nth(1).ok_or_else(|| {
        dh_bot_lib::error::AppError::new("fixture_expression", "表达式缺少 input")
    })?;
    let end = value
        .find(";const ")
        .or_else(|| value.find(";return "))
        .unwrap_or(value.len());
    serde_json::from_str(&value[..end])
        .map_err(|error| dh_bot_lib::error::AppError::new("fixture_expression", error.to_string()))
}

#[tokio::main]
async fn main() {
    let http_port = std::env::var("DH_FIXTURE_HTTP_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(51300);
    let devtools_port = std::env::var("DH_FIXTURE_DEVTOOLS_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(9233);
    let state = HostState {
        gateway: Arc::new(RwLock::new(FixtureGateway::new_default())),
        http_port,
        devtools_port,
    };
    let devtools_state = state.clone();
    let app = Router::new()
        .route("/", get(index))
        .route("/fixture/state", get(snapshot))
        .route("/fixture/actions", get(actions))
        .route("/fixture/reset", post(reset))
        .route("/fixture/events/message", post(emit_message))
        .route("/fixture/events/member-joined", post(member_joined))
        .route("/fixture/events/member-left", post(member_left))
        .route("/fixture/faults", post(faults))
        .with_state(state);
    let fixture_listener = tokio::net::TcpListener::bind(("127.0.0.1", http_port))
        .await
        .expect("DH Fixture 监听失败");
    let devtools = Router::new()
        .route("/json/list", get(devtools_pages))
        .route("/json/version", get(devtools_version))
        .route("/devtools/page/fixture", get(devtools_socket))
        .with_state(devtools_state);
    let devtools_listener = tokio::net::TcpListener::bind(("127.0.0.1", devtools_port))
        .await
        .expect("DH Fixture DevTools 监听失败");
    println!(
        "DH Fixture running at http://127.0.0.1:{http_port} (DevTools target: http://127.0.0.1:{devtools_port})"
    );
    tokio::try_join!(
        axum::serve(fixture_listener, app),
        axum::serve(devtools_listener, devtools)
    )
    .expect("DH Fixture 退出");
}

const INDEX: &str = r#"<!doctype html>
<html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>DH Fixture</title>
<style>*{box-sizing:border-box}body{font:14px system-ui;margin:0;color:#111;background:#f5f5f5}.shell{max-width:1180px;margin:auto;padding:28px}.head{display:flex;align-items:flex-start;justify-content:space-between}.tag{display:inline-block;padding:5px 8px;background:#111;color:#fff;border-radius:4px}h1{margin:10px 0 4px}.grid{display:grid;grid-template-columns:1fr 1fr;gap:12px;margin-top:18px}.panel{background:#fff;border:1px solid #ddd;padding:16px;border-radius:6px}.row{display:flex;gap:8px;flex-wrap:wrap;margin-top:10px}button,input{height:36px;padding:0 11px;border:1px solid #ccc;border-radius:5px;background:#fff}button:hover{border-color:#111}button.primary{background:#111;color:#fff;border-color:#111}input{min-width:240px;flex:1}pre{background:#fafafa;border:1px solid #e5e5e5;padding:12px;white-space:pre-wrap;max-height:480px;overflow:auto}.status{font-weight:650}@media(max-width:760px){.grid{grid-template-columns:1fr}.shell{padding:16px}}</style>
<body><main class="shell"><div class="head"><div><span class="tag">测试环境 · DH Fixture</span><h1>微型旺商聊</h1><p>固定 16 人测试群，不联网，不读取真实账号和旺商聊目录。</p></div><span id="summary" class="status">读取中</span></div>
<div class="grid"><section class="panel"><strong>消息与成员事件</strong><div class="row"><input id="text" value="@DH 测试消息"><button class="primary" onclick="send()">注入消息</button></div><div class="row"><button onclick="join()">成员 10017 入群</button><button onclick="leave()">成员 10017 离群</button><button onclick="duplicate()">重复消息</button><button onclick="outOfOrder()">乱序消息</button></div></section>
<section class="panel"><strong>故障注入</strong><div class="row"><button onclick="setFault('timeout')">下一次超时</button><button onclick="setFault('permission')">权限不足</button><button onclick="setFault('partial')">部分成员名单</button><button onclick="setFault('nim')">NIM 未就绪</button><button onclick="setFault('disconnect')">断开 DevTools</button><button class="primary" onclick="recover()">全部恢复</button></div></section></div>
<div class="row"><button onclick="reset()">重置完整场景</button><button onclick="load()">刷新状态</button></div><h2>当前状态与动作日志</h2><pre id="state">读取中...</pre></main>
<script>const jsonHeaders={'content-type':'application/json'};async function post(url,body){return fetch(url,{method:'POST',headers:jsonHeaders,body:JSON.stringify(body||{})})}async function load(){const data=await(await fetch('/fixture/state')).json();state.textContent=JSON.stringify(data,null,2);summary.textContent=`${data.members.filter(x=>x.present).length} 人 · ${data.queuedMessages} 条待处理 · ${data.actions.length} 个动作`}async function send(extra={}){await post('/fixture/events/message',{version:1,text:text.value,...extra});load()}async function duplicate(){await send({sequence:80,serverMessageId:'fixture-duplicate'});await send({sequence:81,serverMessageId:'fixture-duplicate'})}async function outOfOrder(){await send({sequence:120,serverMessageId:'fixture-later'});await send({sequence:110,serverMessageId:'fixture-earlier'})}async function join(){await post('/fixture/events/member-joined',{version:1,userId:10017,nimId:'fixture-nim-10017',name:'新成员17',role:'member'});load()}async function leave(){await post('/fixture/events/member-left',{version:1,userId:10017});load()}async function reset(){await post('/fixture/reset');load()}async function setFault(kind){const f={devtoolsReady:true,nimReady:true,timeoutNext:false,permissionDenied:false,partialMembers:false};if(kind==='timeout')f.timeoutNext=true;if(kind==='permission')f.permissionDenied=true;if(kind==='partial')f.partialMembers=true;if(kind==='nim')f.nimReady=false;if(kind==='disconnect')f.devtoolsReady=false;await post('/fixture/faults',f);load()}async function recover(){await post('/fixture/faults',{devtoolsReady:true,nimReady:true,timeoutNext:false,permissionDenied:false,partialMembers:false});load()}load();setInterval(load,1500)</script></body></html>"#;
