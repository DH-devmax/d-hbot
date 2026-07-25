use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as SyncMutex, RwLock as SyncRwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async, tungstenite::Message as WsMessage, MaybeTlsStream, WebSocketStream,
};

#[cfg(feature = "fixture")]
use crate::calibration::{
    CalibrationMetadata, DeveloperCalibrationExport, DeveloperCalibrationRecorder,
    DeveloperCalibrationStatus,
};
use crate::contracts::{runtime_capabilities, unverified_production_capabilities};
use crate::error::{
    AppError, AppResult, GatewayErrorKind, GatewayErrorLayer, GatewayErrorMetadata,
};
use crate::models::{
    Group, GroupAnnouncement, Member, MemberRef, MemberRoster, MemberSourceError, Message,
    RosterCompleteness,
};

const GROUP_LIST_ROUTE: &str = "/v1/group/get-group-list";
const GROUP_MEMBERS_ROUTE: &str = "/v1/group/get-group-members";
const GROUP_MUTE_ROUTE: &str = "/v1/group/set-group-mute";
const MEMBER_MUTE_ROUTE: &str = "/v1/group/set-member-mute";
const MEMBER_UNMUTE_ROUTE: &str = "/v1/group/member-mute-cancel";
const MEMBER_RENAME_ROUTE: &str = "/v1/group/set-member-nickname";
const MEMBER_REMOVE_ROUTE: &str = "/v1/group/remove-group-member";
const MESSAGE_RECALL_ROUTE: &str = "/v1/group/message-rollback";
const GROUP_NOTICE_LIST_ROUTE: &str = "/v1/group/notice-list";
const GROUP_NOTICE_ADD_ROUTE: &str = "/v1/group/add-notice";
const GROUP_NOTICE_UPDATE_ROUTE: &str = "/v1/group/notice-opt";
const MAX_GATEWAY_BATCH: usize = 100;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ConnectionStatus {
    Unavailable,
    OtherService,
    DevToolsReady,
    NimNotReady,
    Ready,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSnapshot {
    pub status: ConnectionStatus,
    pub devtools_url: String,
    pub page_title: String,
    pub page_url: String,
    pub nim_account: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum GatewayEvent {
    Message { message: Message },
    MemberJoined { group_id: i64, member: Member },
    MemberLeft { group_id: i64, member: Member },
    MemberUpdated { group_id: i64, member: Member },
    ConnectionChanged { diagnostic: DiagnosticSnapshot },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GatewayRecordKind {
    Message,
    TeamMemberJoined,
    TeamMemberLeft,
    TeamMemberUpdated,
    ConnectionChanged,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GatewayRecord {
    pub session: String,
    pub sequence: u64,
    pub kind: GatewayRecordKind,
    pub source: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GatewayBatch {
    pub session: String,
    pub records: Vec<GatewayRecord>,
    pub remaining: usize,
    pub dropped: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GatewayReceipt {
    #[serde(default)]
    pub route: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub transport_code: Option<i64>,
    #[serde(default)]
    pub transport_errno: Option<i64>,
    #[serde(default)]
    pub business_code: Option<i64>,
    #[serde(default)]
    pub business_errno: Option<i64>,
    #[serde(default)]
    pub business_message: String,
    #[serde(default)]
    pub request_id: String,
    #[serde(default)]
    pub message_id: String,
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub acknowledged_through: u64,
    #[serde(default)]
    pub acknowledged: usize,
    #[serde(default)]
    pub remaining: usize,
    #[serde(default)]
    pub dropped: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
}

impl GatewayReceipt {
    pub fn succeeded(route: &str) -> Self {
        Self {
            route: route.into(),
            status: "succeeded".into(),
            transport_code: None,
            transport_errno: None,
            business_code: Some(0),
            business_errno: Some(0),
            business_message: "OK".into(),
            request_id: String::new(),
            message_id: String::new(),
            session: String::new(),
            acknowledged_through: 0,
            acknowledged: 0,
            remaining: 0,
            dropped: 0,
            verification: Some("not-applicable".into()),
        }
    }

    pub fn failed(route: &str, error: &AppError) -> Self {
        let gateway = error.gateway.as_deref();
        Self {
            route: gateway
                .map(|metadata| metadata.route.clone())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| route.to_string()),
            status: "failed".into(),
            transport_code: gateway.and_then(|metadata| metadata.transport_code),
            transport_errno: gateway.and_then(|metadata| metadata.transport_errno),
            business_code: gateway.and_then(|metadata| metadata.business_code),
            business_errno: gateway.and_then(|metadata| metadata.business_errno),
            business_message: error.message.clone(),
            request_id: String::new(),
            message_id: String::new(),
            session: String::new(),
            acknowledged_through: 0,
            acknowledged: 0,
            remaining: 0,
            dropped: 0,
            verification: Some("not-applicable".into()),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevToolsPage {
    title: String,
    url: String,
    #[serde(rename = "type", default)]
    page_type: String,
    web_socket_debugger_url: Option<String>,
}

fn is_wangshangliao_page(page: &DevToolsPage) -> bool {
    let identity = format!("{} {}", page.title, page.url).to_lowercase();
    identity.contains("旺商聊") || identity.contains("wangshangliao")
}

#[derive(Clone)]
pub struct CdpClient {
    base_url: String,
    http: reqwest::Client,
    next_id: Arc<AtomicU64>,
    request_gate: Arc<GatewayRequestGate>,
    page_cache: Arc<RwLock<Option<(Instant, DevToolsPage)>>>,
    diagnostic_cache: Arc<RwLock<Option<(Instant, DiagnosticSnapshot)>>>,
    session: Arc<tokio::sync::Mutex<Option<CdpSession>>>,
}

struct CdpSession {
    websocket_url: String,
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

const GATE_MIN_INTERVAL: Duration = Duration::from_millis(150);
const GATE_MAX_WAITERS: usize = 64;
const GATE_MAX_WAIT: Duration = Duration::from_secs(10);

struct GatewayRequestGate {
    lock: Arc<tokio::sync::Mutex<()>>,
    state: SyncMutex<GatewayRequestGateState>,
}

#[derive(Debug)]
struct GatewayRequestGateState {
    waiters: usize,
    last_started: Option<Instant>,
}

impl GatewayRequestGate {
    fn new() -> Self {
        Self {
            lock: Arc::new(tokio::sync::Mutex::new(())),
            state: SyncMutex::new(GatewayRequestGateState {
                waiters: 0,
                last_started: None,
            }),
        }
    }

    async fn acquire(&self, route: &str) -> AppResult<tokio::sync::OwnedMutexGuard<()>> {
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| AppError::new("gateway_busy", "网关请求闸门状态不可用"))?;
            if state.waiters >= GATE_MAX_WAITERS {
                return Err(
                    AppError::new("gateway_busy", format!("网关请求排队已满：{route}")).retryable(),
                );
            }
            state.waiters += 1;
        }

        let guard = match timeout(GATE_MAX_WAIT, self.lock.clone().lock_owned()).await {
            Ok(guard) => guard,
            Err(_) => {
                if let Ok(mut state) = self.state.lock() {
                    state.waiters = state.waiters.saturating_sub(1);
                }
                return Err(
                    AppError::new("gateway_busy", format!("网关请求排队超时：{route}")).retryable(),
                );
            }
        };

        let delay = self
            .state
            .lock()
            .ok()
            .and_then(|state| state.last_started)
            .and_then(|started| GATE_MIN_INTERVAL.checked_sub(started.elapsed()));
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        if let Ok(mut state) = self.state.lock() {
            state.waiters = state.waiters.saturating_sub(1);
            state.last_started = Some(Instant::now());
        }
        Ok(guard)
    }
}

impl CdpClient {
    pub fn new(base_url: impl Into<String>) -> AppResult<Self> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let parsed = url::Url::parse(&base_url).map_err(|error| {
            AppError::new("devtools_url", format!("DevTools 地址无效：{error}"))
        })?;
        if parsed.host_str().map(is_loopback_host) != Some(true) {
            return Err(AppError::new(
                "devtools_url",
                "DevTools 地址必须使用本机回环地址",
            ));
        }
        Ok(Self {
            base_url,
            http: reqwest::Client::builder()
                // CDP is loopback-only. System proxies (for example Clash in
                // global mode) must never receive or intercept these requests.
                .no_proxy()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(5))
                .build()
                .map_err(|error| AppError::new("devtools_client", error.to_string()))?,
            next_id: Arc::new(AtomicU64::new(1)),
            request_gate: Arc::new(GatewayRequestGate::new()),
            page_cache: Arc::new(RwLock::new(None)),
            diagnostic_cache: Arc::new(RwLock::new(None)),
            session: Arc::new(tokio::sync::Mutex::new(None)),
        })
    }

    async fn pages(&self) -> AppResult<Vec<DevToolsPage>> {
        let mut errors = Vec::new();
        for path in ["/json/list", "/json"] {
            match self
                .http
                .get(format!("{}{path}", self.base_url))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    return response.json().await.map_err(|error| {
                        AppError::new(
                            "devtools_response",
                            format!("解析 DevTools 页面失败：{error}"),
                        )
                    });
                }
                Ok(response) => {
                    errors.push(format!("{path} 返回 HTTP {}", response.status()));
                }
                Err(error) => {
                    errors.push(format!("{path} 连接失败：{error}"));
                }
            }
        }
        Err(AppError::new(
            "devtools_unavailable",
            format!(
                "9222 端口尚未提供可用的 DevTools HTTP 服务。旺商聊可能已带调试参数启动，但端口仍未接受连接；DH BOT 会继续重试。详情：{}",
                errors.join("；")
            ),
        )
        .retryable())
    }

    async fn page(&self) -> AppResult<DevToolsPage> {
        if let Some((checked_at, page)) = self.page_cache.read().await.clone() {
            if checked_at.elapsed() < Duration::from_secs(1) {
                return Ok(page);
            }
        }
        let pages = self.pages().await?;
        let candidates = pages
            .into_iter()
            .filter(|page| page.page_type == "page")
            .filter(|page| page.web_socket_debugger_url.is_some())
            .collect::<Vec<_>>();
        if let Some(page) = candidates
            .iter()
            .filter(|page| is_wangshangliao_page(page))
            .max_by_key(|page| {
                let identity = format!("{} {}", page.title, page.url).to_lowercase();
                usize::from(identity.contains("旺商聊")) * 2
                    + usize::from(identity.contains("wangshangliao"))
            })
        {
            let page = page.clone();
            *self.page_cache.write().await = Some((Instant::now(), page.clone()));
            return Ok(page);
        }
        if candidates.is_empty() {
            Err(AppError::new("devtools_page", "DevTools 中没有可用的页面").retryable())
        } else {
            Err(AppError::new(
                "devtools_other_service",
                "9222 端口已被其他程序的 DevTools 占用",
            ))
        }
    }

    pub async fn evaluate(&self, expression: &str) -> AppResult<Value> {
        self.evaluate_persistent(expression).await
    }

    async fn evaluate_persistent(&self, expression: &str) -> AppResult<Value> {
        let _gate = self.request_gate.acquire("Runtime.evaluate").await?;
        let page = self.page().await?;
        let websocket = page
            .web_socket_debugger_url
            .ok_or_else(|| AppError::new("devtools_page", "DevTools 页面缺少 WebSocket 地址"))?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut session = self.session.lock().await;
        if session
            .as_ref()
            .is_none_or(|current| current.websocket_url != websocket)
        {
            let (stream, _) = timeout(Duration::from_secs(8), connect_async(websocket.as_str()))
                .await
                .map_err(|_| {
                    AppError::new("cdp_timeout", "连接 DevTools WebSocket 超时").retryable()
                })?
                .map_err(|error| {
                    AppError::new(
                        "cdp_connect",
                        format!("连接 DevTools WebSocket 失败：{error}"),
                    )
                    .retryable()
                })?;
            *session = Some(CdpSession {
                websocket_url: websocket,
                stream,
            });
        }
        let result = evaluate_cdp_session(
            session.as_mut().expect("CDP session initialized"),
            id,
            expression,
        )
        .await;
        if result.is_err() {
            *session = None;
            *self.page_cache.write().await = None;
            *self.diagnostic_cache.write().await = None;
        }
        result
    }

    pub async fn diagnose(&self) -> DiagnosticSnapshot {
        if let Some((checked_at, snapshot)) = self.diagnostic_cache.read().await.clone() {
            if checked_at.elapsed() < Duration::from_secs(2) {
                return snapshot;
            }
        }
        let snapshot = self.diagnose_uncached().await;
        *self.diagnostic_cache.write().await = Some((Instant::now(), snapshot.clone()));
        snapshot
    }

    async fn diagnose_uncached(&self) -> DiagnosticSnapshot {
        let mut snapshot = DiagnosticSnapshot {
            status: ConnectionStatus::Unavailable,
            devtools_url: self.base_url.clone(),
            page_title: String::new(),
            page_url: String::new(),
            nim_account: String::new(),
            detail: String::new(),
        };
        let page = match self.page().await {
            Ok(page) => page,
            Err(error) => {
                if matches!(
                    error.code.as_str(),
                    "devtools_other_service"
                        | "devtools_http"
                        | "devtools_response"
                        | "devtools_page"
                ) {
                    snapshot.status = ConnectionStatus::OtherService;
                }
                snapshot.detail = error.message;
                return snapshot;
            }
        };
        snapshot.status = ConnectionStatus::DevToolsReady;
        snapshot.page_title = page.title;
        snapshot.page_url = page.url;
        match self.evaluate(r#"({nimAccount:String((window.nim&&(window.nim.account||window.nim.options&&window.nim.options.account||window.nim.config&&window.nim.config.account))||"")})"#).await {
            Ok(value) => {
                snapshot.nim_account = value.get("nimAccount").and_then(Value::as_str).unwrap_or_default().to_string();
                if snapshot.nim_account.is_empty() {
                    snapshot.status = ConnectionStatus::NimNotReady;
                    snapshot.detail = "需要您登录账号，等待登录".into();
                } else {
                    snapshot.status = ConnectionStatus::Ready;
                    snapshot.detail = "旺商聊协议会话已就绪".into();
                }
            }
            Err(error) => {
                snapshot.status = ConnectionStatus::NimNotReady;
                snapshot.detail = error.message;
            }
        }
        snapshot
    }
}

async fn evaluate_cdp_session(
    session: &mut CdpSession,
    id: u64,
    expression: &str,
) -> AppResult<Value> {
    let request = json!({
        "id": id,
        "method": "Runtime.evaluate",
        "params": { "expression": expression, "awaitPromise": true, "returnByValue": true }
    });
    session
        .stream
        .send(WsMessage::Text(request.to_string().into()))
        .await
        .map_err(|error| AppError::new("cdp_write", format!("CDP 写入失败：{error}")))?;
    timeout(Duration::from_secs(25), async {
        while let Some(item) = session.stream.next().await {
            let item = item.map_err(|error| AppError::new("cdp_read", error.to_string()))?;
            if !item.is_text() {
                continue;
            }
            let value: Value = serde_json::from_str(item.to_text().unwrap_or_default())
                .map_err(|error| AppError::new("cdp_response", error.to_string()))?;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(exception) = value
                .pointer("/result/exceptionDetails/text")
                .and_then(Value::as_str)
            {
                return Err(AppError::new("cdp_exception", exception.to_string()));
            }
            return Ok(value
                .pointer("/result/result/value")
                .cloned()
                .unwrap_or(Value::Null));
        }
        Err(AppError::new("cdp_closed", "DevTools WebSocket 已关闭").retryable())
    })
    .await
    .map_err(|_| AppError::new("cdp_timeout", "DevTools 执行超时").retryable())?
}

#[async_trait]
pub trait GroupGateway: Send + Sync {
    async fn list_groups(&self) -> AppResult<Vec<Group>>;
    async fn list_members(&self, group_id: i64) -> AppResult<MemberRoster>;
    async fn invalidate_member_cache(&self, _group_id: i64) {}
    async fn send_text(&self, group_id: i64, text: &str) -> AppResult<GatewayReceipt>;
    async fn recall(
        &self,
        group_id: i64,
        sender_user_id: i64,
        message_id: &str,
    ) -> AppResult<GatewayReceipt>;
    async fn mute(
        &self,
        group_id: i64,
        user_id: i64,
        duration_seconds: i64,
    ) -> AppResult<GatewayReceipt>;
    async fn unmute(&self, group_id: i64, user_id: i64) -> AppResult<GatewayReceipt>;
    async fn rename(
        &self,
        group_id: i64,
        member: &MemberRef,
        nickname: &str,
    ) -> AppResult<GatewayReceipt>;
    async fn remove_member(&self, group_id: i64, user_id: i64) -> AppResult<GatewayReceipt>;
    async fn set_group_mute(&self, group_id: i64, muted: bool) -> AppResult<GatewayReceipt>;
    async fn get_group_announcement(&self, _group_id: i64) -> AppResult<Option<GroupAnnouncement>> {
        Ok(None)
    }
    async fn set_group_announcement(
        &self,
        _group_id: i64,
        _text: &str,
    ) -> AppResult<GatewayReceipt> {
        Err(AppError::new(
            "capability_unsupported",
            "当前协议未开放群公告功能",
        ))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CapabilityStatus {
    Supported,
    Unverified,
    Unsupported,
}

impl CapabilityStatus {
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Supported)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GatewayCapabilities {
    pub announcement: CapabilityStatus,
    pub send_text: CapabilityStatus,
    pub mute: CapabilityStatus,
    pub recall: CapabilityStatus,
    pub rename: CapabilityStatus,
    pub remove_member: CapabilityStatus,
    pub group_mute: CapabilityStatus,
    pub member_events: CapabilityStatus,
}

#[async_trait]
pub trait RuntimeGateway: GroupGateway {
    async fn diagnose(&self) -> DiagnosticSnapshot;
    async fn session_identity(&self) -> AppResult<(i64, String)>;
    async fn install_message_listener(&self) -> AppResult<Value>;
    async fn poll_messages(&self) -> AppResult<Value>;
    async fn acknowledge_messages(&self, sequence: u64) -> AppResult<Value>;
    async fn read_batch(&self) -> AppResult<GatewayBatch> {
        legacy_gateway_batch(self.poll_messages().await?)
    }
    async fn ack(&self, session: &str, sequence: u64) -> AppResult<GatewayReceipt> {
        legacy_gateway_receipt(
            session,
            sequence,
            self.acknowledge_messages(sequence).await?,
        )
    }
    async fn poll_events(&self) -> AppResult<Vec<GatewayEvent>> {
        Ok(Vec::new())
    }
    async fn member_events(&self, _records: Vec<GatewayRecord>) -> AppResult<Vec<GatewayEvent>> {
        Ok(Vec::new())
    }
    fn session_epoch(&self) -> u64 {
        0
    }
    fn calibrate_capabilities(
        &self,
        _app_file_version: &str,
        _main_script_sha256: &str,
    ) -> GatewayCapabilities {
        self.capabilities()
    }
    fn capabilities(&self) -> GatewayCapabilities;
    fn member_sync_paused(&self) -> bool {
        false
    }
    #[cfg(feature = "fixture")]
    fn begin_developer_calibration(
        &self,
        _metadata: CalibrationMetadata,
        _capabilities: Vec<String>,
    ) -> AppResult<DeveloperCalibrationStatus> {
        Err(AppError::new(
            "calibration_unsupported",
            "当前网关不支持开发校准采集",
        ))
    }
    #[cfg(feature = "fixture")]
    fn developer_calibration_status(&self) -> DeveloperCalibrationStatus {
        DeveloperCalibrationRecorder::default().status()
    }
    #[cfg(feature = "fixture")]
    fn finish_developer_calibration(
        &self,
        _restored: bool,
    ) -> AppResult<DeveloperCalibrationExport> {
        Err(AppError::new(
            "calibration_unsupported",
            "当前网关不支持开发校准采集",
        ))
    }
    #[cfg(feature = "fixture")]
    fn cancel_developer_calibration(&self) -> DeveloperCalibrationStatus {
        DeveloperCalibrationRecorder::default().status()
    }
}

#[derive(Clone)]
pub struct CdpGateway {
    cdp: CdpClient,
    account_id: Arc<RwLock<String>>,
    sender_id: Arc<RwLock<i64>>,
    listener_session: Arc<RwLock<String>>,
    session_epoch: Arc<AtomicU64>,
    delivered_event_sequences: Arc<RwLock<BTreeMap<String, u64>>>,
    capabilities: Arc<SyncRwLock<GatewayCapabilities>>,
    member_requests: Arc<RwLock<BTreeMap<i64, Arc<MemberRequest>>>>,
    member_transport_gate: Arc<tokio::sync::Mutex<()>>,
    member_throttle: Arc<SyncMutex<MemberThrottle>>,
    identity_checked_at: Arc<SyncMutex<Option<Instant>>>,
    identity_gate: Arc<tokio::sync::Mutex<()>>,
    group_cache: Arc<RwLock<GroupCache>>,
    group_refresh_gate: Arc<tokio::sync::Mutex<()>>,
    member_cache: Arc<RwLock<BTreeMap<i64, (Instant, MemberRoster)>>>,
    #[cfg(feature = "fixture")]
    developer_calibration: Arc<SyncMutex<DeveloperCalibrationRecorder>>,
}

#[derive(Debug, Default)]
struct MemberThrottle {
    consecutive_rate_limits: u32,
    cooldown_until: Option<Instant>,
}

struct MemberRequest {
    result: tokio::sync::OnceCell<AppResult<MemberRoster>>,
    notify: tokio::sync::Notify,
}

impl CdpGateway {
    pub fn new(cdp: CdpClient) -> Self {
        Self {
            cdp,
            account_id: Arc::new(RwLock::new(String::new())),
            sender_id: Arc::new(RwLock::new(0)),
            listener_session: Arc::new(RwLock::new(String::new())),
            session_epoch: Arc::new(AtomicU64::new(0)),
            delivered_event_sequences: Arc::new(RwLock::new(BTreeMap::new())),
            capabilities: Arc::new(SyncRwLock::new(unverified_production_capabilities())),
            member_requests: Arc::new(RwLock::new(BTreeMap::new())),
            member_transport_gate: Arc::new(tokio::sync::Mutex::new(())),
            member_throttle: Arc::new(SyncMutex::new(MemberThrottle::default())),
            identity_checked_at: Arc::new(SyncMutex::new(None)),
            identity_gate: Arc::new(tokio::sync::Mutex::new(())),
            group_cache: Arc::new(RwLock::new(None)),
            group_refresh_gate: Arc::new(tokio::sync::Mutex::new(())),
            member_cache: Arc::new(RwLock::new(BTreeMap::new())),
            #[cfg(feature = "fixture")]
            developer_calibration: Arc::new(
                SyncMutex::new(DeveloperCalibrationRecorder::default()),
            ),
        }
    }

    pub fn cdp(&self) -> &CdpClient {
        &self.cdp
    }

    pub fn member_sync_paused(&self) -> bool {
        self.member_throttle
            .lock()
            .ok()
            .and_then(|state| state.cooldown_until)
            .is_some_and(|until| until > Instant::now())
    }

    pub async fn invalidate_member_cache(&self, group_id: i64) {
        self.member_cache.write().await.remove(&group_id);
    }

    pub async fn invalidate_group_cache(&self) {
        self.group_cache.write().await.take();
    }

    async fn reset_session_state(&self) {
        let had_session = !self.account_id.read().await.is_empty()
            || *self.sender_id.read().await > 0
            || self
                .identity_checked_at
                .lock()
                .ok()
                .and_then(|checked| *checked)
                .is_some();
        if !had_session {
            return;
        }
        *self.account_id.write().await = String::new();
        *self.sender_id.write().await = 0;
        if let Ok(mut checked) = self.identity_checked_at.lock() {
            *checked = None;
        }
        self.group_cache.write().await.take();
        self.member_cache.write().await.clear();
        self.member_requests.write().await.clear();
        self.delivered_event_sequences.write().await.clear();
        if let Ok(mut throttle) = self.member_throttle.lock() {
            *throttle = MemberThrottle::default();
        }
        self.session_epoch.fetch_add(1, Ordering::AcqRel);
    }

    fn member_retry_after(&self) -> Option<Duration> {
        self.member_throttle
            .lock()
            .ok()
            .and_then(|state| state.cooldown_until)
            .and_then(|until| until.checked_duration_since(Instant::now()))
    }

    fn require_capability(
        &self,
        status: CapabilityStatus,
        capability_key: &str,
        capability: &str,
    ) -> AppResult<()> {
        #[cfg(not(feature = "fixture"))]
        let _ = capability_key;
        match status {
            CapabilityStatus::Supported => Ok(()),
            CapabilityStatus::Unverified => {
                #[cfg(feature = "fixture")]
                if self
                    .developer_calibration
                    .lock()
                    .is_ok_and(|recorder| recorder.allows(capability_key))
                {
                    return Ok(());
                }
                Err(AppError::new(
                    "capability_unverified",
                    format!("当前旺商聊版本尚未完成{capability}能力校准"),
                ))
            }
            CapabilityStatus::Unsupported => Err(AppError::new(
                "capability_unsupported",
                format!("当前旺商聊版本不支持{capability}"),
            )),
        }
    }

    #[cfg(feature = "fixture")]
    fn record_calibration_nim(&self, route: &str, payload: Value, result: &Value) {
        if let Ok(mut recorder) = self.developer_calibration.lock() {
            recorder.record_nim(route, payload, result);
        }
    }

    #[cfg(feature = "fixture")]
    fn complete_calibration_operation(
        &self,
        route: &str,
        receipt: &GatewayReceipt,
        normalized_state: Value,
    ) {
        if let Ok(mut recorder) = self.developer_calibration.lock() {
            recorder.complete_operation(route, receipt, normalized_state);
        }
    }

    async fn xclient(&self, route: &str, payload: Value) -> AppResult<Value> {
        require_object_payload(route, &payload)?;
        let expression = ipc_expression("request", route, payload.clone());
        let result = self.cdp.evaluate(&expression).await?;
        #[cfg(feature = "fixture")]
        if let Ok(mut recorder) = self.developer_calibration.lock() {
            recorder.record_ipc(route, "request", payload, &result);
        }
        let response = decode_transport_response(route, &result)?;
        decode_business_response(route, response)
    }

    async fn group_infos(&self) -> AppResult<Vec<GroupInfo>> {
        if let Some((checked_at, groups)) = self.group_cache.read().await.clone() {
            if checked_at.elapsed() < Duration::from_secs(10) {
                return Ok(groups);
            }
        }
        let _refresh = self.group_refresh_gate.lock().await;
        if let Some((checked_at, groups)) = self.group_cache.read().await.clone() {
            if checked_at.elapsed() < Duration::from_secs(10) {
                return Ok(groups);
            }
        }
        let data = self.xclient(GROUP_LIST_ROUTE, json!({"v":"0"})).await?;
        let mut groups: BTreeMap<i64, GroupInfo> = BTreeMap::new();
        for (key, relation) in [("owner", "owner"), ("member", "member")] {
            for value in data
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let group_id = int_field(value, &["groupId"]);
                if group_id <= 0 {
                    continue;
                }
                let candidate = GroupInfo {
                    group_id,
                    cloud_id: text_field(value, &["groupCloudId"]),
                    name: human_name_field(value, &["groupName", "name", "remarkName", "nick"]),
                    owner_user_id: int_field(value, &["ownerUserId", "groupOwnerId", "ownerId"]),
                    member_count: int_field(
                        value,
                        &["memberCount", "groupMemberCount", "userCount"],
                    )
                    .max(0) as usize,
                    relation: relation.into(),
                    mute_mode: text_field(value, &["muteMode", "groupMuteMode", "muteState"]),
                };
                if let Some(existing) = groups.get_mut(&group_id) {
                    if candidate.relation == "owner" {
                        existing.relation = "owner".into();
                    }
                    if existing.cloud_id.is_empty() {
                        existing.cloud_id = candidate.cloud_id;
                    }
                    if existing.name.is_empty() {
                        existing.name = candidate.name;
                    }
                    if existing.owner_user_id <= 0 {
                        existing.owner_user_id = candidate.owner_user_id;
                    }
                    existing.member_count = existing.member_count.max(candidate.member_count);
                } else {
                    groups.insert(group_id, candidate);
                }
            }
        }
        let groups = groups.into_values().collect::<Vec<_>>();
        *self.group_cache.write().await = Some((Instant::now(), groups.clone()));
        Ok(groups)
    }

    async fn cached_group_infos(&self) -> Vec<GroupInfo> {
        self.group_cache
            .read()
            .await
            .as_ref()
            .filter(|(checked_at, _)| checked_at.elapsed() < Duration::from_secs(10))
            .map(|(_, groups)| groups.clone())
            .unwrap_or_default()
    }

    async fn resolve_cloud_id(&self, group_id: i64) -> AppResult<String> {
        if let Some(cloud_id) = self
            .cached_group_infos()
            .await
            .into_iter()
            .find(|group| group.group_id == group_id)
            .map(|group| group.cloud_id)
            .filter(|value| !value.is_empty())
        {
            return Ok(cloud_id);
        }
        self.group_infos()
            .await?
            .into_iter()
            .find(|group| group.group_id == group_id)
            .map(|group| group.cloud_id)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppError::new("group_not_found", "群 ID 未映射到 NIM 群"))
    }

    pub async fn session_identity(&self) -> AppResult<(i64, String)> {
        let _identity_gate = self.identity_gate.lock().await;
        let cached_account = self.account_id.read().await.clone();
        let cached_sender = *self.sender_id.read().await;
        let identity_fresh = self
            .identity_checked_at
            .lock()
            .ok()
            .and_then(|checked| *checked)
            .is_some_and(|checked| checked.elapsed() < Duration::from_secs(30));
        if cached_sender > 0 && !cached_account.is_empty() && identity_fresh {
            return Ok((cached_sender, cached_account));
        }
        let runtime_probe = self.cdp.evaluate(r#"({nimAccount:String((window.nim&&(window.nim.account||window.nim.options&&window.nim.options.account||window.nim.config&&window.nim.config.account))||"")})"#).await?;
        let nim_account_probe = runtime_probe
            .get("nimAccount")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if nim_account_probe.is_empty() {
            return Err(AppError::new("nim_not_ready", "需要您登录账号，等待登录").retryable());
        }
        let cached_account = self.account_id.read().await.clone();
        let cached_sender = *self.sender_id.read().await;
        let identity_fresh = self
            .identity_checked_at
            .lock()
            .ok()
            .and_then(|checked| *checked)
            .is_some_and(|checked| checked.elapsed() < Duration::from_secs(30));
        if cached_account == nim_account_probe && cached_sender > 0 && identity_fresh {
            return Ok((cached_sender, cached_account));
        }
        let nim_account = nim_account_probe.to_string();
        if nim_account.is_empty() {
            return Err(AppError::new(
                "nim_not_ready",
                "NIM 尚未初始化，请先登录旺商聊并等待会话初始化",
            )
            .retryable());
        }
        if cached_account != nim_account_probe {
            *self.sender_id.write().await = 0;
            *self.group_cache.write().await = None;
            self.member_requests.write().await.clear();
            self.member_cache.write().await.clear();
            self.delivered_event_sequences.write().await.clear();
            self.session_epoch.fetch_add(1, Ordering::AcqRel);
            if let Ok(mut checked) = self.identity_checked_at.lock() {
                *checked = None;
            }
        }
        *self.account_id.write().await = nim_account.clone();
        let groups = self.group_infos().await?;
        let mut sender = 0;
        for group in groups {
            let (members, _, _) = self.list_http_members(group.group_id).await?;
            if let Some(member) = members.iter().find(|member| member.nim_id == nim_account) {
                sender = member.user_id;
                break;
            }
        }
        if sender <= 0 {
            return Err(AppError::new("account_identity", "当前账号未映射到群成员").retryable());
        }
        *self.sender_id.write().await = sender;
        if let Ok(mut checked) = self.identity_checked_at.lock() {
            *checked = Some(Instant::now());
        }
        Ok((sender, nim_account))
    }

    async fn list_http_members(
        &self,
        group_id: i64,
    ) -> AppResult<(Vec<Member>, usize, Option<String>)> {
        let mut cursor = None;
        let mut members = BTreeMap::new();
        for page_index in 0..100 {
            let (page, next) = self
                .list_http_members_page(group_id, cursor.as_deref())
                .await?;
            for member in page {
                members
                    .entry((member.user_id, member.nim_id.clone()))
                    .or_insert(member);
            }
            let Some(next) = next else {
                return Ok((members.into_values().collect(), page_index + 1, None));
            };
            if cursor.as_deref() == Some(next.as_str()) {
                return Err(AppError::new(
                    "member_pagination",
                    "旺商聊 HTTP 成员分页返回了重复 cursor",
                ));
            }
            cursor = Some(next);
        }
        Err(AppError::new(
            "member_pagination",
            "旺商聊 HTTP 成员分页超过 100 页",
        ))
    }

    async fn list_http_members_page(
        &self,
        group_id: i64,
        cursor: Option<&str>,
    ) -> AppResult<(Vec<Member>, Option<String>)> {
        let mut payload = json!({"groupId":group_id,"v":"0"});
        if let Some(cursor) = cursor.filter(|value| !value.trim().is_empty()) {
            payload["cursor"] = Value::String(cursor.to_string());
        }
        let data = self.xclient(GROUP_MEMBERS_ROUTE, payload).await?;
        let account_id = self.account_id.read().await.clone();
        let now = Utc::now();
        let mut members = Vec::new();
        for value in data
            .get("groupMemberInfo")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let user_id = int_field(value, &["userId"]);
            if user_id <= 0 {
                continue;
            }
            let nickname = human_name_field(value, &["userNick", "nickname", "userName", "name"]);
            let card_name =
                human_name_field(value, &["groupMemberNick", "nick", "groupNick", "cardName"]);
            members.push(Member {
                account_id: account_id.clone(),
                group_id,
                user_id,
                nim_id: text_field(value, &["nimId"]),
                nickname: if nickname.is_empty() {
                    card_name.clone()
                } else {
                    nickname
                },
                card_name: if card_name.is_empty() {
                    human_name_field(value, &["userNick", "nickname", "userName", "name"])
                } else {
                    card_name
                },
                original_card_name: String::new(),
                managed_card_name: String::new(),
                card_suffix: String::new(),
                role: member_role(&text_field(
                    value,
                    &["groupRole", "role", "identity", "memberRole", "type"],
                )),
                account_state: text_field(
                    value,
                    &[
                        "accountState",
                        "accountStatus",
                        "muteStatus",
                        "muteState",
                        "muteMode",
                    ],
                ),
                blacklisted: false,
                present: true,
                join_source: "baseline".into(),
                prompt_read: true,
                locked_card_name: String::new(),
                violation_count: 0,
                discovered_at: now,
                joined_at: None,
                last_seen_at: now,
                updated_at: now,
            });
        }
        Ok((members, next_cursor(&data)))
    }

    async fn action(&self, route: &str, payload: Value) -> AppResult<GatewayReceipt> {
        require_object_payload(route, &payload)?;
        let expression = ipc_expression("request", route, payload.clone());
        let result = self.cdp.evaluate(&expression).await?;
        #[cfg(feature = "fixture")]
        if let Ok(mut recorder) = self.developer_calibration.lock() {
            recorder.record_ipc(route, "request", payload, &result);
        }
        decode_action_receipt(route, &result)
    }

    async fn team_member_events(
        &self,
        records: Vec<GatewayRecord>,
    ) -> AppResult<Vec<GatewayEvent>> {
        if records.is_empty() {
            return Ok(Vec::new());
        }
        let groups = self.cached_group_infos().await;
        let account_id = self.account_id.read().await.clone();
        let now = Utc::now();
        let mut events = Vec::new();
        for record in records {
            let team_id = text_field(&record.payload, &["teamId"]);
            let group_id = int_field(&record.payload, &["groupId"]);
            let group_id = if group_id > 0 {
                group_id
            } else {
                groups
                    .iter()
                    .find(|group| group.cloud_id == team_id)
                    .map(|group| group.group_id)
                    .unwrap_or(0)
            };
            if group_id <= 0 {
                continue;
            }
            self.member_cache.write().await.remove(&group_id);
            self.group_cache.write().await.take();
            for value in record
                .payload
                .get("members")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let nim_id = text_field(value, &["nimId", "account", "accid"]);
                if nim_id.is_empty() {
                    continue;
                }
                let nickname = human_name_field(value, &["nickname", "nick", "cardName"]);
                let user_id = int_field(value, &["userId"]);
                let member = Member {
                    account_id: account_id.clone(),
                    group_id,
                    user_id: if user_id > 0 {
                        user_id
                    } else {
                        synthetic_nim_user_id(&nim_id)
                    },
                    nim_id,
                    nickname: nickname.clone(),
                    card_name: nickname,
                    original_card_name: String::new(),
                    managed_card_name: String::new(),
                    card_suffix: String::new(),
                    role: member_role(&text_field(value, &["type", "memberType"])),
                    account_state: String::new(),
                    blacklisted: false,
                    present: true,
                    join_source: "nim-team-notification".into(),
                    prompt_read: false,
                    locked_card_name: String::new(),
                    violation_count: 0,
                    discovered_at: now,
                    joined_at: Some(now),
                    last_seen_at: now,
                    updated_at: now,
                };
                events.push(match record.kind {
                    GatewayRecordKind::TeamMemberJoined => {
                        GatewayEvent::MemberJoined { group_id, member }
                    }
                    GatewayRecordKind::TeamMemberLeft => {
                        GatewayEvent::MemberLeft { group_id, member }
                    }
                    GatewayRecordKind::TeamMemberUpdated => {
                        GatewayEvent::MemberUpdated { group_id, member }
                    }
                    _ => continue,
                });
            }
        }
        Ok(events)
    }

    pub async fn install_message_listener(&self) -> AppResult<Value> {
        let value = self.cdp.evaluate(LISTENER_EXPRESSION).await?;
        if let Some(session) = value.get("session").and_then(Value::as_str) {
            let mut current = self.listener_session.write().await;
            if *current != session {
                *current = session.to_string();
                *self.account_id.write().await = String::new();
                *self.sender_id.write().await = 0;
                *self
                    .identity_checked_at
                    .lock()
                    .expect("identity mutex poisoned") = None;
                self.group_cache.write().await.take();
                self.member_cache.write().await.clear();
                self.member_requests.write().await.clear();
                self.delivered_event_sequences.write().await.clear();
                self.session_epoch.fetch_add(1, Ordering::AcqRel);
            }
        }
        Ok(value)
    }

    pub async fn read_batch(&self) -> AppResult<GatewayBatch> {
        let value = self.cdp.evaluate(READ_BATCH_EXPRESSION).await?;
        let mut batch = parse_gateway_batch(value)?;
        let groups = self.cached_group_infos().await;
        for record in &mut batch.records {
            if record.kind != GatewayRecordKind::Message
                || int_field(&record.payload, &["groupId"]) > 0
            {
                continue;
            }
            let team_id = text_field(&record.payload, &["teamId", "to"]);
            if let Some(group_id) = groups
                .iter()
                .find(|group| group.cloud_id == team_id)
                .map(|group| group.group_id)
            {
                if let Some(payload) = record.payload.as_object_mut() {
                    payload.insert("groupId".into(), Value::from(group_id));
                }
            }
        }
        *self.listener_session.write().await = batch.session.clone();
        #[cfg(feature = "fixture")]
        if let Ok(mut recorder) = self.developer_calibration.lock() {
            recorder.record_callbacks(&batch.records);
        }
        Ok(batch)
    }

    pub async fn ack(&self, session: &str, sequence: u64) -> AppResult<GatewayReceipt> {
        if session.trim().is_empty() || sequence == 0 {
            return Err(AppError::new(
                "invalid_argument",
                "监听会话和确认序号必须有效",
            ));
        }
        let value = self
            .cdp
            .evaluate(&ack_expression(session, sequence))
            .await?;
        parse_gateway_receipt(session, sequence, value)
    }

    pub async fn poll_messages(&self) -> AppResult<Value> {
        let batch = self.read_batch().await?;
        let messages = batch
            .records
            .iter()
            .filter(|record| record.kind == GatewayRecordKind::Message)
            .map(legacy_message_value)
            .collect::<Vec<_>>();
        Ok(json!({
            "ok": true,
            "session": batch.session,
            "messages": messages,
            "remaining": batch.remaining,
            "dropped": batch.dropped,
        }))
    }
    pub async fn acknowledge_messages(&self, sequence: u64) -> AppResult<Value> {
        let session = self.listener_session.read().await.clone();
        let receipt = self.ack(&session, sequence).await?;
        Ok(json!({
            "ok": true,
            "session": receipt.session,
            "acknowledgedThrough": receipt.acknowledged_through,
            "acked": receipt.acknowledged,
            "remaining": receipt.remaining,
            "dropped": receipt.dropped,
        }))
    }
}

#[async_trait]
impl RuntimeGateway for CdpGateway {
    async fn diagnose(&self) -> DiagnosticSnapshot {
        let snapshot = self.cdp.diagnose().await;
        if snapshot.status != ConnectionStatus::Ready {
            self.reset_session_state().await;
        }
        snapshot
    }

    async fn session_identity(&self) -> AppResult<(i64, String)> {
        CdpGateway::session_identity(self).await
    }

    async fn install_message_listener(&self) -> AppResult<Value> {
        CdpGateway::install_message_listener(self).await
    }

    async fn poll_messages(&self) -> AppResult<Value> {
        CdpGateway::poll_messages(self).await
    }

    async fn acknowledge_messages(&self, sequence: u64) -> AppResult<Value> {
        CdpGateway::acknowledge_messages(self, sequence).await
    }

    async fn read_batch(&self) -> AppResult<GatewayBatch> {
        CdpGateway::read_batch(self).await
    }

    async fn ack(&self, session: &str, sequence: u64) -> AppResult<GatewayReceipt> {
        CdpGateway::ack(self, session, sequence).await
    }

    async fn poll_events(&self) -> AppResult<Vec<GatewayEvent>> {
        Err(AppError::new(
            "event_poll_requires_batch",
            "成员事件必须从已读取的消息批次中处理",
        ))
    }

    async fn member_events(&self, records: Vec<GatewayRecord>) -> AppResult<Vec<GatewayEvent>> {
        self.team_member_events(records).await
    }

    fn capabilities(&self) -> GatewayCapabilities {
        self.capabilities
            .read()
            .map(|value| value.clone())
            .unwrap_or_else(|_| unverified_production_capabilities())
    }

    fn member_sync_paused(&self) -> bool {
        CdpGateway::member_sync_paused(self)
    }

    #[cfg(feature = "fixture")]
    fn begin_developer_calibration(
        &self,
        metadata: CalibrationMetadata,
        capabilities: Vec<String>,
    ) -> AppResult<DeveloperCalibrationStatus> {
        self.developer_calibration
            .lock()
            .map_err(|_| AppError::new("calibration_state", "校准采集状态不可用"))?
            .start(metadata, capabilities)
    }

    #[cfg(feature = "fixture")]
    fn developer_calibration_status(&self) -> DeveloperCalibrationStatus {
        self.developer_calibration
            .lock()
            .map(|recorder| recorder.status())
            .unwrap_or_else(|_| DeveloperCalibrationRecorder::default().status())
    }

    #[cfg(feature = "fixture")]
    fn finish_developer_calibration(
        &self,
        restored: bool,
    ) -> AppResult<DeveloperCalibrationExport> {
        self.developer_calibration
            .lock()
            .map_err(|_| AppError::new("calibration_state", "校准采集状态不可用"))?
            .finish(restored)
    }

    #[cfg(feature = "fixture")]
    fn cancel_developer_calibration(&self) -> DeveloperCalibrationStatus {
        self.developer_calibration
            .lock()
            .map(|mut recorder| recorder.cancel())
            .unwrap_or_else(|_| DeveloperCalibrationRecorder::default().status())
    }

    fn session_epoch(&self) -> u64 {
        self.session_epoch.load(Ordering::Acquire)
    }

    fn calibrate_capabilities(
        &self,
        app_file_version: &str,
        main_script_sha256: &str,
    ) -> GatewayCapabilities {
        let calibrated = runtime_capabilities(app_file_version, main_script_sha256);
        if let Ok(mut capabilities) = self.capabilities.write() {
            *capabilities = calibrated.clone();
        }
        calibrated
    }
}

impl CdpGateway {
    async fn writable_member(&self, group_id: i64, user_id: i64) -> AppResult<Member> {
        require_positive("groupId", group_id)?;
        require_positive("userId", user_id)?;
        let roster = self.list_members(group_id).await?;
        let sender_id = if *self.sender_id.read().await > 0 {
            *self.sender_id.read().await
        } else {
            self.session_identity().await?.0
        };
        let manager = roster.members.iter().any(|member| {
            member.user_id == sender_id
                && member.user_id > 0
                && member.present
                && matches!(member.role.to_ascii_lowercase().as_str(), "owner" | "admin")
        });
        if !manager {
            return Err(AppError::new(
                "management_required",
                "需要当前账号具备群管理权限",
            ));
        }
        let member = roster
            .members
            .iter()
            .find(|member| member.user_id == user_id && member.present)
            .cloned()
            .ok_or_else(|| AppError::new("member_not_found", "目标成员不属于当前群或已不在线"))?;
        if matches!(member.role.to_ascii_lowercase().as_str(), "owner" | "admin") {
            return Err(AppError::new(
                "member_protected",
                "群主和管理员不能执行成员写操作",
            ));
        }
        if member.user_id <= 0 || member.user_id == synthetic_nim_user_id(&member.nim_id) {
            return Err(AppError::new(
                "member_identity_incomplete",
                "目标成员只有临时身份，不能执行成员写操作",
            )
            .retryable());
        }
        Ok(member)
    }

    async fn manager_roster(&self, group_id: i64) -> AppResult<MemberRoster> {
        let roster = self.list_members(group_id).await?;
        let sender_id = if *self.sender_id.read().await > 0 {
            *self.sender_id.read().await
        } else {
            self.session_identity().await?.0
        };
        if roster.members.iter().any(|member| {
            member.user_id == sender_id
                && member.present
                && matches!(member.role.to_ascii_lowercase().as_str(), "owner" | "admin")
        }) {
            Ok(roster)
        } else {
            Err(AppError::new(
                "management_required",
                "需要当前账号具备群管理权限",
            ))
        }
    }

    async fn verify_renamed_member(
        &self,
        group_id: i64,
        user_id: i64,
        nickname: &str,
        receipt: GatewayReceipt,
    ) -> AppResult<GatewayReceipt> {
        self.member_cache.write().await.remove(&group_id);
        let roster = self.list_members_with_backoff(group_id).await?;
        let matches = roster.members.iter().any(|member| {
            member.user_id == user_id
                && (member.card_name == nickname || member.nickname == nickname)
        });
        if matches {
            Ok(receipt)
        } else {
            Err(AppError::new(
                "write_verify_failed",
                "旺商聊已返回改名成功，但回读的群名片不一致",
            ))
        }
    }

    async fn verify_removed_member(
        &self,
        group_id: i64,
        user_id: i64,
        mut receipt: GatewayReceipt,
    ) -> AppResult<GatewayReceipt> {
        self.member_cache.write().await.remove(&group_id);
        let roster = self.list_members_with_backoff(group_id).await?;
        if roster.authority != "authoritative" {
            receipt.status = "unknown".into();
            receipt.verification = Some("unknown".into());
            return Ok(receipt);
        }
        if roster
            .members
            .iter()
            .any(|member| member.user_id == user_id)
        {
            return Err(AppError::new(
                "write_verify_failed",
                "旺商聊已返回移除成功，但回读仍存在该成员",
            ));
        }
        Ok(receipt)
    }

    async fn verify_member_mute(
        &self,
        group_id: i64,
        user_id: i64,
        muted: bool,
        mut receipt: GatewayReceipt,
    ) -> AppResult<GatewayReceipt> {
        self.member_cache.write().await.remove(&group_id);
        let roster = self.list_members_with_backoff(group_id).await?;
        let Some(member) = roster
            .members
            .iter()
            .find(|member| member.user_id == user_id)
        else {
            receipt.status = "unknown".into();
            receipt.verification = Some("unknown".into());
            return Ok(receipt);
        };
        let raw_state = member.account_state.to_ascii_lowercase();
        let state = if raw_state.contains("unmute")
            || raw_state.contains("not_mute")
            || raw_state.contains("normal")
            || raw_state.contains("good")
            || raw_state.contains("active")
            || raw_state.contains("ok")
        {
            "mute_none".to_string()
        } else {
            raw_state
        };
        let observed = if state.contains("mute") || state.contains("禁言") {
            Some(!state.contains("no") && !state.contains("cancel") && !state.contains("none"))
        } else {
            None
        };
        match observed {
            Some(value) if value == muted => {
                receipt.verification = Some("verified".into());
                Ok(receipt)
            }
            Some(_) => Err(AppError::new(
                "write_verify_failed",
                "旺商聊回读的成员禁言状态与请求不一致",
            )),
            None => {
                receipt.status = "unknown".into();
                receipt.verification = Some("unknown".into());
                Ok(receipt)
            }
        }
    }

    async fn list_groups(&self) -> AppResult<Vec<Group>> {
        let infos = self.group_infos().await?;
        let account_id = self.account_id.read().await.clone();
        let now = Utc::now();
        Ok(infos
            .into_iter()
            .map(|info| Group {
                account_id: account_id.clone(),
                group_id: info.group_id,
                name: info.name,
                owner_user_id: info.owner_user_id,
                enabled: false,
                ai_enabled: false,
                moderation_enabled: false,
                manual_takeover: false,
                welcome_message: String::new(),
                updated_at: now,
            })
            .collect())
    }

    async fn list_members(&self, group_id: i64) -> AppResult<MemberRoster> {
        require_positive("groupId", group_id)?;
        if let Some((checked_at, roster)) = self.member_cache.read().await.get(&group_id) {
            if checked_at.elapsed() < Duration::from_secs(5) {
                return Ok(roster.clone());
            }
        }
        if let Some(delay) = self.member_retry_after() {
            if let Some((_, cached)) = self.member_cache.read().await.get(&group_id) {
                let mut cached = cached.clone();
                cached.status = "rate-limited".into();
                cached.retry_at = Some(
                    Utc::now()
                        + chrono::Duration::from_std(delay)
                            .unwrap_or_else(|_| chrono::Duration::seconds(120)),
                );
                return Ok(cached);
            }
            return Err(AppError::new(
                "gateway_rate_limited",
                "旺商聊成员请求过于频繁，正在等待后重试",
            )
            .retryable());
        }
        let request = {
            let mut requests = self.member_requests.write().await;
            if let Some(existing) = requests.get(&group_id) {
                (existing.clone(), false)
            } else {
                let request = Arc::new(MemberRequest {
                    result: tokio::sync::OnceCell::new(),
                    notify: tokio::sync::Notify::new(),
                });
                requests.insert(group_id, request.clone());
                (request, true)
            }
        };
        if !request.1 {
            loop {
                let notified = request.0.notify.notified();
                if let Some(result) = request.0.result.get() {
                    return result.clone();
                }
                notified.await;
            }
        }
        let request_epoch = self.session_epoch.load(Ordering::Acquire);
        let result = self.list_members_with_backoff(group_id).await;
        if request_epoch == self.session_epoch.load(Ordering::Acquire) {
            if let Ok(roster) = &result {
                self.member_cache
                    .write()
                    .await
                    .insert(group_id, (Instant::now(), roster.clone()));
            }
        }
        let _ = request.0.result.set(result.clone());
        request.0.notify.notify_waiters();
        let mut requests = self.member_requests.write().await;
        if requests
            .get(&group_id)
            .is_some_and(|current| Arc::ptr_eq(current, &request.0))
        {
            requests.remove(&group_id);
        }
        result
    }
}

#[async_trait]
trait MemberRequestGateway {
    async fn list_members_with_backoff(&self, group_id: i64) -> AppResult<MemberRoster>;
    async fn wait_for_member_cooldown(&self);
    fn member_request_rate_limited(&self);
    fn member_request_succeeded(&self);
    async fn list_members_impl(&self, group_id: i64) -> AppResult<MemberRoster>;
}

#[async_trait]
impl MemberRequestGateway for CdpGateway {
    async fn list_members_with_backoff(&self, group_id: i64) -> AppResult<MemberRoster> {
        for attempt in 0..5_u32 {
            self.wait_for_member_cooldown().await;
            let result = {
                let _gate = self.member_transport_gate.lock().await;
                self.list_members_impl(group_id).await
            };
            match result {
                Ok(mut roster) => {
                    roster.status = if roster.complete { "ready" } else { "partial" }.into();
                    self.member_request_succeeded();
                    return Ok(roster);
                }
                Err(error) if is_member_rate_limited(&error) && attempt < 3 => {
                    self.member_request_rate_limited();
                }
                Err(error) if is_member_rate_limited(&error) => {
                    self.member_request_rate_limited();
                    return Err(rate_limited_error(error));
                }
                Err(error) => return Err(error),
            }
        }
        Err(AppError::new("gateway_rate_limited", "旺商聊成员请求过于频繁，请稍后重试").retryable())
    }

    async fn wait_for_member_cooldown(&self) {
        let delay = self
            .member_throttle
            .lock()
            .ok()
            .and_then(|state| state.cooldown_until)
            .and_then(|until| until.checked_duration_since(Instant::now()));
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
    }

    fn member_request_rate_limited(&self) {
        let mut state = self
            .member_throttle
            .lock()
            .expect("member throttle mutex poisoned");
        state.consecutive_rate_limits = state.consecutive_rate_limits.saturating_add(1);
        let seconds = member_backoff_seconds(state.consecutive_rate_limits);
        state.cooldown_until = Some(Instant::now() + Duration::from_secs(seconds));
    }

    fn member_request_succeeded(&self) {
        if let Ok(mut state) = self.member_throttle.lock() {
            state.consecutive_rate_limits = 0;
            state.cooldown_until = None;
        }
    }

    async fn list_members_impl(&self, group_id: i64) -> AppResult<MemberRoster> {
        require_positive("groupId", group_id)?;
        if self.account_id.read().await.is_empty() {
            let _ = self.session_identity().await?;
        }
        let (mut members, http_pages, http_cursor) = self.list_http_members(group_id).await?;
        let http_returned_count = members.len();
        let mut infos = self.cached_group_infos().await;
        if !infos.iter().any(|group| group.group_id == group_id) {
            infos = self.group_infos().await?;
        }
        if infos.is_empty() {
            return Err(AppError::new(
                "group_cache_miss",
                "group cache is missing; refresh groups first",
            )
            .retryable());
        }
        let reported_hint = infos
            .iter()
            .find(|group| group.group_id == group_id)
            .map(|group| group.member_count)
            .unwrap_or(0);
        let cloud_id = infos
            .iter()
            .find(|group| group.group_id == group_id)
            .map(|group| group.cloud_id.clone())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppError::new("group_not_found", "群 ID 未映射到 NIM 群"))?;
        let mut nim_cursor = None;
        let mut nim_values = Vec::new();
        let mut nim_complete = false;
        let mut nim_failed = false;
        let mut nim_pages = 0usize;
        let mut source_errors = Vec::new();
        for page_index in 0..100 {
            nim_pages = page_index + 1;
            let expression = nim_team_members_expression(&cloud_id, nim_cursor.as_deref());
            let nim = match self.cdp.evaluate(&expression).await {
                Ok(value) => value,
                Err(error) => {
                    source_errors.push(MemberSourceError {
                        source: "nim".into(),
                        route: "nim.getTeamMembers".into(),
                        page: nim_pages,
                        cursor: nim_cursor.clone(),
                        reason: format!("{}: {}", error.code, error.message),
                    });
                    nim_failed = true;
                    break;
                }
            };
            #[cfg(feature = "fixture")]
            self.record_calibration_nim(
                "nim.getTeamMembers",
                json!({"teamId": cloud_id, "cursor": nim_cursor}),
                &nim,
            );
            if nim.get("ok").and_then(Value::as_bool) != Some(true) {
                let reason = nim
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .unwrap_or("NIM member request failed")
                    .to_string();
                let source_error = nim_response_error("nim.getTeamMembers", &nim, &reason);
                if is_member_rate_limited(&source_error) {
                    return Err(source_error);
                }
                let structured_code = source_error.gateway.as_deref().and_then(|metadata| {
                    metadata
                        .business_code
                        .or(metadata.business_errno)
                        .or(metadata.transport_code)
                        .or(metadata.transport_errno)
                });
                source_errors.push(MemberSourceError {
                    source: "nim".into(),
                    route: "nim.getTeamMembers".into(),
                    page: nim_pages,
                    cursor: nim_cursor.clone(),
                    reason: match structured_code {
                        Some(code) => {
                            format!("{}({code}): {}", source_error.code, source_error.message)
                        }
                        None => format!("{}: {}", source_error.code, source_error.message),
                    },
                });
                nim_failed = true;
                break;
            }
            nim_values.extend(
                nim.get("members")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            let Some(next) = next_cursor(&nim) else {
                nim_cursor = None;
                nim_complete = true;
                break;
            };
            if nim_cursor.as_deref() == Some(next.as_str()) {
                return Err(AppError::new(
                    "member_pagination",
                    "NIM 成员分页返回了重复 cursor",
                ));
            }
            nim_cursor = Some(next);
        }
        if !nim_complete && !nim_failed {
            return Err(AppError::new(
                "member_pagination",
                "NIM 成员分页超过 100 页",
            ));
        }
        let used_nim = true;
        let nim_returned_count = nim_values.len();
        for value in nim_values.iter() {
            let nim_id = text_field(value, &["nimId"]);
            let card = human_name_field(value, &["cardName"]);
            if let Some(member) = members.iter_mut().find(|member| member.nim_id == nim_id) {
                if wire_name_missing(&member.card_name) && !wire_name_missing(&card) {
                    member.card_name = card;
                }
                continue;
            }
            if nim_id.is_empty() {
                continue;
            }
            let account_id = self.account_id.read().await.clone();
            members.push(Member {
                account_id,
                group_id,
                user_id: synthetic_nim_user_id(&nim_id),
                nim_id,
                nickname: card.clone(),
                card_name: card,
                original_card_name: String::new(),
                managed_card_name: String::new(),
                card_suffix: String::new(),
                role: member_role(&text_field(value, &["type"])),
                account_state: String::new(),
                blacklisted: false,
                present: true,
                join_source: "baseline".into(),
                prompt_read: true,
                locked_card_name: String::new(),
                violation_count: 0,
                discovered_at: Utc::now(),
                joined_at: None,
                last_seen_at: Utc::now(),
                updated_at: Utc::now(),
            });
        }
        let synthetic_user_ids = members
            .iter()
            .filter(|member| member.user_id == synthetic_nim_user_id(&member.nim_id))
            .map(|member| member.user_id)
            .collect::<Vec<_>>();
        let canonical_count = members.len().saturating_sub(synthetic_user_ids.len());
        let resolved = members.len();
        let reported = resolved;
        let complete = nim_complete && source_errors.is_empty() && http_cursor.is_none();
        Ok(MemberRoster {
            status: if complete { "ready" } else { "partial" }.into(),
            members,
            reported_count: reported,
            resolved_count: resolved,
            complete,
            completeness: if complete {
                RosterCompleteness::Complete
            } else if resolved > 0 {
                RosterCompleteness::Partial
            } else {
                RosterCompleteness::Unknown
            },
            completeness_reason: if complete {
                "旺商聊 HTTP 与 NIM 当前成员快照已完成".into()
            } else if resolved > 0 {
                format!("当前已获取 {resolved} 名成员，等待来源完成")
            } else {
                "旺商聊未返回可识别成员".into()
            },
            http_returned_count,
            http_reported_count: http_returned_count.max(reported_hint),
            http_cursor,
            nim_returned_count,
            nim_reported_count: nim_returned_count,
            nim_cursor,
            authority: if complete { "authoritative" } else { "partial" }.into(),
            sources: if used_nim {
                vec!["wangshangliao-http".into(), "nim-team-members".into()]
            } else {
                vec!["wangshangliao-http".into()]
            },
            source_errors,
            retry_at: None,
            canonical_count,
            synthetic_user_ids,
            http_pages,
            nim_pages,
        })
    }
}

#[async_trait]
impl GroupGateway for CdpGateway {
    async fn list_groups(&self) -> AppResult<Vec<Group>> {
        CdpGateway::list_groups(self).await
    }

    async fn list_members(&self, group_id: i64) -> AppResult<MemberRoster> {
        CdpGateway::list_members(self, group_id).await
    }

    async fn invalidate_member_cache(&self, group_id: i64) {
        CdpGateway::invalidate_member_cache(self, group_id).await;
    }

    async fn send_text(&self, group_id: i64, text: &str) -> AppResult<GatewayReceipt> {
        self.require_capability(self.capabilities().send_text, "sendText", "发送消息")?;
        require_positive("groupId", group_id)?;
        if text.trim().is_empty() {
            return Err(AppError::new("invalid_argument", "发送内容为空"));
        }
        let sender = if *self.sender_id.read().await > 0 {
            *self.sender_id.read().await
        } else {
            self.session_identity().await?.0
        };
        let target = self.resolve_cloud_id(group_id).await?;
        let payload = json!({"from":{"id":sender},"to":{"id":group_id},"msgDevice":1,"createdAt":{"seconds":Utc::now().timestamp(),"nanos":0},"msgSession":2,"msgVersion":2,"accountType":0,"msgFormat":0,"msgRole":0,"msgRingtone":0,"appoint":0,"content":{"data":text}});
        let encode = self
            .cdp
            .evaluate(&ipc_expression(
                "encode",
                "/v1/plugins/encode-msg",
                payload.clone(),
            ))
            .await?;
        #[cfg(feature = "fixture")]
        if let Ok(mut recorder) = self.developer_calibration.lock() {
            recorder.record_ipc("/v1/plugins/encode-msg", "encode", payload, &encode);
        }
        let content = decode_transport_response("/v1/plugins/encode-msg", &encode)?;
        if content.is_empty() {
            return Err(AppError::new("encode_failed", "旺商聊消息编码失败"));
        }
        let delivery = self
            .cdp
            .evaluate(&nim_send_expression(&target, content))
            .await?;
        #[cfg(feature = "fixture")]
        self.record_calibration_nim(
            "nim.sendCustomMsg",
            json!({"target": target, "content": content}),
            &delivery,
        );
        if delivery.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(AppError::new(
                "send_failed",
                delivery
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .unwrap_or("NIM 投递失败"),
            )
            .with_gateway(GatewayErrorMetadata::new(
                "nim.sendCustomMsg",
                GatewayErrorLayer::Delivery,
            ))
            .retryable());
        }
        let message_id = delivery_message_id(&delivery)?;
        let receipt = GatewayReceipt {
            route: "nim.sendCustomMsg".into(),
            status: "succeeded".into(),
            transport_code: None,
            transport_errno: None,
            business_code: Some(0),
            business_errno: Some(0),
            business_message: delivery
                .get("message")
                .or_else(|| delivery.get("msg"))
                .and_then(Value::as_str)
                .unwrap_or("OK")
                .into(),
            request_id: receipt_identifier(&delivery, &["requestId", "idClient", "traceId"]),
            message_id,
            session: String::new(),
            acknowledged_through: 0,
            acknowledged: 0,
            remaining: 0,
            dropped: 0,
            verification: Some("not-applicable".into()),
        };
        #[cfg(feature = "fixture")]
        self.complete_calibration_operation(
            "nim.sendCustomMsg",
            &receipt,
            json!({"action": "send_text", "messageId": receipt.message_id}),
        );
        Ok(receipt)
    }

    async fn recall(
        &self,
        group_id: i64,
        sender_user_id: i64,
        message_id: &str,
    ) -> AppResult<GatewayReceipt> {
        self.require_capability(self.capabilities().recall, "recall", "撤回消息")?;
        require_positive("groupId", group_id)?;
        require_positive("userId", sender_user_id)?;
        require_non_empty("messageId", message_id)?;
        let cloud = self.resolve_cloud_id(group_id).await?;
        let mut receipt = self
            .action(
                MESSAGE_RECALL_ROUTE,
                json!({"groupCloudId":cloud,"userId":sender_user_id,"msgId":message_id}),
            )
            .await?;
        if !receipt.message_id.is_empty() && receipt.message_id != message_id {
            return Err(AppError::new(
                "write_verify_failed",
                "旺商聊撤回回执中的消息 ID 与目标不一致",
            ));
        }
        receipt.verification = Some(
            if receipt.message_id.is_empty() {
                "unknown"
            } else {
                "verified"
            }
            .into(),
        );
        if receipt.message_id.is_empty() {
            receipt.status = "unknown".into();
        }
        #[cfg(feature = "fixture")]
        self.complete_calibration_operation(
            MESSAGE_RECALL_ROUTE,
            &receipt,
            json!({
                "action": "recall",
                "messageId": message_id,
                "recalled": receipt.verification.as_deref() == Some("verified"),
            }),
        );
        Ok(receipt)
    }
    async fn mute(
        &self,
        group_id: i64,
        user_id: i64,
        duration_seconds: i64,
    ) -> AppResult<GatewayReceipt> {
        self.require_capability(self.capabilities().mute, "mute", "成员禁言")?;
        require_positive("groupId", group_id)?;
        require_positive("userId", user_id)?;
        require_positive("durationSeconds", duration_seconds)?;
        self.writable_member(group_id, user_id).await?;
        let receipt = self
            .action(
                MEMBER_MUTE_ROUTE,
                json!({"groupId":group_id,"userId":user_id,"min":(duration_seconds+59)/60}),
            )
            .await?;
        let receipt = self
            .verify_member_mute(group_id, user_id, true, receipt)
            .await?;
        #[cfg(feature = "fixture")]
        self.complete_calibration_operation(
            MEMBER_MUTE_ROUTE,
            &receipt,
            json!({
                "action": "mute",
                "groupId": group_id,
                "userId": user_id,
                "durationSeconds": duration_seconds,
                "muted": receipt.verification.as_deref() == Some("verified"),
            }),
        );
        Ok(receipt)
    }
    async fn unmute(&self, group_id: i64, user_id: i64) -> AppResult<GatewayReceipt> {
        self.require_capability(self.capabilities().mute, "mute", "成员解禁")?;
        require_positive("groupId", group_id)?;
        require_positive("userId", user_id)?;
        self.writable_member(group_id, user_id).await?;
        let receipt = self
            .action(
                MEMBER_UNMUTE_ROUTE,
                json!({"groupId":group_id,"userId":user_id}),
            )
            .await?;
        let receipt = self
            .verify_member_mute(group_id, user_id, false, receipt)
            .await?;
        #[cfg(feature = "fixture")]
        self.complete_calibration_operation(
            MEMBER_UNMUTE_ROUTE,
            &receipt,
            json!({
                "action": "unmute",
                "groupId": group_id,
                "userId": user_id,
                "muted": false,
                "verification": receipt.verification,
            }),
        );
        Ok(receipt)
    }
    async fn rename(
        &self,
        group_id: i64,
        member: &MemberRef,
        nickname: &str,
    ) -> AppResult<GatewayReceipt> {
        self.require_capability(self.capabilities().rename, "rename", "修改群名片")?;
        require_positive("groupId", group_id)?;
        require_non_empty("nickname", nickname)?;
        let requested_user_id = member.user_id.ok_or_else(|| {
            AppError::new("member_identity", "成员写操作必须使用 canonical userId")
        })?;
        require_positive("userId", requested_user_id)?;
        let target = self.writable_member(group_id, requested_user_id).await?;
        if member
            .nim_id
            .as_deref()
            .is_some_and(|nim_id| !nim_id.trim().is_empty() && nim_id != target.nim_id)
        {
            return Err(AppError::new(
                "member_identity",
                "提交的成员身份与当前群成员名单不匹配",
            ));
        }
        let mut http_error = None;
        if target.user_id > 0 {
            match self
                .action(
                    MEMBER_RENAME_ROUTE,
                    json!({"groupId":group_id,"userId":target.user_id,"nick":nickname}),
                )
                .await
            {
                Ok(receipt) => {
                    let receipt = self
                        .verify_renamed_member(group_id, target.user_id, nickname, receipt)
                        .await?;
                    #[cfg(feature = "fixture")]
                    self.complete_calibration_operation(
                        MEMBER_RENAME_ROUTE,
                        &receipt,
                        json!({
                            "action": "rename",
                            "groupId": group_id,
                            "userId": target.user_id,
                            "cardName": nickname,
                            "verification": receipt.verification,
                        }),
                    );
                    return Ok(receipt);
                }
                Err(error) if rename_fallback_allowed(&error) => http_error = Some(error),
                Err(error) => return Err(error),
            }
        }
        let nim_id = target.nim_id.trim();
        if nim_id.is_empty() {
            return Err(http_error
                .unwrap_or_else(|| AppError::new("member_identity", "成员缺少 userId 和 nimId")));
        }
        let cloud = match self.resolve_cloud_id(group_id).await {
            Ok(cloud) => cloud,
            Err(fallback_error) => {
                return Err(http_error
                    .unwrap_or(fallback_error)
                    .with_detail("NIM 群标识解析失败"));
            }
        };
        let result = match self
            .cdp
            .evaluate(&nim_update_nick_expression(&cloud, nim_id, nickname))
            .await
        {
            Ok(result) => result,
            Err(fallback_error) => {
                return Err(http_error
                    .unwrap_or(fallback_error)
                    .with_detail("NIM 群名片回退请求失败"));
            }
        };
        #[cfg(feature = "fixture")]
        self.record_calibration_nim(
            "nim.updateNickInTeam",
            json!({"teamId": cloud, "nimId": nim_id, "nickname": nickname}),
            &result,
        );
        if result.get("ok").and_then(Value::as_bool) == Some(true) {
            let receipt = GatewayReceipt {
                route: "nim.updateNickInTeam".into(),
                status: "succeeded".into(),
                transport_code: None,
                transport_errno: None,
                business_code: Some(0),
                business_errno: Some(0),
                business_message: "OK".into(),
                request_id: receipt_identifier(&result, &["requestId", "traceId"]),
                message_id: String::new(),
                session: String::new(),
                acknowledged_through: 0,
                acknowledged: 0,
                remaining: 0,
                dropped: 0,
                verification: None,
            };
            let receipt = self
                .verify_renamed_member(group_id, requested_user_id, nickname, receipt)
                .await?;
            #[cfg(feature = "fixture")]
            self.complete_calibration_operation(
                "nim.updateNickInTeam",
                &receipt,
                json!({
                    "action": "rename",
                    "groupId": group_id,
                    "userId": requested_user_id,
                    "cardName": nickname,
                    "verification": receipt.verification,
                }),
            );
            Ok(receipt)
        } else if let Some(error) = http_error {
            Err(error.with_detail(
                result
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .unwrap_or("NIM 群名片回退失败"),
            ))
        } else {
            Err(AppError::new(
                "rename_failed",
                result
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .unwrap_or("NIM 群名片修改失败"),
            ))
        }
    }
    async fn remove_member(&self, group_id: i64, user_id: i64) -> AppResult<GatewayReceipt> {
        self.require_capability(
            self.capabilities().remove_member,
            "removeMember",
            "移出成员",
        )?;
        require_positive("groupId", group_id)?;
        require_positive("userId", user_id)?;
        self.writable_member(group_id, user_id).await?;
        let receipt = self
            .action(
                MEMBER_REMOVE_ROUTE,
                json!({"groupId":group_id,"groupMemberIds":[user_id]}),
            )
            .await?;
        self.verify_removed_member(group_id, user_id, receipt).await
    }
    async fn set_group_mute(&self, group_id: i64, muted: bool) -> AppResult<GatewayReceipt> {
        self.require_capability(self.capabilities().group_mute, "groupMute", "全群发言控制")?;
        require_positive("groupId", group_id)?;
        self.manager_roster(group_id).await?;
        let mut receipt = self
            .action(
                GROUP_MUTE_ROUTE,
                json!({"groupId":group_id,"muteMode":if muted {"MUTE_MEMBER"} else {"MUTE_NO"}}),
            )
            .await?;
        *self.group_cache.write().await = None;
        let expected = if muted { "MUTE_MEMBER" } else { "MUTE_NO" };
        match self
            .group_infos()
            .await
            .ok()
            .and_then(|groups| groups.into_iter().find(|group| group.group_id == group_id))
            .map(|group| group.mute_mode)
        {
            Some(mode) if !mode.is_empty() && mode.eq_ignore_ascii_case(expected) => {
                receipt.verification = Some("verified".into());
            }
            Some(_) | None => {
                receipt.status = "unknown".into();
                receipt.verification = Some("unknown".into());
            }
        }
        #[cfg(feature = "fixture")]
        self.complete_calibration_operation(
            GROUP_MUTE_ROUTE,
            &receipt,
            json!({
                "action": if muted { "group_mute" } else { "group_unmute" },
                "groupId": group_id,
                "muted": muted,
                "verification": receipt.verification,
            }),
        );
        Ok(receipt)
    }

    async fn get_group_announcement(&self, group_id: i64) -> AppResult<Option<GroupAnnouncement>> {
        require_positive("groupId", group_id)?;
        let data = self
            .xclient(
                GROUP_NOTICE_LIST_ROUTE,
                json!({"groupId": group_id, "v": "0"}),
            )
            .await?;
        let items = data
            .get("noticeInfoList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let sender = *self.sender_id.read().await;
        let owned = if sender > 0 {
            items
                .iter()
                .filter(|value| int_field(value, &["userId", "authorUserId"]) == sender)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let selected = if owned.len() == 1 {
            Some(owned[0])
        } else if items.len() == 1 {
            Some(&items[0])
        } else if items.is_empty() {
            None
        } else {
            return Err(AppError::new(
                "notice_ambiguous",
                "群公告存在多个候选，无法安全确定目标公告",
            ));
        };
        Ok(selected.map(|value| GroupAnnouncement {
            group_id,
            notice_id: text_field(value, &["noticeId", "id"]),
            content: human_name_field(value, &["noticeContent", "content"]),
            mode: text_field(value, &["noticeMode", "mode"]),
            author_user_id: int_field(value, &["userId", "authorUserId"]),
        }))
    }

    async fn set_group_announcement(&self, group_id: i64, text: &str) -> AppResult<GatewayReceipt> {
        self.require_capability(self.capabilities().announcement, "announcement", "群公告")?;
        require_positive("groupId", group_id)?;
        require_non_empty("noticeContent", text)?;
        self.manager_roster(group_id).await?;
        let sender = if *self.sender_id.read().await > 0 {
            *self.sender_id.read().await
        } else {
            self.session_identity().await?.0
        };

        // 旺商聊只允许公告作者走 notice-opt；其他情况下页面会创建一条新公告。
        let current = self
            .xclient(
                GROUP_NOTICE_LIST_ROUTE,
                json!({"groupId": group_id, "v": "0"}),
            )
            .await?;
        let notices = current
            .get("noticeInfoList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let own_notices = notices
            .iter()
            .filter(|value| int_field(value, &["userId", "authorUserId"]) == sender)
            .collect::<Vec<_>>();
        if own_notices.len() > 1 {
            return Err(AppError::new(
                "notice_ambiguous",
                "当前账号存在多个群公告，拒绝更新不确定的公告",
            ));
        }
        if let Some(existing) = own_notices.first() {
            let notice_id = text_field(existing, &["noticeId", "id"]);
            let content = human_name_field(existing, &["noticeContent", "content"]);
            if !notice_id.is_empty() && content == text {
                let mut receipt = GatewayReceipt::succeeded(GROUP_NOTICE_UPDATE_ROUTE);
                receipt.request_id = notice_id;
                receipt.business_message = "群公告内容已经保存，本次未重复广播".into();
                receipt.verification = Some("not-applicable".into());
                return Ok(receipt);
            }
        }
        let own_notice_id = own_notices
            .first()
            .map(|value| text_field(value, &["noticeId", "id"]));
        let (route, payload, notice_id) =
            if let Some(notice_id) = own_notice_id.filter(|value| !value.is_empty()) {
                (
                    GROUP_NOTICE_UPDATE_ROUTE,
                    json!({
                        "groupId": group_id,
                        "noticeId": notice_id,
                        "noticeContent": text,
                        "noticeMode": "COMMON_NOTICE"
                    }),
                    notice_id,
                )
            } else {
                (
                    GROUP_NOTICE_ADD_ROUTE,
                    json!({
                        "groupId": group_id,
                        "noticeContent": text,
                        "noticeMode": "COMMON_NOTICE"
                    }),
                    String::new(),
                )
            };
        let data = self.xclient(route, payload).await?;
        let notice_id = if notice_id.is_empty() {
            text_field(&data, &["noticeId", "id"])
        } else {
            notice_id
        };
        if notice_id.is_empty() {
            return Err(AppError::new(
                "notice_receipt_missing",
                "群公告保存成功但未返回公告 ID",
            ));
        }
        let confirmed = self
            .get_group_announcement(group_id)
            .await?
            .ok_or_else(|| AppError::new("notice_verify", "群公告保存后读取不到公告"))?;
        if confirmed.notice_id != notice_id || confirmed.content != text {
            return Err(AppError::new(
                "notice_verify",
                "群公告保存后的公告 ID 或内容与请求不一致",
            ));
        }

        let broadcast = async {
            let cloud = self.resolve_cloud_id(group_id).await?;
            let group_name = self
                .group_infos()
                .await?
                .into_iter()
                .find(|group| group.group_id == group_id)
                .map(|group| group.name)
                .unwrap_or_default();
            let notice_payload = json!({
                "msgFormat": 8,
                "msgSession": 2,
                "from": {"id": sender, "name": "", "avatar": ""},
                "to": {"id": group_id, "groupCloudId": cloud, "groupName": group_name},
                "groupNotice": {"content": text, "noticeId": notice_id}
            });
            let encoded = self
                .cdp
                .evaluate(&ipc_expression(
                    "encode",
                    "/v1/plugins/encode-msg",
                    notice_payload.clone(),
                ))
                .await?;
            #[cfg(feature = "fixture")]
            if let Ok(mut recorder) = self.developer_calibration.lock() {
                recorder.record_ipc("/v1/plugins/encode-msg", "encode", notice_payload, &encoded);
            }
            let content = decode_transport_response("/v1/plugins/encode-msg", &encoded)?;
            if content.is_empty() {
                return Err(AppError::new("encode_failed", "群公告消息编码失败"));
            }
            let delivery = self
                .cdp
                .evaluate(&nim_send_expression(&cloud, content))
                .await?;
            #[cfg(feature = "fixture")]
            self.record_calibration_nim(
                "nim.sendCustomMsg",
                json!({"target": cloud, "content": content, "noticeId": notice_id}),
                &delivery,
            );
            Ok::<Value, AppError>(delivery)
        }
        .await;
        let delivery = match broadcast {
            Ok(delivery) => delivery,
            Err(error) => {
                let receipt = announcement_delivery_unknown(route, &notice_id, &error.message);
                #[cfg(feature = "fixture")]
                self.complete_calibration_operation(
                    route,
                    &receipt,
                    json!({
                        "action": "group_announcement",
                        "groupId": group_id,
                        "noticeId": notice_id,
                        "content": text,
                        "verification": "unknown",
                    }),
                );
                return Ok(receipt);
            }
        };
        if delivery.get("ok").and_then(Value::as_bool) != Some(true) {
            let receipt = announcement_delivery_unknown(
                route,
                &notice_id,
                delivery
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .unwrap_or("群公告消息投递失败"),
            );
            #[cfg(feature = "fixture")]
            self.complete_calibration_operation(
                route,
                &receipt,
                json!({
                    "action": "group_announcement",
                    "groupId": group_id,
                    "noticeId": notice_id,
                    "content": text,
                    "verification": "unknown",
                }),
            );
            return Ok(receipt);
        }
        let mut receipt = GatewayReceipt::succeeded(route);
        receipt.message_id = match delivery_message_id(&delivery) {
            Ok(message_id) => message_id,
            Err(error) => {
                let receipt = announcement_delivery_unknown(route, &notice_id, &error.message);
                #[cfg(feature = "fixture")]
                self.complete_calibration_operation(
                    route,
                    &receipt,
                    json!({
                        "action": "group_announcement",
                        "groupId": group_id,
                        "noticeId": notice_id,
                        "content": text,
                        "verification": "unknown",
                    }),
                );
                return Ok(receipt);
            }
        };
        receipt.request_id = receipt_identifier(&delivery, &["requestId", "idClient", "traceId"]);
        receipt.verification = Some("verified".into());
        #[cfg(feature = "fixture")]
        self.complete_calibration_operation(
            route,
            &receipt,
            json!({
                "action": "group_announcement",
                "groupId": group_id,
                "noticeId": notice_id,
                "content": text,
                "messageId": receipt.message_id,
                "verification": "verified",
            }),
        );
        Ok(receipt)
    }
}

fn announcement_delivery_unknown(route: &str, notice_id: &str, detail: &str) -> GatewayReceipt {
    let mut receipt = GatewayReceipt::succeeded(route);
    receipt.status = "unknown".into();
    receipt.request_id = notice_id.into();
    receipt.business_message =
        format!("群公告已保存，但广播结果未知：{detail}；相同内容不会重复广播");
    receipt.verification = Some("unknown".into());
    receipt
}

fn delivery_message_id(delivery: &Value) -> AppResult<String> {
    let message_id = delivery
        .get("idServer")
        .or_else(|| delivery.get("idClient"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if message_id.is_empty() {
        return Err(
            AppError::new("send_receipt_missing", "NIM 投递成功但未返回消息 ID").with_gateway(
                GatewayErrorMetadata::new("nim.sendCustomMsg", GatewayErrorLayer::Delivery),
            ),
        );
    }
    Ok(message_id)
}

#[derive(Debug, Clone)]
struct GroupInfo {
    group_id: i64,
    cloud_id: String,
    name: String,
    owner_user_id: i64,
    member_count: usize,
    #[allow(dead_code)]
    relation: String,
    mute_mode: String,
}

type GroupCache = Option<(Instant, Vec<GroupInfo>)>;

fn ipc_expression(kind: &str, route: &str, payload: Value) -> String {
    let input = json!({"type":kind,"route":route,"payload":payload});
    format!(
        r#"(async()=>{{const input={input};const ipc=globalThis.__dhIpc||(typeof require==="function"?require("electron").ipcRenderer:null);if(!ipc)return{{transportCode:503,errno:1,error:"Electron IPC unavailable",requestId:""}};return new Promise(resolve=>{{const channel="dh-rust-"+Date.now()+"-"+Math.random();let settled=false;let handler;const cleanup=()=>{{if(ipc.removeListener&&handler)ipc.removeListener(channel,handler);}};const finish=value=>{{if(settled)return;settled=true;cleanup();resolve(value);}};const timer=setTimeout(()=>finish({{transportCode:504,errno:1,error:"IPC timeout",requestId:channel,timedOut:true}}),15000);handler=(event,value)=>{{clearTimeout(timer);finish({{transportCode:value&&value.code,errno:value&&value.errno,response:value&&value.response,error:value&&value.message,requestId:value&&value.requestId||channel}});}};ipc.once(channel,handler);if(input.type==="request")ipc.send("xclient",{{type:"request",requestId:channel,url:input.route,excuteType:0,params:JSON.stringify(input.payload),key:channel}});else ipc.send("xclient",{{type:"encode",params:JSON.stringify(input.payload),key:channel}});}});}})()"#
    )
}

fn nim_send_expression(target: &str, content: &str) -> String {
    let input = json!({"target":target,"content":content});
    format!(
        r#"(async()=>{{const input={input};return new Promise(resolve=>{{let settled=false;const finish=value=>{{if(settled)return;settled=true;resolve(value);}};if(!window.nim){{finish({{ok:false,errorMessage:"NIM unavailable",errorCode:503}});return;}}const timer=setTimeout(()=>finish({{ok:false,errorMessage:"NIM callback timeout",errorCode:504,timedOut:true}}),15000);window.nim.sendCustomMsg({{scene:"team",to:input.target,content:input.content,isLocal:false,done:(error,message)=>{{clearTimeout(timer);finish({{ok:!error,errorMessage:error&&(error.message||String(error)),errorCode:Number(error&&(error.code||error.status)||0),timedOut:Boolean(error&&error.timedOut),idClient:message&&message.idClient,idServer:message&&message.idServer}});}}}});}});}})()"#
    )
}

fn nim_team_members_expression(team_id: &str, cursor: Option<&str>) -> String {
    let input = json!({"teamId":team_id,"cursor":cursor.unwrap_or_default()});
    format!(
        r#"(async()=>{{const input={input};return new Promise(resolve=>{{let settled=false;const finish=value=>{{if(settled)return;settled=true;resolve(value);}};if(!window.nim){{finish({{ok:false,errorMessage:"NIM unavailable",errorCode:503,members:[]}});return;}}const timer=setTimeout(()=>finish({{ok:false,errorMessage:"NIM callback timeout",errorCode:504,members:[],timedOut:true}}),15000);const options={{teamId:input.teamId,done:(error,value)=>{{clearTimeout(timer);const object=value&&typeof value==="object"&&!Array.isArray(value)?value:null;const source=Array.isArray(value)?value:object&&Array.isArray(object.members)?object.members:[];const nextCursor=object&&(object.nextCursor||object.nextPageToken||object.cursor)||"";finish({{ok:!error,errorMessage:error&&(error.message||String(error)),errorCode:Number(error&&(error.code||error.status)||0),timedOut:Boolean(error&&error.timedOut),nextCursor:String(nextCursor),members:source.map(item=>({{nimId:String(item.account||item.accid||""),cardName:item.nickInTeam||item.nick||"",type:item.type||item.memberType||"normal"}}))}});}}}};if(input.cursor)options.cursor=input.cursor;window.nim.getTeamMembers(options);}});}})()"#
    )
}

fn next_cursor(value: &Value) -> Option<String> {
    [
        value.get("nextCursor"),
        value.get("next_cursor"),
        value.get("nextPageToken"),
        value.pointer("/pageInfo/nextCursor"),
        value.pointer("/pageInfo/nextPageToken"),
    ]
    .into_iter()
    .flatten()
    .find_map(|value| {
        value
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
    })
    .map(ToOwned::to_owned)
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

fn require_object_payload(route: &str, payload: &Value) -> AppResult<()> {
    if payload.is_object() {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_gateway_params",
            format!("{route} 参数必须是 JSON 对象"),
        ))
    }
}

fn require_positive(name: &str, value: i64) -> AppResult<()> {
    if value > 0 {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_argument",
            format!("{name} 必须大于 0"),
        ))
    }
}

fn require_non_empty(name: &str, value: &str) -> AppResult<()> {
    if value.trim().is_empty() {
        Err(AppError::new(
            "invalid_argument",
            format!("{name} 不能为空"),
        ))
    } else {
        Ok(())
    }
}

fn gateway_metadata(
    route: &str,
    layer: GatewayErrorLayer,
    transport_code: Option<i64>,
    transport_errno: Option<i64>,
    business_code: Option<i64>,
    business_errno: Option<i64>,
) -> GatewayErrorMetadata {
    let kind = match layer {
        GatewayErrorLayer::Transport => GatewayErrorKind::Transport,
        GatewayErrorLayer::Response => GatewayErrorKind::Decode,
        GatewayErrorLayer::Delivery => GatewayErrorKind::Business,
        GatewayErrorLayer::Business => match business_code.or(business_errno) {
            Some(401 | 403) => GatewayErrorKind::Permission,
            Some(404 | 1004) => GatewayErrorKind::NotFound,
            Some(405 | 501) => GatewayErrorKind::Unsupported,
            Some(_) => GatewayErrorKind::Business,
            None => GatewayErrorKind::Unknown,
        },
    };
    GatewayErrorMetadata {
        route: route.to_string(),
        layer,
        kind,
        transport_code,
        transport_errno,
        business_code,
        business_errno,
    }
}

fn decode_transport_response<'a>(route: &str, value: &'a Value) -> AppResult<&'a str> {
    let transport_code = value.get("transportCode").and_then(Value::as_i64);
    let transport_errno = value.get("errno").and_then(Value::as_i64);
    let transport_metadata = || {
        gateway_metadata(
            route,
            GatewayErrorLayer::Transport,
            transport_code,
            transport_errno,
            None,
            None,
        )
    };
    let Some(code) = transport_code else {
        return Err(
            AppError::new("gateway_response", "网关响应缺少 transportCode").with_gateway(
                gateway_metadata(
                    route,
                    GatewayErrorLayer::Response,
                    None,
                    transport_errno,
                    None,
                    None,
                ),
            ),
        );
    };
    let Some(errno) = transport_errno else {
        return Err(
            AppError::new("gateway_response", "网关响应缺少 transport errno").with_gateway(
                gateway_metadata(
                    route,
                    GatewayErrorLayer::Response,
                    Some(code),
                    None,
                    None,
                    None,
                ),
            ),
        );
    };
    if code != 200 || errno != 0 {
        let message = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("旺商聊传输请求失败");
        let error_message = if message.eq_ignore_ascii_case("ipc timeout") {
            format!("IPC timeout: {route} after 15000ms")
        } else {
            message.to_string()
        };
        let error =
            AppError::new("gateway_transport", error_message).with_gateway(transport_metadata());
        return Err(if code == 429 || code >= 500 {
            error.retryable()
        } else {
            error
        });
    }
    value
        .get("response")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AppError::new("gateway_response", "网关成功响应缺少字符串 response").with_gateway(
                gateway_metadata(
                    route,
                    GatewayErrorLayer::Response,
                    Some(code),
                    Some(errno),
                    None,
                    None,
                ),
            )
        })
}

