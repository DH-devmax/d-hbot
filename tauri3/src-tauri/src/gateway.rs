use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock as SyncRwLock};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

use crate::contracts::{runtime_capabilities, unverified_production_capabilities};
use crate::error::{
    AppError, AppResult, GatewayErrorKind, GatewayErrorLayer, GatewayErrorMetadata,
};
use crate::models::{Group, Member, MemberRef, MemberRoster, Message, RosterCompleteness};

const GROUP_LIST_ROUTE: &str = "/v1/group/get-group-list";
const GROUP_MEMBERS_ROUTE: &str = "/v1/group/get-group-members";
const GROUP_MUTE_ROUTE: &str = "/v1/group/set-group-mute";
const MEMBER_MUTE_ROUTE: &str = "/v1/group/set-member-mute";
const MEMBER_UNMUTE_ROUTE: &str = "/v1/group/member-mute-cancel";
const MEMBER_RENAME_ROUTE: &str = "/v1/group/set-member-nickname";
const MEMBER_REMOVE_ROUTE: &str = "/v1/group/remove-group-member";
const MESSAGE_RECALL_ROUTE: &str = "/v1/group/message-rollback";
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
                .timeout(Duration::from_secs(5))
                .build()
                .map_err(|error| AppError::new("devtools_client", error.to_string()))?,
            next_id: Arc::new(AtomicU64::new(1)),
        })
    }

    async fn pages(&self) -> AppResult<Vec<DevToolsPage>> {
        let response = self
            .http
            .get(format!("{}/json/list", self.base_url))
            .send()
            .await
            .map_err(|error| {
                AppError::new(
                    "devtools_unavailable",
                    format!("读取 DevTools 页面失败：{error}"),
                )
                .retryable()
            })?;
        if !response.status().is_success() {
            return Err(AppError::new(
                "devtools_http",
                format!("DevTools 返回 HTTP {}", response.status()),
            )
            .retryable());
        }
        response.json().await.map_err(|error| {
            AppError::new(
                "devtools_response",
                format!("解析 DevTools 页面失败：{error}"),
            )
        })
    }

    async fn page(&self) -> AppResult<DevToolsPage> {
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
            return Ok(page.clone());
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
        let page = self.page().await?;
        let websocket = page
            .web_socket_debugger_url
            .ok_or_else(|| AppError::new("devtools_page", "页面缺少 WebSocket 地址"))?;
        let (mut stream, _) = timeout(Duration::from_secs(8), connect_async(websocket.as_str()))
            .await
            .map_err(|_| AppError::new("cdp_timeout", "连接 DevTools WebSocket 超时").retryable())?
            .map_err(|error| {
                AppError::new(
                    "cdp_connect",
                    format!("连接 DevTools WebSocket 失败：{error}"),
                )
                .retryable()
            })?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = json!({
            "id": id,
            "method": "Runtime.evaluate",
            "params": { "expression": expression, "awaitPromise": true, "returnByValue": true }
        });
        stream
            .send(WsMessage::Text(request.to_string().into()))
            .await
            .map_err(|error| {
                AppError::new("cdp_write", format!("写入 DevTools 请求失败：{error}")).retryable()
            })?;
        let result = timeout(Duration::from_secs(25), async {
            while let Some(item) = stream.next().await {
                let item = item.map_err(|error| {
                    AppError::new("cdp_read", format!("读取 DevTools 响应失败：{error}"))
                })?;
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
                    return Err(AppError::new(
                        "cdp_exception",
                        format!("旺商聊页面执行失败：{exception}"),
                    ));
                }
                return Ok(value
                    .pointer("/result/result/value")
                    .cloned()
                    .unwrap_or(Value::Null));
            }
            Err(AppError::new("cdp_closed", "DevTools WebSocket 已关闭").retryable())
        })
        .await
        .map_err(|_| AppError::new("cdp_timeout", "DevTools 执行超时").retryable())??;
        let _ = stream.close(None).await;
        Ok(result)
    }

    pub async fn diagnose(&self) -> DiagnosticSnapshot {
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
                    snapshot.detail = "DevTools 已连接，NIM 尚未初始化".into();
                } else {
                    snapshot.status = ConnectionStatus::Ready;
                    snapshot.detail = "旺商聊协议会话已就绪".into();
                }
            }
            Err(error) => snapshot.detail = error.message,
        }
        snapshot
    }
}

