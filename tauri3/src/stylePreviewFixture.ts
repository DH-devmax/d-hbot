type PreviewRecord = Record<string, unknown>

export function installStylePreviewFixture() {
  document.documentElement.classList.add('style-preview')
  const actions: Array<{ command: string; args: PreviewRecord }> = []
  let callbackId = 1
  let windowMaximized = false
  const defaultRule = (id: number, name: string, matcher: string, pattern = '', threshold = 0, count = 0, windowSeconds = 0, priority = 100, mode = 'automatic', semanticThreshold = 0.8): PreviewRecord => ({
    id, accountId: 'ACCOUNT', groupId: 0, ruleType: matcher === 'semantic' ? 'ai' : 'machine', scope: 'global', groupIds: [], name, matcher, pattern, threshold, count, windowSeconds, cooldownSeconds: 600,
    priority, priorityLevel: priority >= 200 ? 'high' : priority >= 100 ? 'medium' : 'low', mode, enabled: false, semanticThreshold, exemptRoles: ['owner', 'admin'], exemptUserIds: [], whitelistUserIds: [],
    actions: [{ kind: 'recall', durationSeconds: 0, message: '' }],
  })
  let rules: PreviewRecord[] = [
    defaultRule(1, '加权字符超过 100', 'length', '', 100), defaultRule(2, '加权字符超过 200', 'length', '', 200, 0, 0, 200),
    defaultRule(3, '超过 4 行', 'lines', '', 4, 0, 0, 110), defaultRule(4, '图片消息', 'image_count', '', 0, 1),
    defaultRule(5, '10 分钟内图片达到 3 次', 'image_count', '', 0, 3, 600, 200), defaultRule(6, '群名片被修改后恢复', 'rename_count', '', 0, 1),
    defaultRule(7, '群名片累计修改 5 次', 'rename_count', '', 0, 5, 0, 200), defaultRule(8, '黑名单成员发言', 'blacklist', '', 0, 0, 0, 300),
    defaultRule(9, '广告、推广与引流关键词', 'regex', '(?i)(广告|推广|引流|加微|加v|加微信|兼职链接|返利)', 0, 0, 0, 120),
    defaultRule(10, '诈骗、验证码与资金风险关键词', 'regex', '(?i)(诈骗|验证码|转账|先交费|保证金|刷单)', 0, 0, 0, 220),
    defaultRule(11, 'AI 广告识别', 'semantic', 'advertisement', 0, 0, 0, 50, 'observe', 0.85),
    defaultRule(12, 'AI 辱骂识别', 'semantic', 'abuse', 0, 0, 0, 50, 'observe', 0.85),
    defaultRule(13, 'AI 诈骗识别', 'semantic', 'scam', 0, 0, 0, 60, 'observe', 0.9),
  ]
  let tasks: PreviewRecord[] = [{
    id: 1, accountId: 'ACCOUNT', groupId: 101, title: '确认本周群规', description: '管理员确认后更新知识库。',
    status: 'pending', assigneeId: 10002, createdBy: 10001, dueAt: '2026-07-24T10:00:00Z', reminderAt: null,
    reminderSentAt: null, createdAt: '2026-07-22T08:00:00Z', updatedAt: '2026-07-22T08:00:00Z',
  }]
  let schedules: PreviewRecord[] = [{ id: 1, accountId: 'ACCOUNT', name: '日常开放时间', enabled: false, openTime: '08:00', closeTime: '22:00', timezone: 'Asia/Shanghai', groupIds: [101] }]
  const groups = [
    { accountId: 'ACCOUNT', groupId: 101, name: '16 人样式预览群', ownerUserId: 10001, enabled: true, aiEnabled: true, moderationEnabled: true, manualTakeover: false, welcomeMessage: '欢迎 @「[成员]」加入群聊，请先查看群规。' },
    { accountId: 'ACCOUNT', groupId: 102, name: '项目协作群', ownerUserId: 10001, enabled: false, aiEnabled: false, moderationEnabled: false, manualTakeover: false, welcomeMessage: '' },
  ]
  const member = (userId: number, cardName: string, role = 'member', accountState = 'ACCOUNT_STATE_GOOD', blacklisted = false) => ({
    accountId: 'ACCOUNT', groupId: 101, userId, nimId: `NIM-${userId}`, nickname: cardName, cardName, role,
    accountState, blacklisted, present: true, originalCardName: cardName, managedCardName: '', cardSuffix: '',
  })
  const members = [
    member(10001, '群主', 'owner'), member(10002, '广校', 'admin'), member(10003, '该用户已注销', 'member', 'ACCOUNT_STATE_CANCEL'),
    member(10004, '封禁群员', 'member', 'ACCOUNT_STATE_BAN', true),
    ...Array.from({ length: 12 }, (_, index) => member(index + 10005, `DH群员${String(index + 1).padStart(4, '0')}`)),
  ]
  const messages = [
    { id: 3, accountId: 'ACCOUNT', groupId: 101, serverMessageId: 'MESSAGE-3', sequence: 3, userId: 10002, senderName: '广校', kind: 'text', text: '@DH 请说明今天的群规', sentAt: '2026-07-22T08:16:00Z', receivedAt: '2026-07-22T08:16:00Z', processedAt: '2026-07-22T08:16:01Z', acknowledgedAt: '2026-07-22T08:16:01Z', processingState: 'processed' },
    { id: 2, accountId: 'ACCOUNT', groupId: 101, serverMessageId: 'MESSAGE-2', sequence: 2, userId: 10005, senderName: 'DH群员0001', kind: 'image', text: '', sentAt: '2026-07-22T08:12:00Z', receivedAt: '2026-07-22T08:12:00Z', processedAt: null, acknowledgedAt: '2026-07-22T08:12:01Z', processingState: 'queued' },
    { id: 1, accountId: 'ACCOUNT', groupId: 102, serverMessageId: 'MESSAGE-1', sequence: 1, userId: 10006, senderName: 'DH群员0002', kind: 'text', text: '项目进度已更新', sentAt: '2026-07-22T08:00:00Z', receivedAt: '2026-07-22T08:00:00Z', processedAt: '2026-07-22T08:00:01Z', acknowledgedAt: '2026-07-22T08:00:01Z', processingState: 'processed' },
  ]
  const bases = [{ id: 1, accountId: 'ACCOUNT', name: 'DH 默认群规与 FAQ', description: '保守版群规、AI 使用边界和常见问题', enabled: true, builtIn: true, readOnly: true }]
  const documents = [
    { id: 1, baseId: 1, baseName: 'DH 默认群规与 FAQ', title: '文明交流', kind: 'markdown', content: '保持正常交流，不发布骚扰、恶意刷屏、欺诈和恶意链接。尊重其他成员，不进行人身攻击。', source: 'built-in', contentHash: 'STYLE-HASH-1', enabled: true },
    { id: 2, baseId: 1, baseName: 'DH 默认群规与 FAQ', title: '资金、账号与验证码', kind: 'markdown', content: '涉及转账、保证金、账号、密码、验证码和身份信息时，请先联系管理员人工核实。', source: 'built-in', contentHash: 'STYLE-HASH-2', enabled: true },
    { id: 3, baseId: 1, baseName: 'DH 默认群规与 FAQ', title: 'DH 能做什么', kind: 'markdown', content: 'DH 可回答当前群绑定资料中的群规、FAQ、任务、公告和业务流程。', source: 'built-in', contentHash: 'STYLE-HASH-3', enabled: true },
    { id: 4, baseId: 1, baseName: 'DH 默认群规与 FAQ', title: '资料不足时如何处理', kind: 'markdown', content: '当前资料没有说明的问题，要明确告诉用户资料不足，并建议联系群管理员确认。', source: 'built-in', contentHash: 'STYLE-HASH-4', enabled: true },
    { id: 5, baseId: 1, baseName: 'DH 默认群规与 FAQ', title: 'AI 回复边界', kind: 'markdown', content: '只有明确 @DH 或旺商聊提及元数据才触发。普通聊天、单独出现 DH、无关问句保持静默。', source: 'built-in', contentHash: 'STYLE-HASH-5', enabled: true },
    { id: 6, baseId: 1, baseName: 'DH 默认群规与 FAQ', title: '公告、任务与人工确认', kind: 'markdown', content: 'AI 可以协助拟定公告、整理任务和总结消息，发布和处罚继续受权限控制。', source: 'built-in', contentHash: 'STYLE-HASH-6', enabled: true },
    { id: 7, baseId: 1, baseName: 'DH 默认群规与 FAQ', title: '预测说明', kind: 'markdown', content: '预测只展示整理后的彩种、期号、结果、更新时间、趋势和参考度。', source: 'built-in', contentHash: 'STYLE-HASH-7', enabled: true },
  ]
  const audits = [
    { id: 3, accountId: 'ACCOUNT', groupId: 101, userId: 10002, actor: 'DH BOT', event: 'message_received', level: 'info', details: JSON.stringify({ direction: 'incoming', messageId: 3, serverMessageId: 'MESSAGE-3', sequence: 3, kind: 'text', senderName: '广校', contentPreview: '@DH 请说明今天的群规', processingState: 'queued', result: 'persisted-and-acknowledged' }), createdAt: '2026-07-22T08:16:00Z' },
    { id: 2, accountId: 'ACCOUNT', groupId: 101, userId: 10002, actor: 'DH BOT', event: 'message_processed', level: 'info', details: JSON.stringify({ direction: 'incoming', messageId: 3, serverMessageId: 'MESSAGE-3', sequence: 3, kind: 'text', senderName: '广校', contentPreview: '@DH 请说明今天的群规', processingState: 'processed', result: 'rules-and-ai-evaluated' }), createdAt: '2026-07-22T08:16:01Z' },
    { id: 1, accountId: 'ACCOUNT', groupId: 101, userId: 10005, actor: 'rule', event: 'machine_rule_evaluated', level: 'warning', details: JSON.stringify({ messageId: 2, matchedRuleIds: [4], automatic: false, decision: '观察模式命中图片规则，未执行动作' }), createdAt: '2026-07-22T08:12:01Z' },
  ]
  const summaries = [{ id: 1, accountId: 'ACCOUNT', groupId: 101, localDate: '2026-07-22', content: '群内交流正常，今日有 3 条业务消息和 1 项待跟进任务。', source: 'local', createdAt: '2026-07-22T12:00:00Z' }]

  const invoke = async (command: string, args: Record<string, any> = {}) => {
    if (!/^(plugin:event\||diagnose$|database_status$|get_|list_|query_|export_)/.test(command)) actions.push({ command, args })
    switch (command) {
      case 'plugin:event|listen': return callbackId++
      case 'plugin:event|unlisten': return null
      case 'plugin:window|is_maximized': return windowMaximized
      case 'plugin:window|toggle_maximize': windowMaximized = !windowMaximized; return null
      case 'plugin:window|minimize': return null
      case 'plugin:window|close': return null
      case 'diagnose': return { status: 'ready', devtoolsUrl: 'http://127.0.0.1:9233', pageTitle: 'DH Style Preview', pageUrl: 'http://127.0.0.1:5173', nimAccount: 'ACCOUNT', detail: '样式预览数据已加载，修改 CSS 后会自动刷新。' }
      case 'database_status': return { path: '样式预览内存数据', schemaVersion: 6, integrity: 'ok', accounts: 1, groups: groups.length, messages: messages.length }
      case 'get_ai_settings': return { base_url: '', webhook_url: '', api_backend: 'chat_completions', model: 'deepseek-v4-pro', api_key_configured: false }
      case 'list_ai_provider_endpoints': return [{ id: 1, accountId: 'ACCOUNT', name: '主连接', baseUrl: 'https://api.example.invalid/v1', webhookUrl: '', apiBackend: 'chat_completions', model: 'deepseek-v4-pro', priority: 0, enabled: true, apiKeyConfigured: true, healthStatus: 'healthy', failureCount: 0, cooldownUntil: null, lastError: '', lastCheckedAt: '2026-07-22T08:00:00Z', createdAt: '2026-07-22T08:00:00Z', updatedAt: '2026-07-22T08:00:00Z' }, { id: 2, accountId: 'ACCOUNT', name: '备用连接', baseUrl: 'https://backup.example.invalid/v1', webhookUrl: '', apiBackend: 'responses', model: 'deepseek-v4-pro', priority: 1, enabled: true, apiKeyConfigured: true, healthStatus: 'unchecked', failureCount: 0, cooldownUntil: null, lastError: '', lastCheckedAt: null, createdAt: '2026-07-22T08:00:00Z', updatedAt: '2026-07-22T08:00:00Z' }]
      case 'test_ai_provider_endpoint': return { decision: { reply: '你好，我可以协助处理群规、FAQ 和群内任务。', reason: '样式预览', confidence: 0.95 }, elapsedMs: 1280, model: 'deepseek-v4-pro' }
      case 'get_wang_startup_settings': return { path: '', autoStart: true }
      case 'get_close_behavior': return 'ask'
      case 'get_developer_calibration_status': return { active: false, finishing: false, startedAt: '', appFileVersion: '样式预览', mainScriptSha256: 'STYLE-PREVIEW-HASH', pageTitle: 'DH Style Preview', pageUrl: 'http://127.0.0.1:5174', capabilities: ['sendText', 'recall', 'mute', 'rename', 'announcement', 'groupMute', 'memberEvents'], operationCount: 0, callbackCount: 0, writeOperationCount: 0, baselineCount: 0, restoredBaselineCount: 0, restorationVerified: false, restorationError: '' }
      case 'list_groups': case 'list_cached_groups': return groups
      case 'list_audit': return audits
      case 'query_audit': return { items: audits, nextCursor: null }
      case 'export_audit': return JSON.stringify(audits, null, 2)
      case 'list_daily_summaries': return summaries
      case 'query_messages': return { items: messages, nextCursor: null }
      case 'send_text_batch': return (args.groupIds || []).map((groupId: number) => ({ groupId, success: true, messageId: `SENT-${groupId}`, error: '' }))
      case 'execute_group_batch': return (args.input?.groupIds || []).map((groupId: number) => ({ groupId, success: true, status: 'succeeded', requestId: `STYLE-REQUEST-${groupId}`, messageId: args.input?.action === 'announcement' ? `STYLE-NOTICE-${groupId}` : '', error: '' }))
      case 'list_members': return { members, reportedCount: 16, resolvedCount: 16, complete: true, completeness: 'complete', completenessReason: '', httpReturnedCount: 16, httpReportedCount: 16, nimReturnedCount: 16, nimReportedCount: 16, authority: 'style-preview', sources: ['style-preview'] }
      case 'local_members': return members
      case 'search_rule_members': return { items: members.filter(member => !args.keyword || `${member.cardName} ${member.nickname} ${member.userId} ${member.nimId}`.includes(String(args.keyword))), nextCursor: null }
      case 'get_card_settings': return { prefix: 'DH', autoRename: false, paused: false }
      case 'get_ai_automation_settings': return { enabled: true, reply: true, tasks: true, recall: false, mute: false, remove: false, manualTakeover: false }
      case 'get_group_management_context': return { groupId: args.groupId, senderId: 10001, isManager: true, capabilities: { announcement: 'supported', sendText: 'supported', mute: 'supported', recall: 'supported', rename: 'supported', removeMember: 'supported', groupMute: 'supported', memberEvents: 'supported' }, memberCount: 16, announcementStatus: '可用' }
      case 'get_group_announcement': return { groupId: args.groupId, noticeId: 'STYLE-NOTICE-1', content: '文明交流，涉及资金、账号和验证码时请先联系管理员核实。', mode: 'COMMON_NOTICE', authorUserId: 10001 }
      case 'list_group_announcements': return [{ groupId: args.groupId, noticeId: 'STYLE-NOTICE-1', content: '文明交流，涉及资金、账号和验证码时请先联系管理员核实。', mode: 'COMMON_NOTICE', authorUserId: 10001 }]
      case 'list_card_rename_jobs': return []
      case 'list_rules': return rules
      case 'save_rule': { const next = { ...args.rule, id: args.rule.id || rules.length + 1 }; rules = rules.filter(rule => rule.id !== next.id).concat(next); return next.id }
      case 'delete_rule': rules = rules.filter(rule => rule.id !== args.ruleId); return null
      case 'export_rules': return JSON.stringify({ version: 1, rules }, null, 2)
      case 'import_rules': return 0
      case 'list_knowledge_bases': return bases
      case 'list_knowledge_documents': return documents
      case 'list_knowledge_bindings': return [{ baseId: args.baseId, accountId: 'ACCOUNT', groupId: 101, enabled: true }]
      case 'list_business_apps': return [{ accountId: 'ACCOUNT', appId: 'prediction', name: '预测', description: '读取公开开奖并生成统计参考', version: '1.0.0', enabled: true, status: 'ready', statusDetail: '2 个彩种数据可用', lastCheckedAt: '2026-07-22T08:00:00Z', updatedAt: '2026-07-22T08:00:00Z' }]
      case 'list_business_app_runs': return [{ id: 1, accountId: 'ACCOUNT', appId: 'prediction', groupId: 101, messageId: 3, runKey: 'message:3', status: 'succeeded', freshness: 'fresh', aiUsed: true, reply: '【PC28】第2026202期\n开奖：9 + 0 + 1 = 10（小双）\n趋势：近期样本稳定。', error: '', elapsedMs: 420, createdAt: '2026-07-22T08:16:02Z', completedAt: '2026-07-22T08:16:02Z' }]
      case 'get_business_app_health': return { appId: 'prediction', status: 'ready', detail: '2 个彩种数据可用', checkedAt: '2026-07-22T08:00:00Z', games: [{ id: 'pcdd', name: 'PC28', status: 'ready', detail: '中国福彩网官方快乐8开奖，DH 按公开规则派生 28 结果' }, { id: 'jnd', name: '加拿大28', status: 'ready', detail: 'BCLC 官方 Keno 原始开奖，DH 按公开规则派生 28 结果' }, { id: 'btc28', name: '比特币28', status: 'unavailable', detail: '公开区块数据存在，但28派生算法尚未核验' }, { id: 'tx28', name: '腾讯分分彩28', status: 'unavailable', detail: '没有可核验的官方公开数据源' }] }
      case 'test_business_app': return { appId: 'prediction', status: 'succeeded', freshness: 'fresh', reply: '【PC28】第2026202期\n开奖：9 + 0 + 1 = 10（小双）\n时间：07-31 21:30\n趋势：根据近 30 期频率和遗漏计算候选方向\n参考方向：4、7、9\n参考度：中高\n说明：基于已核验历史数据统计，仅作信息参考。', aiUsed: true, error: '', elapsedMs: 420 }
      case 'update_knowledge_base': case 'bind_knowledge_base': case 'save_knowledge_document': return 1
      case 'list_tasks': return tasks
      case 'save_task': { const next = { ...args.task, id: args.task.id || tasks.length + 1 }; tasks = tasks.filter(task => task.id !== next.id).concat(next); return next.id }
      case 'list_schedules': return schedules
      case 'save_schedule': { const next = { ...args.schedule, id: args.schedule.id || schedules.length + 1 }; schedules = schedules.filter(schedule => schedule.id !== next.id).concat(next); return next.id }
      case 'list_schedule_runs': return [{ id: 1, scheduleId: 1, accountId: 'ACCOUNT', groupId: 101, localDate: '2026-07-22', action: 'open', success: true, error: '', attempts: 1, createdAt: '2026-07-22T00:00:00Z' }]
      case 'get_summary_settings': return { accountId: 'ACCOUNT', enabled: true, time: '23:00', groupIds: [101], timezone: 'Asia/Shanghai' }
      case 'test_ai': return { decision: { reply: String(args.message || '').includes('群公告') ? '请文明交流。涉及资金、账号或验证码时，请先联系管理员核实。' : '你好，我可以协助处理群规、FAQ 和群内任务。当前资料不足时，我会建议联系管理员确认。', reason: '样式预览', confidence: 0.95 }, elapsedMs: 18, model: 'deepseek-v4-pro', knowledgeSource: args.includeBuiltInKnowledge ? 'DH 默认群规与 FAQ' : '空上下文' }
      case 'get_wang_profile_status': return { state: '样式预览', scriptHash: 'STYLE-PREVIEW-HASH', backupPath: null, requiresElevation: false, detail: '当前只用于页面样式调整。' }
      case 'get_gateway_capabilities': return { announcement: 'supported', sendText: 'supported', mute: 'supported', recall: 'supported', rename: 'supported', removeMember: 'supported', groupMute: 'supported', memberEvents: 'supported' }
      case 'get_runtime_work_snapshot': return { active: { id: 'STYLE-TASK', kind: 'message', label: '处理群消息', scopeLabel: '大海兼职群', state: 'running', percent: 42, queued: 3, startedAt: '2026-08-06T00:00:00Z', retryAt: null, completedAt: null, error: '' }, items: [{ id: 'STYLE-TASK', kind: 'message', label: '处理群消息', scopeLabel: '大海兼职群', state: 'running', percent: 42, queued: 3, startedAt: '2026-08-06T00:00:00Z', retryAt: null, completedAt: null, error: '' }], counts: { running: 1, queued: 3, retrying: 0, failed: 0 }, updatedAt: '2026-08-06T00:00:00Z' }
      case 'acknowledge_runtime_work_failures': return null
      case 'get_runtime_mode': return { mode: 'fixture', dataDir: '样式预览内存数据', restartRequired: false }
      case 'take_wang_startup_status': return null
      default:
        if (/^(save_|set_|delete_|create_|clone_|execute_|preview_|apply_|retry_|recall_|mute_|unmute_|rename_|remove_|generate_|resolve_)/.test(command)) return command.includes('preview_card_names') ? { items: [], willRename: 0, alreadyManaged: 16, excluded: 0, missingIdentity: 0 } : null
        throw new Error(`样式预览 IPC 尚未覆盖：${command}`)
    }
  }

  Object.assign(window, {
    __DH_STYLE_PREVIEW__: true,
    __DH_STYLE_ACTIONS__: actions,
    __TAURI_INTERNALS__: {
      invoke,
      transformCallback() { return callbackId++ },
      unregisterCallback() {},
      runCallback() {},
      convertFileSrc(path: string) { return path },
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
    },
    __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
  })
}
