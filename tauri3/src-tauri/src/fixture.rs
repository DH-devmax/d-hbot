use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::contracts::ContractReplayEngine;
use crate::error::{AppError, AppResult};
use crate::gateway::{
    DiagnosticSnapshot, GatewayBatch, GatewayCapabilities, GatewayEvent, GatewayReceipt,
    GatewayRecord, GatewayRecordKind, GroupGateway, RuntimeGateway,
};
use crate::models::{
    Group, GroupAnnouncement, GroupMuteState, Member, MemberRef, MemberRoster, RosterCompleteness,
};

pub const FIXTURE_ACCOUNT: &str = "fixture-nim-10001";
pub const FIXTURE_GROUP: i64 = 1_143_980;
pub const FIXTURE_SECOND_GROUP: i64 = 1_143_981;
pub const FIXTURE_DEVTOOLS_URL: &str = "http://127.0.0.1:9233";
const FIXTURE_SESSION: &str = "fixture-session-1";

fn fixture_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 21, 0, 0, 0)
        .single()
        .expect("fixture timestamp")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FixtureFaults {
    pub devtools_ready: bool,
    pub nim_ready: bool,
    pub permission_denied: bool,
    pub timeout_next: bool,
    pub partial_members: bool,
    pub member_page_size: usize,
    pub nim_members_error_code: Option<i64>,
    pub nim_members_error_message: Option<String>,
    pub nim_send_timeout_after_effect: bool,
}

