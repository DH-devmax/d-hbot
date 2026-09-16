//! Mapping of the reviewed business HTTP contract to the shared UI models.
use axum::http::StatusCode;
use chrono::Utc;
use dh_core::models::{Group, Member, MemberRoster, RosterCompleteness};
use serde_json::Value;
use std::collections::BTreeMap;

fn integer(value: &Value, names: &[&str]) -> i64 {
    names
        .iter()
        .filter_map(|name| value.get(*name))
        .find_map(|v| v.as_i64().or_else(|| v.as_str()?.trim().parse().ok()))
        .unwrap_or(0)
}
fn text(value: &Value, names: &[&str]) -> String {
    names
        .iter()
        .filter_map(|name| value.get(*name))
        .find_map(|v| {
            v.as_str()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        })
        .unwrap_or_default()
}
pub fn groups(account: &str, data: &Value) -> Result<Vec<Group>, StatusCode> {
    if account.is_empty()
        || !["owner", "member"]
            .iter()
            .any(|k| data.get(k).is_some_and(Value::is_array))
    {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let mut groups: BTreeMap<i64, Group> = BTreeMap::new();
    for key in ["owner", "member"] {
        let Some(values) = data.get(key) else {
            continue;
        };
        for value in values.as_array().ok_or(StatusCode::BAD_GATEWAY)? {
            let id = integer(value, &["groupId"]);
            if id <= 0 {
                return Err(StatusCode::BAD_GATEWAY);
            }
            let group = Group {
                account_id: account.into(),
                group_id: id,
                name: text(value, &["groupName", "name", "remarkName", "nick"]),
                owner_user_id: integer(value, &["ownerUserId", "groupOwnerId", "ownerId"]),
                enabled: false,
                ai_enabled: false,
                moderation_enabled: false,
                machine_rules_enabled: false,
                ai_rules_enabled: false,
                manual_takeover: false,
                welcome_message: String::new(),
                updated_at: Utc::now(),
            };
            if let Some(existing) = groups.get_mut(&id) {
                if existing.name.is_empty() {
                    existing.name = group.name;
                }
                if existing.owner_user_id <= 0 {
                    existing.owner_user_id = group.owner_user_id;
                }
            } else {
                groups.insert(id, group);
            }
        }
    }
    Ok(groups.into_values().collect())
}
pub fn members(
    account: &str,
    group: i64,
    data: &Value,
) -> Result<(Vec<Member>, Option<String>), StatusCode> {
    let values = data
        .get("groupMemberInfo")
        .and_then(Value::as_array)
        .ok_or(StatusCode::BAD_GATEWAY)?;
    let mut members = Vec::with_capacity(values.len());
    for value in values {
        let user_id = integer(value, &["userId"]);
        if user_id <= 0 {
            return Err(StatusCode::BAD_GATEWAY);
        }
        let nickname = text(value, &["userNick", "nickname", "userName", "name"]);
        let mut card = text(value, &["groupMemberNick", "nick", "groupNick", "cardName"]);
        if card.len() == 32 && card.bytes().all(|b| b.is_ascii_hexdigit()) {
            card.clear();
        }
        let role = match text(
            value,
            &["groupRole", "role", "identity", "memberRole", "type"],
        )
        .trim()
        .to_ascii_uppercase()
        .as_str()
        {
            "1" | "OWNER" | "MASTER" | "GROUP_OWNER" | "GROUP_ROLE_OWNER" | "MSG_MASTER" => "owner",
            "2" | "ADMIN" | "MANAGER" | "ADMINISTRATOR" | "GROUP_ADMIN" | "GROUP_ROLE_ADMIN"
            | "MSG_ADMIN" => "admin",
            _ => "member",
        };
        let now = Utc::now();
        members.push(Member {
            account_id: account.into(),
            group_id: group,
            user_id,
            nim_id: text(value, &["nimId"]),
            nickname: if nickname.is_empty() {
                card.clone()
            } else {
                nickname.clone()
            },
            card_name: if card.is_empty() { nickname } else { card },
            original_card_name: String::new(),
            managed_card_name: String::new(),
            card_suffix: String::new(),
            role: role.into(),
            account_state: text(
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
    let mut cursor = None;
    for path in [
        "/nextCursor",
        "/next_cursor",
        "/nextPageToken",
        "/pageInfo/nextCursor",
        "/pageInfo/nextPageToken",
    ] {
        if let Some(value) = data.pointer(path).filter(|v| !v.is_null()) {
            let value = value.as_str().ok_or(StatusCode::BAD_GATEWAY)?.trim();
            if !value.is_empty() {
                if value.len() > 8192 {
                    return Err(StatusCode::BAD_GATEWAY);
                }
                cursor = Some(value.into());
                break;
            }
        }
    }
    Ok((members, cursor))
}
pub fn roster(members: Vec<Member>, pages: usize) -> MemberRoster {
    let count = members.len();
    MemberRoster {
        status: "partial".into(),
        members,
        reported_count: count,
        resolved_count: count,
        complete: false,
        completeness: if count > 0 {
            RosterCompleteness::Partial
        } else {
            RosterCompleteness::Unknown
        },
        completeness_reason: "业务成员分页已读取，尚未与即时通信成员快照核对".into(),
        http_returned_count: count,
        http_reported_count: 0,
        http_cursor: None,
        nim_returned_count: 0,
        nim_reported_count: 0,
        nim_cursor: None,
        authority: "http".into(),
        sources: vec!["http".into()],
        source_errors: vec![],
        retry_at: None,
        canonical_count: count,
        synthetic_user_ids: vec![],
        http_pages: pages,
        nim_pages: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn groups_deduplicate_and_reject_missing_data() {
        let rows=groups("synthetic",&json!({"owner":[{"groupId":"7","groupName":"Group","ownerUserId":9}],"member":[{"groupId":7}]})).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].owner_user_id, 9);
        assert!(!rows[0].enabled);
        assert!(groups("synthetic", &json!({})).is_err());
        assert!(groups("synthetic", &json!({"member":[{"groupId":0}]})).is_err());
        assert!(groups("synthetic", &json!({"owner":[],"member":[]}))
            .unwrap()
            .is_empty());
    }
    #[test]
    fn members_preserve_identity_cursor_and_partial_authority() {
        let (rows,cursor)=members("synthetic",7,&json!({"groupMemberInfo":[{"userId":"9","nimId":"nim-synthetic","userNick":"Name","groupRole":"ADMIN"}],"nextCursor":"page-2"})).unwrap();
        assert_eq!(rows[0].role, "admin");
        assert_eq!(rows[0].card_name, "Name");
        assert_eq!(cursor.as_deref(), Some("page-2"));
        assert!(!roster(rows, 1).complete);
        assert!(members("synthetic", 7, &json!({})).is_err());
        assert!(members(
            "synthetic",
            7,
            &json!({"groupMemberInfo":[],"nextCursor":123})
        )
        .is_err());
    }
}
