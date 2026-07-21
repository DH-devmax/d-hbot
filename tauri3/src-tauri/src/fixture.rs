use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::error::{AppError, AppResult};
use crate::gateway::{DiagnosticSnapshot, GatewayCapabilities, GroupGateway, RuntimeGateway};
use crate::models::{Group, Member, MemberRef, MemberRoster};

pub const FIXTURE_ACCOUNT: &str = "fixture-nim-10001";
pub const FIXTURE_GROUP: i64 = 1_143_980;
pub const FIXTURE_DEVTOOLS_URL: &str = "http://127.0.0.1:9233";

fn fixture_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 21, 0, 0, 0)
        .single()
        .expect("fixture timestamp")
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FixtureFaults {
    pub devtools_ready: bool,
    pub nim_ready: bool,
    pub permission_denied: bool,
    pub timeout_next: bool,
    pub partial_members: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureAction {
    pub kind: String,
    pub group_id: i64,
    pub user_id: i64,
    pub text: String,
    pub duration_seconds: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureSnapshot {
    pub groups: Vec<Group>,
    pub members: Vec<Member>,
    pub queued_messages: usize,
    pub actions: Vec<FixtureAction>,
    pub faults: FixtureFaults,
}

#[derive(Debug)]
struct FixtureState {
    groups: Vec<Group>,
    members: HashMap<(i64, i64), Member>,
    messages: VecDeque<Value>,
    actions: Vec<FixtureAction>,
    faults: FixtureFaults,
    next_sequence: u64,
}

#[derive(Clone)]
pub struct FixtureGateway {
    state: Arc<RwLock<FixtureState>>,
}

impl FixtureGateway {
    pub fn new_default() -> Self {
        let now = fixture_now();
        let group = Group {
            account_id: FIXTURE_ACCOUNT.into(),
            group_id: FIXTURE_GROUP,
            name: "DH Fixture 测试群".into(),
            owner_user_id: 10001,
            enabled: false,
            ai_enabled: false,
            moderation_enabled: false,
            manual_takeover: false,
            welcome_message: "欢迎 @「[成员]」加入 DH Fixture 测试群。".into(),
            updated_at: now,
        };
        let mut members = HashMap::new();
        for user_id in 10001..=10016 {
            let role = if user_id == 10001 {
                "owner"
            } else if user_id == 10002 {
                "admin"
            } else {
                "member"
            };
            let name = match user_id {
                10003 => "已封禁用户".into(),
                10004 => "1".into(),
                10005 => ".".into(),
                _ => format!("Fixture成员{}", user_id - 10000),
            };
            members.insert(
                (FIXTURE_GROUP, user_id),
                Member {
                    account_id: FIXTURE_ACCOUNT.into(),
                    group_id: FIXTURE_GROUP,
                    user_id,
                    nim_id: format!("fixture-nim-{user_id}"),
                    nickname: name.clone(),
                    card_name: name.clone(),
                    original_card_name: name,
                    managed_card_name: String::new(),
                    card_suffix: String::new(),
                    role: role.into(),
                    account_state: if user_id == 10003 {
                        "ACCOUNT_STATE_BAN".into()
                    } else {
                        "ACCOUNT_STATE_GOOD".into()
                    },
                    blacklisted: user_id == 10003,
                    present: true,
                    join_source: "baseline".into(),
                    prompt_read: true,
                    locked_card_name: String::new(),
                    violation_count: 0,
                    discovered_at: now,
                    joined_at: None,
                    last_seen_at: now,
                    updated_at: now,
                },
            );
        }
        Self {
            state: Arc::new(RwLock::new(FixtureState {
                groups: vec![group],
                members,
                messages: VecDeque::new(),
                actions: Vec::new(),
                faults: FixtureFaults {
                    devtools_ready: true,
                    nim_ready: true,
                    ..FixtureFaults::default()
                },
                next_sequence: 1,
            })),
        }
    }

    pub async fn set_faults(&self, faults: FixtureFaults) {
        self.state.write().await.faults = faults;
    }

    pub async fn snapshot(&self) -> FixtureSnapshot {
        let state = self.state.read().await;
        FixtureSnapshot {
            groups: state.groups.clone(),
            members: state.members.values().cloned().collect(),
            queued_messages: state.messages.len(),
            actions: state.actions.clone(),
            faults: state.faults.clone(),
        }
    }

    pub async fn emit_text(&self, group_id: i64, user_id: i64, text: &str) -> AppResult<u64> {
        self.emit_message(group_id, user_id, text, None, None).await
    }

    pub async fn emit_message(
        &self,
        group_id: i64,
        user_id: i64,
        text: &str,
        sequence: Option<u64>,
        server_message_id: Option<String>,
    ) -> AppResult<u64> {
        let mut state = self.state.write().await;
        if !state.groups.iter().any(|group| group.group_id == group_id) {
            return Err(AppError::new("group_not_found", "Fixture 群不存在"));
        }
        let sequence = sequence.unwrap_or(state.next_sequence);
        state.next_sequence = state.next_sequence.max(sequence.saturating_add(1));
        let member_name = state
            .members
            .get(&(group_id, user_id))
            .map(|member| member.card_name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| format!("Fixture成员{user_id}"));
        state.messages.push_back(json!({
            "seq": sequence,
            "idServer": server_message_id.unwrap_or_else(|| format!("fixture-message-{sequence}")),
            "fromNick": member_name,
            "time": fixture_now().timestamp_millis() + sequence as i64,
            "decoded": {
                "from": {"id": user_id, "name": member_name},
                "to": {"id": group_id},
                "content": {"data": text},
                "msgFormat": 0
            }
        }));
        Ok(sequence)
    }

    pub async fn member_joined(
        &self,
        group_id: i64,
        user_id: i64,
        nim_id: Option<String>,
        name: Option<String>,
        role: Option<String>,
    ) -> AppResult<()> {
        let mut state = self.state.write().await;
        if !state.groups.iter().any(|group| group.group_id == group_id) {
            return Err(AppError::new("group_not_found", "Fixture 群不存在"));
        }
        let display_name = name
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| format!("Fixture成员{user_id}"));
        let member = state.members.entry((group_id, user_id)).or_insert(Member {
            account_id: FIXTURE_ACCOUNT.into(),
            group_id,
            user_id,
            nim_id: nim_id
                .clone()
                .unwrap_or_else(|| format!("fixture-nim-{user_id}")),
            nickname: display_name.clone(),
            card_name: display_name.clone(),
            original_card_name: display_name.clone(),
            managed_card_name: String::new(),
            card_suffix: String::new(),
            role: role.clone().unwrap_or_else(|| "member".into()),
            account_state: "ACCOUNT_STATE_GOOD".into(),
            blacklisted: false,
            present: true,
            join_source: "online-joined".into(),
            prompt_read: false,
            locked_card_name: String::new(),
            violation_count: 0,
            discovered_at: fixture_now(),
            joined_at: Some(fixture_now()),
            last_seen_at: fixture_now(),
            updated_at: fixture_now(),
        });
        member.present = true;
        member.updated_at = fixture_now();
        if let Some(value) = nim_id.filter(|value| !value.trim().is_empty()) {
            member.nim_id = value;
        }
        if let Some(value) = name.filter(|value| !value.trim().is_empty()) {
            member.nickname = value.clone();
            member.card_name = value;
        }
        if let Some(value) = role.filter(|value| !value.trim().is_empty()) {
            member.role = value;
        }
        let _ = member;
        state.actions.push(Self::action(
            "member_joined",
            group_id,
            user_id,
            &display_name,
            0,
        ));
        Ok(())
    }

    pub async fn member_left(&self, group_id: i64, user_id: i64) -> AppResult<()> {
        let mut state = self.state.write().await;
        let member = state
            .members
            .get_mut(&(group_id, user_id))
            .ok_or_else(|| AppError::new("member_not_found", "Fixture 成员不存在"))?;
        member.present = false;
        member.updated_at = fixture_now();
        state
            .actions
            .push(Self::action("member_left", group_id, user_id, "", 0));
        Ok(())
    }

    pub async fn wire_group_list(&self) -> Value {
        let snapshot = self.snapshot().await;
        let owner = snapshot
            .groups
            .iter()
            .filter(|group| group.owner_user_id > 0)
            .map(|group| {
                json!({
                    "groupId": group.group_id,
                    "groupCloudId": format!("fixture-cloud-{}", group.group_id),
                    "groupName": group.name,
                    "ownerUserId": group.owner_user_id,
                    "memberCount": snapshot.members.iter().filter(|member| member.group_id == group.group_id && member.present).count()
                })
            })
            .collect::<Vec<_>>();
        json!({"owner": owner, "member": []})
    }

    pub async fn wire_group_members(&self, group_id: i64) -> Value {
        let snapshot = self.snapshot().await;
        let mut members = snapshot
            .members
            .into_iter()
            .filter(|member| member.group_id == group_id && member.present)
            .map(|member| {
                json!({
                    "groupId": member.group_id,
                    "userId": member.user_id,
                    "nimId": member.nim_id,
                    "userNick": member.nickname,
                    "groupMemberNick": member.card_name,
                    "groupRole": member.role,
                    "accountState": member.account_state
                })
            })
            .collect::<Vec<_>>();
        if snapshot.faults.partial_members {
            members.truncate(members.len().min(8));
        }
        json!({"groupMemberInfo": members})
    }

    pub async fn wire_nim_members(&self, group_id: i64) -> Value {
        let snapshot = self.snapshot().await;
        if !snapshot.faults.nim_ready {
            return json!({"ok":false,"errorMessage":"Fixture NIM 尚未初始化","members":[]});
        }
        let mut members = snapshot
            .members
            .into_iter()
            .filter(|member| member.group_id == group_id && member.present)
            .map(|member| json!({"nimId":member.nim_id,"cardName":member.card_name,"type":member.role}))
            .collect::<Vec<_>>();
        if snapshot.faults.partial_members {
            members.truncate(members.len().min(8));
        }
        json!({"ok":true,"members":members})
    }

    pub async fn wire_route(&self, route: &str, payload: Value) -> AppResult<Value> {
        {
            let mut state = self.state.write().await;
            Self::fault_error(&mut state.faults)?;
        }
        match route {
            "/v1/group/get-group-list" => Ok(json!({"code":0,"data":self.wire_group_list().await})),
            "/v1/group/get-group-members" => Ok(
                json!({"code":0,"data":self.wire_group_members(payload.get("groupId").and_then(Value::as_i64).unwrap_or(FIXTURE_GROUP)).await}),
            ),
            "/v1/group/set-member-mute" => {
                self.mute(
                    payload.get("groupId").and_then(Value::as_i64).unwrap_or(0),
                    payload.get("userId").and_then(Value::as_i64).unwrap_or(0),
                    payload.get("min").and_then(Value::as_i64).unwrap_or(0) * 60,
                )
                .await?;
                Ok(json!({"code":0,"data":{}}))
            }
            "/v1/group/member-mute-cancel" => {
                self.unmute(
                    payload.get("groupId").and_then(Value::as_i64).unwrap_or(0),
                    payload.get("userId").and_then(Value::as_i64).unwrap_or(0),
                )
                .await?;
                Ok(json!({"code":0,"data":{}}))
            }
            "/v1/group/set-member-nickname" => {
                self.rename(
                    payload.get("groupId").and_then(Value::as_i64).unwrap_or(0),
                    &MemberRef {
                        user_id: payload.get("userId").and_then(Value::as_i64),
                        nim_id: None,
                    },
                    payload
                        .get("nick")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
                .await?;
                Ok(json!({"code":0,"data":{}}))
            }
            "/v1/group/remove-group-member" => {
                for user_id in payload
                    .get("groupMemberIds")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_i64)
                {
                    self.remove_member(
                        payload.get("groupId").and_then(Value::as_i64).unwrap_or(0),
                        user_id,
                    )
                    .await?;
                }
                Ok(json!({"code":0,"data":{}}))
            }
            "/v1/group/set-group-mute" => {
                self.set_group_mute(
                    payload.get("groupId").and_then(Value::as_i64).unwrap_or(0),
                    payload
                        .get("muteMode")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        == "MUTE_MEMBER",
                )
                .await?;
                Ok(json!({"code":0,"data":{}}))
            }
            _ => Err(AppError::new(
                "fixture_route",
                format!("Fixture 未实现路由：{route}"),
            )),
        }
    }

    fn fault_error(faults: &mut FixtureFaults) -> AppResult<()> {
        if faults.timeout_next {
            faults.timeout_next = false;
            return Err(AppError::new("fixture_timeout", "Fixture 注入了超时").retryable());
        }
        if faults.permission_denied {
            return Err(AppError::new(
                "management_required",
                "Fixture 注入了权限不足",
            ));
        }
        Ok(())
    }

    fn action(
        kind: &str,
        group_id: i64,
        user_id: i64,
        text: &str,
        duration_seconds: i64,
    ) -> FixtureAction {
        FixtureAction {
            kind: kind.into(),
            group_id,
            user_id,
            text: text.into(),
            duration_seconds,
            created_at: fixture_now().to_rfc3339(),
        }
    }
}

#[async_trait]
impl GroupGateway for FixtureGateway {
    async fn list_groups(&self) -> AppResult<Vec<Group>> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        Ok(state.groups.clone())
    }

    async fn list_members(&self, group_id: i64) -> AppResult<MemberRoster> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        let mut members = state
            .members
            .values()
            .filter(|member| member.group_id == group_id && member.present)
            .cloned()
            .collect::<Vec<_>>();
        let reported_count = members.len();
        if state.faults.partial_members {
            members.truncate(members.len().min(8));
        }
        Ok(MemberRoster {
            resolved_count: members.len(),
            reported_count,
            complete: members.len() == reported_count,
            members,
            sources: vec!["fixture-http".into(), "fixture-nim".into()],
        })
    }

    async fn send_text(&self, group_id: i64, text: &str) -> AppResult<String> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        state
            .actions
            .push(Self::action("send_text", group_id, 0, text, 0));
        Ok(format!("fixture-delivery-{}", state.actions.len()))
    }

    async fn recall(&self, group_id: i64, sender_user_id: i64, message_id: &str) -> AppResult<()> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        state.actions.push(Self::action(
            "recall",
            group_id,
            sender_user_id,
            message_id,
            0,
        ));
        Ok(())
    }

    async fn mute(&self, group_id: i64, user_id: i64, duration_seconds: i64) -> AppResult<()> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        state.actions.push(Self::action(
            "mute",
            group_id,
            user_id,
            "",
            duration_seconds,
        ));
        Ok(())
    }

    async fn unmute(&self, group_id: i64, user_id: i64) -> AppResult<()> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        state
            .actions
            .push(Self::action("unmute", group_id, user_id, "", 0));
        Ok(())
    }

    async fn rename(&self, group_id: i64, member: &MemberRef, nickname: &str) -> AppResult<()> {
        let user_id = member
            .user_id
            .ok_or_else(|| AppError::new("member_identity", "Fixture 缺少 userId"))?;
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        let target = state
            .members
            .get_mut(&(group_id, user_id))
            .ok_or_else(|| AppError::new("member_not_found", "Fixture 成员不存在"))?;
        target.card_name = nickname.into();
        state
            .actions
            .push(Self::action("rename", group_id, user_id, nickname, 0));
        Ok(())
    }

    async fn remove_member(&self, group_id: i64, user_id: i64) -> AppResult<()> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        let target = state
            .members
            .get_mut(&(group_id, user_id))
            .ok_or_else(|| AppError::new("member_not_found", "Fixture 成员不存在"))?;
        target.present = false;
        state
            .actions
            .push(Self::action("remove_member", group_id, user_id, "", 0));
        Ok(())
    }

    async fn set_group_mute(&self, group_id: i64, muted: bool) -> AppResult<()> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        state.actions.push(Self::action(
            if muted { "group_mute" } else { "group_unmute" },
            group_id,
            0,
            "",
            0,
        ));
        Ok(())
    }
}