impl Default for FixtureFaults {
    fn default() -> Self {
        Self {
            devtools_ready: false,
            nim_ready: false,
            permission_denied: false,
            timeout_next: false,
            partial_members: false,
            member_page_size: 50,
            nim_members_error_code: None,
            nim_members_error_message: None,
            nim_send_timeout_after_effect: false,
        }
    }
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
pub struct FixtureEvent {
    pub kind: String,
    pub group_id: i64,
    pub user_id: i64,
    pub sequence: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureMemberMute {
    pub group_id: i64,
    pub user_id: i64,
    pub duration_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureAnnouncement {
    pub group_id: i64,
    pub notice_id: String,
    pub content: String,
    pub mode: String,
}

#[derive(Debug, Clone)]
pub struct FixtureMessageOptions {
    pub msg_format: i64,
    pub flow: String,
    pub scene: String,
    pub decoded: bool,
    pub source: String,
}

impl Default for FixtureMessageOptions {
    fn default() -> Self {
        Self {
            msg_format: 0,
            flow: "in".into(),
            scene: "team".into(),
            decoded: true,
            source: "onmsg".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureSnapshot {
    pub groups: Vec<Group>,
    pub members: Vec<Member>,
    pub queued_messages: usize,
    pub queued_events: usize,
    pub actions: Vec<FixtureAction>,
    pub events: Vec<FixtureEvent>,
    pub member_mutes: Vec<FixtureMemberMute>,
    pub group_mutes: Vec<i64>,
    pub announcements: Vec<FixtureAnnouncement>,
    pub recalled_messages: Vec<String>,
    pub faults: FixtureFaults,
}

#[derive(Debug)]
struct FixtureState {
    groups: Vec<Group>,
    members: BTreeMap<(i64, i64), Member>,
    queue: BTreeMap<u64, GatewayRecord>,
    seen_message_ids: BTreeMap<String, u64>,
    actions: Vec<FixtureAction>,
    events: Vec<FixtureEvent>,
    member_mutes: BTreeMap<(i64, i64), i64>,
    group_mutes: BTreeSet<i64>,
    announcements: BTreeMap<i64, FixtureAnnouncement>,
    recalled_messages: BTreeSet<String>,
    faults: FixtureFaults,
    next_sequence: u64,
    delivered_event_sequence: u64,
}

#[derive(Clone)]
pub struct FixtureGateway {
    state: Arc<RwLock<FixtureState>>,
    contract: Arc<ContractReplayEngine>,
    capabilities: GatewayCapabilities,
}

impl FixtureGateway {
    fn generated_member(group_id: i64, user_id: i64) -> Member {
        let now = fixture_now();
        let role = if user_id == 10001 {
            "owner"
        } else if user_id == 10002 {
            "admin"
        } else {
            "member"
        };
        let name = match (group_id, user_id) {
            (FIXTURE_GROUP, 10003) => "已封禁用户".into(),
            (FIXTURE_GROUP, 10004) => "1".into(),
            (FIXTURE_GROUP, 10005) => ".".into(),
            (FIXTURE_SECOND_GROUP, _) => format!("第二群成员{}", user_id - 10000),
            _ => format!("Fixture成员{}", user_id - 10000),
        };
        Member {
            account_id: FIXTURE_ACCOUNT.into(),
            group_id,
            user_id,
            nim_id: if group_id == FIXTURE_SECOND_GROUP {
                format!("fixture-second-nim-{user_id}")
            } else {
                format!("fixture-nim-{user_id}")
            },
            nickname: name.clone(),
            card_name: name.clone(),
            original_card_name: name,
            managed_card_name: String::new(),
            card_suffix: String::new(),
            role: role.into(),
            account_state: if group_id == FIXTURE_GROUP && user_id == 10003 {
                "ACCOUNT_STATE_BAN".into()
            } else {
                "ACCOUNT_STATE_GOOD".into()
            },
            blacklisted: group_id == FIXTURE_GROUP && user_id == 10003,
            present: true,
            join_source: "baseline".into(),
            prompt_read: true,
            locked_card_name: String::new(),
            violation_count: 0,
            discovered_at: now,
            joined_at: None,
            last_seen_at: now,
            updated_at: now,
        }
    }

    fn receipt(&self, route: &str, ordinal: usize, message_id: String) -> GatewayReceipt {
        self.contract
            .success_receipt(route, ordinal, message_id)
            .expect("Fixture action must exist in frozen Contract v2")
    }

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
            machine_rules_enabled: false,
            ai_rules_enabled: false,
            manual_takeover: false,
            welcome_message: "欢迎 @「[成员]」加入 DH Fixture 测试群。".into(),
            updated_at: now,
        };
        let second_group = Group {
            account_id: FIXTURE_ACCOUNT.into(),
            group_id: FIXTURE_SECOND_GROUP,
            name: "DH Fixture 第二测试群".into(),
            owner_user_id: 10001,
            enabled: false,
            ai_enabled: false,
            moderation_enabled: false,
            machine_rules_enabled: false,
            ai_rules_enabled: false,
            manual_takeover: false,
            welcome_message: "欢迎 @「[成员]」加入 DH Fixture 第二测试群。".into(),
            updated_at: now,
        };
        let mut members = BTreeMap::new();
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
        for user_id in 10001..=10004 {
            let role = if user_id == 10001 {
                "owner"
            } else if user_id == 10002 {
                "admin"
            } else {
                "member"
            };
            let name = format!("第二群成员{}", user_id - 10000);
            members.insert(
                (FIXTURE_SECOND_GROUP, user_id),
                Member {
                    account_id: FIXTURE_ACCOUNT.into(),
                    group_id: FIXTURE_SECOND_GROUP,
                    user_id,
                    nim_id: format!("fixture-second-nim-{user_id}"),
                    nickname: name.clone(),
                    card_name: name.clone(),
                    original_card_name: name,
                    managed_card_name: String::new(),
                    card_suffix: String::new(),
                    role: role.into(),
                    account_state: "ACCOUNT_STATE_GOOD".into(),
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
                },
            );
        }
        let contract = Arc::new(ContractReplayEngine::frozen());
        let metadata = contract.metadata();
        let capabilities =
            contract.capabilities_for(&metadata.app_file_version, &metadata.main_script_sha256);
        Self {
            state: Arc::new(RwLock::new(FixtureState {
                groups: vec![group, second_group],
                members,
                queue: BTreeMap::new(),
                seen_message_ids: BTreeMap::new(),
                actions: Vec::new(),
                events: Vec::new(),
                member_mutes: BTreeMap::new(),
                group_mutes: BTreeSet::new(),
                announcements: BTreeMap::new(),
                recalled_messages: BTreeSet::new(),
                faults: FixtureFaults {
                    devtools_ready: true,
                    nim_ready: true,
                    ..FixtureFaults::default()
                },
                next_sequence: 1,
                delivered_event_sequence: 0,
            })),
            contract,
            capabilities,
        }
    }

    pub fn new_with_calibration(app_file_version: &str, main_script_sha256: &str) -> Self {
        let mut fixture = Self::new_default();
        fixture.capabilities = fixture
            .contract
            .capabilities_for(app_file_version, main_script_sha256);
        fixture
    }

    pub async fn set_faults(&self, faults: FixtureFaults) {
        self.state.write().await.faults = faults;
    }

    pub async fn resize_group_members(&self, group_id: i64, count: usize) -> AppResult<()> {
        if !(1..=2_000).contains(&count) {
            return Err(AppError::new(
                "fixture_member_count",
                "Fixture 成员数必须介于 1 和 2000",
            ));
        }
        let mut state = self.state.write().await;
        if !state.groups.iter().any(|group| group.group_id == group_id) {
            return Err(AppError::new("group_not_found", "Fixture 群不存在"));
        }
        state
            .members
            .retain(|(current_group, _), _| *current_group != group_id);
        state
            .member_mutes
            .retain(|(current_group, _), _| *current_group != group_id);
        for offset in 0..count {
            let user_id = 10001 + offset as i64;
            state.members.insert(
                (group_id, user_id),
                Self::generated_member(group_id, user_id),
            );
        }
        Ok(())
    }

    pub async fn snapshot(&self) -> FixtureSnapshot {
        let state = self.state.read().await;
        FixtureSnapshot {
            groups: state.groups.clone(),
            members: state.members.values().cloned().collect(),
            queued_messages: state
                .queue
                .values()
                .filter(|record| record.kind == GatewayRecordKind::Message)
                .count(),
            queued_events: state
                .queue
                .values()
                .filter(|record| record.kind != GatewayRecordKind::Message)
                .count(),
            actions: state.actions.clone(),
            events: state.events.clone(),
            member_mutes: state
                .member_mutes
                .iter()
                .map(
                    |(&(group_id, user_id), &duration_seconds)| FixtureMemberMute {
                        group_id,
                        user_id,
                        duration_seconds,
                    },
                )
                .collect(),
            group_mutes: state.group_mutes.iter().copied().collect(),
            announcements: state.announcements.values().cloned().collect(),
            recalled_messages: state.recalled_messages.iter().cloned().collect(),
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
        self.emit_message_variant(
            group_id,
            user_id,
            text,
            sequence,
            server_message_id,
            FixtureMessageOptions::default(),
        )
        .await
    }

    pub async fn emit_message_variant(
        &self,
        group_id: i64,
        user_id: i64,
        text: &str,
        sequence: Option<u64>,
        server_message_id: Option<String>,
        options: FixtureMessageOptions,
    ) -> AppResult<u64> {
        let mut state = self.state.write().await;
        if !state.groups.iter().any(|group| group.group_id == group_id) {
            return Err(AppError::new("group_not_found", "Fixture 群不存在"));
        }
        let server_message_id = server_message_id
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                format!(
                    "fixture-message-{}",
                    sequence.unwrap_or(state.next_sequence)
                )
            });
        if let Some(existing) = state.seen_message_ids.get(&server_message_id) {
            return Ok(*existing);
        }
        let sequence = sequence.unwrap_or(state.next_sequence);
        if sequence == 0 || state.queue.contains_key(&sequence) {
            return Err(AppError::new(
                "fixture_sequence",
                "Fixture 消息序号必须为非零且不重复",
            ));
        }
        state.next_sequence = state.next_sequence.max(sequence.saturating_add(1));
        let member_name = state
            .members
            .get(&(group_id, user_id))
            .map(|member| member.card_name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| format!("Fixture成员{user_id}"));
        let decoded = options.decoded.then(|| {
            json!({
                "from": {"id": user_id, "name": member_name.clone()},
                "to": {"id": group_id},
                "content": {"data": text},
                "msgFormat": options.msg_format,
                "msgSession": 2
            })
        });
        let payload = json!({
            "idServer": server_message_id.clone(),
            "fromNick": member_name,
            "time": fixture_now().timestamp_millis() + sequence as i64,
            "from": user_id.to_string(),
            "to": group_id.to_string(),
            "flow": options.flow,
            "scene": options.scene,
            "type": "custom",
            "text": text,
            "content": text,
            "msgFormat": options.msg_format,
            "decoded": decoded
        });
        state.seen_message_ids.insert(server_message_id, sequence);
        state.queue.insert(
            sequence,
            GatewayRecord {
                session: FIXTURE_SESSION.into(),
                sequence,
                kind: GatewayRecordKind::Message,
                source: options.source,
                payload,
            },
        );
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
        member.join_source = "online-joined".into();
        member.joined_at = Some(fixture_now());
        member.last_seen_at = fixture_now();
        let member = member.clone();
        let sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.saturating_add(1);
        state
            .events
            .push(Self::event("member_joined", group_id, user_id, sequence));
        state.queue.insert(
            sequence,
            GatewayRecord {
                session: FIXTURE_SESSION.into(),
                sequence,
                kind: GatewayRecordKind::TeamMemberJoined,
                source: "onaddteammembers".into(),
                payload: json!({
                    "teamId": format!("fixture-cloud-{group_id}"),
                    "groupId": group_id,
                    "members": [{
                        "nimId": member.nim_id,
                        "userId": member.user_id,
                        "nickname": member.card_name,
                        "type": member.role,
                    }],
                    "confirmed": true,
                }),
            },
        );
        Ok(())
    }

    pub async fn member_left(&self, group_id: i64, user_id: i64) -> AppResult<()> {
        let mut state = self.state.write().await;
        let member = state
            .members
            .get_mut(&(group_id, user_id))
            .ok_or_else(|| AppError::new("member_not_found", "Fixture 成员不存在"))?;
        member.present = false;
        member.last_seen_at = fixture_now();
        member.updated_at = fixture_now();
        let member = member.clone();
        state.member_mutes.remove(&(group_id, user_id));
        let sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.saturating_add(1);
        state
            .events
            .push(Self::event("member_left", group_id, user_id, sequence));
        state.queue.insert(
            sequence,
            GatewayRecord {
                session: FIXTURE_SESSION.into(),
                sequence,
                kind: GatewayRecordKind::TeamMemberLeft,
                source: "onremoveteammembers".into(),
                payload: Self::member_event_payload(group_id, &member),
            },
        );
        Ok(())
    }

    pub async fn member_updated(
        &self,
        group_id: i64,
        user_id: i64,
        name: Option<String>,
        role: Option<String>,
        account_state: Option<String>,
        blacklisted: Option<bool>,
    ) -> AppResult<()> {
        let mut state = self.state.write().await;
        let member = state
            .members
            .get_mut(&(group_id, user_id))
            .ok_or_else(|| AppError::new("member_not_found", "Fixture 成员不存在"))?;
        if let Some(value) = name.filter(|value| !value.trim().is_empty()) {
            member.nickname = value.clone();
            member.card_name = value;
        }
        if let Some(value) = role.filter(|value| !value.trim().is_empty()) {
            member.role = value;
        }
        if let Some(value) = account_state.filter(|value| !value.trim().is_empty()) {
            member.account_state = value;
        }
        if let Some(value) = blacklisted {
            member.blacklisted = value;
        }
        member.last_seen_at = fixture_now();
        member.updated_at = fixture_now();
        let member = member.clone();
        let sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.saturating_add(1);
        state
            .events
            .push(Self::event("member_updated", group_id, user_id, sequence));
        state.queue.insert(
            sequence,
            GatewayRecord {
                session: FIXTURE_SESSION.into(),
                sequence,
                kind: GatewayRecordKind::TeamMemberUpdated,
                source: "onupdateteammember".into(),
                payload: Self::member_event_payload(group_id, &member),
            },
        );
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
                    "memberCount": snapshot.members.iter().filter(|member| member.group_id == group.group_id && member.present).count(),
                    "muteMode": if snapshot.group_mutes.contains(&group.group_id) { "MUTE_MEMBER" } else { "MUTE_NO" }
                })
            })
            .collect::<Vec<_>>();
        json!({"owner": owner, "member": []})
    }

    pub async fn wire_notice_list(&self, group_id: i64) -> Value {
        let state = self.state.read().await;
        let notices = state
            .announcements
            .get(&group_id)
            .map(|notice| {
                vec![json!({
                    "groupId": group_id,
                    "noticeId": notice.notice_id,
                    "noticeContent": notice.content,
                    "noticeMode": "COMMON_NOTICE",
                    "userId": 10001
                })]
            })
            .unwrap_or_default();
        json!({"noticeInfoList": notices})
    }

    async fn update_group_announcement(
        &self,
        group_id: i64,
        notice_id: &str,
        text: &str,
    ) -> AppResult<()> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        let notice = state
            .announcements
            .get_mut(&group_id)
            .filter(|notice| notice.notice_id == notice_id)
            .ok_or_else(|| AppError::new("notice_not_found", "Fixture 群公告不存在"))?;
        notice.content = text.into();
        state
            .actions
            .push(Self::action("group_announcement", group_id, 0, text, 0));
        Ok(())
    }

    pub async fn wire_group_members_page(&self, group_id: i64, cursor: Option<&str>) -> Value {
        let snapshot = self.snapshot().await;
        let mut members = snapshot
            .members
            .into_iter()
            .filter(|member| member.group_id == group_id && member.present)
            .map(|member| {
                let account_state =
                    if snapshot.member_mutes.iter().any(|mute| {
                        mute.group_id == member.group_id && mute.user_id == member.user_id
                    }) {
                        "ACCOUNT_STATE_MUTE"
                    } else if member.account_state.is_empty() {
                        "ACCOUNT_STATE_GOOD"
                    } else {
                        member.account_state.as_str()
                    };
                json!({
                    "groupId": member.group_id,
                    "userId": member.user_id,
                    "nimId": member.nim_id,
                    "userNick": member.nickname,
                    "groupMemberNick": member.card_name,
                    "groupRole": member.role,
                    "accountState": account_state
                })
            })
            .collect::<Vec<_>>();
        if snapshot.faults.partial_members {
            members.truncate(members.len().min(8));
        }
        let offset = cursor
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0)
            .min(members.len());
        let page_size = snapshot.faults.member_page_size.clamp(1, 1_000);
        let end = offset.saturating_add(page_size).min(members.len());
        let next_cursor = (end < members.len()).then(|| end.to_string());
        json!({"groupMemberInfo": members[offset..end], "nextCursor": next_cursor})
    }

    pub async fn wire_nim_members(&self, group_id: i64) -> Value {
        self.wire_nim_members_page(group_id, None).await
    }

    pub async fn wire_nim_members_page(&self, group_id: i64, cursor: Option<&str>) -> Value {
        let snapshot = self.snapshot().await;
        if !snapshot.faults.nim_ready {
            return json!({"ok":false,"errorCode":503,"errorMessage":"Fixture NIM 尚未初始化","members":[]});
        }
        if let Some(code) = snapshot.faults.nim_members_error_code {
            return json!({
                "ok": false,
                "errorCode": code,
                "errorMessage": snapshot
                    .faults
                    .nim_members_error_message
                    .as_deref()
                    .unwrap_or("Fixture NIM 成员请求失败"),
                "members": []
            });
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
        let offset = cursor
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0)
            .min(members.len());
        let page_size = snapshot.faults.member_page_size.clamp(1, 1_000);
        let end = offset.saturating_add(page_size).min(members.len());
        let next_cursor = (end < members.len()).then(|| end.to_string());
        json!({"ok":true,"members":members[offset..end],"nextCursor":next_cursor})
    }

    pub async fn wire_route(&self, route: &str, payload: Value) -> AppResult<Value> {
        {
            let mut state = self.state.write().await;
            if state.faults.timeout_next {
                state.faults.timeout_next = false;
                return Err(AppError::new("fixture_timeout", "Fixture 注入了超时").retryable());
            }
            if state.faults.permission_denied {
                return Ok(Self::business_error(403, 403, "Fixture 注入了权限不足"));
            }
        }
        if !matches!(route, "/v1/group/notice-list" | "/v1/group/notice-opt") {
            self.contract.validate_request(route, &payload)?;
        }
        let result: AppResult<Value> = async {
            match route {
                "/v1/group/get-group-list" => Ok(self.wire_group_list().await),
                "/v1/group/get-group-members" => Ok(self
                    .wire_group_members_page(
                        payload
                            .get("groupId")
                            .and_then(Value::as_i64)
                            .unwrap_or(FIXTURE_GROUP),
                        payload.get("cursor").and_then(Value::as_str),
                    )
                    .await),
                "/v1/group/set-member-mute" => Self::receipt_value(
                    self.mute(
                        payload.get("groupId").and_then(Value::as_i64).unwrap_or(0),
                        payload.get("userId").and_then(Value::as_i64).unwrap_or(0),
                        payload.get("min").and_then(Value::as_i64).unwrap_or(0) * 60,
                    )
                    .await?,
                ),
                "/v1/group/member-mute-cancel" => Self::receipt_value(
                    self.unmute(
                        payload.get("groupId").and_then(Value::as_i64).unwrap_or(0),
                        payload.get("userId").and_then(Value::as_i64).unwrap_or(0),
                    )
                    .await?,
                ),
                "/v1/group/set-member-nickname" => Self::receipt_value(
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
                    .await?,
                ),
                "/v1/group/message-rollback" => Self::receipt_value(
                    self.recall(
                        payload
                            .get("groupId")
                            .and_then(Value::as_i64)
                            .unwrap_or(FIXTURE_GROUP),
                        payload.get("userId").and_then(Value::as_i64).unwrap_or(0),
                        payload
                            .get("msgId")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )
                    .await?,
                ),
                "/v1/group/remove-group-member" => {
                    let mut receipt = None;
                    for user_id in payload
                        .get("groupMemberIds")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_i64)
                    {
                        receipt = Some(
                            self.remove_member(
                                payload.get("groupId").and_then(Value::as_i64).unwrap_or(0),
                                user_id,
                            )
                            .await?,
                        );
                    }
                    Self::receipt_value(receipt.ok_or_else(|| {
                        AppError::new("invalid_argument", "Fixture 移出成员列表为空")
                    })?)
                }
                "/v1/group/set-group-mute" => Self::receipt_value(
                    self.set_group_mute(
                        payload.get("groupId").and_then(Value::as_i64).unwrap_or(0),
                        payload
                            .get("muteMode")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            == "MUTE_MEMBER",
                    )
                    .await?,
                ),
                "/v1/group/notice-list" => Ok(self
                    .wire_notice_list(payload.get("groupId").and_then(Value::as_i64).unwrap_or(0))
                    .await),
                "/v1/group/add-notice" => {
                    let group_id = payload.get("groupId").and_then(Value::as_i64).unwrap_or(0);
                    self.set_group_announcement(
                        group_id,
                        payload
                            .get("noticeContent")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )
                    .await?;
                    let notice = self
                        .get_group_announcement(group_id)
                        .await?
                        .ok_or_else(|| AppError::new("notice_not_found", "Fixture 群公告不存在"))?;
                    Ok(json!({"noticeId":notice.notice_id}))
                }
                "/v1/group/notice-opt" => {
                    let group_id = payload.get("groupId").and_then(Value::as_i64).unwrap_or(0);
                    let notice_id = payload
                        .get("noticeId")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    self.update_group_announcement(
                        group_id,
                        notice_id,
                        payload
                            .get("noticeContent")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )
                    .await?;
                    Ok(json!({"noticeId":notice_id}))
                }
                _ => Err(AppError::new(
                    "fixture_route",
                    format!("Fixture 未实现路由：{route}"),
                )),
            }
        }
        .await;
        match result {
            Ok(data) => Ok(Self::business_ok(data)),
            Err(error) => Ok(Self::business_error(
                Self::business_status(&error),
                Self::business_status(&error),
                &error.message,
            )),
        }
    }

    fn business_ok(data: Value) -> Value {
        json!({"code":0,"errno":0,"msg":"ok","data":data})
    }

    fn receipt_value(receipt: GatewayReceipt) -> AppResult<Value> {
        serde_json::to_value(receipt)
            .map_err(|error| AppError::new("fixture_receipt", error.to_string()))
    }

    fn business_error(code: i64, errno: i64, message: &str) -> Value {
        json!({"code":code,"errno":errno,"msg":message,"data":null})
    }

    fn business_status(error: &AppError) -> i64 {
        match error.code.as_str() {
            "management_required" => 403,
            "member_not_found" | "group_not_found" | "fixture_route" => 404,
            "invalid_argument" | "member_identity" => 400,
            _ => 500,
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

    fn event(kind: &str, group_id: i64, user_id: i64, sequence: u64) -> FixtureEvent {
        FixtureEvent {
            kind: kind.into(),
            group_id,
            user_id,
            sequence,
            created_at: fixture_now().to_rfc3339(),
        }
    }

    fn member_event_payload(group_id: i64, member: &Member) -> Value {
        json!({
            "teamId": format!("fixture-cloud-{group_id}"),
            "groupId": group_id,
            "members": [{
                "nimId": member.nim_id,
                "userId": member.user_id,
                "nickname": member.card_name,
                "type": member.role,
                "accountState": member.account_state,
                "present": member.present,
            }],
            "confirmed": true,
        })
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
        let source_reported_count = members.len();
        let partial_members = state.faults.partial_members;
        if partial_members {
            members.truncate(members.len().min(8));
        }
        let reported_count = members.len();
        Ok(MemberRoster {
            status: if partial_members { "partial" } else { "ready" }.into(),
            resolved_count: members.len(),
            reported_count,
            complete: !partial_members,
            completeness: if partial_members {
                RosterCompleteness::Partial
            } else {
                RosterCompleteness::Complete
            },
            completeness_reason: if !partial_members {
                "Fixture 权威完整名单".into()
            } else {
                "Fixture 注入了部分成员名单".into()
            },
            http_returned_count: members.len(),
            http_reported_count: source_reported_count,
            http_cursor: None,
            nim_returned_count: members.len(),
            nim_reported_count: source_reported_count,
            nim_cursor: None,
            authority: if partial_members {
                "partial"
            } else {
                "authoritative"
            }
            .into(),
            members,
            sources: vec!["fixture-http".into(), "fixture-nim".into()],
            source_errors: Vec::new(),
            retry_at: None,
            canonical_count: reported_count,
            synthetic_user_ids: Vec::new(),
            http_pages: 1,
            nim_pages: 1,
        })
    }

    async fn send_text(&self, group_id: i64, text: &str) -> AppResult<GatewayReceipt> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        if !state.groups.iter().any(|group| group.group_id == group_id) {
            return Err(AppError::new("group_not_found", "Fixture 群不存在"));
        }
        if text.trim().is_empty() {
            return Err(AppError::new("invalid_argument", "Fixture 发送内容为空"));
        }
        state
            .actions
            .push(Self::action("send_text", group_id, 0, text, 0));
        let ordinal = state.actions.len();
        Ok(self.receipt(
            "nim.sendCustomMsg",
            ordinal,
            format!("fixture-delivery-{ordinal}"),
        ))
    }

    async fn recall(
        &self,
        group_id: i64,
        sender_user_id: i64,
        message_id: &str,
    ) -> AppResult<GatewayReceipt> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        if !state.groups.iter().any(|group| group.group_id == group_id) {
            return Err(AppError::new("group_not_found", "Fixture 群不存在"));
        }
        if sender_user_id <= 0 || message_id.trim().is_empty() {
            return Err(AppError::new("invalid_argument", "Fixture 撤回参数无效"));
        }
        state.queue.retain(|_, record| {
            record.payload.get("idServer").and_then(Value::as_str) != Some(message_id)
        });
        state.recalled_messages.insert(message_id.into());
        state.actions.push(Self::action(
            "recall",
            group_id,
            sender_user_id,
            message_id,
            0,
        ));
        Ok(self.receipt(
            "/v1/group/message-rollback",
            state.actions.len(),
            message_id.into(),
        ))
    }

    async fn mute(
        &self,
        group_id: i64,
        user_id: i64,
        duration_seconds: i64,
    ) -> AppResult<GatewayReceipt> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        let member = state
            .members
            .get(&(group_id, user_id))
            .filter(|member| member.present)
            .ok_or_else(|| AppError::new("member_not_found", "Fixture 成员不存在"))?;
        if member.role == "owner" {
            return Err(AppError::new(
                "management_required",
                "Fixture 群主不可被禁言",
            ));
        }
        if duration_seconds <= 0 {
            return Err(AppError::new(
                "invalid_argument",
                "Fixture 禁言时长必须大于零",
            ));
        }
        state
            .member_mutes
            .insert((group_id, user_id), duration_seconds);
        state.actions.push(Self::action(
            "mute",
            group_id,
            user_id,
            "",
            duration_seconds,
        ));
        Ok(self.receipt(
            "/v1/group/set-member-mute",
            state.actions.len(),
            String::new(),
        ))
    }

    async fn unmute(&self, group_id: i64, user_id: i64) -> AppResult<GatewayReceipt> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        if !state.members.contains_key(&(group_id, user_id)) {
            return Err(AppError::new("member_not_found", "Fixture 成员不存在"));
        }
        state.member_mutes.remove(&(group_id, user_id));
        state
            .actions
            .push(Self::action("unmute", group_id, user_id, "", 0));
        Ok(self.receipt(
            "/v1/group/member-mute-cancel",
            state.actions.len(),
            String::new(),
        ))
    }

    async fn rename(
        &self,
        group_id: i64,
        member: &MemberRef,
        nickname: &str,
    ) -> AppResult<GatewayReceipt> {
        let user_id = member
            .user_id
            .ok_or_else(|| AppError::new("member_identity", "Fixture 缺少 userId"))?;
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        let target = state
            .members
            .get_mut(&(group_id, user_id))
            .ok_or_else(|| AppError::new("member_not_found", "Fixture 成员不存在"))?;
        if nickname.trim().is_empty() {
            return Err(AppError::new("invalid_argument", "Fixture 群名片为空"));
        }
        target.card_name = nickname.into();
        target.managed_card_name = nickname.into();
        target.updated_at = fixture_now();
        state
            .actions
            .push(Self::action("rename", group_id, user_id, nickname, 0));
        Ok(self.receipt(
            "/v1/group/set-member-nickname",
            state.actions.len(),
            String::new(),
        ))
    }

    async fn remove_member(&self, group_id: i64, user_id: i64) -> AppResult<GatewayReceipt> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        let target = state
            .members
            .get_mut(&(group_id, user_id))
            .ok_or_else(|| AppError::new("member_not_found", "Fixture 成员不存在"))?;
        target.present = false;
        target.last_seen_at = fixture_now();
        target.updated_at = fixture_now();
        state.member_mutes.remove(&(group_id, user_id));
        state
            .actions
            .push(Self::action("remove_member", group_id, user_id, "", 0));
        Ok(self.receipt(
            "/v1/group/remove-group-member",
            state.actions.len(),
            String::new(),
        ))
    }

    async fn set_group_mute(&self, group_id: i64, muted: bool) -> AppResult<GatewayReceipt> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        if !state.groups.iter().any(|group| group.group_id == group_id) {
            return Err(AppError::new("group_not_found", "Fixture 群不存在"));
        }
        if muted {
            state.group_mutes.insert(group_id);
        } else {
            state.group_mutes.remove(&group_id);
        }
        state.actions.push(Self::action(
            if muted { "group_mute" } else { "group_unmute" },
            group_id,
            0,
            "",
            0,
        ));
        Ok(self.receipt(
            "/v1/group/set-group-mute",
            state.actions.len(),
            String::new(),
        ))
    }

    async fn get_group_mute_state(&self, group_id: i64) -> AppResult<GroupMuteState> {
        let state = self.state.read().await;
        if !state.groups.iter().any(|group| group.group_id == group_id) {
            return Err(AppError::new("group_not_found", "Fixture 群不存在"));
        }
        Ok(GroupMuteState {
            group_id,
            muted: state.group_mutes.contains(&group_id),
            source: "fixture-state".into(),
            checked_at: fixture_now(),
        })
    }

    async fn get_group_announcement(&self, group_id: i64) -> AppResult<Option<GroupAnnouncement>> {
        let state = self.state.read().await;
        Ok(state
            .announcements
            .get(&group_id)
            .map(|value| GroupAnnouncement {
                group_id,
                notice_id: value.notice_id.clone(),
                content: value.content.clone(),
                mode: value.mode.clone(),
                author_user_id: 10001,
            }))
    }

    async fn list_group_announcements(&self, group_id: i64) -> AppResult<Vec<GroupAnnouncement>> {
        Ok(self
            .get_group_announcement(group_id)
            .await?
            .into_iter()
            .collect())
    }

    async fn set_group_announcement(&self, group_id: i64, text: &str) -> AppResult<GatewayReceipt> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        if !state.groups.iter().any(|group| group.group_id == group_id) {
            return Err(AppError::new("group_not_found", "Fixture 群不存在"));
        }
        if text.trim().is_empty() {
            return Err(AppError::new("invalid_argument", "Fixture 群公告为空"));
        }
        let ordinal = state.actions.len() + 1;
        let notice_id = format!("fixture-notice-{ordinal}");
        state.announcements.insert(
            group_id,
            FixtureAnnouncement {
                group_id,
                notice_id,
                content: text.into(),
                mode: "COMMON_NOTICE".into(),
            },
        );
        state
            .actions
            .push(Self::action("group_announcement", group_id, 0, text, 0));
        Ok(self.receipt("/v1/group/add-notice", ordinal, String::new()))
    }

    async fn update_group_announcement(
        &self,
        group_id: i64,
        notice_id: &str,
        text: &str,
        mode: &str,
    ) -> AppResult<GatewayReceipt> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        let announcement = state
            .announcements
            .get_mut(&group_id)
            .filter(|announcement| announcement.notice_id == notice_id)
            .ok_or_else(|| AppError::new("notice_not_found", "Fixture 公告不存在"))?;
        announcement.content = text.into();
        announcement.mode = mode.into();
        let ordinal = state.actions.len() + 1;
        state.actions.push(Self::action(
            "group_announcement_updated",
            group_id,
            0,
            text,
            0,
        ));
        let mut receipt = self.receipt("/v1/group/notice-opt", ordinal, String::new());
        receipt.verification = Some("verified".into());
        Ok(receipt)
    }

    async fn delete_group_announcement(
        &self,
        group_id: i64,
        notice_id: &str,
    ) -> AppResult<GatewayReceipt> {
        let mut state = self.state.write().await;
        Self::fault_error(&mut state.faults)?;
        let exists = state
            .announcements
            .get(&group_id)
            .is_some_and(|announcement| announcement.notice_id == notice_id);
        if !exists {
            return Err(AppError::new("notice_not_found", "Fixture 公告不存在"));
        }
        state.announcements.remove(&group_id);
        let ordinal = state.actions.len() + 1;
        state.actions.push(Self::action(
            "group_announcement_deleted",
            group_id,
            0,
            notice_id,
            0,
        ));
        let mut receipt = self.receipt("/v1/group/notice-del", ordinal, String::new());
        receipt.verification = Some("verified".into());
        Ok(receipt)
    }
}

