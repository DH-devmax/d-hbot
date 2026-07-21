use std::collections::HashSet;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::Serialize;

use crate::models::{Member, ModerationRule, RuleAction};

#[derive(Debug, Clone)]
pub struct RecentEvent {
    pub user_id: i64,
    pub kind: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ModerationInput<'a> {
    pub member: &'a Member,
    pub kind: &'a str,
    pub text: &'a str,
    pub now: DateTime<Utc>,
    pub recent: &'a [RecentEvent],
    pub rename_violations: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleMatch {
    pub rule_id: i64,
    pub rule_name: String,
    pub mode: String,
    pub reason: String,
    pub actions: Vec<RuleAction>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModerationDecision {
    pub matches: Vec<RuleMatch>,
    pub actions: Vec<RuleAction>,
    pub automatic: bool,
}

pub fn evaluate(rules: &[ModerationRule], input: &ModerationInput<'_>) -> ModerationDecision {
    let mut sorted = rules.iter().filter(|rule| rule.enabled).collect::<Vec<_>>();
    sorted.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then(left.id.cmp(&right.id))
    });
    let mut matches = Vec::new();
    for rule in sorted {
        if rule.group_id != 0 && rule.group_id != input.member.group_id {
            continue;
        }
        if rule
            .exempt_roles
            .iter()
            .any(|role| role == &input.member.role)
            || rule.exempt_user_ids.contains(&input.member.user_id)
        {
            continue;
        }
        if let Some(reason) = matches_rule(rule, input) {
            matches.push(RuleMatch {
                rule_id: rule.id,
                rule_name: rule.name.clone(),
                mode: rule.mode.clone(),
                reason,
                actions: rule.actions.clone(),
            });
        }
    }
    let automatic = matches.iter().any(|matched| matched.mode == "auto");
    let executable = matches
        .iter()
        .filter(|matched| matched.mode == "auto")
        .flat_map(|matched| matched.actions.clone())
        .collect::<Vec<_>>();
    ModerationDecision {
        matches,
        actions: merge_actions(executable),
        automatic,
    }
}

fn matches_rule(rule: &ModerationRule, input: &ModerationInput<'_>) -> Option<String> {
    match rule.matcher.as_str() {
        "exact" if input.text.trim() == rule.pattern.trim() => Some("精确命中".into()),
        "contains" if !rule.pattern.is_empty() && input.text.contains(&rule.pattern) => {
            Some("包含关键词".into())
        }
        "prefix" if !rule.pattern.is_empty() && input.text.starts_with(&rule.pattern) => {
            Some("前缀命中".into())
        }
        "regex" => Regex::new(&rule.pattern)
            .ok()
            .filter(|regex| regex.is_match(input.text))
            .map(|_| "正则命中".into()),
        "length" if weighted_length(input.text) > rule.threshold.max(0) as usize => Some(format!(
            "加权字符 {} 超过 {}",
            weighted_length(input.text),
            rule.threshold
        )),
        "lines" if line_count(input.text) > rule.threshold.max(0) as usize => Some(format!(
            "行数 {} 超过 {}",
            line_count(input.text),
            rule.threshold
        )),
        "image_count" if input.kind == "image" => {
            let start = input.now - chrono::Duration::seconds(rule.window_seconds.max(1));
            let count = 1 + input
                .recent
                .iter()
                .filter(|event| {
                    event.user_id == input.member.user_id
                        && event.kind == "image"
                        && event.at >= start
                        && event.at <= input.now
                })
                .count() as i64;
            (count >= rule.count.max(1)).then(|| format!("时间窗内图片达到 {count} 次"))
        }
        "blacklist" if input.member.blacklisted => Some("黑名单成员".into()),
        "rename_count" if input.rename_violations >= rule.count.max(1) => {
            Some(format!("改名次数达到 {}", input.rename_violations))
        }
        // Semantic rules are evaluated by AI and enter this engine only after
        // an explicit category/score result has been validated.
        "semantic" => None,
        _ => None,
    }
}

