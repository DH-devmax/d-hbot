#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use dh_bot_lib::fixture::{FixtureFaults, FixtureGateway, FixtureSnapshot, FIXTURE_GROUP};
use dh_bot_lib::gateway::GroupGateway;
use dh_bot_lib::models::MemberRef;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{Mutex, Notify, RwLock};

#[derive(Clone)]
struct HostState {
    gateway: Arc<RwLock<FixtureGateway>>,
    browser_events: Arc<RwLock<VecDeque<BrowserEvent>>>,
    browser: Arc<Mutex<Option<BrowserProcess>>>,
    http_port: u16,
    devtools_port: u16,
    shutdown: Arc<Notify>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserEvent {
    callback: String,
    args: Vec<Value>,
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
    account_state: Option<String>,
    blacklisted: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BurstInput {
    version: Option<u8>,
    count: usize,
    start_sequence: Option<u64>,
    group_id: Option<i64>,
    user_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IpcInput {
    #[serde(rename = "type")]
    kind: String,
    url: Option<String>,
    params: Option<String>,
    key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NimTeamInput {
    team_id: Option<String>,
    account: Option<String>,
    nick_in_team: Option<String>,
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamQuery {
    team_id: Option<String>,
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
    state.browser_events.write().await.clear();
    Json(serde_json::json!({"ok":true,"groupId":FIXTURE_GROUP}))
}

async fn emit_message(
    State(state): State<HostState>,
    Json(input): Json<MessageInput>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    validate_version(input.version)?;
    let gateway = state.gateway.read().await.clone();
    let server_message_id = input
        .server_message_id
        .clone()
        .unwrap_or_else(|| format!("fixture-browser-{}", input.sequence.unwrap_or_default()));
    let group_id = input.group_id.unwrap_or(FIXTURE_GROUP);
    let user_id = input.user_id.unwrap_or(10006);
    let result = gateway
        .emit_message(
            group_id,
            user_id,
            input.text.trim(),
            input.sequence,
            Some(server_message_id.clone()),
        )
        .await;
    match result {
        Ok(sequence) => {
            push_browser_event(
                &state,
                "onmsg",
                vec![json!({
                    "idServer": server_message_id,
                    "scene": "team",
                    "from": format!("fixture-nim-{user_id}"),
                    "to": format!("fixture-cloud-{group_id}"),
                    "time": sequence,
                    "type": "text",
                    "flow": "in",
                    "content": input.text,
                    "fromNick": format!("Fixture成员{}", user_id - 10000),
                })],
            )
            .await;
            Ok(Json(json!({"ok":true,"sequence":sequence})))
        }
        Err(error) => Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"ok":false,"error":error.message})),
        )),
    }
}

async fn emit_burst(
    State(state): State<HostState>,
    Json(input): Json<BurstInput>,
) -> Result<Json<Value>, (axum::http::StatusCode, Json<Value>)> {
    validate_version(input.version)?;
    if input.count == 0 || input.count > 10_000 {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"ok":false,"error":"count 必须介于 1 和 10000"})),
        ));
    }
    let gateway = state.gateway.read().await.clone();
    let group_id = input.group_id.unwrap_or(FIXTURE_GROUP);
    let user_id = input.user_id.unwrap_or(10006);
    let start = input.start_sequence.unwrap_or(1);
    let mut browser_events = Vec::with_capacity(input.count);
    for offset in 0..input.count {
        let business_sequence = start + offset as u64;
        let message_id = format!("fixture-burst-{business_sequence}");
        gateway
            .emit_message(
                group_id,
                user_id,
                &format!("burst-{business_sequence}"),
                Some(business_sequence),
                Some(message_id.clone()),
            )
            .await
            .map_err(|error| {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(json!({"ok":false,"error":error.message})),
                )
            })?;
        browser_events.push(BrowserEvent {
            callback: "onmsg".into(),
            args: vec![json!({
                "idServer": message_id,
                "scene": "team",
                "from": format!("fixture-nim-{user_id}"),
                "to": format!("fixture-cloud-{group_id}"),
                "time": business_sequence,
                "type": "text",
                "flow": "in",
                "content": format!("burst-{business_sequence}"),
                "fromNick": format!("Fixture成员{}", user_id - 10000),
            })],
        });
    }
    state.browser_events.write().await.extend(browser_events);
    Ok(Json(json!({"ok":true,"count":input.count})))
}

