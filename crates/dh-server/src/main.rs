mod admin;
mod business_reads;
mod conversations;
mod login_view;
mod protocol_login;
mod rust_gateway;
mod vault;
#[cfg(test)]
use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
use axum::{
    extract::{ws::Message, Path, State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use dh_core::{
    database::{Database, DatabaseExecutor},
    gateway::{CdpClient, CdpGateway},
    paths::AppPaths,
    service::BusinessService,
};
use fs2::FileExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tower_http::services::{ServeDir, ServeFile};

struct Server {
    rust_mode: bool,
    protocol: Arc<tokio::sync::Mutex<Option<protocol_login::Session>>>,
    business: dh_core::headless::AppState,
    paths: AppPaths,
    downloads: Mutex<HashMap<String, (PathBuf, Instant)>>,
    service: BusinessService,
    limits: Mutex<admin::Limits>,
    trusted_proxies: Vec<std::net::IpAddr>,
    admin: Arc<admin::Admin>,
    revocation: tokio::sync::watch::Sender<u64>,
    sessions: Mutex<HashMap<String, Instant>>,
    activity: Mutex<HashMap<String, Instant>>,

    origin: String,
}

type ApiResult = Result<Json<Value>, StatusCode>;
fn session_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(';')
                .find_map(|p| p.trim().strip_prefix("dh_session="))
        })
}
fn authorized(s: &Server, headers: &HeaderMap) -> bool {
    if !s.admin.ready() {
        return false;
    }
    let token = session_token(headers).map(admin::digest);
    let mut sessions = s.sessions.lock().unwrap();
    sessions.retain(|_, expiry| *expiry > Instant::now());
    token.is_some_and(|t| {
        let mut activity = s.activity.lock().unwrap();
        activity.retain(|key, _| sessions.contains_key(key));
        sessions.contains_key(&t)
            && activity
                .get(&t)
                .is_none_or(|at| at.elapsed() < Duration::from_secs(1800))
    })
}
fn same_origin(s: &Server, headers: &HeaderMap) -> bool {
    headers.get("origin").and_then(|v| v.to_str().ok()) == Some(s.origin.as_str())
}
fn valid_public_origin(origin: &str) -> bool {
    url::Url::parse(origin).is_ok_and(|url| {
        (url.scheme() == "https"
            || (url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1"))))
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.origin().ascii_serialization() == origin
    })
}
#[derive(Deserialize)]
struct Login {
    username: String,
    password: String,
}
impl Drop for Login {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.password.zeroize();
    }
}
async fn login(
    State(s): State<Arc<Server>>,
    headers: HeaderMap,
    Json(input): Json<Login>,
) -> Response {
    if !same_origin(&s, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if input.password.len() > 512 || input.username.len() > 32 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Ok(permit) = s.admin.hashing.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let generation = *s.revocation.borrow();
    let admin = s.admin.clone();
    let valid = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        admin.check(&input.username, &input.password)
    })
    .await;
    let valid = match valid {
        Ok(Ok(value)) => value,
        Ok(Err(code)) => return code.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    s.admin.audit(if valid {
        "login_success"
    } else {
        "login_failure"
    });
    if !valid {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if *s.revocation.borrow() != generation {
        return StatusCode::CONFLICT.into_response();
    }
    let token = admin::token();
    let mut sessions = s.sessions.lock().unwrap();
    sessions.retain(|_, expiry| *expiry > Instant::now());
    if let Some(token) = session_token(&headers) {
        sessions.remove(&admin::digest(token));
    }
    if sessions.len() >= 32 {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    if *s.revocation.borrow() != generation {
        return StatusCode::CONFLICT.into_response();
    }
    sessions.insert(
        admin::digest(&token),
        Instant::now() + Duration::from_secs(8 * 3600),
    );
    s.activity
        .lock()
        .unwrap()
        .insert(admin::digest(&token), Instant::now());
    let secure = if s.origin.starts_with("https:") {
        "; Secure"
    } else {
        ""
    };
    let cookie =
        format!("dh_session={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=28800{secure}");
    ([("set-cookie", cookie)], Json(json!({"ok":true}))).into_response()
}
async fn logout(State(s): State<Arc<Server>>, headers: HeaderMap) -> Response {
    if !same_origin(&s, &headers) || !authorized(&s, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(token) = session_token(&headers) {
        s.sessions.lock().unwrap().remove(&admin::digest(token));
        s.revocation.send_modify(|n| *n += 1);
    }
    (
        [(
            "set-cookie",
            "dh_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        )],
        StatusCode::NO_CONTENT,
    )
        .into_response()
}
async fn activity(State(s): State<Arc<Server>>, headers: HeaderMap) -> StatusCode {
    if !same_origin(&s, &headers) || !authorized(&s, &headers) {
        return StatusCode::UNAUTHORIZED;
    }
    if let Some(token) = session_token(&headers) {
        s.activity
            .lock()
            .unwrap()
            .insert(admin::digest(token), Instant::now());
    }
    StatusCode::NO_CONTENT
}
async fn session(State(s): State<Arc<Server>>, headers: HeaderMap) -> StatusCode {
    if authorized(&s, &headers) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::UNAUTHORIZED
    }
}
async fn download(
    State(s): State<Arc<Server>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !authorized(&s, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let path = {
        let mut files = s.downloads.lock().unwrap();
        files.retain(|_, (_, expiry)| *expiry > Instant::now());
        files.get(&id).map(|(path, _)| path.clone())
    };
    let Some(path) = path else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tokio::fs::read(path).await {
        Ok(bytes) => (
            [
                ("content-type", "application/zip"),
                (
                    "content-disposition",
                    "attachment; filename=\"dh-support.zip\"",
                ),
                ("cache-control", "no-store"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
fn arg<T: serde::de::DeserializeOwned>(v: &Value, name: &str) -> Result<T, StatusCode> {
    serde_json::from_value(v.get(name).cloned().unwrap_or(Value::Null))
        .map_err(|_| StatusCode::BAD_REQUEST)
}
fn result<T: serde::Serialize>(r: dh_core::error::AppResult<T>) -> ApiResult {
    r.map(|v| Json(json!(v)))
        .map_err(|_| StatusCode::BAD_GATEWAY)
}
async fn command(
    State(s): State<Arc<Server>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(v): Json<Value>,
) -> Response {
    if !same_origin(&s, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !authorized(&s, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if name == "diagnose" {
        return Json(diagnostic(&s).await).into_response();
    }
    if dh_core::headless::WEB_COMMANDS.contains(&name.as_str()) {
        return match dh_core::headless::dispatch(&s.business, &name, v).await {
            Ok(value) => Json(value).into_response(),
            Err(error) => {
                let status = match error.code.as_str() {
                    "gateway_rate_limited" => StatusCode::TOO_MANY_REQUESTS,
                    "business_transport" => StatusCode::BAD_GATEWAY,
                    _ => StatusCode::BAD_REQUEST,
                };
                (status,Json(json!({"code":error.code,"message":dh_core::diagnostics::redact(&error.message)}))).into_response()
            }
        };
    }
    system_command(State(s), headers, Path(name), Json(v))
        .await
        .into_response()
}
async fn system_command(
    State(s): State<Arc<Server>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(v): Json<Value>,
) -> ApiResult {
    if !same_origin(&s, &headers) {
        return Err(StatusCode::FORBIDDEN);
    }
    if !authorized(&s, &headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let db = &s.service.database;
    match name.as_str() {
        "export_support_bundle" => {
            let database = db
                .status()
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let audits = db
                .list_support_audit(200)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let diagnostic = s.service.gateway.diagnose().await;
            let capabilities = s.service.gateway.capabilities();
            let stats = s.business.runtime_coordination.dispatch_stats.snapshot();
            let paths = s.paths.clone();
            let bundle = tokio::task::spawn_blocking(move || {
                dh_core::diagnostics::create_support_bundle(
                    &paths,
                    database,
                    audits,
                    diagnostic,
                    capabilities,
                    stats,
                )
            })
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let id = uuid::Uuid::new_v4().to_string();
            let mut files = s.downloads.lock().unwrap();
            files.retain(|_, (_, expiry)| *expiry > Instant::now());
            if files.len() >= 32 {
                files.clear();
            }
            files.insert(
                id.clone(),
                (
                    PathBuf::from(&bundle.path),
                    Instant::now() + Duration::from_secs(600),
                ),
            );
            Ok(Json(
                json!({"path":"dh-support.zip","downloadUrl":format!("/api/downloads/{id}"),"sha256":bundle.sha256,"includedFiles":bundle.included_files,"generatedAt":bundle.generated_at}),
            ))
        }
        "health" => Ok(Json(
            json!({"name":"DH BOT","version":"3.0.0-beta.1","runtimeMode":if s.rust_mode{"rust"}else{"cdp"},"buildChannel":"web","fixtureAvailable":false}),
        )),
        "get_runtime_work_snapshot" => Ok(Json(json!(s
            .business
            .runtime_coordination
            .tracker
            .snapshot()))),
        "acknowledge_runtime_work_failures" => {
            s.business
                .runtime_coordination
                .tracker
                .acknowledge_failures(&arg::<Vec<String>>(&v, "ids")?);
            Ok(Json(Value::Null))
        }
        "get_gateway_capabilities" => Ok(Json(json!(s.service.gateway.capabilities()))),
        "diagnose" => Ok(Json(diagnostic(&s).await)),
        "database_status" => result(db.status().await),
        _ => Err(StatusCode::NOT_IMPLEMENTED),
    }
}
async fn diagnostic(s: &Server) -> Value {
    let mut value = json!(s.service.gateway.diagnose().await);
    if let Some(session) = s.protocol.lock().await.as_ref() {
        if session.client.is_some() {
            value["accountName"] = json!(session.account_name);
        }
    }
    value
}
async fn events(
    State(s): State<Arc<Server>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !same_origin(&s, &headers) || !authorized(&s, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.on_upgrade(move |mut socket|async move {
        let mut revoked=s.revocation.subscribe();
        let mut interval=tokio::time::interval(Duration::from_secs(5));
        let mut events=s.business.events.0.subscribe();
        loop {
            tokio::select! {
                _=revoked.changed()=>{if !authorized(&s,&headers){let _=socket.send(Message::Close(None)).await;break}},
                event=events.recv()=>{
                    if !authorized(&s,&headers){break}
                    match event {
                        Ok(mut event)=>{
                            if event["event"] == "connection-status" { event["payload"] = diagnostic(&s).await; }
                            if socket.send(Message::Text(event.to_string().into())).await.is_err(){break}
                        },
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>{let _=socket.send(Message::Text(json!({"event":"resync-required","payload":{}}).to_string().into())).await;},
                        Err(_)=>break,
                    }
                },
                incoming=socket.recv()=> match incoming { Some(Ok(Message::Close(_)))|None|Some(Err(_))=>break,_=>{} },
                _=interval.tick()=>{
                    if !authorized(&s,&headers){let _=socket.send(Message::Close(None)).await;break}
                    let snapshot=json!({"event":"connection-status","payload":diagnostic(&s).await});
                    if socket.send(Message::Text(snapshot.to_string().into())).await.is_err(){break}
                }
            }
        }
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3
        || !matches!(
            args[1].as_str(),
            "init-admin" | "reset-admin" | "serve-cdp" | "serve-rust" | "probe-send"
        )
    {
        return Err("Usage: dh-server init-admin DATA_DIR | reset-admin DATA_DIR | serve-cdp DATA_DIR UI_DIR [PUBLIC_ORIGIN] | serve-rust DATA_DIR UI_DIR [PUBLIC_ORIGIN]. init-admin/reset-admin print a one-time code; initial protocol import uses DH_PROTOCOL_CONFIG.".into());
    }
    let dir = PathBuf::from(&args[2]);
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let admin = Arc::new(admin::Admin::open(&dir)?);
    if matches!(args[1].as_str(), "init-admin" | "reset-admin") {
        println!("{}", admin.issue(args[1] == "reset-admin")?);
        return Ok(());
    }
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("server.lock"))?;
    lock.try_lock_exclusive()?;
    if args[1] == "probe-send" {
        if !admin.ready() {return Err("Activate the administrator before probing".into());}
        let group_name=args.get(3).ok_or("probe-send requires exact group name")?;
        let text=args.get(4).ok_or("probe-send requires test text")?;
        if !text.contains("DH BOT 测试") || text.len()>1000 {return Err("Probe text requires DH BOT 测试 label and max 1000 bytes".into());}
        let paths=AppPaths {root:dir.clone(),v3:dir.clone(),database:dir.join("dh.db"),secrets:dir.join("secrets.dat"),logs:dir.join("logs"),legacy_backups:dir.join("legacy-backups"),runtime_mode_file:dir.join("runtime-mode")};
        let vault=vault::Vault::open(&dir)?;
        let config=serde_json::from_slice(&vault.read("deployment.sealed")?.ok_or("Deployment missing")?)?;
        let mut session=protocol_login::Session::new(config).map_err(|_|"Invalid deployment")?;
        session.attach_vault(vault)?;
        session.maintain_connection().await;
        if session.nim.is_none(){return Err("NIM connection not ready".into());}
        let protocol=Arc::new(tokio::sync::Mutex::new(Some(session)));
        let db=DatabaseExecutor::start(Database::open(&paths)?)?;
        let gateway=rust_gateway::RustGateway::new(protocol.clone(),db.clone()).await;
        use dh_core::gateway::{GroupGateway,RuntimeGateway};
        let groups=gateway.list_groups().await?;
        let matches=groups.iter().filter(|g|&g.name==group_name).collect::<Vec<_>>();
        if matches.len()!=1 {return Err("Exact group name missing or ambiguous".into());}
        let (_,account)=gateway.session_identity().await?;
        let result=gateway.send_text(matches[0].group_id,text).await;
        let outcome=match &result {Ok(r) if r.status=="unknown"=>"unknown",Ok(_)=>"sent",Err(_)=>"rejected"};
        db.record_audit(dh_core::models::AuditEvent {id:0,account_id:account,group_id:matches[0].group_id,user_id:0,actor:"local-operator".into(),event:"probe_send".into(),level:"info".into(),details:serde_json::json!({"status":outcome,"label":"DH BOT 测试"}).to_string(),created_at:chrono::Utc::now()}).await?;
        println!("probe_send status={outcome}; provider acceptance is not peer receipt");
        if let Some(session)=protocol.lock().await.as_mut(){if let Some(nim)=session.nim.take(){let _=nim.close().await;}}
        result?;
        return Ok(());
    }
    let ui = PathBuf::from(args.get(3).ok_or("UI_DIR required")?);
    if !ui.join("index.html").is_file() {
        return Err("UI index.html missing".into());
    }
    let bind: std::net::SocketAddr = std::env::var("DH_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8787".into())
        .parse()?;
    if !bind.ip().is_loopback() || bind.port() == 0 {
        return Err("DH_BIND must use a loopback address and a nonzero port".into());
    }
    let origin = args
        .get(4)
        .cloned()
        .unwrap_or_else(|| format!("http://{bind}"));
    if !valid_public_origin(&origin) {
        return Err("Public origin must be an HTTPS origin without trailing slash".into());
    }
    let paths = AppPaths {
        root: dir.clone(),
        v3: dir.clone(),
        database: dir.join("dh.db"),
        secrets: dir.join("secrets.dat"),
        logs: dir.join("logs"),
        legacy_backups: dir.join("legacy-backups"),
        runtime_mode_file: dir.join("runtime-mode"),
    };
    paths.prepare()?;
    let rust_mode = args[1] == "serve-rust";
    let protocol = Arc::new(tokio::sync::Mutex::new(if rust_mode {
        let vault = vault::Vault::open(&dir)?;
        let bytes = if let Some(path) = std::env::var_os("DH_PROTOCOL_CONFIG") {
            let path = PathBuf::from(path);
            if path == std::path::Path::new("/dev/stdin") {
                use std::io::Read;
                let mut bytes = zeroize::Zeroizing::new(Vec::new());
                std::io::stdin().take(65537).read_to_end(&mut bytes)?;
                if bytes.len() > 65536 {
                    return Err("Protocol configuration too large".into());
                }
                bytes
            } else {
                vault::read_private(&path)?
            }
        } else {
            vault.read("deployment.sealed")?.unwrap_or_default()
        };
        if bytes.is_empty() {
            None
        } else {
            let config =
                serde_json::from_slice(&bytes).map_err(|_| "Invalid protocol configuration")?;
            let mut session = protocol_login::Session::new(config)
                .map_err(|_| "Invalid protocol configuration")?;
            vault.save("deployment.sealed", &bytes)?;
            session.attach_vault(vault)?;
            Some(session)
        }
    } else {
        None
    }));
    let database = DatabaseExecutor::start(Database::open(&paths)?)?;
    let service = BusinessService {
        database: database.clone(),
        gateway: if rust_mode {
            Arc::new(rust_gateway::RustGateway::new(protocol.clone(), database.clone()).await)
        } else {
            Arc::new(CdpGateway::new(CdpClient::new("http://127.0.0.1:9222")?))
        },
    };
    let business = dh_core::headless::AppState::new(
        service.database.clone(),
        dh_core::secrets::SecretStore::new(paths.secrets.clone()),
        service.gateway.clone(),
        dh_core::diagnostics::Logger::new(paths.logs.clone()),
    )?;
    let state = Arc::new(Server {
        rust_mode,
        protocol,
        business,
        paths: paths.clone(),
        downloads: Mutex::new(HashMap::new()),
        service,
        limits: Mutex::new(admin::Limits::default()),
        trusted_proxies: std::env::var("DH_TRUSTED_PROXIES")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::parse)
            .collect::<Result<_, _>>()?,
        admin,
        revocation: tokio::sync::watch::channel(0).0,
        sessions: Mutex::new(HashMap::new()),
        activity: Mutex::new(HashMap::new()),

        origin,
    });
    let receive_state = Arc::downgrade(&state);
    let receiver = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(200));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let Some(state) = receive_state.upgrade() else {
                break;
            };
            if !state.admin.ready() {
                continue;
            }
            if let Err(error) = conversations::receive(&state).await {
                eprintln!("conversation_receiver: {}", error.code);
            }
            if let Ok(mut guard) = state.protocol.try_lock() {
                if let Some(session) = guard.as_mut() {
                    session.maintain_connection().await;
                }
            };
        }
    });
    let shutdown = Arc::new(dh_core::shutdown::ShutdownSignal::default());
    let worker_state = state.clone();
    let worker_shutdown = shutdown.clone();
    let workers = tokio::spawn(async move {
        let state = worker_state;
        let shutdown = worker_shutdown;
        while !state.admin.ready() {
            tokio::select! { _=shutdown.cancelled()=>return, _=tokio::time::sleep(Duration::from_millis(500))=>{} }
        }
        let workers = dh_core::runtime::BackendRuntime::new_with_ai_pool(
            state.business.database_executor.clone(),
            state.business.gateway.clone(),
            state.business.secrets.clone(),
            shutdown.clone(),
            state.business.logger.clone(),
            state.business.ai_pool.clone(),
            state.business.prediction_source.clone(),
        )
        .with_coordination(state.business.runtime_coordination.clone())
        .spawn_with_events(Arc::new(state.business.events.clone()));
        for mut worker in workers {
            tokio::select! { _=&mut worker=>{}, _=shutdown.cancelled()=>{if tokio::time::timeout(Duration::from_secs(5), &mut worker).await.is_err(){worker.abort();}} }
        }
    });
    let router = Router::new()
        .route("/api/downloads/{id}", get(download))
        .route("/api/setup/status", get(admin::status))
        .route("/api/setup/activate", post(admin::activate))
        .route("/api/setup/migrate", post(admin::migrate))
        .route("/api/admin/reset", post(admin::reset))
        .route("/api/admin/credentials", post(admin::credentials))
        .route("/api/session", get(session))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/session/activity", post(activity))
        .route("/api/commands/{name}", post(command))
        .route("/api/events", get(events))
        .route("/api/login-view", get(login_view::upgrade))
        .route("/api/wang/status", post(protocol_login::status))
        .route("/api/wang/forget-login", post(protocol_login::forget_login))
        .route("/api/wang/login", post(protocol_login::login))
        .route("/api/wang/start-sms", post(protocol_login::start_sms))
        .route("/api/wang/verify-sms", post(protocol_login::verify_sms))
        .route("/api/wang/resend-sms", post(protocol_login::resend_sms))
        .route("/api/wang/refresh", post(protocol_login::refresh))
        .route("/api/wang/logout", post(protocol_login::logout))
        .route("/api/conversations/list", post(conversations::list))
        .route("/api/conversations/history", post(conversations::history))
        .route("/api/conversations/policy", post(conversations::policy))
        .fallback_service(
            ServeDir::new(&ui).not_found_service(ServeFile::new(ui.join("index.html"))),
        )
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            admin::guard,
        ))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    println!(
        "DH Web: {bind} ({}; business workers require activation)",
        if rust_mode {
            "Rust authentication"
        } else {
            "CDP gateway"
        }
    );
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        #[cfg(unix)]
        {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        shutdown.cancel();
    })
    .await?;
    receiver.abort();
    let mut workers = workers;
    if tokio::time::timeout(Duration::from_secs(10), &mut workers)
        .await
        .is_err()
    {
        workers.abort();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    #[test]
    fn public_origin_rejects_paths_and_credentials() {
        for origin in [
            "https://example.test",
            "https://example.test:9443",
            "http://127.0.0.1:8787",
        ] {
            assert!(valid_public_origin(origin));
        }
        for origin in [
            "https://example.test/path",
            "https://example.test?x=1",
            "https://user@example.test",
            "https://",
            "http://example.test",
        ] {
            assert!(!valid_public_origin(origin));
        }
    }
    #[tokio::test]
    async fn browser_sessions_do_not_evict_or_log_each_other_out() {
        let (_dir, s) = setup();
        let router = Router::new()
            .route("/api/login", post(login))
            .route("/api/logout", post(logout))
            .with_state(s.clone());
        let request = |path: &str, cookie: &str, body: &str| {
            Request::builder()
                .method("POST")
                .uri(path)
                .header("origin", "http://127.0.0.1:8787")
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(body.to_owned()))
                .unwrap()
        };
        let mut cookies = Vec::new();
        for _ in 0..2 {
            let response = router
                .clone()
                .oneshot(request(
                    "/api/login",
                    "",
                    r#"{"username":"admin","password":"synthetic-test-password"}"#,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            cookies.push(
                response.headers()["set-cookie"]
                    .to_str()
                    .unwrap()
                    .to_owned(),
            );
        }
        let headers = |cookie: &str| {
            let mut h = HeaderMap::new();
            h.insert("cookie", cookie.parse().unwrap());
            h
        };
        assert!(authorized(&s, &headers(&cookies[0])));
        assert!(authorized(&s, &headers(&cookies[1])));
        assert_eq!(
            router
                .oneshot(request("/api/logout", &cookies[0], "{}"))
                .await
                .unwrap()
                .status(),
            204
        );
        assert!(!authorized(&s, &headers(&cookies[0])));
        assert!(authorized(&s, &headers(&cookies[1])));
    }
    #[tokio::test]
    async fn downloads_require_authentication_and_expire_without_path_input() {
        let (_dir, s) = setup();
        let archive = s.paths.root.join("synthetic.zip");
        std::fs::write(&archive, b"synthetic archive").unwrap();
        s.downloads.lock().unwrap().insert(
            "opaque-id".into(),
            (archive, Instant::now() + Duration::from_secs(60)),
        );
        s.sessions.lock().unwrap().insert(
            admin::digest("synthetic-session"),
            Instant::now() + Duration::from_secs(60),
        );
        let router = Router::new()
            .route("/api/downloads/{id}", get(download))
            .with_state(s.clone());
        let request = |id: &str, cookie: &str| {
            Request::builder()
                .uri(format!("/api/downloads/{id}"))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(
            router
                .clone()
                .oneshot(request("opaque-id", ""))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let response = router
            .clone()
            .oneshot(request("opaque-id", "dh_session=synthetic-session"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap()
                .as_ref(),
            b"synthetic archive"
        );
        s.downloads.lock().unwrap().get_mut("opaque-id").unwrap().1 =
            Instant::now() - Duration::from_secs(1);
        assert_eq!(
            router
                .oneshot(request("opaque-id", "dh_session=synthetic-session"))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    pub(super) fn setup() -> (tempfile::TempDir, Arc<Server>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let paths = AppPaths {
            root: root.clone(),
            v3: root.clone(),
            database: root.join("dh.db"),
            secrets: root.join("secrets.dat"),
            logs: root.join("logs"),
            legacy_backups: root.join("backups"),
            runtime_mode_file: root.join("mode"),
        };
        paths.prepare().unwrap();
        let hash = Argon2::default()
            .hash_password(
                b"synthetic-test-password",
                &SaltString::generate(&mut rand_core::OsRng),
            )
            .unwrap()
            .to_string();
        let service = BusinessService {
            database: DatabaseExecutor::start(Database::open(&paths).unwrap()).unwrap(),
            gateway: Arc::new(CdpGateway::new(
                CdpClient::new("http://127.0.0.1:9222").unwrap(),
            )),
        };
        let business = dh_core::headless::AppState::new(
            service.database.clone(),
            dh_core::secrets::SecretStore::new(paths.secrets.clone()),
            service.gateway.clone(),
            dh_core::diagnostics::Logger::new(paths.logs.clone()),
        )
        .unwrap();
        let s = Arc::new(Server {
            paths: paths.clone(),
            downloads: Mutex::new(HashMap::new()),
            rust_mode: false,
            protocol: Arc::new(tokio::sync::Mutex::new(None)),
            service,
            business,
            limits: Mutex::new(admin::Limits::default()),
            trusted_proxies: Vec::new(),
            admin: {
                let admin = admin::Admin::open(&root).unwrap();
                admin.seed_test(&hash);
                Arc::new(admin)
            },
            revocation: tokio::sync::watch::channel(0).0,
            sessions: Mutex::new(HashMap::new()),
            activity: Mutex::new(HashMap::new()),

            origin: "http://127.0.0.1:8787".into(),
        });
        (dir, s)
    }
    #[tokio::test]
    async fn protocol_routes_require_session_origin_and_configuration() {
        let (_dir, s) = setup();
        s.sessions.lock().unwrap().insert(
            admin::digest("synthetic-session"),
            Instant::now() + Duration::from_secs(60),
        );
        let router = Router::new()
            .route("/api/wang/status", post(protocol_login::status))
            .route("/api/wang/login", post(protocol_login::login))
            .route("/api/wang/start-sms", post(protocol_login::start_sms))
            .route("/api/wang/verify-sms", post(protocol_login::verify_sms))
            .route("/api/wang/resend-sms", post(protocol_login::resend_sms))
            .route("/api/wang/refresh", post(protocol_login::refresh))
            .route("/api/wang/logout", post(protocol_login::logout))
            .route("/api/conversations/list", post(conversations::list))
            .route("/api/conversations/history", post(conversations::history))
            .route("/api/conversations/policy", post(conversations::policy))
            .with_state(s.clone());
        for action in [
            "status",
            "login",
            "refresh",
            "logout",
            "verify-sms",
            "resend-sms",
        ] {
            for (cookie, origin, expected) in [
                ("", "http://127.0.0.1:8787", StatusCode::UNAUTHORIZED),
                (
                    "dh_session=synthetic-session",
                    "https://other.invalid",
                    StatusCode::FORBIDDEN,
                ),
                (
                    "dh_session=synthetic-session",
                    "http://127.0.0.1:8787",
                    if action == "status" {
                        StatusCode::OK
                    } else {
                        StatusCode::SERVICE_UNAVAILABLE
                    },
                ),
            ] {
                let body = if action.ends_with("-sms") {
                    r#"{"challengeId":"synthetic","verificationCode":"123456","validateStr":"synthetic"}"#
                } else {
                    r#"{"account":"synthetic","password":"synthetic","validateStr":"synthetic"}"#
                };
                let response = router
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri(format!("/api/wang/{action}"))
                            .header("origin", origin)
                            .header("cookie", cookie)
                            .header("content-type", "application/json")
                            .body(Body::from(body))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), expected, "{action}");
            }
        }
        let _guard = s.protocol.lock().await;
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/wang/status")
                    .header("origin", "http://127.0.0.1:8787")
                    .header("cookie", "dh_session=synthetic-session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }
    #[tokio::test]
    async fn authenticated_reads_origin_checks_logout_and_expiry() {
        let (_dir, s) = setup();
        let router = Router::new()
            .route("/api/login", post(login))
            .route("/api/logout", post(logout))
            .route("/api/session", get(session))
            .route("/api/commands/{name}", post(command))
            .with_state(s.clone());
        let request = |path: &str, cookie: &str, origin: &str, body: &str| {
            Request::builder()
                .method("POST")
                .uri(path)
                .header("origin", origin)
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let origin = "http://127.0.0.1:8787";
        let denied = router
            .clone()
            .oneshot(request(
                "/api/commands/list_cached_groups",
                "",
                origin,
                "{}",
            ))
            .await
            .unwrap();
        assert_eq!(denied.status(), 401);
        let denied = router
            .clone()
            .oneshot(request(
                "/api/login",
                "",
                "https://other.invalid",
                r#"{"username":"admin","password":"synthetic-test-password"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(denied.status(), 403);
        let wrong = router
            .clone()
            .oneshot(request(
                "/api/login",
                "",
                origin,
                r#"{"username":"admin","password":"wrong"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(wrong.status(), 401);
        let response = router
            .clone()
            .oneshot(request(
                "/api/login",
                "",
                origin,
                r#"{"username":"admin","password":"synthetic-test-password"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .to_string();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        let ok = router
            .clone()
            .oneshot(request(
                "/api/commands/list_cached_groups",
                &cookie,
                origin,
                "{}",
            ))
            .await
            .unwrap();
        assert_eq!(ok.status(), 200);
        for endpoint in ["query_messages", "query_audit"] {
            let body = r#"{"query":{"accountId":"synthetic-account","limit":10}}"#;
            let response = router
                .clone()
                .oneshot(request(
                    &format!("/api/commands/{endpoint}"),
                    &cookie,
                    origin,
                    body,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            let page: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(page["items"].is_array());
            assert!(page.get("nextCursor").is_some());
        }
        let unknown = router
            .clone()
            .oneshot(request(
                "/api/commands/not_registered",
                &cookie,
                origin,
                "{}",
            ))
            .await
            .unwrap();
        assert_eq!(unknown.status(), 501);
        let invalid = router
            .clone()
            .oneshot(request("/api/commands/send_text", &cookie, origin, "{}"))
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(invalid.into_body(), 4096)
            .await
            .unwrap();
        let error: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(error["code"], "invalid_argument");
        let denied = router
            .clone()
            .oneshot(request(
                "/api/commands/list_cached_groups",
                &cookie,
                "https://other.invalid",
                "{}",
            ))
            .await
            .unwrap();
        assert_eq!(denied.status(), 403);
        let logout = router
            .clone()
            .oneshot(request("/api/logout", &cookie, origin, "{}"))
            .await
            .unwrap();
        assert_eq!(logout.status(), 204);
        let denied = router
            .oneshot(request(
                "/api/commands/list_cached_groups",
                &cookie,
                origin,
                "{}",
            ))
            .await
            .unwrap();
        assert_eq!(denied.status(), 401);
        s.sessions
            .lock()
            .unwrap()
            .insert("expired".into(), Instant::now() - Duration::from_secs(1));
        let mut headers = HeaderMap::new();
        headers.insert("cookie", "dh_session=expired".parse().unwrap());
        assert!(!authorized(&s, &headers));
    }
}
