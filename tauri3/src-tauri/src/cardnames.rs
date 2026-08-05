use std::collections::HashSet;

use crate::error::{AppError, AppResult};
use crate::models::{CardPlan, CardPreview, Member};

pub const CAPACITY: usize = 261_081;
pub const MAX_CARD_CHARS: usize = 20;

pub fn validate_card_name(value: &str) -> AppResult<String> {
    let value = value.trim();
    let length = value.chars().count();
    if length == 0 || length > MAX_CARD_CHARS {
        return Err(AppError::new(
            "invalid_card_name",
            format!("群名片必须为 1 到 {MAX_CARD_CHARS} 个字符"),
        ));
    }
    Ok(value.to_string())
}

pub fn normalize(value: &str) -> Vec<char> {
    value
        .trim()
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect()
}

pub fn missing(value: &str) -> bool {
    let trimmed = value.trim();
    if matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "" | "1" | "." | "null" | "undefined" | "unknown" | "none" | "nil" | "n/a"
    ) {
        return true;
    }
    let normalized = normalize(trimmed);
    normalized.len() < 2
        || normalized
            .iter()
            .all(|character| character.is_ascii_digit())
}

/// Returns whether the current group card should be repaired by the automatic
/// card workflow. A locked DH card is always authoritative; otherwise only
/// clearly unusable placeholders are corrected automatically.
pub fn needs_automatic_correction(member: &Member) -> bool {
    if !member.managed_card_name.trim().is_empty() {
        return member.card_name.trim() != member.managed_card_name.trim();
    }
    missing(&member.card_name)
}

/// Accounts which the upstream has marked as banned, cancelled, or logged out
/// cannot accept a group-card update. Keep them visible in previews, but never
/// add them to an automatic rename queue.
pub fn inactive_account(member: &Member) -> bool {
    let state = member.account_state.trim().to_ascii_uppercase();
    let visible_name = format!("{} {}", member.card_name, member.nickname);
    state.contains("_BAN")
        || state.contains("BLOCK")
        || state.contains("CANCEL")
        || state.contains("LOGOUT")
        || visible_name.contains("已封禁用户")
        || visible_name.contains("已注销")
        || visible_name.contains("该用户已注销")
}

pub fn automatic_correction_eligible(member: &Member) -> bool {
    !inactive_account(member) && needs_automatic_correction(member)
}