async fn member_joined(
    State(state): State<HostState>,
    Json(input): Json<MemberEventInput>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    validate_version(input.version)?;
    let gateway = state.gateway.read().await.clone();
    let group_id = input.group_id.unwrap_or(FIXTURE_GROUP);
    let nim_id = input
        .nim_id
        .clone()
        .unwrap_or_else(|| format!("fixture-nim-{}", input.user_id));
    let name = input.name.clone().unwrap_or_default();
    let role = input.role.clone().unwrap_or_else(|| "member".into());
    gateway
        .member_joined(
            group_id,
            input.user_id,
            Some(nim_id.clone()),
            Some(name.clone()),
            Some(role.clone()),
        )
        .await
        .map_err(|error| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"ok":false,"error":error.message})),
            )
        })?;
    push_browser_event(
        &state,
        "onaddteammembers",
        vec![json!({
            "teamId": format!("fixture-cloud-{group_id}"),
            "members": [{"account":nim_id,"userId":input.user_id,"nickInTeam":name,"type":role}],
            "time": chrono::Utc::now().timestamp_millis(),
        })],
    )
    .await;
    Ok(Json(json!({"ok":true})))
}

async fn member_left(
    State(state): State<HostState>,
    Json(input): Json<MemberEventInput>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    validate_version(input.version)?;
    let gateway = state.gateway.read().await.clone();
    let group_id = input.group_id.unwrap_or(FIXTURE_GROUP);
    let nim_id = input
        .nim_id
        .clone()
        .unwrap_or_else(|| format!("fixture-nim-{}", input.user_id));
    gateway
        .member_left(input.group_id.unwrap_or(FIXTURE_GROUP), input.user_id)
        .await
        .map_err(|error| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"ok":false,"error":error.message})),
            )
        })?;
    push_browser_event(
        &state,
        "onremoveteammembers",
        vec![json!({
            "teamId": format!("fixture-cloud-{group_id}"),
            "members": [{"account":nim_id,"userId":input.user_id}],
            "time": chrono::Utc::now().timestamp_millis(),
        })],
    )
    .await;
    Ok(Json(json!({"ok":true})))
}

async fn member_updated(
    State(state): State<HostState>,
    Json(input): Json<MemberEventInput>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    validate_version(input.version)?;
    let gateway = state.gateway.read().await.clone();
    let group_id = input.group_id.unwrap_or(FIXTURE_GROUP);
    let name = input.name.clone().unwrap_or_default();
    let role = input.role.clone().unwrap_or_else(|| "member".into());
    let nim_id = input
        .nim_id
        .clone()
        .unwrap_or_else(|| format!("fixture-nim-{}", input.user_id));
    gateway
        .member_updated(
            group_id,
            input.user_id,
            input.name,
            input.role,
            input.account_state,
            input.blacklisted,
        )
        .await
        .map_err(|error| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"ok":false,"error":error.message})),
            )
        })?;
    push_browser_event(
        &state,
        "onupdateteammember",
        vec![json!({
            "teamId": format!("fixture-cloud-{group_id}"),
            "members": [{"account":nim_id,"userId":input.user_id,"nickInTeam":name,"type":role}],
            "time": chrono::Utc::now().timestamp_millis(),
        })],
    )
    .await;
    Ok(Json(json!({"ok":true})))
}