#[async_trait]
pub trait GroupGateway: Send + Sync {
    async fn list_groups(&self) -> AppResult<Vec<Group>>;
    async fn list_members(&self, group_id: i64) -> AppResult<MemberRoster>;
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
    fn calibrate_capabilities(
        &self,
        _app_file_version: &str,
        _main_script_sha256: &str,
    ) -> GatewayCapabilities {
        self.capabilities()
    }
    fn capabilities(&self) -> GatewayCapabilities;
}

#[derive(Clone)]
pub struct CdpGateway {
    cdp: CdpClient,
    account_id: Arc<RwLock<String>>,
    sender_id: Arc<RwLock<i64>>,
    listener_session: Arc<RwLock<String>>,
    delivered_event_sequences: Arc<RwLock<BTreeMap<String, u64>>>,
    capabilities: Arc<SyncRwLock<GatewayCapabilities>>,
}

impl CdpGateway {
    pub fn new(cdp: CdpClient) -> Self {
        Self {
            cdp,
            account_id: Arc::new(RwLock::new(String::new())),
            sender_id: Arc::new(RwLock::new(0)),
            listener_session: Arc::new(RwLock::new(String::new())),
            delivered_event_sequences: Arc::new(RwLock::new(BTreeMap::new())),
            capabilities: Arc::new(SyncRwLock::new(unverified_production_capabilities())),
        }
    }

    pub fn cdp(&self) -> &CdpClient {
        &self.cdp
    }

    fn require_capability(&self, status: CapabilityStatus, capability: &str) -> AppResult<()> {
        match status {
            CapabilityStatus::Supported => Ok(()),
            CapabilityStatus::Unverified => Err(AppError::new(
                "capability_unverified",
                format!("当前旺商聊版本尚未完成{capability}能力校准"),
            )),
            CapabilityStatus::Unsupported => Err(AppError::new(
                "capability_unsupported",
                format!("当前旺商聊版本不支持{capability}"),
            )),
        }
    }

    async fn xclient(&self, route: &str, payload: Value) -> AppResult<Value> {
        require_object_payload(route, &payload)?;
        let expression = ipc_expression("request", route, payload);
        let result = self.cdp.evaluate(&expression).await?;
        let response = decode_transport_response(route, &result)?;
        decode_business_response(route, response)
    }