fn decode_business_response(route: &str, response: &str) -> AppResult<Value> {
    let envelope: Value = serde_json::from_str(response).map_err(|error| {
        AppError::new(
            "gateway_response",
            format!("解析旺商聊业务响应失败：{error}"),
        )
        .with_gateway(gateway_metadata(
            route,
            GatewayErrorLayer::Response,
            Some(200),
            Some(0),
            None,
            None,
        ))
    })?;
    let business_code = envelope.get("code").and_then(Value::as_i64);
    let business_errno = envelope.get("errno").and_then(Value::as_i64);
    let metadata = || {
        gateway_metadata(
            route,
            GatewayErrorLayer::Business,
            Some(200),
            Some(0),
            business_code,
            business_errno,
        )
    };
    let Some(code) = business_code else {
        return Err(AppError::new("gateway_response", "业务响应缺少 code").with_gateway(metadata()));
    };
    // 旺商聊 2.7.7 的成功业务 envelope 只有 code/data/msg；旧版本还会返回 errno。
    // errno 出现时继续严格校验，缺省时仅在 code=0 的情况下按 0 处理。
    let errno = business_errno.unwrap_or(0);
    if code != 0 || errno != 0 {
        let message = envelope
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("旺商聊业务请求失败");
        let mut gateway = metadata();
        if message.to_ascii_lowercase().contains("nim")
            && (message.contains("未就绪") || message.contains("尚未初始化"))
        {
            gateway.kind = GatewayErrorKind::NimNotReady;
        }
        let error = AppError::new("gateway_business", message).with_gateway(gateway);
        return Err(
            if matches!(code, 429 | 502 | 503 | 504) || matches!(errno, 429 | 502 | 503 | 504) {
                error.retryable()
            } else {
                error
            },
        );
    }
    Ok(envelope.get("data").cloned().unwrap_or(Value::Null))
}