#[async_trait]
impl RuntimeGateway for FixtureGateway {
    async fn diagnose(&self) -> DiagnosticSnapshot {
        let state = self.state.read().await;
        let metadata = self.contract.metadata();
        if !state.faults.devtools_ready {
            return DiagnosticSnapshot {
                status: crate::gateway::ConnectionStatus::Unavailable,
                devtools_url: FIXTURE_DEVTOOLS_URL.into(),
                page_title: metadata.page_title.clone(),
                page_url: metadata.page_url.clone(),
                nim_account: String::new(),
                detail: "Fixture DevTools 已断开".into(),
                rate_limit_hits: 0,
            };
        }
        DiagnosticSnapshot {
            status: if state.faults.nim_ready {
                crate::gateway::ConnectionStatus::Ready
            } else {
                crate::gateway::ConnectionStatus::NimNotReady
            },
            devtools_url: FIXTURE_DEVTOOLS_URL.into(),
            page_title: metadata.page_title.clone(),
            page_url: metadata.page_url.clone(),
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
            rate_limit_hits: 0,
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
        let state = self.state.read().await;
        Ok(json!({
            "ok": true,
            "session": FIXTURE_SESSION,
            "installed": [
                "onmsg",
                "onmsgs",
                "onaddteammembers",
                "onremoveteammembers",
                "onupdateteammember"
            ],
            "queued": state.queue.len(),
            "dropped": 0,
        }))
    }

    async fn poll_messages(&self) -> AppResult<Value> {
        let state = self.state.read().await;
        if !state.faults.devtools_ready {
            return Err(
                AppError::new("fixture_disconnected", "Fixture DevTools 已断开").retryable(),
            );
        }
        let messages = state
            .queue
            .values()
            .filter(|record| record.kind == GatewayRecordKind::Message)
            .take(100)
            .map(|record| {
                let mut payload = record.payload.clone();
                if let Some(object) = payload.as_object_mut() {
                    object.insert("seq".into(), Value::from(record.sequence));
                    object.insert("listenerSession".into(), Value::from(FIXTURE_SESSION));
                }
                payload
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "ok": true,
            "session": FIXTURE_SESSION,
            "messages": messages,
            "remaining": state.queue.len(),
            "dropped": 0,
        }))
    }