pub fn candidates(value: &str) -> Vec<String> {
    let cleaned = normalize(value);
    if cleaned.len() < 2 || missing(value) {
        return Vec::new();
    }
    if cleaned.len() == 2 {
        return vec![cleaned.iter().collect()];
    }
    let all_han = cleaned.iter().all(
        |character| matches!(*character as u32,0x3400..=0x4dbf|0x4e00..=0x9fff|0xf900..=0xfaff),
    );
    let primary = if all_han && cleaned.len() >= 4 {
        cleaned.len() / 2
    } else {
        cleaned.len() - 1
    };
    let mut pairs = vec![(0, primary)];
    if all_han && cleaned.len() >= 4 {
        let middle = cleaned.len() / 2;
        for left in 0..middle {
            for right in middle..cleaned.len() {
                pairs.push((left, right));
            }
        }
    }
    for left in 0..cleaned.len() - 1 {
        for right in left + 1..cleaned.len() {
            pairs.push((left, right));
        }
    }
    let mut seen = HashSet::new();
    pairs
        .into_iter()
        .map(|(left, right)| format!("{}{}", cleaned[left], cleaned[right]))
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

pub fn suffix(index: usize) -> AppResult<String> {
    if !(1..=CAPACITY).contains(&index) {
        return Err(AppError::new("card_capacity", "群名片固定四位编号已用完"));
    }
    Ok(match index {
        1..=9_999 => format!("{index:04}"),
        10_000..=35_973 => {
            let offset = index - 10_000;
            format!(
                "{}{:03}",
                (b'a' + (offset / 999) as u8) as char,
                offset % 999 + 1
            )
        }
        35_974..=102_897 => {
            let offset = index - 35_974;
            let letters = offset / 99;
            format!(
                "{}{}{:02}",
                (b'a' + (letters / 26) as u8) as char,
                (b'a' + (letters % 26) as u8) as char,
                offset % 99 + 1
            )
        }
        _ => {
            let offset = index - 102_898;
            let letters = offset / 9;
            format!(
                "{}{}{}{}",
                (b'a' + (letters / (26 * 26)) as u8) as char,
                (b'a' + ((letters / 26) % 26) as u8) as char,
                (b'a' + (letters % 26) as u8) as char,
                offset % 9 + 1
            )
        }
    })
}

pub fn numbered_name(prefix: &str, index: usize) -> AppResult<(String, String)> {
    let suffix = suffix(index)?;
    let name = format!(
        "{}群员{}",
        if prefix.trim().is_empty() {
            "DH"
        } else {
            prefix.trim()
        },
        suffix
    );
    Ok((validate_card_name(&name)?, suffix))
}

pub fn preview(
    group_id: i64,
    prefix: &str,
    mut members: Vec<Member>,
    self_id: i64,
) -> AppResult<CardPreview> {
    members.sort_by_key(|member| member.user_id);
    let mut used_names = HashSet::new();
    let mut used_suffixes = HashSet::new();
    for member in &members {
        if excluded(member, self_id) {
            used_names.insert(resolved_name(member));
        }
        if !member.card_suffix.is_empty() {
            used_suffixes.insert(member.card_suffix.clone());
        }
    }
    let mut result = CardPreview {
        group_id,
        prefix: if prefix.trim().is_empty() {
            "DH".into()
        } else {
            prefix.trim().into()
        },
        items: Vec::new(),
        will_rename: 0,
        already_managed: 0,
        excluded: 0,
        conflicts: 0,
        missing_identity: 0,
    };
    let mut next_index = 1;
    for member in members {
        let original = resolved_name(&member);
        let automatic_correction = automatic_correction_eligible(&member);
        if excluded(&member, self_id) {
            result.excluded += 1;
            result.items.push(CardPlan {
                member,
                original_name: original,
                suggested_name: String::new(),
                suffix: String::new(),
                status: "excluded".into(),
                reason: "群主、管理员、当前账号或系统账号".into(),
            });
            continue;
        }
        if inactive_account(&member) {
            result.excluded += 1;
            result.items.push(CardPlan {
                member,
                original_name: original,
                suggested_name: String::new(),
                suffix: String::new(),
                status: "excluded".into(),
                reason: "该成员已封禁、注销或离线，跳过自动改名".into(),
            });
            continue;
        }
        if member.user_id == 0 && member.nim_id.is_empty() {
            result.missing_identity += 1;
            result.items.push(CardPlan {
                member,
                original_name: original,
                suggested_name: String::new(),
                suffix: String::new(),
                status: "conflict".into(),
                reason: "缺少 userId 和 nimId".into(),
            });
            continue;
        }
        if !member.managed_card_name.is_empty() {
            let suggested = member.managed_card_name.clone();
            let fixed = member.card_suffix.clone();
            if member.card_name != member.managed_card_name {
                used_names.insert(suggested.clone());
                result.will_rename += 1;
                result.items.push(CardPlan {
                    member,
                    original_name: original,
                    suggested_name: suggested,
                    suffix: fixed,
                    status: "planned".into(),
                    reason: "旺商聊当前群名片与已管理名称不一致".into(),
                });
                continue;
            }
            result.already_managed += 1;
            result.items.push(CardPlan {
                member,
                original_name: original,
                suggested_name: suggested,
                suffix: fixed,
                status: "verified".into(),
                reason: String::new(),
            });
            continue;
        }
        let mut suggested = candidates(&original)
            .into_iter()
            .find(|candidate| !used_names.contains(candidate))
            .unwrap_or_default();
        let mut fixed = String::new();
        while suggested.is_empty() && next_index <= CAPACITY {
            let (name, suffix) = numbered_name(&result.prefix, next_index)?;
            next_index += 1;
            if used_names.contains(&name) || used_suffixes.contains(&suffix) {
                continue;
            }
            suggested = name;
            fixed = suffix.clone();
            used_suffixes.insert(suffix);
        }
        if suggested.is_empty() {
            result.conflicts += 1;
            result.items.push(CardPlan {
                member,
                original_name: original,
                suggested_name: String::new(),
                suffix: String::new(),
                status: "conflict".into(),
                reason: "编号容量已用完".into(),
            });
        } else {
            used_names.insert(suggested.clone());
            result.will_rename += 1;
            result.items.push(CardPlan {
                member,
                original_name: original,
                suggested_name: suggested,
                suffix: fixed,
                status: "planned".into(),
                reason: if automatic_correction {
                    "当前群名片异常，自动纠正".into()
                } else {
                    String::new()
                },
            });
        }
    }
    Ok(result)
}

fn excluded(member: &Member, self_id: i64) -> bool {
    member.user_id == self_id || matches!(member.role.as_str(), "owner" | "admin" | "bot")
}
fn resolved_name(member: &Member) -> String {
    [
        member.original_card_name.as_str(),
        member.card_name.as_str(),
        member.nickname.as_str(),
    ]
    .into_iter()
    .find(|value| !missing(value))
    .or_else(|| {
        [
            member.original_card_name.as_str(),
            member.card_name.as_str(),
            member.nickname.as_str(),
        ]
        .into_iter()
        .find(|value| !value.trim().is_empty())
    })
    .unwrap_or("")
    .trim()
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abbreviations_follow_rules() {
        assert_eq!(candidates("广州校长")[0], "广校");
        assert_eq!(candidates("张三丰")[0], "张丰");
    }
    #[test]
    fn suffix_boundaries_match_fixed_four_chars() {
        for (index, expected) in [
            (1, "0001"),
            (9999, "9999"),
            (10000, "a001"),
            (35973, "z999"),
            (35974, "aa01"),
            (102897, "zz99"),
            (102898, "aaa1"),
            (CAPACITY, "zzz9"),
        ] {
            assert_eq!(suffix(index).unwrap(), expected);
        }
    }

    #[test]
    fn managed_name_drift_reuses_the_existing_target_and_suffix() {
        let member = Member {
            account_id: "ACCOUNT".into(),
            group_id: 1,
            user_id: 10,
            nim_id: "NIM".into(),
            nickname: "原名称".into(),
            card_name: "旺商聊旧名".into(),
            original_card_name: "原名称".into(),
            managed_card_name: "DH群员0001".into(),
            card_suffix: "0001".into(),
            role: "member".into(),
            account_state: String::new(),
            blacklisted: false,
            present: true,
            join_source: "baseline".into(),
            prompt_read: true,
            locked_card_name: "DH群员0001".into(),
            violation_count: 0,
            discovered_at: chrono::Utc::now(),
            joined_at: None,
            last_seen_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let preview = preview(1, "DH", vec![member], 999).unwrap();
        assert_eq!(preview.will_rename, 1);
        assert_eq!(preview.items[0].suggested_name, "DH群员0001");
        assert_eq!(preview.items[0].suffix, "0001");
        assert_eq!(preview.items[0].status, "planned");
    }

    #[test]
    fn automatic_correction_detects_placeholder_card_and_uses_a_valid_backup_name() {
        let member = Member {
            account_id: "ACCOUNT".into(),
            group_id: 1,
            user_id: 10,
            nim_id: "NIM".into(),
            nickname: "广州校长".into(),
            card_name: "1".into(),
            original_card_name: "1".into(),
            managed_card_name: String::new(),
            card_suffix: String::new(),
            role: "member".into(),
            account_state: String::new(),
            blacklisted: false,
            present: true,
            join_source: "baseline".into(),
            prompt_read: true,
            locked_card_name: String::new(),
            violation_count: 0,
            discovered_at: chrono::Utc::now(),
            joined_at: None,
            last_seen_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        assert!(needs_automatic_correction(&member));
        let preview = preview(1, "DH", vec![member], 999).unwrap();
        assert_eq!(preview.items[0].suggested_name, "广校");
        assert_eq!(preview.items[0].reason, "当前群名片异常，自动纠正");
    }

    #[test]
    fn automatic_correction_leaves_a_normal_unmanaged_card_alone() {
        let member = Member {
            account_id: "ACCOUNT".into(),
            group_id: 1,
            user_id: 10,
            nim_id: "NIM".into(),
            nickname: "广州校长".into(),
            card_name: "广州校长".into(),
            original_card_name: "广州校长".into(),
            managed_card_name: String::new(),
            card_suffix: String::new(),
            role: "member".into(),
            account_state: String::new(),
            blacklisted: false,
            present: true,
            join_source: "baseline".into(),
            prompt_read: true,
            locked_card_name: String::new(),
            violation_count: 0,
            discovered_at: chrono::Utc::now(),
            joined_at: None,
            last_seen_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        assert!(!needs_automatic_correction(&member));
    }

    #[test]
    fn inactive_accounts_are_excluded_from_automatic_correction() {
        let member = Member {
            account_id: "ACCOUNT".into(),
            group_id: 1,
            user_id: 10,
            nim_id: "NIM".into(),
            nickname: "已封禁用户".into(),
            card_name: "1".into(),
            original_card_name: "1".into(),
            managed_card_name: String::new(),
            card_suffix: String::new(),
            role: "member".into(),
            account_state: "ACCOUNT_STATE_BAN".into(),
            blacklisted: false,
            present: true,
            join_source: "baseline".into(),
            prompt_read: true,
            locked_card_name: String::new(),
            violation_count: 0,
            discovered_at: chrono::Utc::now(),
            joined_at: None,
            last_seen_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        assert!(needs_automatic_correction(&member));
        assert!(!automatic_correction_eligible(&member));
        let preview = preview(1, "DH", vec![member], 999).unwrap();
        assert_eq!(preview.items[0].status, "excluded");
        assert_eq!(
            preview.items[0].reason,
            "该成员已封禁、注销或离线，跳过自动改名"
        );
    }

    #[test]
    fn manual_card_name_validation_matches_wang_limit() {
        assert_eq!(validate_card_name("  测试名片  ").unwrap(), "测试名片");
        assert!(validate_card_name("").is_err());
        assert!(validate_card_name(&"名".repeat(MAX_CARD_CHARS + 1)).is_err());
        assert!(numbered_name(&"前".repeat(MAX_CARD_CHARS), 1).is_err());
    }
}