fn nim_response_error(route: &str, value: &Value, message: &str) -> AppError {
    let code = int_field(value, &["errorCode", "status", "code"]);
    let mut metadata = gateway_metadata(
        route,
        GatewayErrorLayer::Business,
        Some(200),
        Some(0),
        (code != 0).then_some(code),
        None,
    );
    if code == 503 || message.to_ascii_lowercase().contains("nim unavailable") {
        metadata.kind = GatewayErrorKind::NimNotReady;
    }
    let error =
        AppError::new("nim_members", format!("NIM 成员请求失败：{message}")).with_gateway(metadata);
    if matches!(code, 429 | 502 | 503 | 504)
        || value.get("timedOut").and_then(Value::as_bool) == Some(true)
    {
        error.retryable()
    } else {
        error
    }
}

fn is_member_rate_limited(error: &AppError) -> bool {
    if error.code == "gateway_rate_limited" {
        return true;
    }
    if error.gateway.as_deref().is_some_and(|metadata| {
        metadata.transport_code == Some(429)
            || metadata.transport_errno == Some(429)
            || metadata.business_code == Some(429)
            || metadata.business_errno == Some(429)
    }) {
        return true;
    }
    let text = format!("{} {}", error.code, error.message).to_ascii_lowercase();
    text.contains("too frequent")
        || text.contains("rate limit")
        || text.contains("network busy")
        || text.contains("频繁")
        || text.contains("网络繁忙")
        || text.contains("稍后重试")
}