    async fn acknowledge_messages(&self, sequence: u64) -> AppResult<Value> {
        let mut state = self.state.write().await;
        let before = state.queue.len();
        state
            .queue
            .retain(|item_sequence, _| *item_sequence > sequence);
        Ok(json!({
            "ok": true,
            "session": FIXTURE_SESSION,
            "acknowledgedThrough": sequence,
            "acked": before - state.queue.len(),
            "remaining": state.queue.len(),
            "dropped": 0,
        }))
    }

    async fn read_batch(&self) -> AppResult<GatewayBatch> {
        let state = self.state.read().await;
        if !state.faults.devtools_ready {
            return Err(
                AppError::new("fixture_disconnected", "Fixture DevTools 已断开").retryable(),
            );
        }
        Ok(GatewayBatch {
            session: FIXTURE_SESSION.into(),
            records: state.queue.values().take(100).cloned().collect(),
            remaining: state.queue.len(),
            dropped: 0,
        })
    }

    async fn ack(&self, session: &str, sequence: u64) -> AppResult<GatewayReceipt> {
        if session != FIXTURE_SESSION || sequence == 0 {
            return Err(AppError::new(
                "listener_ack_mismatch",
                "Fixture 监听会话或确认序号无效",
            ));
        }
        let value = self.acknowledge_messages(sequence).await?;
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
            session: FIXTURE_SESSION.into(),
            acknowledged_through: sequence,
            acknowledged: value.get("acked").and_then(Value::as_u64).unwrap_or(0) as usize,
            remaining: value.get("remaining").and_then(Value::as_u64).unwrap_or(0) as usize,
            dropped: 0,
            verification: Some("not-applicable".into()),
        })
    }

    async fn poll_events(&self) -> AppResult<Vec<GatewayEvent>> {
        let mut state = self.state.write().await;
        if !state.faults.devtools_ready {
            return Err(
                AppError::new("fixture_disconnected", "Fixture DevTools 已断开").retryable(),
            );
        }
        let cursor = state.delivered_event_sequence;
        let pending = state
            .events
            .iter()
            .filter(|event| event.sequence > cursor)
            .cloned()
            .collect::<Vec<_>>();
        let mut delivered = Vec::with_capacity(pending.len());
        for event in &pending {
            let Some(member) = state.members.get(&(event.group_id, event.user_id)).cloned() else {
                continue;
            };
            let item = match event.kind.as_str() {
                "member_joined" => GatewayEvent::MemberJoined {
                    group_id: event.group_id,
                    member,
                },
                "member_left" => GatewayEvent::MemberLeft {
                    group_id: event.group_id,
                    member,
                },
                "member_updated" => GatewayEvent::MemberUpdated {
                    group_id: event.group_id,
                    member,
                },
                _ => continue,
            };
            delivered.push(item);
        }
        if let Some(sequence) = pending.iter().map(|event| event.sequence).max() {
            state.delivered_event_sequence = sequence;
        }
        Ok(delivered)
    }

    fn capabilities(&self) -> GatewayCapabilities {
        self.capabilities.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::CapabilityStatus;

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
    async fn group_announcement_round_trips_through_verified_fixture_gateway() {
        let fixture = FixtureGateway::new_default();
        assert_eq!(fixture.list_groups().await.unwrap().len(), 2);
        assert_eq!(
            fixture.capabilities().announcement,
            CapabilityStatus::Supported
        );
        let receipt = fixture
            .set_group_announcement(FIXTURE_GROUP, "Fixture 群公告")
            .await
            .unwrap();
        assert_eq!(receipt.route, "/v1/group/add-notice");
        let announcement = fixture
            .get_group_announcement(FIXTURE_GROUP)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(announcement.content, "Fixture 群公告");
        <FixtureGateway as GroupGateway>::update_group_announcement(
            &fixture,
            FIXTURE_GROUP,
            &announcement.notice_id,
            "Fixture 编辑后的群公告",
            "TOP_NOTICE",
        )
        .await
        .unwrap();
        let updated = fixture
            .get_group_announcement(FIXTURE_GROUP)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.content, "Fixture 编辑后的群公告");
        assert_eq!(updated.mode, "TOP_NOTICE");
        fixture
            .delete_group_announcement(FIXTURE_GROUP, &updated.notice_id)
            .await
            .unwrap();
        assert!(fixture
            .get_group_announcement(FIXTURE_GROUP)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            fixture.snapshot().await.actions.last().unwrap().kind,
            "group_announcement_deleted"
        );
        fixture
            .set_group_announcement(FIXTURE_SECOND_GROUP, "第二群公告")
            .await
            .unwrap();
        assert_eq!(
            fixture
                .get_group_announcement(FIXTURE_SECOND_GROUP)
                .await
                .unwrap()
                .unwrap()
                .content,
            "第二群公告"
        );
    }

    #[tokio::test]
    async fn two_fixture_groups_track_batch_mute_and_unmute_state_independently() {
        let fixture = FixtureGateway::new_default();
        assert_eq!(
            fixture.capabilities().group_mute,
            CapabilityStatus::Supported
        );
        for group_id in [FIXTURE_GROUP, FIXTURE_SECOND_GROUP] {
            fixture.set_group_mute(group_id, true).await.unwrap();
            assert!(fixture.get_group_mute_state(group_id).await.unwrap().muted);
        }
        assert_eq!(
            fixture.snapshot().await.group_mutes,
            vec![FIXTURE_GROUP, FIXTURE_SECOND_GROUP]
        );
        for group_id in [FIXTURE_GROUP, FIXTURE_SECOND_GROUP] {
            fixture.set_group_mute(group_id, false).await.unwrap();
            assert!(!fixture.get_group_mute_state(group_id).await.unwrap().muted);
        }
        assert!(fixture.snapshot().await.group_mutes.is_empty());
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

    #[tokio::test]
    async fn fixture_batches_are_ordered_session_scoped_and_acknowledged() {
        let fixture = FixtureGateway::new_default();
        fixture
            .emit_message(FIXTURE_GROUP, 10006, "later", Some(20), Some("m20".into()))
            .await
            .unwrap();
        fixture
            .emit_message(
                FIXTURE_GROUP,
                10007,
                "earlier",
                Some(10),
                Some("m10".into()),
            )
            .await
            .unwrap();
        let batch = fixture.read_batch().await.unwrap();
        assert_eq!(batch.session, FIXTURE_SESSION);
        assert_eq!(
            batch
                .records
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![10, 20]
        );
        assert!(fixture.ack("wrong-session", 10).await.is_err());
        let receipt = fixture.ack(FIXTURE_SESSION, 10).await.unwrap();
        assert_eq!(receipt.acknowledged_through, 10);
        assert_eq!(receipt.acknowledged, 1);
        assert_eq!(fixture.read_batch().await.unwrap().records[0].sequence, 20);
    }

    #[tokio::test]
    async fn one_hundred_and_one_records_are_delivered_as_two_batches() {
        let fixture = FixtureGateway::new_default();
        for sequence in 1..=101_u64 {
            fixture
                .emit_message(
                    FIXTURE_GROUP,
                    10006,
                    &format!("batch-{sequence}"),
                    Some(sequence),
                    Some(format!("batch-message-{sequence}")),
                )
                .await
                .unwrap();
        }
        let first = fixture.read_batch().await.unwrap();
        assert_eq!(first.records.len(), 100);
        assert_eq!(first.records.last().unwrap().sequence, 100);
        assert_eq!(first.remaining, 101);
        fixture.ack(&first.session, 100).await.unwrap();
        let second = fixture.read_batch().await.unwrap();
        assert_eq!(second.records.len(), 1);
        assert_eq!(second.records[0].sequence, 101);
        fixture.ack(&second.session, 101).await.unwrap();
        assert!(fixture.read_batch().await.unwrap().records.is_empty());
    }

    #[tokio::test]
    async fn duplicate_stable_ids_are_ignored_and_business_order_is_preserved() {
        let fixture = FixtureGateway::new_default();
        fixture
            .emit_message(
                FIXTURE_GROUP,
                10006,
                "later",
                Some(30),
                Some("stable-later".into()),
            )
            .await
            .unwrap();
        fixture
            .emit_message(
                FIXTURE_GROUP,
                10006,
                "earlier",
                Some(10),
                Some("stable-earlier".into()),
            )
            .await
            .unwrap();
        let duplicate_sequence = fixture
            .emit_message(
                FIXTURE_GROUP,
                10006,
                "duplicate payload is ignored",
                Some(20),
                Some("stable-later".into()),
            )
            .await
            .unwrap();
        assert_eq!(duplicate_sequence, 30);
        let batch = fixture.read_batch().await.unwrap();
        assert_eq!(
            batch
                .records
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![10, 30]
        );
    }

    #[tokio::test]
    async fn nim_not_ready_and_partial_roster_match_contract_states() {
        let fixture = FixtureGateway::new_default();
        fixture
            .set_faults(FixtureFaults {
                devtools_ready: true,
                nim_ready: false,
                ..FixtureFaults::default()
            })
            .await;
        let error = fixture.session_identity().await.unwrap_err();
        assert_eq!(error.code, "nim_not_ready");
        assert!(error.retryable);
        assert_eq!(fixture.wire_nim_members(FIXTURE_GROUP).await["ok"], false);

        fixture
            .set_faults(FixtureFaults {
                devtools_ready: true,
                nim_ready: true,
                partial_members: true,
                ..FixtureFaults::default()
            })
            .await;
        let roster = fixture.list_members(FIXTURE_GROUP).await.unwrap();
        assert_eq!(roster.reported_count, 8);
        assert_eq!(roster.resolved_count, 8);
        assert_eq!(roster.completeness, RosterCompleteness::Partial);
        assert_eq!(roster.authority, "partial");
    }

    #[tokio::test]
    async fn unacknowledged_state_survives_gateway_restart_boundary() {
        let fixture = FixtureGateway::new_default();
        fixture
            .emit_message(
                FIXTURE_GROUP,
                10006,
                "persist before ack",
                Some(1),
                Some("crash-boundary-1".into()),
            )
            .await
            .unwrap();
        fixture.mute(FIXTURE_GROUP, 10006, 60).await.unwrap();

        let restarted = fixture.clone();
        let replay = restarted.read_batch().await.unwrap();
        assert_eq!(replay.records.len(), 1);
        assert_eq!(replay.records[0].sequence, 1);
        assert_eq!(restarted.snapshot().await.member_mutes.len(), 1);

        restarted.ack(&replay.session, 1).await.unwrap();
        assert!(fixture.read_batch().await.unwrap().records.is_empty());
        assert_eq!(fixture.snapshot().await.actions.len(), 1);
    }

    #[tokio::test]
    async fn unknown_wangshangliao_build_keeps_reads_and_marks_writes_unverified() {
        let fixture = FixtureGateway::new_with_calibration("unknown", &"f".repeat(64));
        assert_eq!(fixture.list_groups().await.unwrap().len(), 2);
        assert_eq!(
            fixture
                .list_members(FIXTURE_GROUP)
                .await
                .unwrap()
                .reported_count,
            16
        );
        let capabilities = fixture.capabilities();
        assert_eq!(capabilities.mute, CapabilityStatus::Unverified);
        assert_eq!(capabilities.recall, CapabilityStatus::Unverified);
        assert_eq!(capabilities.rename, CapabilityStatus::Unverified);
        assert_eq!(capabilities.remove_member, CapabilityStatus::Unverified);
        assert_eq!(capabilities.group_mute, CapabilityStatus::Unverified);
    }

    #[tokio::test]
    async fn member_lifecycle_events_are_delivered_once_in_sequence_order() {
        let fixture = FixtureGateway::new_default();
        fixture
            .member_joined(
                FIXTURE_GROUP,
                10017,
                Some("fixture-nim-10017".into()),
                Some("新成员17".into()),
                None,
            )
            .await
            .unwrap();
        fixture
            .member_updated(
                FIXTURE_GROUP,
                10017,
                Some("新名称17".into()),
                None,
                None,
                None,
            )
            .await
            .unwrap();
        fixture.member_left(FIXTURE_GROUP, 10017).await.unwrap();
        let batch = fixture.read_batch().await.unwrap();
        assert_eq!(
            batch
                .records
                .iter()
                .map(|record| record.kind.clone())
                .collect::<Vec<_>>(),
            vec![
                GatewayRecordKind::TeamMemberJoined,
                GatewayRecordKind::TeamMemberUpdated,
                GatewayRecordKind::TeamMemberLeft,
            ]
        );
        let events = fixture.poll_events().await.unwrap();
        assert!(matches!(events[0], GatewayEvent::MemberJoined { .. }));
        assert!(matches!(events[1], GatewayEvent::MemberUpdated { .. }));
        assert!(matches!(events[2], GatewayEvent::MemberLeft { .. }));
        assert!(fixture.poll_events().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn wire_contract_keeps_transport_and_business_failures_separate() {
        let fixture = FixtureGateway::new_default();
        let success = fixture
            .wire_route("/v1/group/get-group-list", json!({"v":"0"}))
            .await
            .unwrap();
        assert_eq!(success["code"], 0);
        assert_eq!(success["errno"], 0);

        fixture
            .set_faults(FixtureFaults {
                devtools_ready: true,
                nim_ready: true,
                permission_denied: true,
                ..FixtureFaults::default()
            })
            .await;
        let permission = fixture
            .wire_route("/v1/group/get-group-list", json!({"v":"0"}))
            .await
            .unwrap();
        assert_eq!(permission["code"], 403);
        assert_eq!(permission["errno"], 403);

        fixture
            .set_faults(FixtureFaults {
                devtools_ready: true,
                nim_ready: true,
                timeout_next: true,
                ..FixtureFaults::default()
            })
            .await;
        let timeout = fixture
            .wire_route("/v1/group/get-group-list", json!({"v":"0"}))
            .await
            .unwrap_err();
        assert!(timeout.retryable);
    }

    #[tokio::test]
    async fn thousand_message_burst_has_no_drop_and_preserves_sequence() {
        let fixture = FixtureGateway::new_default();
        for sequence in 1..=1000_u64 {
            fixture
                .emit_message(
                    FIXTURE_GROUP,
                    10006,
                    &format!("burst-{sequence}"),
                    Some(sequence),
                    Some(format!("burst-message-{sequence}")),
                )
                .await
                .unwrap();
        }
        let mut received = Vec::new();
        loop {
            let batch = fixture.read_batch().await.unwrap();
            assert_eq!(batch.dropped, 0);
            if batch.records.is_empty() {
                break;
            }
            assert!(batch.records.len() <= 100);
            received.extend(batch.records.iter().map(|record| record.sequence));
            let last = batch.records.last().unwrap().sequence;
            fixture.ack(&batch.session, last).await.unwrap();
        }
        assert_eq!(received.len(), 1000);
        assert_eq!(received, (1..=1000).collect::<Vec<_>>());
        assert_eq!(fixture.snapshot().await.queued_messages, 0);
    }
}