#[async_trait]
impl RuntimeGateway for FixtureGateway {
    async fn diagnose(&self) -> DiagnosticSnapshot {
        let state = self.state.read().await;
        if !state.faults.devtools_ready {
            return DiagnosticSnapshot {
                status: crate::gateway::ConnectionStatus::Unavailable,
                devtools_url: FIXTURE_DEVTOOLS_URL.into(),
                page_title: "DH Fixture".into(),
                page_url: "http://127.0.0.1:51300".into(),
                nim_account: String::new(),
                detail: "Fixture DevTools 已断开".into(),
            };
        }
        DiagnosticSnapshot {
            status: if state.faults.nim_ready {
                crate::gateway::ConnectionStatus::Ready
            } else {
                crate::gateway::ConnectionStatus::NimNotReady
            },
            devtools_url: FIXTURE_DEVTOOLS_URL.into(),
            page_title: "DH Fixture".into(),
            page_url: "http://127.0.0.1:51300".into(),
            nim_account: if state.faults.nim_ready {
                FIXTURE_ACCOUNT.into()
            } else {
                String::new()
            },
            detail: if state.faults.nim_ready {
                "Fixture 协议会话已就绪".into()
            } else {
                "Fixture NIM 尚未初始化".into()
            },
        }
    }

    async fn session_identity(&self) -> AppResult<(i64, String)> {
        let state = self.state.read().await;
        if !state.faults.nim_ready {
            return Err(AppError::new("nim_not_ready", "Fixture NIM 尚未初始化").retryable());
        }
        Ok((10001, FIXTURE_ACCOUNT.into()))
    }