    async fn group_infos(&self) -> AppResult<Vec<GroupInfo>> {
        let data = self.xclient(GROUP_LIST_ROUTE, json!({"v":"0"})).await?;
        let mut groups = Vec::new();
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
                groups.push(GroupInfo {
                    group_id,
                    cloud_id: text_field(value, &["groupCloudId"]),
                    name: human_name_field(value, &["groupName", "name", "remarkName", "nick"]),
                    owner_user_id: int_field(value, &["ownerUserId", "groupOwnerId", "ownerId"]),
                    member_count: int_field(
                        value,
                        &[
                            "memberCount",
                            "groupMemberCount",
                            "groupMemberNum",
                            "memberNum",
                            "userCount",
                        ],
                    )
                    .max(0) as usize,
                    relation: relation.into(),
                });
            }
        }
        Ok(groups)
    }

    async fn resolve_cloud_id(&self, group_id: i64) -> AppResult<String> {
        self.group_infos()
            .await?
            .into_iter()
            .find(|group| group.group_id == group_id)
            .map(|group| group.cloud_id)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppError::new("group_not_found", "群 ID 未映射到 NIM 群"))
    }

    pub async fn session_identity(&self) -> AppResult<(i64, String)> {
        let runtime = self.cdp.evaluate(r#"({nimAccount:String((window.nim&&(window.nim.account||window.nim.options&&window.nim.options.account||window.nim.config&&window.nim.config.account))||"")})"#).await?;
        let nim_account = runtime
            .get("nimAccount")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if nim_account.is_empty() {
            return Err(AppError::new(
                "nim_not_ready",
                "NIM 尚未初始化，请先登录旺商聊并等待会话初始化",
            )
            .retryable());
        }
        let groups = self.group_infos().await?;
        let first = groups
            .first()
            .ok_or_else(|| AppError::new("no_groups", "当前账号没有可用群"))?;
        let members = self.list_http_members(first.group_id).await?;
        let sender = members
            .iter()
            .find(|member| member.nim_id == nim_account)
            .map(|member| member.user_id)
            .unwrap_or(0);
        if sender <= 0 {
            return Err(AppError::new("account_identity", "当前账号未映射到群成员"));
        }
        *self.account_id.write().await = nim_account.clone();
        *self.sender_id.write().await = sender;
        Ok((sender, nim_account))
    }

    async fn list_http_members(&self, group_id: i64) -> AppResult<Vec<Member>> {
        let data = self
            .xclient(GROUP_MEMBERS_ROUTE, json!({"groupId":group_id,"v":"0"}))
            .await?;
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
                account_state: text_field(value, &["accountState", "accountStatus"]),
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
        Ok(members)
    }

    async fn action(&self, route: &str, payload: Value) -> AppResult<GatewayReceipt> {
        require_object_payload(route, &payload)?;
        let expression = ipc_expression("request", route, payload);
        let result = self.cdp.evaluate(&expression).await?;
        decode_action_receipt(route, &result)
    }

    async fn team_member_events(
        &self,
        records: Vec<GatewayRecord>,
    ) -> AppResult<Vec<GatewayEvent>> {
        if records.is_empty() {
            return Ok(Vec::new());
        }
        let groups = self.group_infos().await?;
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
            *self.listener_session.write().await = session.to_string();
        }
        Ok(value)
    }

    pub async fn read_batch(&self) -> AppResult<GatewayBatch> {
        let value = self.cdp.evaluate(READ_BATCH_EXPRESSION).await?;
        let batch = parse_gateway_batch(value)?;
        *self.listener_session.write().await = batch.session.clone();
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
        self.cdp.diagnose().await
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
        let batch = self.read_batch().await?;
        let cursor = self
            .delivered_event_sequences
            .read()
            .await
            .get(&batch.session)
            .copied()
            .unwrap_or(0);
        let records = batch
            .records
            .iter()
            .filter(|record| {
                record.kind != GatewayRecordKind::Message
                    && record.kind != GatewayRecordKind::ConnectionChanged
                    && record.sequence > cursor
            })
            .cloned()
            .collect::<Vec<_>>();
        let events = self.team_member_events(records.clone()).await?;
        if let Some(sequence) = records.iter().map(|record| record.sequence).max() {
            let mut delivered = self.delivered_event_sequences.write().await;
            if !delivered.contains_key(&batch.session) {
                delivered.clear();
            }
            delivered.insert(batch.session, sequence);
        }
        Ok(events)
    }

    fn capabilities(&self) -> GatewayCapabilities {
        self.capabilities
            .read()
            .map(|value| value.clone())
            .unwrap_or_else(|_| unverified_production_capabilities())
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

#[async_trait]
impl GroupGateway for CdpGateway {
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
        if self.account_id.read().await.is_empty() {
            let _ = self.session_identity().await?;
        }
        let mut members = self.list_http_members(group_id).await?;
        let http_returned_count = members.len();
        let infos = self.group_infos().await?;
        let mut reported = infos
            .iter()
            .find(|group| group.group_id == group_id)
            .map(|group| group.member_count)
            .unwrap_or(0);
        let cloud_id = self.resolve_cloud_id(group_id).await?;
        let expression = nim_team_members_expression(&cloud_id);
        let nim = self.cdp.evaluate(&expression).await.unwrap_or(Value::Null);
        let mut used_nim = false;
        let mut nim_returned_count = 0;
        if nim.get("ok").and_then(Value::as_bool) == Some(true) {
            used_nim = true;
            nim_returned_count = nim
                .get("members")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            for value in nim
                .get("members")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
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
        }
        reported = reported.max(members.len());
        let resolved = members.len();
        let complete = reported == resolved && reported > 0;
        Ok(MemberRoster {
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
                "HTTP 与 NIM 合并数量达到旺商聊报告人数".into()
            } else if resolved > 0 {
                format!("旺商聊报告 {reported} 人，当前解析 {resolved} 人")
            } else {
                "旺商聊未返回可识别成员".into()
            },
            http_returned_count,
            http_reported_count: reported,
            http_cursor: None,
            nim_returned_count,
            nim_reported_count: nim_returned_count,
            nim_cursor: None,
            authority: if complete { "authoritative" } else { "partial" }.into(),
            sources: if used_nim {
                vec!["wangshangliao-http".into(), "nim-team-members".into()]
            } else {
                vec!["wangshangliao-http".into()]
            },
        })
    }

    async fn send_text(&self, group_id: i64, text: &str) -> AppResult<GatewayReceipt> {
        self.require_capability(self.capabilities().send_text, "发送消息")?;
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
            .evaluate(&ipc_expression("encode", "/v1/plugins/encode-msg", payload))
            .await?;
        let content = decode_transport_response("/v1/plugins/encode-msg", &encode)?;
        if content.is_empty() {
            return Err(AppError::new("encode_failed", "旺商聊消息编码失败"));
        }
        let delivery = self
            .cdp
            .evaluate(&nim_send_expression(&target, content))
            .await?;
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
        Ok(GatewayReceipt {
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
        })
    }

    async fn recall(
        &self,
        group_id: i64,
        sender_user_id: i64,
        message_id: &str,
    ) -> AppResult<GatewayReceipt> {
        self.require_capability(self.capabilities().recall, "撤回消息")?;
        require_positive("groupId", group_id)?;
        require_positive("userId", sender_user_id)?;
        require_non_empty("messageId", message_id)?;
        let cloud = self.resolve_cloud_id(group_id).await?;
        self.action(
            MESSAGE_RECALL_ROUTE,
            json!({"groupCloudId":cloud,"userId":sender_user_id,"msgId":message_id}),
        )
        .await
    }
    async fn mute(
        &self,
        group_id: i64,
        user_id: i64,
        duration_seconds: i64,
    ) -> AppResult<GatewayReceipt> {
        self.require_capability(self.capabilities().mute, "成员禁言")?;
        require_positive("groupId", group_id)?;
        require_positive("userId", user_id)?;
        require_positive("durationSeconds", duration_seconds)?;
        self.action(
            MEMBER_MUTE_ROUTE,
            json!({"groupId":group_id,"userId":user_id,"min":(duration_seconds+59)/60}),
        )
        .await
    }
    async fn unmute(&self, group_id: i64, user_id: i64) -> AppResult<GatewayReceipt> {
        self.require_capability(self.capabilities().mute, "成员解禁")?;
        require_positive("groupId", group_id)?;
        require_positive("userId", user_id)?;
        self.action(
            MEMBER_UNMUTE_ROUTE,
            json!({"groupId":group_id,"userId":user_id}),
        )
        .await
    }
    async fn rename(
        &self,
        group_id: i64,
        member: &MemberRef,
        nickname: &str,
    ) -> AppResult<GatewayReceipt> {
        self.require_capability(self.capabilities().rename, "修改群名片")?;
        require_positive("groupId", group_id)?;
        require_non_empty("nickname", nickname)?;
        let mut http_error = None;
        if let Some(user_id) = member.user_id.filter(|value| *value > 0) {
            match self
                .action(
                    MEMBER_RENAME_ROUTE,
                    json!({"groupId":group_id,"userId":user_id,"nick":nickname}),
                )
                .await
            {
                Ok(receipt) => return Ok(receipt),
                Err(error) if rename_fallback_allowed(&error) => http_error = Some(error),
                Err(error) => return Err(error),
            }
        }
        let Some(nim_id) = member
            .nim_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Err(http_error
                .unwrap_or_else(|| AppError::new("member_identity", "成员缺少 userId 和 nimId")));
        };
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
        if result.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(GatewayReceipt {
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
            })
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
        self.require_capability(self.capabilities().remove_member, "移出成员")?;
        require_positive("groupId", group_id)?;
        require_positive("userId", user_id)?;
        self.action(
            MEMBER_REMOVE_ROUTE,
            json!({"groupId":group_id,"groupMemberIds":[user_id]}),
        )
        .await
    }
    async fn set_group_mute(&self, group_id: i64, muted: bool) -> AppResult<GatewayReceipt> {
        self.require_capability(self.capabilities().group_mute, "全群发言控制")?;
        require_positive("groupId", group_id)?;
        self.action(
            GROUP_MUTE_ROUTE,
            json!({"groupId":group_id,"muteMode":if muted {"MUTE_MEMBER"} else {"MUTE_NO"}}),
        )
        .await
    }
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