fn nim_update_nick_expression(team_id: &str, nim_id: &str, nickname: &str) -> String {
    let input = json!({"teamId":team_id,"account":nim_id,"nickInTeam":nickname});
    format!(
        r#"(async()=>{{const input={input};return new Promise(resolve=>{{let settled=false;const finish=value=>{{if(settled)return;settled=true;resolve(value);}};if(!window.nim){{finish({{ok:false,errorMessage:"NIM unavailable"}});return;}}const timer=setTimeout(()=>finish({{ok:false,errorMessage:"NIM callback timeout",timedOut:true}}),15000);window.nim.updateNickInTeam({{teamId:input.teamId,account:input.account,nickInTeam:input.nickInTeam,done:(error,value)=>{{clearTimeout(timer);finish({{ok:!error,errorMessage:error&&(error.message||String(error)),member:value||null}});}}}});}});}})()"#
    )
}

fn member_backoff_seconds(level: u32) -> u64 {
    12_u64
        .checked_mul(2_u64.saturating_pow(level.saturating_sub(1)))
        .unwrap_or(120)
        .min(120)
}

fn rate_limited_error(mut error: AppError) -> AppError {
    error.code = "gateway_rate_limited".into();
    error.message = "旺商聊成员请求过于频繁，请稍后重试".into();
    error.retryable = true;
    error
}