async fn push_browser_event(state: &HostState, callback: &str, args: Vec<Value>) {
    state.browser_events.write().await.push_back(BrowserEvent {
        callback: callback.into(),
        args,
    });
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
    let disconnect = !input.devtools_ready;
    let gateway = state.gateway.read().await.clone();
    gateway.set_faults(input).await;
    if disconnect {
        state.browser.lock().await.take();
        return Json(json!({"ok":true,"browser":"stopped"}));
    }
    let mut browser = state.browser.lock().await;
    if browser.is_none() {
        match spawn_browser(state.http_port, state.devtools_port) {
            Ok(process) => *browser = Some(process),
            Err(error) => return Json(json!({"ok":false,"error":error})),
        }
    }
    Json(serde_json::json!({"ok":true,"browser":"running"}))
}

async fn index() -> axum::response::Html<&'static str> {
    axum::response::Html(INDEX)
}

async fn browser_events(State(state): State<HostState>) -> Json<Vec<BrowserEvent>> {
    let mut queue = state.browser_events.write().await;
    Json(queue.drain(..).collect())
}

async fn fixture_ipc(State(state): State<HostState>, Json(input): Json<IpcInput>) -> Json<Value> {
    let request_id = input.key.unwrap_or_else(|| "fixture-ipc".into());
    if input.kind == "encode" {
        return Json(json!({
            "code": 200,
            "errno": 0,
            "response": "fixture-encoded-message",
            "requestId": request_id,
        }));
    }
    let route = input.url.unwrap_or_default();
    let payload = input
        .params
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_else(|| json!({}));
    let gateway = state.gateway.read().await.clone();
    match gateway.wire_route(&route, payload).await {
        Ok(response) => Json(json!({
            "code": 200,
            "errno": 0,
            "response": response.to_string(),
            "requestId": request_id,
        })),
        Err(error) => Json(json!({
            "code": if error.retryable {504} else {500},
            "errno": 1,
            "message": error.message,
            "requestId": request_id,
        })),
    }
}

fn fixture_group_from_team_id(value: Option<&str>) -> i64 {
    value
        .and_then(|value| value.rsplit('-').next())
        .and_then(|value| value.parse().ok())
        .unwrap_or(FIXTURE_GROUP)
}

async fn nim_members(
    State(state): State<HostState>,
    Query(query): Query<TeamQuery>,
) -> Json<Value> {
    let gateway = state.gateway.read().await.clone();
    Json(
        gateway
            .wire_nim_members(fixture_group_from_team_id(query.team_id.as_deref()))
            .await,
    )
}

async fn nim_send(State(state): State<HostState>, Json(input): Json<NimTeamInput>) -> Json<Value> {
    let gateway = state.gateway.read().await.clone();
    let group_id = fixture_group_from_team_id(input.team_id.as_deref());
    match gateway
        .send_text(group_id, input.content.as_deref().unwrap_or_default())
        .await
    {
        Ok(receipt) => Json(json!({
            "ok":true,
            "idServer":receipt.message_id,
            "idClient":receipt.request_id,
            "requestId":receipt.request_id,
        })),
        Err(error) => Json(json!({"ok":false,"errorMessage":error.message})),
    }
}

async fn nim_rename(
    State(state): State<HostState>,
    Json(input): Json<NimTeamInput>,
) -> Json<Value> {
    let gateway = state.gateway.read().await.clone();
    let account = input.account.unwrap_or_default();
    let snapshot = gateway.snapshot().await;
    let user_id = snapshot
        .members
        .into_iter()
        .find(|member| member.nim_id == account)
        .map(|member| member.user_id);
    let member = MemberRef {
        user_id,
        nim_id: Some(account),
    };
    match gateway
        .rename(
            fixture_group_from_team_id(input.team_id.as_deref()),
            &member,
            input.nick_in_team.as_deref().unwrap_or_default(),
        )
        .await
    {
        Ok(_) => Json(json!({"ok":true,"member":{"nickInTeam":input.nick_in_team}})),
        Err(error) => Json(json!({"ok":false,"errorMessage":error.message})),
    }
}

