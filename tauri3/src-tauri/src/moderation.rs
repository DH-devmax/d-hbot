use std::collections::{HashMap, HashSet};
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::models::{Member, ModerationRule, RuleAction};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RuleMode {
    DryRun,
    #[default]
    Observe,
    Automatic,
}

impl RuleMode {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "automatic" => Self::Automatic,
            "dry-run" | "dry_run" | "dryrun" => Self::DryRun,
            _ => Self::Observe,
        }
    }

    pub fn is_automatic(self) -> bool {
        self == Self::Automatic
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::DryRun => "dry-run",
            Self::Observe => "observe",
            Self::Automatic => "automatic",
        }
    }

    // Runtime integrations historically use `auto` as the execution marker.
    fn execution_key(self) -> &'static str {
        match self {
            Self::Automatic => "auto",
            _ => self.as_str(),
        }
    }
}

impl FromStr for RuleMode {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(value))
    }
}

impl fmt::Display for RuleMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for RuleMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RuleMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::parse(&value))
    }
}

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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActionContributor {
    pub rule_id: i64,
    pub rule_name: String,
    pub priority: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActionIntent {
    pub action: RuleAction,
    pub contributors: Vec<ActionContributor>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModerationDecision {
    pub matches: Vec<RuleMatch>,
    pub actions: Vec<RuleAction>,
    pub action_intents: Vec<ActionIntent>,
    pub automatic: bool,
}

#[cfg(test)]
pub fn evaluate(rules: &[ModerationRule], input: &ModerationInput<'_>) -> ModerationDecision {
    evaluate_with_scores(rules, input, &HashMap::new())
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticClassification {
    pub scores: HashMap<String, f64>,
}

pub trait SemanticClassifier: Send + Sync {
    fn classify(&self, text: &str, categories: &[String]) -> SemanticClassification;
}

#[derive(Debug, Clone)]
pub struct DeterministicSemanticClassifier {
    keywords: HashMap<String, Vec<String>>,
}

impl DeterministicSemanticClassifier {
    pub fn new(keywords: HashMap<String, Vec<String>>) -> Self {
        Self { keywords }
    }
}

impl Default for DeterministicSemanticClassifier {
    fn default() -> Self {
        Self::new(HashMap::from([
            (
                "advertisement".into(),
                vec!["advertisement".into(), "广告".into(), "推广".into()],
            ),
            (
                "abuse".into(),
                vec!["abuse".into(), "辱骂".into(), "谩骂".into()],
            ),
            (
                "scam".into(),
                vec!["scam".into(), "诈骗".into(), "骗局".into()],
            ),
        ]))
    }
}

impl SemanticClassifier for DeterministicSemanticClassifier {
    fn classify(&self, text: &str, categories: &[String]) -> SemanticClassification {
        let normalized = text.to_lowercase();
        let scores = categories
            .iter()
            .map(|category| {
                let normalized_category = category.trim().to_lowercase();
                let keywords = self
                    .keywords
                    .get(&normalized_category)
                    .cloned()
                    .unwrap_or_else(|| vec![normalized_category.clone()]);
                let matched = keywords
                    .iter()
                    .filter(|keyword| !keyword.is_empty() && normalized.contains(keyword.as_str()))
                    .count();
                let score = if matched == 0 {
                    0.0
                } else {
                    (0.8 + 0.05 * matched as f64).min(1.0)
                };
                (category.clone(), score)
            })
            .collect();
        SemanticClassification { scores }
    }
}

pub fn evaluate_with_classifier<C: SemanticClassifier + ?Sized>(
    rules: &[ModerationRule],
    input: &ModerationInput<'_>,
    classifier: &C,
) -> ModerationDecision {
    let mut categories = rules
        .iter()
        .filter(|rule| rule.enabled && rule.matcher == "semantic")
        .map(|rule| rule.pattern.trim().to_string())
        .filter(|category| !category.is_empty())
        .collect::<Vec<_>>();
    categories.sort();
    categories.dedup();
    let classification = classifier.classify(input.text, &categories);
    evaluate_with_scores(rules, input, &classification.scores)
}

pub fn evaluate_with_semantic_scores(
    rules: &[ModerationRule],
    input: &ModerationInput<'_>,
    scores: &HashMap<String, f64>,
) -> ModerationDecision {
    evaluate_with_scores(rules, input, scores)
}

fn evaluate_with_scores(
    rules: &[ModerationRule],
    input: &ModerationInput<'_>,
    semantic_scores: &HashMap<String, f64>,
) -> ModerationDecision {
    let mut sorted = rules.iter().filter(|rule| rule.enabled).collect::<Vec<_>>();
    sorted.sort_by(|left, right| {
        right
            .priority_rank()
            .cmp(&left.priority_rank())
            .then(left.id.cmp(&right.id))
    });
    let mut matches = Vec::new();
    for rule in sorted {
        if !rule.applies_to_group(input.member.group_id) {
            continue;
        }
        if rule.whitelist_user_ids.contains(&input.member.user_id) {
            continue;
        }
        if let Some(reason) = matches_rule(rule, input, semantic_scores) {
            let mode = RuleMode::parse(&rule.mode);
            matches.push(RuleMatch {
                rule_id: rule.id,
                rule_name: rule.name.clone(),
                mode: mode.execution_key().into(),
                reason,
                actions: rule.actions.clone(),
            });
        }
    }
    let automatic = matches
        .iter()
        .any(|matched| RuleMode::parse(&matched.mode).is_automatic());
    let priorities = rules
        .iter()
        .map(|rule| (rule.id, rule.priority_rank()))
        .collect::<HashMap<_, _>>();
    let executable = matches
        .iter()
        .filter(|matched| RuleMode::parse(&matched.mode).is_automatic());
    let intents = executable
        .flat_map(|matched| {
            let contributor = ActionContributor {
                rule_id: matched.rule_id,
                rule_name: matched.rule_name.clone(),
                priority: priorities
                    .get(&matched.rule_id)
                    .copied()
                    .unwrap_or_default(),
            };
            matched
                .actions
                .iter()
                .cloned()
                .map(move |action| ActionIntent {
                    action,
                    contributors: vec![contributor.clone()],
                })
        })
        .collect::<Vec<_>>();
    let action_intents = merge_action_intents(intents);
    ModerationDecision {
        matches,
        actions: action_intents
            .iter()
            .map(|intent| intent.action.clone())
            .collect(),
        action_intents,
        automatic,
    }
}

fn matches_rule(
    rule: &ModerationRule,
    input: &ModerationInput<'_>,
    semantic_scores: &HashMap<String, f64>,
) -> Option<String> {
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
            let count = count_recent_events(
                input.recent,
                input.member.user_id,
                "image",
                input.now,
                rule.window_seconds,
            ) as i64;
            (count >= rule.count.max(1)).then(|| format!("时间窗内图片达到 {count} 次"))
        }
        "blacklist" if input.member.blacklisted => Some("黑名单成员".into()),
        "rename_count"
            if input.kind == "member_updated" && input.rename_violations >= rule.count.max(1) =>
        {
            Some(format!("改名次数达到 {}", input.rename_violations))
        }
        "semantic" => {
            let score = semantic_scores.get(rule.pattern.trim()).copied()?;
            let threshold = if rule.semantic_threshold > 0.0 {
                rule.semantic_threshold
            } else if rule.threshold > 0 {
                rule.threshold as f64 / 100.0
            } else {
                0.8
            };
            (score >= threshold).then(|| format!("语义分类 {}={score:.3}", rule.pattern.trim()))
        }
        _ => None,
    }
}

pub fn count_recent_events(
    recent: &[RecentEvent],
    user_id: i64,
    kind: &str,
    now: DateTime<Utc>,
    window_seconds: i64,
) -> usize {
    let start = now - chrono::Duration::seconds(window_seconds.max(1));
    recent
        .iter()
        .filter(|event| {
            event.user_id == user_id && event.kind == kind && event.at >= start && event.at <= now
        })
        .count()
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

pub fn merge_action_intents(actions: Vec<ActionIntent>) -> Vec<ActionIntent> {
    let highest_priority = actions
        .iter()
        .flat_map(|intent| intent.contributors.iter().map(|value| value.priority))
        .max()
        .unwrap_or_default();
    let actions = actions
        .into_iter()
        .filter(|intent| {
            intent.contributors.is_empty()
                || intent
                    .contributors
                    .iter()
                    .any(|value| value.priority == highest_priority)
        })
        .collect::<Vec<_>>();
    let has_remove = actions.iter().any(|intent| intent.action.kind == "remove");
    let has_mute = actions.iter().any(|intent| intent.action.kind == "mute");
    let mut seen = HashSet::new();
    let mut merged: Vec<ActionIntent> = Vec::new();
    for mut intent in actions {
        let action = &intent.action;
        if has_remove && matches!(action.kind.as_str(), "mute" | "reply") {
            continue;
        }
        if has_mute && action.kind == "reply" {
            continue;
        }
        if !seen.insert(action.kind.clone()) {
            if let Some(existing) = merged
                .iter_mut()
                .find(|existing| existing.action.kind == action.kind)
            {
                if action.kind == "mute"
                    && action.duration_seconds > existing.action.duration_seconds
                {
                    existing.action = intent.action.clone();
                }
                existing.contributors.append(&mut intent.contributors);
                existing.contributors.sort_by(|left, right| {
                    right
                        .priority
                        .cmp(&left.priority)
                        .then(left.rule_id.cmp(&right.rule_id))
                });
                existing
                    .contributors
                    .dedup_by(|left, right| left.rule_id == right.rule_id);
            }
            continue;
        }
        merged.push(intent);
    }
    merged.sort_by_key(|intent| match intent.action.kind.as_str() {
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
            rule_type: if matcher == "semantic" {
                "ai"
            } else {
                "machine"
            }
            .into(),
            scope: "global".into(),
            group_ids: Vec::new(),
            priority_level: "medium".into(),
            whitelist_user_ids: Vec::new(),
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
    fn legacy_role_exemptions_do_not_disable_machine_rules() {
        let mut current = member();
        current.role = "admin".into();
        let mut configured = rule("length", 0, vec![]);
        configured.exempt_roles.push("admin".into());
        assert_eq!(
            evaluate(
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
            .len(),
            1
        );
    }

    #[test]
    fn high_priority_rule_wins_and_same_priority_uses_longer_mute() {
        let current = member();
        let now = Utc::now();
        let mut low_remove = rule(
            "contains",
            0,
            vec![RuleAction {
                kind: "remove".into(),
                duration_seconds: 0,
                message: String::new(),
            }],
        );
        low_remove.id = 1;
        low_remove.pattern = "测试".into();
        low_remove.priority_level = "low".into();
        let mut high_short_mute = rule(
            "contains",
            0,
            vec![RuleAction {
                kind: "mute".into(),
                duration_seconds: 60,
                message: String::new(),
            }],
        );
        high_short_mute.id = 2;
        high_short_mute.pattern = "测试".into();
        high_short_mute.priority_level = "high".into();
        let mut high_long_mute = high_short_mute.clone();
        high_long_mute.id = 3;
        high_long_mute.actions[0].duration_seconds = 600;
        let result = evaluate(
            &[low_remove, high_short_mute, high_long_mute],
            &ModerationInput {
                member: &current,
                kind: "text",
                text: "测试",
                now,
                recent: &[],
                rename_violations: 0,
            },
        );
        assert_eq!(result.actions.len(), 1);
        assert_eq!(result.actions[0].kind, "mute");
        assert_eq!(result.actions[0].duration_seconds, 600);
        assert_eq!(
            result.action_intents[0]
                .contributors
                .iter()
                .map(|value| value.rule_id)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    fn image_events(current: &Member, now: DateTime<Utc>, count: usize) -> Vec<RecentEvent> {
        (0..count)
            .map(|offset| RecentEvent {
                user_id: current.user_id,
                kind: "image".into(),
                at: now - chrono::Duration::seconds(offset as i64),
            })
            .collect()
    }

    #[test]
    fn image_window_matches_at_three_and_remains_matched_at_four() {
        let current = member();
        let now = Utc::now();
        let mut configured = rule(
            "image_count",
            0,
            vec![RuleAction {
                kind: "recall".into(),
                duration_seconds: 0,
                message: String::new(),
            }],
        );
        configured.count = 3;

        for (count, expected) in [(2, false), (3, true), (4, true)] {
            let recent = image_events(&current, now, count);
            let decision = evaluate(
                &[configured.clone()],
                &ModerationInput {
                    member: &current,
                    kind: "image",
                    text: "",
                    now,
                    recent: &recent,
                    rename_violations: 0,
                },
            );
            assert_eq!(
                !decision.matches.is_empty(),
                expected,
                "unexpected match result for {count} images"
            );
        }
    }

    #[test]
    fn image_window_ignores_other_members_kinds_and_expired_events() {
        let current = member();
        let now = Utc::now();
        let mut configured = rule("image_count", 0, vec![]);
        configured.count = 2;
        configured.window_seconds = 60;
        let recent = vec![
            RecentEvent {
                user_id: current.user_id,
                kind: "image".into(),
                at: now,
            },
            RecentEvent {
                user_id: current.user_id + 1,
                kind: "image".into(),
                at: now,
            },
            RecentEvent {
                user_id: current.user_id,
                kind: "text".into(),
                at: now,
            },
            RecentEvent {
                user_id: current.user_id,
                kind: "image".into(),
                at: now - chrono::Duration::seconds(61),
            },
        ];
        assert!(evaluate(
            &[configured],
            &ModerationInput {
                member: &current,
                kind: "image",
                text: "",
                now,
                recent: &recent,
                rename_violations: 0,
            }
        )
        .matches
        .is_empty());
    }

    #[test]
    fn auto_and_automatic_both_execute_actions() {
        let current = member();
        for mode in ["auto", "automatic"] {
            let mut configured = rule(
                "contains",
                0,
                vec![RuleAction {
                    kind: "recall".into(),
                    duration_seconds: 0,
                    message: String::new(),
                }],
            );
            configured.pattern = "测试".into();
            configured.mode = mode.into();
            let decision = evaluate(
                &[configured],
                &ModerationInput {
                    member: &current,
                    kind: "text",
                    text: "这是测试消息",
                    now: Utc::now(),
                    recent: &[],
                    rename_violations: 0,
                },
            );
            assert!(decision.automatic, "{mode} should be automatic");
            assert_eq!(decision.actions.len(), 1);
        }
        assert_eq!(RuleMode::parse("AUTO"), RuleMode::Automatic);
        assert_eq!(RuleMode::parse("automatic"), RuleMode::Automatic);
    }

    struct FixedClassifier(f64);

    impl SemanticClassifier for FixedClassifier {
        fn classify(&self, _text: &str, categories: &[String]) -> SemanticClassification {
            SemanticClassification {
                scores: categories
                    .iter()
                    .map(|category| (category.clone(), self.0))
                    .collect(),
            }
        }
    }

    #[test]
    fn semantic_threshold_is_inclusive_and_observe_mode_has_no_actions() {
        let current = member();
        let mut configured = rule(
            "semantic",
            0,
            vec![RuleAction {
                kind: "recall".into(),
                duration_seconds: 0,
                message: String::new(),
            }],
        );
        configured.pattern = "advertisement".into();
        configured.semantic_threshold = 0.85;
        configured.mode = "observe".into();
        let input = ModerationInput {
            member: &current,
            kind: "text",
            text: "推广内容",
            now: Utc::now(),
            recent: &[],
            rename_violations: 0,
        };

        assert!(
            evaluate_with_classifier(&[configured.clone()], &input, &FixedClassifier(0.849))
                .matches
                .is_empty()
        );
        let boundary = evaluate_with_classifier(&[configured], &input, &FixedClassifier(0.85));
        assert_eq!(boundary.matches.len(), 1);
        assert!(!boundary.automatic);
        assert!(boundary.actions.is_empty());
    }

    #[test]
    fn rename_count_only_matches_member_update_events() {
        let current = member();
        let configured = rule(
            "rename_count",
            0,
            vec![RuleAction {
                kind: "recall".into(),
                duration_seconds: 0,
                message: String::new(),
            }],
        );
        let message = ModerationInput {
            member: &current,
            kind: "text",
            text: "普通群消息",
            now: Utc::now(),
            recent: &[],
            rename_violations: 5,
        };
        assert!(evaluate(std::slice::from_ref(&configured), &message)
            .matches
            .is_empty());

        let member_update = ModerationInput {
            kind: "member_updated",
            ..message
        };
        assert_eq!(evaluate(&[configured], &member_update).matches.len(), 1);
    }
}