fn decode_action_receipt(route: &str, value: &Value) -> AppResult<GatewayReceipt> {
    let response = decode_transport_response(route, value)?;
    let envelope: Value = serde_json::from_str(response).map_err(|error| {
        AppError::new(
            "gateway_response",
            format!("解析旺商聊业务响应失败：{error}"),
        )
        .with_gateway(gateway_metadata(
            route,
            GatewayErrorLayer::Response,
            Some(200),
            Some(0),
            None,
            None,
        ))
    })?;
    // Reuse the strict double-envelope validation before archiving any success.
    let data = decode_business_response(route, response)?;
    let request_id = receipt_identifier(value, &["requestId", "requestID", "traceId"])
        .or_else_non_empty(|| receipt_identifier(&envelope, &["requestId", "requestID", "traceId"]))
        .or_else_non_empty(|| receipt_identifier(&data, &["requestId", "requestID", "traceId"]));
    let message_id = receipt_identifier(
        &data,
        &["messageId", "msgId", "idServer", "serverMessageId"],
    );
    Ok(GatewayReceipt {
        route: route.into(),
        status: "succeeded".into(),
        transport_code: value.get("transportCode").and_then(Value::as_i64),
        transport_errno: value.get("errno").and_then(Value::as_i64),
        business_code: envelope.get("code").and_then(Value::as_i64),
        business_errno: envelope.get("errno").and_then(Value::as_i64),
        business_message: envelope
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("OK")
            .into(),
        request_id,
        message_id,
        session: String::new(),
        acknowledged_through: 0,
        acknowledged: 0,
        remaining: 0,
        dropped: 0,
        verification: Some("not-applicable".into()),
    })
}