#[derive(Debug)]
struct GroupInfo {
    group_id: i64,
    cloud_id: String,
    name: String,
    owner_user_id: i64,
    member_count: usize,
    #[allow(dead_code)]
    relation: String,
}

fn ipc_expression(kind: &str, route: &str, payload: Value) -> String {
    let input = json!({"type":kind,"route":route,"payload":payload});
    format!(
        r#"(async()=>{{const input={input};const ipc=globalThis.__dhFixtureIpc||(typeof require==="function"?require("electron").ipcRenderer:null);if(!ipc)return{{transportCode:503,errno:1,error:"Electron IPC 未就绪",requestId:""}};return new Promise(resolve=>{{const channel="dh-rust-"+Date.now()+"-"+Math.random();const timer=setTimeout(()=>resolve({{transportCode:504,errno:1,error:"IPC timeout",requestId:channel}}),15000);ipc.once(channel,(event,value)=>{{clearTimeout(timer);resolve({{transportCode:value&&value.code,errno:value&&value.errno,response:value&&value.response,error:value&&value.message,requestId:value&&value.requestId||channel}});}});if(input.type==="request")ipc.send("xclient",{{type:"request",requestId:channel,url:input.route,excuteType:0,params:JSON.stringify(input.payload),key:channel}});else ipc.send("xclient",{{type:"encode",params:JSON.stringify(input.payload),key:channel}});}});}})()"#
    )
}