async fn shutdown_fixture(State(state): State<HostState>) -> Json<Value> {
    state.shutdown.notify_waiters();
    Json(json!({"ok":true}))
}

struct BrowserProcess {
    child: Child,
    profile: PathBuf,
}

impl Drop for BrowserProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

fn browser_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("DH_FIXTURE_BROWSER") {
        candidates.push(PathBuf::from(path));
    }
    #[cfg(target_os = "windows")]
    {
        for root in [
            std::env::var_os("PROGRAMFILES"),
            std::env::var_os("PROGRAMFILES(X86)"),
            std::env::var_os("LOCALAPPDATA"),
        ]
        .into_iter()
        .flatten()
        {
            let root = PathBuf::from(root);
            candidates.push(root.join("Microsoft/Edge/Application/msedge.exe"));
            candidates.push(root.join("Google/Chrome/Application/chrome.exe"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from(
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ));
        candidates.push(PathBuf::from(
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ));
        candidates.push(PathBuf::from(
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        for path in [
            "/usr/bin/microsoft-edge",
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
        ] {
            candidates.push(PathBuf::from(path));
        }
    }
    candidates
}

fn locate_browser() -> Result<PathBuf, String> {
    browser_candidates()
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| "未找到 Edge/Chrome/Chromium；可用 DH_FIXTURE_BROWSER 指定开发浏览器".into())
}

fn spawn_browser(http_port: u16, devtools_port: u16) -> Result<BrowserProcess, String> {
    let executable = locate_browser()?;
    let profile = std::env::temp_dir().join(format!(
        "dh-fixture-browser-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&profile).map_err(|error| error.to_string())?;
    let mut command = Command::new(&executable);
    command.args([
        format!("--remote-debugging-port={devtools_port}"),
        "--remote-allow-origins=*".into(),
        format!("--user-data-dir={}", profile.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-background-networking".into(),
        "--disable-component-update".into(),
        "--disable-sync".into(),
        "--window-size=1280,900".into(),
    ]);
    if std::env::var("DH_FIXTURE_HEADLESS").as_deref() == Ok("1") {
        command.arg("--headless=new");
    }
    #[cfg(target_os = "linux")]
    if std::env::var_os("CI").is_some() {
        command.arg("--no-sandbox");
    }
    command
        .arg(format!("http://127.0.0.1:{http_port}/"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let child = command
        .spawn()
        .map_err(|error| format!("启动 Fixture 浏览器 {} 失败：{error}", executable.display()))?;
    Ok(BrowserProcess { child, profile })
}

async fn wait_for_devtools(port: u16, timeout: Duration) -> Result<(), String> {
    let client = reqwest::Client::new();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(response) = client
            .get(format!("http://127.0.0.1:{port}/json/list"))
            .send()
            .await
        {
            if let Ok(pages) = response.json::<Vec<Value>>().await {
                if pages.iter().any(|page| {
                    page.get("title")
                        .and_then(Value::as_str)
                        .is_some_and(|title| title.contains("旺商聊"))
                }) {
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("Fixture 浏览器 DevTools {port} 在限时内未就绪"))
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
    let shutdown = Arc::new(Notify::new());
    let browser = Arc::new(Mutex::new(None));
    let state = HostState {
        gateway: Arc::new(RwLock::new(FixtureGateway::new_default())),
        browser_events: Arc::new(RwLock::new(VecDeque::new())),
        browser: browser.clone(),
        http_port,
        devtools_port,
        shutdown: shutdown.clone(),
    };
    let app = Router::new()
        .route("/", get(index))
        .route("/fixture/state", get(snapshot))
        .route("/fixture/actions", get(actions))
        .route("/fixture/reset", post(reset))
        .route("/fixture/events/message", post(emit_message))
        .route("/fixture/events/burst", post(emit_burst))
        .route("/fixture/events/member-joined", post(member_joined))
        .route("/fixture/events/member-left", post(member_left))
        .route("/fixture/events/member-updated", post(member_updated))
        .route("/fixture/browser-events", get(browser_events))
        .route("/fixture/ipc", post(fixture_ipc))
        .route("/fixture/nim/members", get(nim_members))
        .route("/fixture/nim/send", post(nim_send))
        .route("/fixture/nim/rename", post(nim_rename))
        .route("/fixture/faults", post(faults))
        .route("/fixture/shutdown", post(shutdown_fixture))
        .with_state(state);
    let fixture_listener = tokio::net::TcpListener::bind(("127.0.0.1", http_port))
        .await
        .expect("DH Fixture 监听失败");
    let server_shutdown = shutdown.clone();
    let server = tokio::spawn(async move {
        axum::serve(fixture_listener, app)
            .with_graceful_shutdown(async move { server_shutdown.notified().await })
            .await
    });
    *browser.lock().await =
        Some(spawn_browser(http_port, devtools_port).expect("DH Fixture 浏览器启动失败"));
    wait_for_devtools(devtools_port, Duration::from_secs(15))
        .await
        .expect("DH Fixture DevTools 未就绪");
    println!(
        "DH Fixture browser running at http://127.0.0.1:{http_port} (real DevTools: http://127.0.0.1:{devtools_port})"
    );
    shutdown.notified().await;
    let _ = server.await;
}

const INDEX: &str = r#"<!doctype html>
<html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>DH Fixture / 旺商聊</title>
<style>*{box-sizing:border-box}body{font:14px system-ui;margin:0;color:#111;background:#f5f5f5}.shell{max-width:1180px;margin:auto;padding:28px}.head{display:flex;align-items:flex-start;justify-content:space-between}.tag{display:inline-block;padding:5px 8px;background:#111;color:#fff;border-radius:4px}h1{margin:10px 0 4px}.grid{display:grid;grid-template-columns:1fr 1fr;gap:12px;margin-top:18px}.panel{background:#fff;border:1px solid #ddd;padding:16px;border-radius:6px}.row{display:flex;gap:8px;flex-wrap:wrap;margin-top:10px}button,input{height:36px;padding:0 11px;border:1px solid #ccc;border-radius:5px;background:#fff}button:hover{border-color:#111}button.primary{background:#111;color:#fff;border-color:#111}input{min-width:240px;flex:1}pre{background:#fafafa;border:1px solid #e5e5e5;padding:12px;white-space:pre-wrap;max-height:480px;overflow:auto}.status{font-weight:650}@media(max-width:760px){.grid{grid-template-columns:1fr}.shell{padding:16px}}</style>
<body><main class="shell"><div class="head"><div><span class="tag">测试环境 · DH Fixture</span><h1>微型旺商聊</h1><p>固定 16 人测试群，不联网，不读取真实账号和旺商聊目录。</p></div><span id="summary" class="status">读取中</span></div>
<div class="grid"><section class="panel"><strong>消息与成员事件</strong><div class="row"><input id="text" value="@DH 测试消息"><button class="primary" onclick="send()">注入消息</button></div><div class="row"><button onclick="join()">成员 10017 入群</button><button onclick="leave()">成员 10017 离群</button><button onclick="duplicate()">重复消息</button><button onclick="outOfOrder()">乱序消息</button></div></section>
<section class="panel"><strong>故障注入</strong><div class="row"><button onclick="setFault('timeout')">下一次超时</button><button onclick="setFault('permission')">权限不足</button><button onclick="setFault('partial')">部分成员名单</button><button onclick="setFault('nim')">NIM 未就绪</button><button onclick="setFault('disconnect')">断开 DevTools</button><button class="primary" onclick="recover()">全部恢复</button></div></section></div>
<div class="row"><button onclick="reset()">重置完整场景</button><button onclick="load()">刷新状态</button></div><h2>当前状态与动作日志</h2><pre id="state">读取中...</pre></main>
<script>
const jsonHeaders={'content-type':'application/json'};
async function json(url,options){const response=await fetch(url,options);return response.json()}
async function post(url,body){return json(url,{method:'POST',headers:jsonHeaders,body:JSON.stringify(body||{})})}
const fixtureState={nimReady:true};
const ipcListeners=new Map();
window.__dhFixtureIpc={
  once(channel,callback){ipcListeners.set(channel,callback)},
  async send(name,payload){
    const value=await post('/fixture/ipc',payload);
    const callback=ipcListeners.get(payload.key);
    ipcListeners.delete(payload.key);
    if(callback)callback(null,value);
  }
};
const nimCore={
  options:{account:'fixture-nim-10001'},
  config:{account:'fixture-nim-10001'},
  get account(){return fixtureState.nimReady?'fixture-nim-10001':''},
  async getTeamMembers(input){
    try{
      const value=await json(`/fixture/nim/members?teamId=${encodeURIComponent(input.teamId||'')}`);
      const members=(value.members||[]).map(item=>({account:item.nimId,nickInTeam:item.cardName,type:item.type}));
      input.done(value.ok?null:new Error(value.errorMessage||'NIM members error'),members);
    }catch(error){input.done(error,[])}
  },
  async updateNickInTeam(input){
    try{
      const value=await post('/fixture/nim/rename',{teamId:input.teamId,account:input.account,nickInTeam:input.nickInTeam});
      input.done(value.ok?null:new Error(value.errorMessage||'NIM rename error'),value.member||null);
    }catch(error){input.done(error,null)}
  },
  async sendCustomMsg(input){
    try{
      const value=await post('/fixture/nim/send',{teamId:input.to,content:input.content});
      input.done(value.ok?null:new Error(value.errorMessage||'NIM send error'),value);
    }catch(error){input.done(error,null)}
  }
};
Object.defineProperty(window,'nim',{configurable:true,get(){return fixtureState.nimReady?nimCore:null}});
async function pumpFixture(){
  try{
    const current=await json('/fixture/state');
    fixtureState.nimReady=Boolean(current.faults&&current.faults.nimReady);
    if(fixtureState.nimReady){
      const events=await json('/fixture/browser-events');
      for(const event of events){
        const callback=nimCore.options[event.callback];
        if(typeof callback==='function')callback(...event.args);
      }
    }
  }catch{}
  setTimeout(pumpFixture,5);
}
async function load(){const data=await json('/fixture/state');state.textContent=JSON.stringify(data,null,2);summary.textContent=`${data.members.filter(x=>x.present).length} 人 · ${data.queuedMessages} 条待处理 · ${data.actions.length} 个动作`}
async function send(extra={}){await post('/fixture/events/message',{version:1,text:text.value,...extra});load()}
async function duplicate(){await send({sequence:80,serverMessageId:'fixture-duplicate'});await send({sequence:81,serverMessageId:'fixture-duplicate'})}
async function outOfOrder(){await send({sequence:120,serverMessageId:'fixture-later'});await send({sequence:110,serverMessageId:'fixture-earlier'})}
async function join(){await post('/fixture/events/member-joined',{version:1,userId:10017,nimId:'fixture-nim-10017',name:'新成员17',role:'member'});load()}
async function leave(){await post('/fixture/events/member-left',{version:1,userId:10017});load()}
async function reset(){await post('/fixture/reset');location.reload()}
async function setFault(kind){const f={devtoolsReady:true,nimReady:true,timeoutNext:false,permissionDenied:false,partialMembers:false};if(kind==='timeout')f.timeoutNext=true;if(kind==='permission')f.permissionDenied=true;if(kind==='partial')f.partialMembers=true;if(kind==='nim')f.nimReady=false;if(kind==='disconnect')f.devtoolsReady=false;await post('/fixture/faults',f);load()}
async function recover(){await post('/fixture/faults',{devtoolsReady:true,nimReady:true,timeoutNext:false,permissionDenied:false,partialMembers:false});load()}
window.__dhFixtureReady=true;
load();pumpFixture();setInterval(load,1500);
</script></body></html>"#;