trait NonEmptyStringExt {
    fn or_else_non_empty(self, fallback: impl FnOnce() -> String) -> String;
}

impl NonEmptyStringExt for String {
    fn or_else_non_empty(self, fallback: impl FnOnce() -> String) -> String {
        if self.is_empty() {
            fallback()
        } else {
            self
        }
    }
}

fn receipt_identifier(value: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| {
            value.get(*key).and_then(|field| match field {
                Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            })
        })
        .unwrap_or_default()
}

fn rename_fallback_allowed(error: &AppError) -> bool {
    if matches!(
        error.code.as_str(),
        "member_identity" | "member_not_found" | "group_not_found" | "account_identity"
    ) {
        return true;
    }
    let metadata_match = error.gateway.as_ref().is_some_and(|metadata| {
        metadata.business_code == Some(404)
            || metadata.business_errno == Some(404)
            || metadata.business_errno == Some(1004)
    });
    let message = error.message.to_ascii_lowercase();
    metadata_match
        || message.contains("not found")
        || message.contains("identity")
        || message.contains("不存在")
        || message.contains("找不到")
        || message.contains("身份")
}

fn text_field(value: &Value, names: &[&str]) -> String {
    for name in names {
        if let Some(value) = value.get(*name) {
            if let Some(text) = value.as_str() {
                if !text.trim().is_empty() {
                    return text.to_string();
                }
            }
            if let Some(number) = value.as_i64() {
                return number.to_string();
            }
        }
    }
    String::new()
}

