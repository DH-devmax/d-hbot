use crate::models::{ModerationRule, RuleAction};

pub const DEFAULT_KNOWLEDGE_BASE_NAME: &str = "DH 默认群规与 FAQ";
pub const DEFAULT_KNOWLEDGE_BASE_DESCRIPTION: &str = "保守版群规、AI 使用边界和常见问题";

#[derive(Debug, Clone, Copy)]
pub struct DefaultKnowledgeDocument {
    pub title: &'static str,
    pub content: &'static str,
}

pub const DEFAULT_KNOWLEDGE_DOCUMENTS: &[DefaultKnowledgeDocument] = &[
    DefaultKnowledgeDocument {
        title: "文明交流",
        content: "保持正常交流，不发布骚扰、恶意刷屏、欺诈和恶意链接。尊重其他成员，不进行人身攻击。",
    },
    DefaultKnowledgeDocument {
        title: "资金、账号与验证码",
        content: "涉及转账、保证金、账号、密码、验证码和身份信息时，请先联系管理员人工核实。DH 不要求成员私下提供敏感信息，任何资金结果以人工确认信息为准。",
    },
    DefaultKnowledgeDocument {
        title: "DH 能做什么",
        content: "DH 可回答当前群绑定资料中的群规、FAQ、任务、公告和业务流程。确定性群管规则与 AI 回复分开运行，AI 不替管理员作最终处罚和权限判断。",
    },
    DefaultKnowledgeDocument {
        title: "资料不足时如何处理",
        content: "当前资料没有说明的问题，要明确告诉用户资料不足，并建议联系群管理员确认。不猜测、不补造事实、不引用其他群资料。",
    },
    DefaultKnowledgeDocument {
        title: "AI 回复边界",
        content: "只有明确 @DH 或旺商聊提及元数据才触发。普通聊天、单独出现 DH、无关问句保持静默。回答自然简洁，优先给出直接结论。",
    },
    DefaultKnowledgeDocument {
        title: "公告、任务与人工确认",
        content: "AI 可以协助拟定公告、整理任务和总结消息。发布公告、执行处罚、创建提醒等行为继续受群权限和功能开关控制。争议、身份、资金和封禁问题交由管理员处理。",
    },
    DefaultKnowledgeDocument {
        title: "预测说明",
        content: "预测只展示整理后的彩种、期号、结果、更新时间、趋势和参考度。不展示接口地址、认证信息、供应方字段和原始响应结构。数据过期或缺失时明确标注状态，不把统计结果描述成保证结果。",
    },
];

fn recall_action() -> Vec<RuleAction> {
    vec![RuleAction {
        kind: "recall".into(),
        duration_seconds: 0,
        message: String::new(),
    }]
}

#[derive(Debug, Clone, Copy)]
struct DefaultRuleSpec {
    name: &'static str,
    matcher: &'static str,
    pattern: &'static str,
    threshold: i64,
    count: i64,
    window_seconds: i64,
    priority: i64,
    mode: &'static str,
    semantic_threshold: f64,
}