pub fn weighted_length(value: &str) -> usize {
    value
        .chars()
        .map(|character| if is_cjk(character) { 2 } else { 1 })
        .sum()
}

fn is_cjk(character: char) -> bool {
    matches!(character as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff)
}

fn line_count(value: &str) -> usize {
    if value.is_empty() {
        0
    } else {
        value.lines().count()
    }
}

pub(crate) fn merge_actions(actions: Vec<RuleAction>) -> Vec<RuleAction> {
    let has_remove = actions.iter().any(|action| action.kind == "remove");
    let has_mute = actions.iter().any(|action| action.kind == "mute");
    let mut seen = HashSet::new();
    let mut merged = Vec::new();
    for action in actions {
        if has_remove && matches!(action.kind.as_str(), "mute" | "reply") {
            continue;
        }
        if has_mute && action.kind == "reply" {
            continue;
        }
        if !seen.insert(action.kind.clone()) {
            if action.kind == "mute" {
                if let Some(existing) = merged
                    .iter_mut()
                    .find(|existing: &&mut RuleAction| existing.kind == "mute")
                {
                    existing.duration_seconds =
                        existing.duration_seconds.max(action.duration_seconds);
                }
            }
            continue;
        }
        merged.push(action);
    }
    merged.sort_by_key(|action| match action.kind.as_str() {
        "recall" => 0,
        "remove" => 1,
        "mute" => 2,
        "blacklist" => 3,
        "notify" => 4,
        "reply" => 5,
        _ => 6,
    });
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Member;

    fn member() -> Member {
        Member {
            account_id: "a".into(),
            group_id: 1,
            user_id: 2,
            nim_id: String::new(),
            nickname: "成员".into(),
            card_name: "成员".into(),
            original_card_name: String::new(),
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
            discovered_at: Utc::now(),
            joined_at: None,
            last_seen_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
    fn rule(matcher: &str, threshold: i64, actions: Vec<RuleAction>) -> ModerationRule {
        ModerationRule {
            id: 1,
            account_id: "a".into(),
            group_id: 0,
            name: "规则".into(),
            matcher: matcher.into(),
            pattern: String::new(),
            threshold,
            count: 1,
            window_seconds: 600,
            cooldown_seconds: 0,
            priority: 1,
            mode: "auto".into(),
            enabled: true,
            semantic_threshold: 0.8,
            exempt_roles: vec![],
            exempt_user_ids: vec![],
            actions,
        }
    }

    #[test]
    fn chinese_characters_have_weight_two() {
        assert_eq!(weighted_length("中a文"), 5);
    }
    #[test]
    fn remove_suppresses_mute_and_reply_but_keeps_recall() {
        let mut remove = rule(
            "length",
            1,
            vec![
                RuleAction {
                    kind: "reply".into(),
                    duration_seconds: 0,
                    message: String::new(),
                },
                RuleAction {
                    kind: "mute".into(),
                    duration_seconds: 60,
                    message: String::new(),
                },
                RuleAction {
                    kind: "remove".into(),
                    duration_seconds: 0,
                    message: String::new(),
                },
                RuleAction {
                    kind: "recall".into(),
                    duration_seconds: 0,
                    message: String::new(),
                },
            ],
        );
        remove.pattern.clear();
        let current = member();
        let result = evaluate(
            &[remove],
            &ModerationInput {
                member: &current,
                kind: "text",
                text: "中文",
                now: Utc::now(),
                recent: &[],
                rename_violations: 0,
            },
        );
        assert_eq!(
            result
                .actions
                .iter()
                .map(|action| action.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["recall", "remove"]
        );
    }
    #[test]
    fn admins_are_exempt_when_configured() {
        let mut current = member();
        current.role = "admin".into();
        let mut configured = rule("length", 0, vec![]);
        configured.exempt_roles.push("admin".into());
        assert!(evaluate(
            &[configured],
            &ModerationInput {
                member: &current,
                kind: "text",
                text: "x",
                now: Utc::now(),
                recent: &[],
                rename_violations: 0
            }
        )
        .matches
        .is_empty());
    }
}