fn human_name_field(value: &Value, names: &[&str]) -> String {
    names
        .iter()
        .filter_map(|name| value.get(*name).and_then(Value::as_str))
        .find(|value| !value.trim().is_empty())
        .unwrap_or_default()
        .to_string()
}
fn int_field(value: &Value, names: &[&str]) -> i64 {
    for name in names {
        if let Some(value) = value.get(*name) {
            if let Some(number) = value.as_i64() {
                return number;
            }
            if let Some(text) = value.as_str() {
                if let Ok(number) = text.trim().parse() {
                    return number;
                }
            }
        }
    }
    0
}
fn member_role(value: &str) -> String {
    match value.trim().to_ascii_uppercase().as_str() {
        "1" | "OWNER" | "MASTER" | "GROUP_OWNER" | "GROUP_ROLE_OWNER" | "MSG_MASTER" => "owner",
        "2" | "ADMIN" | "MANAGER" | "ADMINISTRATOR" | "GROUP_ADMIN" | "GROUP_ROLE_ADMIN"
        | "MSG_ADMIN" => "admin",
        _ => "member",
    }
    .into()
}
fn wire_name_missing(value: &str) -> bool {
    matches!(value.trim(), "" | "1" | ".")
}
fn synthetic_nim_user_id(value: &str) -> i64 {
    let mut hash: u64 = 14695981039346656037;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1099511628211);
    }
    -((hash & 0x3fff_ffff_ffff_ffff) as i64) - 1
}

fn legacy_gateway_batch(value: Value) -> AppResult<GatewayBatch> {
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(AppError::new(
            "listener_read",
            value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("消息监听器读取失败"),
        )
        .retryable());
    }
    let session = value
        .get("session")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("legacy")
        .to_string();
    let messages = value
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if messages.len() > MAX_GATEWAY_BATCH {
        return Err(AppError::new(
            "listener_batch_limit",
            "网关批次超过 100 条上限",
        ));
    }
    let mut records = messages
        .into_iter()
        .enumerate()
        .map(|(index, payload)| GatewayRecord {
            session: session.clone(),
            sequence: payload
                .get("seq")
                .and_then(Value::as_u64)
                .unwrap_or(index as u64 + 1),
            kind: GatewayRecordKind::Message,
            source: payload
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("legacy")
                .to_string(),
            payload,
        })
        .collect::<Vec<_>>();
    records.sort_by_key(|record| record.sequence);
    Ok(GatewayBatch {
        session,
        records,
        remaining: value.get("remaining").and_then(Value::as_u64).unwrap_or(0) as usize,
        dropped: value.get("dropped").and_then(Value::as_u64).unwrap_or(0),
    })
}

fn parse_gateway_batch(value: Value) -> AppResult<GatewayBatch> {
    if value.get("records").is_none() {
        return legacy_gateway_batch(value);
    }
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(AppError::new(
            "listener_read",
            value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("消息监听器读取失败"),
        )
        .retryable());
    }
    let mut batch: GatewayBatch = serde_json::from_value(json!({
        "session": value.get("session").cloned().unwrap_or(Value::Null),
        "records": value.get("records").cloned().unwrap_or(Value::Null),
        "remaining": value.get("remaining").cloned().unwrap_or(Value::Null),
        "dropped": value.get("dropped").cloned().unwrap_or(Value::Null),
    }))
    .map_err(|error| AppError::new("listener_response", error.to_string()))?;
    if batch.session.is_empty() {
        return Err(AppError::new("listener_response", "监听器响应缺少会话标识"));
    }
    if batch.records.len() > MAX_GATEWAY_BATCH {
        return Err(AppError::new(
            "listener_batch_limit",
            "网关批次超过 100 条上限",
        ));
    }
    if batch
        .records
        .iter()
        .any(|record| record.session != batch.session || record.sequence == 0)
    {
        return Err(AppError::new(
            "listener_response",
            "监听器记录会话或序号无效",
        ));
    }
    batch.records.sort_by_key(|record| record.sequence);
    if batch
        .records
        .windows(2)
        .any(|pair| pair[0].sequence == pair[1].sequence)
    {
        return Err(AppError::new("listener_response", "监听器记录序号重复"));
    }
    Ok(batch)
}

fn legacy_gateway_receipt(session: &str, sequence: u64, value: Value) -> AppResult<GatewayReceipt> {
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(AppError::new(
            "listener_ack",
            value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("消息确认失败"),
        ));
    }
    Ok(GatewayReceipt {
        route: "listener.ack".into(),
        status: "succeeded".into(),
        transport_code: None,
        transport_errno: None,
        business_code: None,
        business_errno: None,
        business_message: "OK".into(),
        request_id: String::new(),
        message_id: String::new(),
        session: value
            .get("session")
            .and_then(Value::as_str)
            .unwrap_or(session)
            .to_string(),
        acknowledged_through: value
            .get("acknowledgedThrough")
            .and_then(Value::as_u64)
            .unwrap_or(sequence),
        acknowledged: value.get("acked").and_then(Value::as_u64).unwrap_or(0) as usize,
        remaining: value.get("remaining").and_then(Value::as_u64).unwrap_or(0) as usize,
        dropped: value.get("dropped").and_then(Value::as_u64).unwrap_or(0),
        verification: Some("not-applicable".into()),
    })
}

fn parse_gateway_receipt(session: &str, sequence: u64, value: Value) -> AppResult<GatewayReceipt> {
    let response_session = value
        .get("session")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::new("listener_ack_protocol", "监听 ACK 缺少 session"))?;
    let acknowledged_through = value
        .get("acknowledgedThrough")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            AppError::new("listener_ack_protocol", "监听 ACK 缺少 acknowledgedThrough")
        })?;
    let dropped = value.get("dropped").and_then(Value::as_u64).unwrap_or(0);
    let receipt = legacy_gateway_receipt(session, sequence, value)?;
    if response_session != session || acknowledged_through != sequence || dropped > 0 {
        return Err(AppError::new(
            "listener_ack_mismatch",
            "消息确认收据与请求不匹配",
        ));
    }
    Ok(receipt)
}

fn legacy_message_value(record: &GatewayRecord) -> Value {
    let mut value = record.payload.clone();
    if let Some(object) = value.as_object_mut() {
        object.insert("seq".into(), Value::from(record.sequence));
        object.insert(
            "listenerSession".into(),
            Value::from(record.session.clone()),
        );
    }
    value
}

fn ack_expression(session: &str, sequence: u64) -> String {
    let input = json!({"session":session,"sequence":sequence});
    format!(
        r#"(()=>{{const input={input};const state=window.__dhBridgeMessages;if(!state)return{{ok:false,error:"LISTENER_NOT_READY",acked:0}};if(state.session!==input.session)return{{ok:false,error:"LISTENER_SESSION_MISMATCH",session:state.session,acked:0,remaining:state.queue.length,dropped:state.dropped||0}};const before=state.queue.length;state.queue=state.queue.filter(item=>Number(item.seq)>{sequence});return{{ok:true,session:state.session,acknowledgedThrough:input.sequence,acked:before-state.queue.length,remaining:state.queue.length,dropped:state.dropped||0}};}})()"#
    )
}

