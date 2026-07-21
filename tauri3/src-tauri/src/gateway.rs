use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

use crate::error::{AppError, AppResult};
use crate::models::{Group, Member, MemberRef, MemberRoster, Message};

const GROUP_LIST_ROUTE: &str = "/v1/group/get-group-list";
const GROUP_MEMBERS_ROUTE: &str = "/v1/group/get-group-members";
const GROUP_MUTE_ROUTE: &str = "/v1/group/set-group-mute";
const MEMBER_MUTE_ROUTE: &str = "/v1/group/set-member-mute";
const MEMBER_UNMUTE_ROUTE: &str = "/v1/group/member-mute-cancel";
const MEMBER_RENAME_ROUTE: &str = "/v1/group/set-member-nickname";
const MEMBER_REMOVE_ROUTE: &str = "/v1/group/remove-group-member";
const MESSAGE_RECALL_ROUTE: &str = "/v1/group/message-rollback";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevToolsPage {
    title: String,
    url: String,
    web_socket_debugger_url: Option<String>,
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
        pages
            .into_iter()
            .filter(|page| page.web_socket_debugger_url.is_some())
            .max_by_key(|page| {
                let value = format!("{} {}", page.title, page.url).to_lowercase();
                if value.contains("旺商聊") || value.contains("wangshangliao") {
                    3
                } else if value.contains("rsbuild") {
                    2
                } else {
                    1
                }
            })
            .ok_or_else(|| {
                AppError::new("devtools_page", "DevTools 中没有可用的旺商聊页面").retryable()
            })
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
    async fn send_text(&self, group_id: i64, text: &str) -> AppResult<String>;
    async fn recall(&self, group_id: i64, sender_user_id: i64, message_id: &str) -> AppResult<()>;
    async fn mute(&self, group_id: i64, user_id: i64, duration_seconds: i64) -> AppResult<()>;
    async fn unmute(&self, group_id: i64, user_id: i64) -> AppResult<()>;
    async fn rename(&self, group_id: i64, member: &MemberRef, nickname: &str) -> AppResult<()>;
    async fn remove_member(&self, group_id: i64, user_id: i64) -> AppResult<()>;
    async fn set_group_mute(&self, group_id: i64, muted: bool) -> AppResult<()>;
    async fn set_group_announcement(&self, _group_id: i64, _text: &str) -> AppResult<()> {
        Err(AppError::new(
            "capability_unsupported",
            "当前协议未开放群公告功能",
        ))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GatewayCapabilities {
    pub announcement: bool,
    pub mute: bool,
    pub recall: bool,
    pub rename: bool,
    pub remove_member: bool,
    pub group_mute: bool,
    pub member_events: bool,
}

#[async_trait]
pub trait RuntimeGateway: GroupGateway {
    async fn diagnose(&self) -> DiagnosticSnapshot;
    async fn session_identity(&self) -> AppResult<(i64, String)>;
    async fn install_message_listener(&self) -> AppResult<Value>;
    async fn poll_messages(&self) -> AppResult<Value>;
    async fn acknowledge_messages(&self, sequence: u64) -> AppResult<Value>;
    async fn poll_events(&self) -> AppResult<Vec<GatewayEvent>> {
        Ok(Vec::new())
    }
    fn capabilities(&self) -> GatewayCapabilities;
}

#[derive(Clone)]
pub struct CdpGateway {
    cdp: CdpClient,
    account_id: Arc<RwLock<String>>,
    sender_id: Arc<RwLock<i64>>,
}

impl CdpGateway {
    pub fn new(cdp: CdpClient) -> Self {
        Self {
            cdp,
            account_id: Arc::new(RwLock::new(String::new())),
            sender_id: Arc::new(RwLock::new(0)),
        }
    }

    pub fn cdp(&self) -> &CdpClient {
        &self.cdp
    }

    async fn xclient(&self, route: &str, payload: Value) -> AppResult<Value> {
        let expression = ipc_expression("request", route, payload);
        let result = self.cdp.evaluate(&expression).await?;
        let code = result
            .get("transportCode")
            .and_then(Value::as_i64)
            .unwrap_or(502);
        let response = result
            .get("response")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if code != 200 || response.is_empty() {
            let detail = result
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("xclient 请求失败");
            return Err(AppError::new("protocol_error", detail).retryable());
        }
        let envelope: Value = serde_json::from_str(response).map_err(|error| {
            AppError::new("protocol_response", format!("解析旺商聊响应失败：{error}"))
        })?;
        if envelope.get("code").and_then(Value::as_i64).unwrap_or(-1) != 0 {
            return Err(AppError::new(
                "protocol_business",
                envelope
                    .get("msg")
                    .and_then(Value::as_str)
                    .unwrap_or("旺商聊业务请求失败"),
            ));
        }
        Ok(envelope.get("data").cloned().unwrap_or(Value::Null))
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
                    name: text_field(value, &["groupName", "name", "remarkName", "nick"]),
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
            let nickname = text_field(value, &["userNick", "nickname", "userName", "name"]);
            let card_name =
                text_field(value, &["groupMemberNick", "nick", "groupNick", "cardName"]);
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
                    text_field(value, &["userNick", "nickname", "userName", "name"])
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

    async fn action(&self, route: &str, payload: Value) -> AppResult<()> {
        self.xclient(route, payload).await.map(|_| ())
    }

    pub async fn install_message_listener(&self) -> AppResult<Value> {
        self.cdp.evaluate(LISTENER_EXPRESSION).await
    }
    pub async fn poll_messages(&self) -> AppResult<Value> {
        self.cdp.evaluate(POLL_EXPRESSION).await
    }
    pub async fn acknowledge_messages(&self, sequence: u64) -> AppResult<Value> {
        self.cdp.evaluate(&format!(r#"(()=>{{const state=window.__dhBridgeMessages;if(!state)return{{ok:false,error:"LISTENER_NOT_READY",acked:0}};const before=state.queue.length;state.queue=state.queue.filter(item=>Number(item.seq)>{sequence});return{{ok:true,acked:before-state.queue.length,remaining:state.queue.length,dropped:state.dropped||0}};}})()"#)).await
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

    fn capabilities(&self) -> GatewayCapabilities {
        GatewayCapabilities {
            announcement: false,
            mute: true,
            recall: true,
            rename: true,
            remove_member: true,
            group_mute: true,
            member_events: false,
        }
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
        if self.account_id.read().await.is_empty() {
            let _ = self.session_identity().await?;
        }
        let mut members = self.list_http_members(group_id).await?;
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
        if nim.get("ok").and_then(Value::as_bool) == Some(true) {
            used_nim = true;
            for value in nim
                .get("members")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let nim_id = text_field(value, &["nimId"]);
                let card = text_field(value, &["cardName"]);
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
        Ok(MemberRoster {
            members,
            reported_count: reported,
            resolved_count: resolved,
            complete: reported == resolved,
            sources: if used_nim {
                vec!["wangshangliao-http".into(), "nim-team-members".into()]
            } else {
                vec!["wangshangliao-http".into()]
            },
        })
    }

    async fn send_text(&self, group_id: i64, text: &str) -> AppResult<String> {
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
        let content = encode
            .get("response")
            .and_then(Value::as_str)
            .unwrap_or_default();
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
            .retryable());
        }
        Ok(delivery
            .get("idServer")
            .or_else(|| delivery.get("idClient"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    async fn recall(&self, group_id: i64, sender_user_id: i64, message_id: &str) -> AppResult<()> {
        let cloud = self.resolve_cloud_id(group_id).await?;
        self.action(
            MESSAGE_RECALL_ROUTE,
            json!({"groupCloudId":cloud,"userId":sender_user_id,"msgId":message_id}),
        )
        .await
    }
    async fn mute(&self, group_id: i64, user_id: i64, duration_seconds: i64) -> AppResult<()> {
        self.action(
            MEMBER_MUTE_ROUTE,
            json!({"groupId":group_id,"userId":user_id,"min":(duration_seconds.max(1)+59)/60}),
        )
        .await
    }
    async fn unmute(&self, group_id: i64, user_id: i64) -> AppResult<()> {
        self.action(
            MEMBER_UNMUTE_ROUTE,
            json!({"groupId":group_id,"userId":user_id}),
        )
        .await
    }
    async fn rename(&self, group_id: i64, member: &MemberRef, nickname: &str) -> AppResult<()> {
        if let Some(user_id) = member.user_id.filter(|value| *value > 0) {
            if self
                .action(
                    MEMBER_RENAME_ROUTE,
                    json!({"groupId":group_id,"userId":user_id,"nick":nickname}),
                )
                .await
                .is_ok()
            {
                return Ok(());
            }
        }
        let nim_id = member
            .nim_id
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppError::new("member_identity", "成员缺少 userId 和 nimId"))?;
        let cloud = self.resolve_cloud_id(group_id).await?;
        let result = self
            .cdp
            .evaluate(&nim_update_nick_expression(&cloud, nim_id, nickname))
            .await?;
        if result.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(())
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
    async fn remove_member(&self, group_id: i64, user_id: i64) -> AppResult<()> {
        self.action(
            MEMBER_REMOVE_ROUTE,
            json!({"groupId":group_id,"groupMemberIds":[user_id]}),
        )
        .await
    }
    async fn set_group_mute(&self, group_id: i64, muted: bool) -> AppResult<()> {
        self.action(
            GROUP_MUTE_ROUTE,
            json!({"groupId":group_id,"muteMode":if muted {"MUTE_MEMBER"} else {"MUTE_NO"}}),
        )
        .await
    }
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
        r#"(async()=>{{const input={input};const ipc=require("electron").ipcRenderer;return new Promise(resolve=>{{const channel="dh-rust-"+Date.now()+"-"+Math.random();const timer=setTimeout(()=>resolve({{transportCode:504,errno:1,error:"IPC timeout"}}),15000);ipc.once(channel,(event,value)=>{{clearTimeout(timer);resolve({{transportCode:value&&value.code,errno:value&&value.errno,response:value&&value.response,error:value&&value.message}});}});if(input.type==="request")ipc.send("xclient",{{type:"request",requestId:channel,url:input.route,excuteType:0,params:JSON.stringify(input.payload||{{}}),key:channel}});else ipc.send("xclient",{{type:"encode",params:JSON.stringify(input.payload),key:channel}});}});}})()"#
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

const LISTENER_EXPRESSION: &str = r#"(()=>{const nim=window.nim;if(!nim)return{ok:false,error:"NIM_NOT_READY",queued:0};if(!window.__dhBridgeMessages){const state={queue:[],seen:new Set(),installed:[],limit:5000,nextSeq:1,dropped:0};const collect=(value,source)=>{if(Array.isArray(value)){value.forEach(item=>collect(item,source));return;}if(!value||typeof value!=="object")return;const id=value.idClient||value.idServer||[value.time||Date.now(),value.from||"",value.to||""].join("-");if(state.seen.has(id))return;state.seen.add(id);state.queue.push({seq:state.nextSeq++,source,idClient:value.idClient,idServer:value.idServer,scene:value.scene,from:value.from,to:value.to,time:value.time,type:value.type,flow:value.flow,content:value.content,attach:value.attach,custom:value.custom,msgFormat:value.msgFormat,mentions:value.mentions||value.aite,quote:value.quote,fromNick:value.fromNick,sessionId:value.sessionId});if(state.queue.length>state.limit){const overflow=state.queue.length-state.limit;state.queue.splice(0,overflow);state.dropped+=overflow;}};["onmsg","onmsgs","onofflinemsgs","onroamingmsgs"].forEach(name=>{const original=nim.options&&nim.options[name];if(typeof original!=="function")return;nim.options[name]=function(...args){try{collect(args[0],name);}catch{}return original.apply(this,args);};state.installed.push(name);});window.__dhBridgeMessages=state;}const state=window.__dhBridgeMessages;return{ok:true,installed:state.installed,queued:state.queue.length,dropped:state.dropped};})()"#;
const POLL_EXPRESSION: &str = r#"(async()=>{const state=window.__dhBridgeMessages;if(!state)return{ok:false,error:"LISTENER_NOT_READY",messages:[]};const batch=state.queue.slice(0,100);let common=window.__dhBridgeCommon||null;try{if(!common){const main=Array.from(document.scripts).map(item=>item.src).find(name=>/\/main-[^/]+\.js(?:\?|$)/.test(name));if(main){const source=await(await fetch(main)).text();const match=source.match(/zh-cn-[a-z0-9]+\.js/);if(match){common=(await import(new URL(match[0],main).href)).D;window.__dhBridgeCommon=common;}}}}catch{}const messages=[];for(const item of batch){let decoded=null,decodeError="";if(common&&item.type==="custom"&&typeof item.content==="string"){try{decoded=await common.decodeMsg(item.content);if(decoded&&decoded.mentions===undefined&&decoded.aite!==undefined)decoded={...decoded,mentions:decoded.aite};}catch(error){decodeError=error&&error.message||String(error);}}messages.push({...item,decoded,decodeError});}return{ok:true,messages,remaining:state.queue.length,dropped:state.dropped||0};})()"#;

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
}