    async fn install_message_listener(&self) -> AppResult<Value> {
        Ok(json!({"ok":true,"installed":["fixture"],"queued":0}))
    }

    async fn poll_messages(&self) -> AppResult<Value> {
        let state = self.state.read().await;
        if !state.faults.devtools_ready {
            return Err(
                AppError::new("fixture_disconnected", "Fixture DevTools 已断开").retryable(),
            );
        }
        Ok(json!({"ok":true,"messages":state.messages,"remaining":state.messages.len()}))
    }

    async fn acknowledge_messages(&self, sequence: u64) -> AppResult<Value> {
        let mut state = self.state.write().await;
        let before = state.messages.len();
        state
            .messages
            .retain(|message| message.get("seq").and_then(Value::as_u64).unwrap_or(0) > sequence);
        Ok(json!({"ok":true,"acked":before-state.messages.len(),"remaining":state.messages.len()}))
    }

    fn capabilities(&self) -> GatewayCapabilities {
        GatewayCapabilities {
            announcement: false,
            mute: true,
            recall: true,
            rename: true,
            remove_member: true,
            group_mute: true,
            member_events: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_fixture_has_sixteen_members_and_records_actions() {
        let fixture = FixtureGateway::new_default();
        let roster = fixture.list_members(FIXTURE_GROUP).await.unwrap();
        assert_eq!(roster.reported_count, 16);
        fixture.mute(FIXTURE_GROUP, 10006, 600).await.unwrap();
        fixture
            .rename(
                FIXTURE_GROUP,
                &MemberRef {
                    user_id: Some(10006),
                    nim_id: None,
                },
                "DH群员0001",
            )
            .await
            .unwrap();
        assert_eq!(fixture.snapshot().await.actions.len(), 2);
    }

    #[tokio::test]
    async fn fixture_message_is_idempotently_acknowledged() {
        let fixture = FixtureGateway::new_default();
        fixture
            .emit_text(FIXTURE_GROUP, 10006, "@DH 测试")
            .await
            .unwrap();
        let batch = fixture.poll_messages().await.unwrap();
        assert_eq!(batch["messages"].as_array().unwrap().len(), 1);
        fixture.acknowledge_messages(1).await.unwrap();
        assert_eq!(fixture.snapshot().await.queued_messages, 0);
    }
}
