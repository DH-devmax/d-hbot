//! RuntimeGateway implementation backed by the authenticated Rust clients.
use crate::{business_reads, protocol_login::Session};
use async_trait::async_trait;
use dh_core::{
    error::{AppError, AppResult},
    gateway::*,
    models::*,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

type GroupCache = Option<(u64, Instant, Value, Vec<Group>)>;
pub struct RustGateway {
    session: Arc<Mutex<Option<Session>>>,
    epoch: Arc<AtomicU64>,
    groups: Mutex<GroupCache>,
    members: Mutex<BTreeMap<i64, (u64, Instant, MemberRoster)>>,
    writes: Mutex<()>,
    database: dh_core::database::DatabaseExecutor,
    message_verified: AtomicU64,
}
fn mapping(_: axum::http::StatusCode) -> AppError {
    AppError::new("business_shape", "业务返回结构无法识别，请稍后重试")
}
fn protocol(error: dh_protocol::business_client::Error) -> AppError {
    use dh_protocol::business_client::Error;
    match error {
        Error::Expired | Error::Business(401) => {
            AppError::new("login_required", "旺商聊会话已过期，请重新登录")
        }
        Error::Business(429) | Error::Http(429) => {
            AppError::new("gateway_rate_limited", "请求过于频繁，请稍后重试")
        }
        Error::Transport => AppError::new("business_transport", "业务网络请求失败，请稍后重试"),
        Error::Business(_) => AppError::new(
            "business_rejected",
            "业务服务拒绝了操作，请检查账号权限和参数",
        ),
        _ => AppError::new(
            "business_request",
            "业务请求未通过，请检查当前账号权限或稍后重试",
        ),
    }
}
fn unavailable() -> AppError {
    AppError::new("login_required", "请先登录旺商聊账号")
}
fn pending_protocol() -> AppError {
    AppError::new("protocol_not_ready", "该操作的独立协议尚未完成验证")
}
impl RustGateway {
    pub async fn new(
        session: Arc<Mutex<Option<Session>>>,
        database: dh_core::database::DatabaseExecutor,
    ) -> Self {
        let epoch = session
            .lock()
            .await
            .as_ref()
            .map(|s| s.epoch.clone())
            .unwrap_or_else(|| Arc::new(AtomicU64::new(0)));
        Self {
            session,
            database,
            message_verified: AtomicU64::new(u64::MAX),
            epoch,
            groups: Mutex::new(None),
            members: Mutex::new(BTreeMap::new()),
            writes: Mutex::new(()),
        }
    }
    pub async fn group_data(&self) -> AppResult<(Value, Vec<Group>)> {
        let epoch = self.epoch.load(Ordering::SeqCst);
        if let Some((saved, at, data, groups)) = self.groups.lock().await.as_ref() {
            if *saved == epoch && at.elapsed() < Duration::from_secs(10) {
                return Ok((data.clone(), groups.clone()));
            }
        }
        let mut guard = self.session.lock().await;
        let session = guard.as_mut().ok_or_else(unavailable)?;
        if session.epoch.load(Ordering::SeqCst) != epoch {
            return Err(unavailable());
        }
        let account = session.nim_account.clone().ok_or_else(unavailable)?;
        let data = session
            .client
            .as_mut()
            .ok_or_else(unavailable)?
            .groups()
            .await
            .map_err(protocol)?;
        let groups = business_reads::groups(&account, &data).map_err(mapping)?;
        for value in ["owner", "member"]
            .into_iter()
            .flat_map(|k| data.get(k).and_then(Value::as_array).into_iter().flatten())
        {
            let id = value
                .get("groupId")
                .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()));
            let cloud = value.get("groupCloudId").map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| v.to_string())
            });
            if let (Some(id), Some(cloud)) = (id, cloud) {
                if id > 0 && cloud.bytes().all(|b| b.is_ascii_digit()) && !cloud.is_empty() {
                    let account = account.clone();
                    self.database
                        .execute(move |db| db.save_conversation_route(&account, id, &cloud))
                        .await?;
                }
            }
        }
        *self.groups.lock().await = Some((epoch, Instant::now(), data.clone(), groups.clone()));
        Ok((data, groups))
    }
    async fn manager(&self, group: i64, user: Option<i64>) -> AppResult<()> {
        self.invalidate_member_cache(group).await;
        let roster = self.list_members(group).await?;
        let (sender, _) = self.session_identity().await?;
        if !roster
            .members
            .iter()
            .any(|m| m.user_id == sender && matches!(m.role.as_str(), "owner" | "admin"))
        {
            return Err(AppError::new(
                "management_required",
                "当前账号不具备群管理权限",
            ));
        }
        if let Some(user) = user {
            if !roster
                .members
                .iter()
                .any(|m| m.user_id == user && user > 0 && m.role == "member")
            {
                return Err(AppError::new(
                    "member_protected",
                    "成员身份无效或目标为群管理员",
                ));
            }
        }
        Ok(())
    }
    async fn request(&self, route: &str, params: &Value, epoch: u64) -> AppResult<Value> {
        let mut guard = self.session.lock().await;
        let session = guard.as_mut().ok_or_else(unavailable)?;
        if session.epoch.load(Ordering::SeqCst) != epoch {
            return Err(unavailable());
        }
        session
            .client
            .as_mut()
            .ok_or_else(unavailable)?
            .group_request(route, params)
            .await
            .map_err(protocol)
    }
    async fn write(
        &self,
        route: &str,
        params: Value,
        user: Option<i64>,
    ) -> AppResult<GatewayReceipt> {
        self.write_with_data(route, params, user).await.map(|(receipt, _)| receipt)
    }
    async fn write_with_data(
        &self,
        route: &str,
        params: Value,
        user: Option<i64>,
    ) -> AppResult<(GatewayReceipt, Value)> {
        let _write = self.writes.lock().await;
        let epoch = self.session_epoch();
        let group = params["groupId"].as_i64().ok_or_else(unavailable)?;
        self.manager(group, user).await?;
        let data = self.request(route, &params, epoch).await.map_err(|error| {
            if matches!(
                error.code.as_str(),
                "business_transport" | "business_request"
            ) {
                AppError::new(
                    "delivery_unknown",
                    "请求已提交但结果未能确认，请先刷新状态，不要重复提交",
                )
                .with_gateway(dh_core::error::GatewayErrorMetadata::new(
                    route,
                    dh_core::error::GatewayErrorLayer::Delivery,
                ))
            } else {
                error
            }
        })?;
        *self.groups.lock().await = None;
        self.invalidate_member_cache(group).await;
        let mut receipt = GatewayReceipt::succeeded(route);
        receipt.status = "unknown".into();
        receipt.verification = Some("unknown".into());
        // Server acceptance is not proof of readback; each operation below verifies independently.
        Ok((receipt, data))
    }
    async fn member_mute(
        &self,
        group: i64,
        user: i64,
        seconds: Option<i64>,
    ) -> AppResult<GatewayReceipt> {
        if seconds.is_some_and(|s| s <= 0 || s > 31536000) {
            return Err(AppError::new("invalid_duration", "禁言时长无效"));
        }
        let (route, params) = if let Some(seconds) = seconds {
            (
                "/v1/group/set-member-mute",
                json!({"groupId":group,"userId":user,"min":(seconds+59)/60}),
            )
        } else {
            (
                "/v1/group/member-mute-cancel",
                json!({"groupId":group,"userId":user}),
            )
        };
        let epoch = self.session_epoch();
        let roster = self.list_members(group).await?;
        let nim_account = roster.members.iter().find(|m| m.user_id == user && !m.nim_id.is_empty())
            .map(|m| m.nim_id.clone()).ok_or_else(|| AppError::new("member_identity", "成员即时通信身份缺失"))?;
        let (_, account) = self.session_identity().await?;
        let lookup = account.clone();
        let team = self.database.execute(move |db| db.conversation_route(&lookup,group)).await?
            .and_then(|id| id.parse::<u64>().ok()).filter(|id| *id > 0).ok_or_else(pending_protocol)?;
        let mut receipt = self.write(route, params, Some(user)).await?;
        let mut guard = self.session.lock().await;
        if let Some(session) = guard.as_mut().filter(|s| s.epoch.load(Ordering::SeqCst) == epoch && s.nim_account.as_deref() == Some(account.as_str())) {
            if let Some(nim) = session.nim.as_mut() {
                if nim.set_member_mute_verified(team,&nim_account,seconds.is_some()).await == Ok(true) {
                    receipt.status = "succeeded".into();
                    receipt.verification = Some("verified".into());
                }
            }
        }
        Ok(receipt)
    }
}
#[async_trait]
impl GroupGateway for RustGateway {
    async fn list_groups(&self) -> AppResult<Vec<Group>> {
        Ok(self.group_data().await?.1)
    }
    async fn list_members(&self, group: i64) -> AppResult<MemberRoster> {
        if group <= 0 {
            return Err(AppError::new("invalid_group_id", "群标识无效"));
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        if let Some((saved, at, roster)) = self.members.lock().await.get(&group) {
            if *saved == epoch && at.elapsed() < Duration::from_secs(5) {
                return Ok(roster.clone());
            }
        }
        if !self
            .list_groups()
            .await?
            .iter()
            .any(|g| g.group_id == group)
        {
            return Err(AppError::new("group_not_found", "当前账号不可访问该群"));
        }
        let result = tokio::time::timeout(Duration::from_secs(45), async {
            let mut guard = self.session.lock().await;
            let session = guard.as_mut().ok_or_else(unavailable)?;
            if session.epoch.load(Ordering::SeqCst) != epoch {
                return Err(unavailable());
            }
            let account = session.nim_account.clone().ok_or_else(unavailable)?;
            let client = session.client.as_mut().ok_or_else(unavailable)?;
            let mut values = BTreeMap::new();
            let mut seen = HashSet::new();
            let mut cursor: Option<String> = None;
            for page in 1..=100 {
                let data = client
                    .members_page(group, cursor.as_deref())
                    .await
                    .map_err(protocol)?;
                let (members, next) =
                    business_reads::members(&account, group, &data).map_err(mapping)?;
                for member in members {
                    values.insert(member.user_id, member);
                }
                if values.len() > 100000 {
                    return Err(AppError::new("member_limit", "成员数量超过单次读取上限"));
                }
                match next {
                    None => {
                        return Ok(business_reads::roster(values.into_values().collect(), page))
                    }
                    Some(next) => {
                        if !seen.insert(next.clone()) {
                            return Err(AppError::new("member_cursor", "成员分页游标重复"));
                        }
                        cursor = Some(next);
                    }
                }
            }
            Err(AppError::new("member_limit", "成员分页超过读取上限"))
        })
        .await
        .map_err(|_| AppError::new("member_timeout", "成员读取超时，请稍后重试"))??;
        let mut cache = self.members.lock().await;
        if cache.len() >= 100 {
            cache.clear();
        }
        cache.insert(group, (epoch, Instant::now(), result.clone()));
        Ok(result)
    }
    async fn invalidate_member_cache(&self, group: i64) {
        self.members.lock().await.remove(&group);
    }
    async fn send_text(&self, group: i64, text: &str) -> AppResult<GatewayReceipt> {
        use dh_protocol::{
            message_envelope::{self, ApplicationMessage, Content, Source},
            nim_client::Delivery,
            nim_message::Scene,
        };
        use rand_core::{OsRng, RngCore};
        let _write = self.writes.lock().await;
        if text.trim().is_empty() || text.len() > 3500 {
            return Err(AppError::new("message_length", "消息为空或超过长度限制"));
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        let (_, groups) = self.group_data().await?;
        let target = groups
            .into_iter()
            .find(|g| g.group_id == group)
            .ok_or_else(|| AppError::new("group_not_found", "当前账号不可访问该群"))?;
        let mut guard = self.session.lock().await;
        let session = guard.as_mut().ok_or_else(unavailable)?;
        if session.epoch.load(Ordering::SeqCst) != epoch {
            return Err(unavailable());
        }
        let account = session.nim_account.clone().ok_or_else(unavailable)?;
        let sender = session
            .client
            .as_ref()
            .ok_or_else(unavailable)?
            .user_id()
            .map_err(protocol)?;
        let key = session.message_key().map_err(|_| pending_protocol())?;
        let lookup = account.clone();
        let cloud = self
            .database
            .execute(move |db| db.conversation_route(&lookup, group))
            .await?
            .ok_or_else(pending_protocol)?;
        if session.nim.is_none() {
            return Err(AppError::new("nim_offline", "即时通信未连接"));
        }
        let client = uuid::Uuid::new_v4().to_string();
        let nonce = OsRng.next_u64();
        let aad = OsRng.next_u64().max(1);
        let now = chrono::Utc::now();
        let message = ApplicationMessage {
            from: Some(Source {
                id: sender.try_into().map_err(|_| pending_protocol())?,
                name: session.account_name.clone().unwrap_or_default(),
            }),
            to: Some(Source {
                id: group.try_into().map_err(|_| pending_protocol())?,
                name: target.name,
            }),
            device: 1,
            session: 2,
            version: 2,
            created_at: now.timestamp_millis(),
            client_id: client.clone(),
            content: Some(Content { data: text.into() }),
            ..Default::default()
        };
        let content = message_envelope::seal(*key, &message, nonce, aad, now.timestamp() as u64)
            .map_err(|_| AppError::new("message_encode", "消息编码失败"))?;
        let save = client.clone();
        self.database
            .execute(move |db| {
                db.reserve_conversation_send(&account, group, &save, &nonce.to_string())
            })
            .await?;
        let result = session
            .nim
            .as_mut()
            .unwrap()
            .send_custom(Scene::Group, &cloud, &client, &content)
            .await;
        let (state, server_id) = match result {
            Ok(Delivery::Sent { server_id }) => ("sent", server_id),
            Ok(Delivery::Rejected { .. }) => ("rejected", String::new()),
            _ => ("unknown", String::new()),
        };
        if state == "unknown" {
            session.nim = None;
            session.sync_started = false;
        }
        let save = client.clone();
        let saved_id = server_id.clone();
        self.database
            .execute(move |db| db.finish_conversation_send(&save, state, &saved_id))
            .await
            .map_err(|_| {
                AppError::new(
                    "effect_unknown",
                    "消息已提交，但本地确认保存失败，请人工核实",
                )
                .retryable()
            })?;
        if state == "rejected" {
            return Err(AppError::new("message_rejected", "即时通信服务拒绝了消息"));
        }
        let mut receipt = GatewayReceipt::succeeded("rust.nim.send_text");
        receipt.request_id = client;
        receipt.message_id = server_id;
        if state == "unknown" {
            receipt.status = "unknown".into();
        }
        receipt.verification = Some(
            if state == "sent" {
                "provider-confirmed"
            } else {
                "unknown"
            }
            .into(),
        );
        Ok(receipt)
    }
    async fn recall(&self, group: i64, sender: i64, message: &str) -> AppResult<GatewayReceipt> {
        let _write = self.writes.lock().await;
        self.manager(group, Some(sender)).await?;
        let (_, account) = self.session_identity().await?;
        let lookup = account.clone();
        let id = message.to_owned();
        let raw = self.database.execute(move |db| db.recall_target_metadata(&lookup,group,sender,&id)).await?
            .ok_or_else(|| AppError::new("recall_target_missing", "当前群记录中没有找到匹配的消息与发送者"))?;
        let metadata: Value = serde_json::from_str(&raw).map_err(|_| pending_protocol())?;
        let field = |key: &str| metadata[key].as_str().filter(|v| !v.is_empty()).ok_or_else(pending_protocol);
        let target = field("to")?.to_owned();
        let from = field("from")?;
        let client = field("idClient")?;
        let time = metadata["time"].as_u64().ok_or_else(pending_protocol)?;
        let lookup = account.clone();
        if self.database.execute(move |db| db.conversation_route(&lookup,group)).await?.as_deref() != Some(target.as_str()) {
            return Err(AppError::new("recall_target_mismatch", "消息目标群不匹配"));
        }
        let delivery = {
            let mut guard = self.session.lock().await;
            let session = guard.as_mut().ok_or_else(unavailable)?;
            if session.nim_account.as_deref() != Some(account.as_str()) { return Err(unavailable()); }
            session.nim.as_mut().ok_or_else(pending_protocol)?.recall_group(&target,from,client,message,time).await
                .map_err(|_| pending_protocol())?
        };
        if matches!(delivery, dh_protocol::nim_client::Delivery::Rejected { .. }) {
            return Err(AppError::new("recall_rejected", "即时通信服务未接受撤回请求"));
        }
        let mut receipt = GatewayReceipt::succeeded("rust.nim.recall");
        receipt.message_id = message.into();
        if matches!(delivery, dh_protocol::nim_client::Delivery::Sent { .. }) {
            // NIM 7/13 is an authoritative response to this exact serial. Actor
            // clients need not receive 7/14; peer notification is recorded separately.
            receipt.verification = Some("server-accepted".into());
            let (a,t,id) = (account.clone(),target.clone(),message.to_owned());
            self.database.execute(move |db| db.mark_conversation_recalled(&a,&t,&id,"server-response")).await?;
            return Ok(receipt);
        }
        receipt.status = "unknown".into();
        receipt.verification = Some("unknown".into());
        for _ in 0..20 {
            let (a,t,id) = (account.clone(),target.clone(),message.to_owned());
            if self.database.execute(move |db| db.conversation_recall_observed(&a,&t,&id)).await? {
                receipt.status = "succeeded".into();
                receipt.verification = Some("verified".into());
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(receipt)
    }
    async fn mute(&self, group: i64, user: i64, seconds: i64) -> AppResult<GatewayReceipt> {
        self.member_mute(group, user, Some(seconds)).await
    }
    async fn unmute(&self, group: i64, user: i64) -> AppResult<GatewayReceipt> {
        self.member_mute(group, user, None).await
    }
    async fn rename(
        &self,
        group: i64,
        member: &MemberRef,
        name: &str,
    ) -> AppResult<GatewayReceipt> {
        if name.trim().is_empty() || name.chars().count() > 128 {
            return Err(AppError::new("invalid_name", "名片内容无效"));
        }
        let user = member
            .user_id
            .filter(|id| *id > 0)
            .ok_or_else(|| AppError::new("member_identity", "缺少有效成员身份"))?;
        let roster = self.list_members(group).await?;
        if member.nim_id.as_ref().is_some_and(|id| {
            !id.is_empty()
                && !roster
                    .members
                    .iter()
                    .any(|m| m.user_id == user && &m.nim_id == id)
        }) {
            return Err(AppError::new("member_identity", "成员身份不一致"));
        }
        let mut receipt = self
            .write(
                "/v1/group/set-member-nickname",
                json!({"groupId":group,"userId":user,"nick":name}),
                Some(user),
            )
            .await?;
        if let Ok(roster) = self.list_members(group).await {
            if roster
                .members
                .iter()
                .any(|m| m.user_id == user && m.card_name == name)
            {
                receipt.status = "succeeded".into();
                receipt.verification = Some("verified".into());
            }
        }
        Ok(receipt)
    }
    async fn remove_member(&self, group: i64, user: i64) -> AppResult<GatewayReceipt> {
        self.write(
            "/v1/group/remove-group-member",
            json!({"groupId":group,"groupMemberIds":[user]}),
            Some(user),
        )
        .await
    }
    async fn set_group_mute(&self, group: i64, muted: bool) -> AppResult<GatewayReceipt> {
        let mut receipt = self
            .write(
                "/v1/group/set-group-mute",
                json!({"groupId":group,"muteMode":if muted{"MUTE_MEMBER"}else{"MUTE_NO"}}),
                None,
            )
            .await?;
        if self
            .get_group_mute_state(group)
            .await
            .is_ok_and(|state| state.muted == muted)
        {
            receipt.status = "succeeded".into();
            receipt.verification = Some("verified".into());
        }
        Ok(receipt)
    }
    async fn get_group_mute_state(&self, group: i64) -> AppResult<GroupMuteState> {
        let (data, _) = self.group_data().await?;
        let row = ["owner", "member"]
            .iter()
            .filter_map(|k| data[*k].as_array())
            .flatten()
            .find(|v| {
                v["groupId"]
                    .as_i64()
                    .or_else(|| v["groupId"].as_str()?.parse().ok())
                    == Some(group)
            })
            .ok_or_else(|| AppError::new("group_not_found", "当前账号不可访问该群"))?;
        let mode = row
            .get("muteMode")
            .or_else(|| row.get("groupMuteMode"))
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::new("group_state", "当前业务数据未提供群发言状态"))?;
        let muted = match mode {
            "MUTE_NO" => false,
            "MUTE_MEMBER" | "MUTE_ALL" => true,
            _ => return Err(AppError::new("group_state", "群发言状态尚无法识别")),
        };
        Ok(GroupMuteState {
            group_id: group,
            muted,
            source: "business".into(),
            checked_at: chrono::Utc::now(),
        })
    }
    async fn list_group_announcements(&self, group: i64) -> AppResult<Vec<GroupAnnouncement>> {
        let epoch = self.session_epoch();
        if !self
            .list_groups()
            .await?
            .iter()
            .any(|g| g.group_id == group)
        {
            return Err(AppError::new("group_not_found", "当前账号不可访问该群"));
        }
        let data = self
            .request(
                "/v1/group/notice-list",
                &json!({"groupId":group,"v":"0"}),
                epoch,
            )
            .await?;
        let rows = data["noticeInfoList"]
            .as_array()
            .ok_or_else(|| AppError::new("notice_shape", "公告列表结构无法识别"))?;
        rows.iter()
            .map(|row| {
                let text = |keys: &[&str]| {
                    keys.iter()
                        .filter_map(|key| row.get(*key))
                        .find_map(|v| {
                            v.as_str()
                                .map(str::to_owned)
                                .or_else(|| v.as_i64().map(|n| n.to_string()))
                        })
                        .unwrap_or_default()
                };
                let id = text(&["noticeId", "id"]);
                if id.is_empty() {
                    return Err(AppError::new("notice_identity", "公告身份缺失"));
                }
                Ok(GroupAnnouncement {
                    group_id: group,
                    notice_id: id,
                    content: text(&["noticeContent", "content"]),
                    mode: text(&["noticeMode", "mode"]),
                    author_user_id: text(&["userId", "authorUserId"]).parse().unwrap_or(0),
                })
            })
            .collect()
    }
    async fn set_group_announcement(&self, group: i64, text: &str) -> AppResult<GatewayReceipt> {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > 1000 {
            return Err(AppError::new("notice_params", "公告内容应为 1 到 1000 个字符"));
        }
        let (mut receipt, data) = self.write_with_data(
            "/v1/group/add-notice",
            json!({"groupId":group,"noticeContent":text,"noticeMode":"COMMON_NOTICE"}),
            None,
        ).await?;
        let id = data.get("noticeId").or_else(|| data.get("id"))
            .and_then(|v| v.as_str().map(str::to_owned).or_else(|| v.as_i64().map(|v| v.to_string())))
            .unwrap_or_default();
        receipt.message_id = id.clone();
        if !id.is_empty() && self.list_group_announcements(group).await.is_ok_and(|rows| {
            rows.iter().any(|row| row.notice_id == id && row.content == text && row.mode == "COMMON_NOTICE")
        }) {
            receipt.status = "succeeded".into();
            receipt.verification = Some("verified".into());
        }
        Ok(receipt)
    }
    async fn get_group_announcement(&self, group: i64) -> AppResult<Option<GroupAnnouncement>> {
        Ok(self
            .list_group_announcements(group)
            .await?
            .into_iter()
            .next())
    }
    async fn update_group_announcement(
        &self,
        group: i64,
        id: &str,
        text: &str,
        mode: &str,
    ) -> AppResult<GatewayReceipt> {
        if id.is_empty()
            || text.trim().is_empty()
            || text.chars().count() > 1000
            || !matches!(mode, "COMMON_NOTICE" | "TOP_NOTICE")
        {
            return Err(AppError::new("notice_params", "公告参数无效"));
        }
        let mut receipt = self
            .write(
                "/v1/group/notice-opt",
                json!({"groupId":group,"noticeId":id,"noticeContent":text,"noticeMode":mode}),
                None,
            )
            .await?;
        if self
            .list_group_announcements(group)
            .await
            .is_ok_and(|rows| {
                rows.iter()
                    .any(|r| r.notice_id == id && r.content == text && r.mode == mode)
            })
        {
            receipt.status = "succeeded".into();
            receipt.verification = Some("verified".into());
        }
        Ok(receipt)
    }
    async fn delete_group_announcement(&self, group: i64, id: &str) -> AppResult<GatewayReceipt> {
        if id.is_empty() {
            return Err(AppError::new("notice_params", "公告身份缺失"));
        }
        let mut receipt = self
            .write(
                "/v1/group/notice-del",
                json!({"groupId":group,"noticeId":id}),
                None,
            )
            .await?;
        if self
            .list_group_announcements(group)
            .await
            .is_ok_and(|rows| !rows.iter().any(|r| r.notice_id == id))
        {
            receipt.status = "succeeded".into();
            receipt.verification = Some("verified".into());
        }
        Ok(receipt)
    }
}
#[async_trait]
impl RuntimeGateway for RustGateway {
    async fn diagnose(&self) -> DiagnosticSnapshot {
        let guard = self.session.lock().await;
        let session = guard.as_ref();
        let ready = session.is_some_and(|s| s.nim.is_some() && s.client.is_some());
        DiagnosticSnapshot {
            status: if ready {
                ConnectionStatus::Ready
            } else {
                ConnectionStatus::NimNotReady
            },
            devtools_url: String::new(),
            page_title: "DH Rust".into(),
            page_url: String::new(),
            nim_account: session
                .and_then(|s| s.nim_account.clone())
                .unwrap_or_default(),
            detail: if ready {
                "账号与即时通信已连接"
            } else {
                "等待账号登录或即时通信连接"
            }
            .into(),
            rate_limit_hits: 0,
        }
    }
    async fn session_identity(&self) -> AppResult<(i64, String)> {
        let guard = self.session.lock().await;
        let session = guard.as_ref().ok_or_else(unavailable)?;
        Ok((
            session
                .client
                .as_ref()
                .ok_or_else(unavailable)?
                .user_id()
                .map_err(protocol)?,
            session.nim_account.clone().ok_or_else(unavailable)?,
        ))
    }
    async fn install_message_listener(&self) -> AppResult<Value> {
        let epoch = self.epoch.load(Ordering::SeqCst);
        let (_, account) = self.session_identity().await?;
        let enabled = self
            .database
            .execute(move |db| db.get_setting(&format!("rust.message_verified.{account}")))
            .await?
            == Some("true".into());
        self.message_verified
            .store(if enabled { epoch } else { u64::MAX }, Ordering::SeqCst);
        Ok(json!({"ok":true,"source":"rust-nim"}))
    }
    async fn read_batch(&self) -> AppResult<GatewayBatch> {
        let (_, account) = self.session_identity().await?;
        self.database
            .execute(move |db| db.conversation_gateway_batch(&account))
            .await
    }
    async fn ack(&self, session: &str, sequence: u64) -> AppResult<GatewayReceipt> {
        let (_, account) = self.session_identity().await?;
        if session != format!("rust:{account}") {
            return Err(AppError::new("account_changed", "账号已切换"));
        }
        self.database
            .execute(move |db| db.acknowledge_conversation_gateway(&account, sequence))
            .await?;
        let mut receipt = GatewayReceipt::succeeded("rust.ledger.ack");
        receipt.session = session.into();
        receipt.acknowledged_through = sequence;
        Ok(receipt)
    }
    async fn poll_messages(&self) -> AppResult<Value> {
        Err(pending_protocol())
    }
    async fn acknowledge_messages(&self, _sequence: u64) -> AppResult<Value> {
        Err(pending_protocol())
    }
    fn session_epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }
    fn capabilities(&self) -> GatewayCapabilities {
        let unavailable = GatewayCapability::unsupported("独立协议尚待验证");
        let manual = GatewayCapability::manual_verification(
            CapabilitySource::WangElectron,
            "Rust HTTP 已接入，真实写操作待人工验收",
            "rust-http-v1",
        );
        GatewayCapabilities {
            announcement: manual.clone(),
            send_text: if self.message_verified.load(Ordering::SeqCst)
                == self.epoch.load(Ordering::SeqCst)
            {
                GatewayCapability::supported(
                    CapabilitySource::WangElectron,
                    "Rust 消息收发已完成对端回读验证",
                    "rust-nim-v1",
                )
            } else {
                GatewayCapability::manual_verification(
                    CapabilitySource::WangElectron,
                    "Rust 消息收发待对端回读验证",
                    "rust-nim-v1",
                )
            },
            mute: manual.clone(),
            recall: manual.clone(),
            rename: manual.clone(),
            remove_member: manual.clone(),
            group_mute: manual,
            member_events: unavailable,
        }
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;
    #[test]
    fn explicit_rejection_is_not_an_uncertain_transport_result() {
        assert_eq!(
            protocol(dh_protocol::business_client::Error::Business(1001)).code,
            "business_rejected"
        );
        assert_eq!(
            protocol(dh_protocol::business_client::Error::Transport).code,
            "business_transport"
        );
        assert_eq!(
            protocol(dh_protocol::business_client::Error::Business(401)).code,
            "login_required"
        );
    }
}