fn nim_send_expression(target: &str, content: &str) -> String {
    let input = json!({"target":target,"content":content});
    format!(
        r#"(async()=>{{const input={input};return new Promise(resolve=>{{if(!window.nim){{resolve({{ok:false,errorMessage:"NIM 未就绪"}});return;}}const timer=setTimeout(()=>resolve({{ok:false,errorMessage:"NIM send timeout"}}),15000);window.nim.sendCustomMsg({{scene:"team",to:input.target,content:input.content,isLocal:false,done:(error,message)=>{{clearTimeout(timer);resolve({{ok:!error,errorMessage:error&&(error.message||String(error)),idClient:message&&message.idClient,idServer:message&&message.idServer}});}}}});}});}})()"#
    )
}

fn nim_team_members_expression(team_id: &str) -> String {
    let input = json!({"teamId":team_id});
    format!(
        r#"(async()=>{{const input={input};return new Promise(resolve=>{{if(!window.nim){{resolve({{ok:false,errorMessage:"NIM 未就绪",members:[]}});return;}}const timer=setTimeout(()=>resolve({{ok:false,errorMessage:"NIM team members timeout",members:[]}}),15000);window.nim.getTeamMembers({{teamId:input.teamId,done:(error,value)=>{{clearTimeout(timer);const source=Array.isArray(value)?value:value&&Array.isArray(value.members)?value.members:[];resolve({{ok:!error,errorMessage:error&&(error.message||String(error)),members:source.map(item=>({{nimId:String(item.account||item.accid||""),cardName:item.nickInTeam||item.nick||"",type:item.type||item.memberType||"normal"}}))}});}}}});}});}})()"#
    )
}

fn nim_update_nick_expression(team_id: &str, nim_id: &str, nickname: &str) -> String {
    let input = json!({"teamId":team_id,"account":nim_id,"nickInTeam":nickname});
    format!(
        r#"(async()=>{{const input={input};return new Promise(resolve=>{{if(!window.nim){{resolve({{ok:false,errorMessage:"NIM 未就绪"}});return;}}const timer=setTimeout(()=>resolve({{ok:false,errorMessage:"NIM update nick timeout"}}),15000);window.nim.updateNickInTeam({{teamId:input.teamId,account:input.account,nickInTeam:input.nickInTeam,done:(error,value)=>{{clearTimeout(timer);resolve({{ok:!error,errorMessage:error&&(error.message||String(error)),member:value||null}});}}}});}});}})()"#
    )
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
        let error = AppError::new("gateway_transport", message).with_gateway(transport_metadata());
        return Err(if code >= 500 {
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
    let Some(errno) = business_errno else {
        return Err(
            AppError::new("gateway_response", "业务响应缺少 errno").with_gateway(metadata())
        );
    };
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
        return Err(AppError::new("gateway_business", message).with_gateway(gateway));
    }
    Ok(envelope.get("data").cloned().unwrap_or(Value::Null))
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
    })
}

fn parse_gateway_receipt(session: &str, sequence: u64, value: Value) -> AppResult<GatewayReceipt> {
    let receipt = legacy_gateway_receipt(session, sequence, value)?;
    if receipt.session != session || receipt.acknowledged_through != sequence {
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