const LISTENER_EXPRESSION: &str = r#"(()=>{const nim=window.nim;if(!nim)return{ok:false,error:"NIM_NOT_READY",queued:0};if(!window.__dhBridgeMessages||window.__dhBridgeMessages.version!==4){const session=globalThis.crypto&&typeof globalThis.crypto.randomUUID==="function"?globalThis.crypto.randomUUID():Date.now().toString(36)+Math.random().toString(36).slice(2);const state={version:4,session,queue:[],seen:new Set(),seenOrder:[],installed:[],queueLimit:5000,seenLimit:10000,nextSeq:1,dropped:0};const remember=key=>{if(state.seen.has(key))return false;state.seen.add(key);state.seenOrder.push(key);while(state.seenOrder.length>state.seenLimit){state.seen.delete(state.seenOrder.shift());}return true;};const enqueue=(kind,source,payload,key)=>{if(!remember(kind+":"+key))return;state.queue.push({session:state.session,seq:state.nextSeq++,kind,source,payload});if(state.queue.length>state.queueLimit){const overflow=state.queue.length-state.queueLimit;state.queue.splice(0,overflow);state.dropped+=overflow;}};const collectMessage=(value,source)=>{if(Array.isArray(value)){value.forEach(item=>collectMessage(item,source));return;}if(!value||typeof value!=="object")return;const id=String(value.idClient||value.idServer||[value.time||Date.now(),value.from||"",value.to||""].join("-"));enqueue("message",source,{idClient:value.idClient,idServer:value.idServer,scene:value.scene,from:value.from,to:value.to,time:value.time,type:value.type,flow:value.flow,content:value.content,attach:value.attach,custom:value.custom,msgFormat:value.msgFormat,mentions:value.mentions||value.aite,quote:value.quote,fromNick:typeof value.fromNick==="string"?value.fromNick:"",sessionId:value.sessionId},id);};const collectTeamEvent=(args,source,kind)=>{const root=args[0];if(!root)return;const team=root.team&&typeof root.team==="object"?root.team:root;const teamId=String(team.teamId||team.id||root.teamId||"");const raw=root.members||root.accounts||root.teamMembers||args[1]||[];const list=Array.isArray(raw)?raw:[raw];const members=list.map(item=>{if(typeof item==="string")return{nimId:item,nickname:"",type:"member"};if(!item||typeof item!=="object")return null;const nimId=String(item.account||item.accid||item.nimId||"");if(!nimId)return null;const name=item.nickInTeam??item.nick??item.nickname??"";return{nimId,userId:item.userId,nickname:typeof name==="string"?name:"",type:item.type||item.memberType||"member"};}).filter(Boolean);if(!teamId||members.length===0)return;const key=[teamId,...members.map(item=>item.nimId).sort(),team.updateTime||root.time||Date.now()].join(":");enqueue(kind,source,{teamId,members,confirmed:true},key);};const install=(name,collector)=>{const original=nim.options&&nim.options[name];if(!nim.options)return;nim.options[name]=function(...args){try{collector(args,name);}catch{}if(typeof original==="function")return original.apply(this,args);};state.installed.push(name);};["onmsg","onmsgs","onofflinemsgs","onroamingmsgs"].forEach(name=>install(name,(args,source)=>collectMessage(args[0],source)));["onaddteammembers","onAddTeamMembers"].forEach(name=>install(name,(args,source)=>collectTeamEvent(args,source,"teamMemberJoined")));["onremoveteammembers","onRemoveTeamMembers"].forEach(name=>install(name,(args,source)=>collectTeamEvent(args,source,"teamMemberLeft")));["onupdateteammember","onUpdateTeamMember","onupdateteammembers","onUpdateTeamMembers"].forEach(name=>install(name,(args,source)=>collectTeamEvent(args,source,"teamMemberUpdated")));window.__dhBridgeMessages=state;}const state=window.__dhBridgeMessages;return{ok:true,session:state.session,installed:state.installed,queued:state.queue.length,dropped:state.dropped};})()"#;
const READ_BATCH_EXPRESSION: &str = r#"(async()=>{const state=window.__dhBridgeMessages;if(!state)return{ok:false,error:"LISTENER_NOT_READY",records:[]};const batch=state.queue.slice(0,100);let common=window.__dhBridgeCommon||null;try{if(!common){const main=Array.from(document.scripts).map(item=>item.src).find(name=>/\/main-[^/]+\.js(?:\?|$)/.test(name));if(main){const source=await(await fetch(main)).text();const match=source.match(/zh-cn-[a-z0-9]+\.js/);if(match){common=(await import(new URL(match[0],main).href)).D;window.__dhBridgeCommon=common;}}}}catch{}const records=[];for(const item of batch){let payload=item.payload;if(item.kind==="message"){let decoded=null,decodeError="";if(common&&payload.type==="custom"&&typeof payload.content==="string"){try{decoded=await common.decodeMsg(payload.content);if(decoded&&decoded.mentions===undefined&&decoded.aite!==undefined)decoded={...decoded,mentions:decoded.aite};}catch(error){decodeError=error&&error.message||String(error);}}payload={...payload,decoded,decodeError};}records.push({session:item.session,sequence:item.seq,kind:item.kind,source:item.source,payload});}return{ok:true,session:state.session,records,remaining:state.queue.length,dropped:state.dropped||0};})()"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_devtools_is_accepted() {
        assert!(CdpClient::new("http://127.0.0.1:9222").is_ok());
        assert!(CdpClient::new("https://example.com").is_err());
    }
    #[test]
    fn member_roles_are_normalized() {
        assert_eq!(member_role("MSG_ADMIN"), "admin");
        assert_eq!(member_role("owner"), "owner");
        assert_eq!(member_role("unknown"), "member");
    }

    #[test]
    fn member_rate_limit_errors_are_detected_and_labeled() {
        let error = AppError::new("gateway_business", "请求太频繁，请稍后重试");
        assert!(is_member_rate_limited(&error));
        let transport_429 = decode_transport_response(
            "/v1/group/get-group-members",
            &json!({"transportCode":429,"errno":0,"error":"busy"}),
        )
        .unwrap_err();
        assert!(transport_429.retryable);
        assert!(is_member_rate_limited(&transport_429));
        let business_429 = decode_business_response(
            "/v1/group/get-group-members",
            r#"{"code":429,"errno":429,"msg":"busy","data":null}"#,
        )
        .unwrap_err();
        assert!(business_429.retryable);
        assert!(is_member_rate_limited(&business_429));
        let labeled = rate_limited_error(error);
        assert_eq!(labeled.code, "gateway_rate_limited");
        assert!(labeled.retryable);
        assert!(labeled.message.contains("过于频繁"));
    }

    #[test]
    fn structured_gateway_5xx_errors_are_retryable_but_not_rate_limited() {
        for code in [502_i64, 503, 504] {
            let transport = decode_transport_response(
                "/v1/group/get-group-members",
                &json!({"transportCode":code,"errno":1,"error":"upstream unavailable"}),
            )
            .unwrap_err();
            assert!(transport.retryable);
            assert!(!is_member_rate_limited(&transport));

            let response = format!(
                r#"{{"code":{code},"errno":{code},"msg":"upstream unavailable","data":null}}"#
            );
            let business =
                decode_business_response("/v1/group/get-group-members", &response).unwrap_err();
            assert!(business.retryable);
            assert!(!is_member_rate_limited(&business));
        }
    }

    #[test]
    fn member_backoff_is_bounded_and_exponential() {
        assert_eq!(member_backoff_seconds(1), 12);
        assert_eq!(member_backoff_seconds(2), 24);
        assert_eq!(member_backoff_seconds(3), 48);
        assert_eq!(member_backoff_seconds(4), 96);
        assert_eq!(member_backoff_seconds(5), 120);
    }
    #[test]
    fn synthetic_nim_ids_are_stable_and_negative() {
        let one = synthetic_nim_user_id("abc");
        assert_eq!(one, synthetic_nim_user_id("abc"));
        assert!(one < 0);
    }

    #[test]
    fn devtools_targets_expose_an_explicit_page_type() {
        let page: DevToolsPage = serde_json::from_value(json!({
            "title": "旺商聊",
            "url": "http://127.0.0.1",
            "type": "page",
            "webSocketDebuggerUrl": "ws://127.0.0.1/devtools/page/1"
        }))
        .unwrap();
        assert_eq!(page.page_type, "page");
        assert!(is_wangshangliao_page(&page));
        let unrelated: DevToolsPage = serde_json::from_value(json!({
            "title": "Chrome DevTools",
            "url": "https://example.test/",
            "type": "page",
            "webSocketDebuggerUrl": "ws://127.0.0.1/devtools/page/2"
        }))
        .unwrap();
        assert!(!is_wangshangliao_page(&unrelated));
    }

    #[tokio::test]
    async fn unrelated_devtools_is_reported_as_other_service() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            let body = serde_json::json!([{
                "title": "Chrome DevTools",
                "url": "https://example.test/",
                "type": "page",
                "webSocketDebuggerUrl": "ws://127.0.0.1/devtools/page/chrome"
            }])
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let client = CdpClient::new(format!("http://{address}")).unwrap();
        let diagnostic = client.diagnose().await;
        server.await.unwrap();
        assert_eq!(diagnostic.status, ConnectionStatus::OtherService);
        assert!(diagnostic.detail.contains("9222"));
    }

    #[tokio::test]
    async fn devtools_pages_fall_back_to_json_endpoint() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 1024];
                let size = stream.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..size]);
                if index == 0 {
                    assert!(request.starts_with("GET /json/list "));
                    stream
                        .write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await
                        .unwrap();
                } else {
                    assert!(request.starts_with("GET /json "));
                    let body = serde_json::json!([{
                        "title": "旺商聊",
                        "url": "http://127.0.0.1",
                        "type": "page",
                        "webSocketDebuggerUrl": "ws://127.0.0.1/devtools/page/wang"
                    }])
                    .to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                }
            }
        });
        let client = CdpClient::new(format!("http://{address}")).unwrap();
        let pages = client.pages().await.unwrap();
        server.await.unwrap();
        assert_eq!(pages.len(), 1);
        assert!(is_wangshangliao_page(&pages[0]));
    }

    #[test]
    fn transport_and_business_layers_require_code_and_errno_success() {
        let transport_errno = json!({
            "transportCode": 200,
            "errno": 7,
            "response": "{\"code\":0,\"errno\":0,\"data\":{}}"
        });
        let error = decode_transport_response("/fixture", &transport_errno).unwrap_err();
        assert_eq!(error.code, "gateway_transport");
        assert_eq!(error.gateway.unwrap().transport_errno, Some(7));

        let business_errno = r#"{"code":0,"errno":9,"msg":"failed","data":{}}"#;
        let error = decode_business_response("/fixture", business_errno).unwrap_err();
        assert_eq!(error.code, "gateway_business");
        assert_eq!(error.gateway.unwrap().business_errno, Some(9));

        let success = decode_business_response(
            "/fixture",
            r#"{"code":0,"errno":0,"msg":"OK","data":{"id":1}}"#,
        )
        .unwrap();
        assert_eq!(success["id"], 1);

        let current_success =
            decode_business_response("/fixture", r#"{"code":0,"msg":"OK","data":{"id":2}}"#)
                .unwrap();
        assert_eq!(current_success["id"], 2);
    }

    #[test]
    fn action_receipt_preserves_both_envelopes_and_identifiers() {
        let receipt = decode_action_receipt(
            "/v1/group/message-rollback",
            &json!({
                "transportCode": 200,
                "errno": 0,
                "requestId": "dh-rust-request-7",
                "response": r#"{"code":0,"errno":0,"msg":"OK","data":{"messageId":"server-9"}}"#
            }),
        )
        .unwrap();
        assert_eq!(receipt.status, "succeeded");
        assert_eq!(receipt.transport_code, Some(200));
        assert_eq!(receipt.transport_errno, Some(0));
        assert_eq!(receipt.business_code, Some(0));
        assert_eq!(receipt.business_errno, Some(0));
        assert_eq!(receipt.request_id, "dh-rust-request-7");
        assert_eq!(receipt.message_id, "server-9");
    }

    #[test]
    fn failed_receipt_preserves_gateway_error_metadata() {
        let error = AppError::new("gateway_business", "权限不足").with_gateway(gateway_metadata(
            "/v1/group/set-member-mute",
            GatewayErrorLayer::Business,
            Some(200),
            Some(0),
            Some(403),
            Some(403),
        ));
        let receipt = GatewayReceipt::failed("mute", &error);
        assert_eq!(receipt.status, "failed");
        assert_eq!(receipt.route, "/v1/group/set-member-mute");
        assert_eq!(receipt.business_code, Some(403));
        assert_eq!(receipt.business_errno, Some(403));
        assert_eq!(receipt.business_message, "权限不足");
    }

    #[test]
    fn human_names_accept_strings_only() {
        let value = json!({"name": 123, "nickname": "Alice"});
        assert_eq!(human_name_field(&value, &["name"]), "");
        assert_eq!(human_name_field(&value, &["name", "nickname"]), "Alice");
        assert_eq!(text_field(&value, &["name"]), "123");
    }

    #[test]
    fn successful_delivery_requires_a_non_empty_message_id() {
        let error = delivery_message_id(&json!({"idServer":"", "idClient":""})).unwrap_err();
        assert_eq!(error.code, "send_receipt_missing");
        assert_eq!(error.gateway.unwrap().layer, GatewayErrorLayer::Delivery);
        assert_eq!(
            delivery_message_id(&json!({"idServer":"server-1"})).unwrap(),
            "server-1"
        );
    }

    #[test]
    fn rename_fallback_is_limited_to_identity_and_not_found_errors() {
        assert!(rename_fallback_allowed(&AppError::new(
            "member_not_found",
            "missing"
        )));
        assert!(rename_fallback_allowed(
            &AppError::new("gateway_business", "not found").with_gateway(gateway_metadata(
                MEMBER_RENAME_ROUTE,
                GatewayErrorLayer::Business,
                Some(200),
                Some(0),
                Some(404),
                Some(404),
            ))
        ));
        assert!(!rename_fallback_allowed(&AppError::new(
            "gateway_business",
            "permission denied"
        )));
    }

    #[test]
    fn gateway_batches_are_ordered_bounded_and_session_scoped() {
        let batch = parse_gateway_batch(json!({
            "ok": true,
            "session": "s1",
            "records": [
                {"session":"s1","sequence":2,"kind":"message","source":"onmsg","payload":{}},
                {"session":"s1","sequence":1,"kind":"teamMemberJoined","source":"onaddteammembers","payload":{}}
            ],
            "remaining": 2,
            "dropped": 0
        }))
        .unwrap();
        assert_eq!(
            batch
                .records
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );

        let mismatch = parse_gateway_batch(json!({
            "ok": true,
            "session": "s1",
            "records": [{"session":"s2","sequence":1,"kind":"message","source":"onmsg","payload":{}}],
            "remaining": 1,
            "dropped": 0
        }));
        assert!(mismatch.is_err());
    }

    #[test]
    fn listener_contract_has_fixed_limits_and_confirmed_join_hook() {
        assert!(LISTENER_EXPRESSION.contains("queueLimit:5000"));
        assert!(LISTENER_EXPRESSION.contains("seenLimit:10000"));
        assert!(LISTENER_EXPRESSION.contains("onaddteammembers"));
        assert!(LISTENER_EXPRESSION.contains("confirmed:true"));
        assert!(READ_BATCH_EXPRESSION.contains("state.queue.slice(0,100)"));
        assert!(!ipc_expression("request", "/fixture", json!({})).contains("input.payload||"));
    }
}