const DEFAULT_RULE_SPECS: &[DefaultRuleSpec] = &[
    DefaultRuleSpec {
        name: "加权字符超过 100",
        matcher: "length",
        pattern: "",
        threshold: 100,
        count: 0,
        window_seconds: 0,
        priority: 100,
        mode: "automatic",
        semantic_threshold: 0.8,
    },
    DefaultRuleSpec {
        name: "加权字符超过 200",
        matcher: "length",
        pattern: "",
        threshold: 200,
        count: 0,
        window_seconds: 0,
        priority: 200,
        mode: "automatic",
        semantic_threshold: 0.8,
    },
    DefaultRuleSpec {
        name: "超过 4 行",
        matcher: "lines",
        pattern: "",
        threshold: 4,
        count: 0,
        window_seconds: 0,
        priority: 110,
        mode: "automatic",
        semantic_threshold: 0.8,
    },
    DefaultRuleSpec {
        name: "图片消息",
        matcher: "image_count",
        pattern: "",
        threshold: 0,
        count: 1,
        window_seconds: 0,
        priority: 100,
        mode: "automatic",
        semantic_threshold: 0.8,
    },
    DefaultRuleSpec {
        name: "10 分钟内图片达到 3 次",
        matcher: "image_count",
        pattern: "",
        threshold: 0,
        count: 3,
        window_seconds: 600,
        priority: 200,
        mode: "automatic",
        semantic_threshold: 0.8,
    },
    DefaultRuleSpec {
        name: "群名片被修改后恢复",
        matcher: "rename_count",
        pattern: "",
        threshold: 0,
        count: 1,
        window_seconds: 0,
        priority: 100,
        mode: "automatic",
        semantic_threshold: 0.8,
    },
    DefaultRuleSpec {
        name: "群名片累计修改 5 次",
        matcher: "rename_count",
        pattern: "",
        threshold: 0,
        count: 5,
        window_seconds: 0,
        priority: 200,
        mode: "automatic",
        semantic_threshold: 0.8,
    },
    DefaultRuleSpec {
        name: "黑名单成员发言",
        matcher: "blacklist",
        pattern: "",
        threshold: 0,
        count: 0,
        window_seconds: 0,
        priority: 300,
        mode: "automatic",
        semantic_threshold: 0.8,
    },
    DefaultRuleSpec {
        name: "广告、推广与引流关键词",
        matcher: "regex",
        pattern: "(?i)(广告|推广|引流|加微|加v|加微信|兼职链接|返利)",
        threshold: 0,
        count: 0,
        window_seconds: 0,
        priority: 120,
        mode: "automatic",
        semantic_threshold: 0.8,
    },
    DefaultRuleSpec {
        name: "诈骗、验证码与资金风险关键词",
        matcher: "regex",
        pattern: "(?i)(诈骗|验证码|转账|先交费|保证金|刷单)",
        threshold: 0,
        count: 0,
        window_seconds: 0,
        priority: 220,
        mode: "automatic",
        semantic_threshold: 0.8,
    },
    DefaultRuleSpec {
        name: "AI 广告识别",
        matcher: "semantic",
        pattern: "advertisement",
        threshold: 0,
        count: 0,
        window_seconds: 0,
        priority: 50,
        mode: "observe",
        semantic_threshold: 0.85,
    },
    DefaultRuleSpec {
        name: "AI 辱骂识别",
        matcher: "semantic",
        pattern: "abuse",
        threshold: 0,
        count: 0,
        window_seconds: 0,
        priority: 50,
        mode: "observe",
        semantic_threshold: 0.85,
    },
    DefaultRuleSpec {
        name: "AI 诈骗识别",
        matcher: "semantic",
        pattern: "scam",
        threshold: 0,
        count: 0,
        window_seconds: 0,
        priority: 60,
        mode: "observe",
        semantic_threshold: 0.9,
    },
];

fn rule(account_id: &str, spec: DefaultRuleSpec) -> ModerationRule {
    ModerationRule {
        id: 0,
        account_id: account_id.into(),
        group_id: 0,
        name: spec.name.into(),
        matcher: spec.matcher.into(),
        pattern: spec.pattern.into(),
        threshold: spec.threshold,
        count: spec.count,
        window_seconds: spec.window_seconds,
        cooldown_seconds: 600,
        priority: spec.priority,
        mode: spec.mode.into(),
        enabled: false,
        semantic_threshold: spec.semantic_threshold,
        exempt_roles: vec!["owner".into(), "admin".into()],
        exempt_user_ids: Vec::new(),
        actions: recall_action(),
    }
}

pub fn default_rules(account_id: &str) -> Vec<ModerationRule> {
    DEFAULT_RULE_SPECS
        .iter()
        .copied()
        .map(|spec| rule(account_id, spec))
        .collect()
}
