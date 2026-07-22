type PreviewRecord = Record<string, unknown>

export function installStylePreviewFixture() {
  const actions: Array<{ command: string; args: PreviewRecord }> = []
  let callbackId = 1
  let rules: PreviewRecord[] = [{
    id: 1, accountId: 'ACCOUNT', groupId: 0, name: '广告关键词撤回', matcher: 'contains', pattern: '广告',
    threshold: 0, count: 0, windowSeconds: 0, cooldownSeconds: 30, priority: 100, mode: 'observe',
    enabled: false, semanticThreshold: 0.8, exemptRoles: ['owner', 'admin'], exemptUserIds: [],
    actions: [{ kind: 'recall', durationSeconds: 0, message: '' }],
  }]
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
  const bases = [{ id: 1, accountId: 'ACCOUNT', name: 'DH 群规与 FAQ', description: '当前群使用的业务资料', enabled: true, builtIn: false, readOnly: false }]
  const documents = [{ id: 1, baseId: 1, baseName: 'DH 群规与 FAQ', title: '文明交流', kind: 'markdown', content: '文明交流，不发布骚扰、欺诈和恶意链接。', source: 'manual', contentHash: 'STYLE-HASH', enabled: true }]
  const audits = [
    { id: 2, accountId: 'ACCOUNT', groupId: 101, userId: 10002, actor: 'DH BOT', event: 'message_processed', level: 'info', details: '消息已处理并写入本地审计', createdAt: '2026-07-22T08:16:01Z' },
    { id: 1, accountId: 'ACCOUNT', groupId: 101, userId: 10005, actor: 'rule', event: 'rule_matched', level: 'warning', details: '观察模式命中图片规则，未执行动作', createdAt: '2026-07-22T08:12:01Z' },
  ]
  const summaries = [{ id: 1, accountId: 'ACCOUNT', groupId: 101, localDate: '2026-07-22', content: '群内交流正常，今日有 3 条业务消息和 1 项待跟进任务。', source: 'local', createdAt: '2026-07-22T12:00:00Z' }]

  const invoke = async (command: string, args: Record<string, any> = {}) => {
    if (!/^(plugin:event\||diagnose$|database_status$|get_|list_|query_|export_)/.test(command)) actions.push({ command, args })
    switch (command) {
      case 'plugin:event|listen': return callbackId++
      case 'plugin:event|unlisten': return null
      case 'diagnose': return { status: 'ready', devtoolsUrl: 'http://127.0.0.1:9233', pageTitle: 'DH Style Preview', pageUrl: 'http://127.0.0.1:5173', nimAccount: 'ACCOUNT', detail: '样式预览数据已加载，修改 CSS 后会自动刷新。' }
      case 'database_status': return { path: '样式预览内存数据', schemaVersion: 5, integrity: 'ok', accounts: 1, groups: groups.length, messages: messages.length }
      case 'get_ai_settings': return { base_url: '', webhook_url: '', model: 'deepseek-v4-pro', api_key_configured: false }
      case 'get_wang_startup_settings': return { path: '', autoStart: true }
      case 'get_close_behavior': return 'ask'
      case 'list_groups': case 'list_cached_groups': return groups
      case 'list_audit': return audits
      case 'query_audit': return { items: audits, nextCursor: null }
      case 'export_audit': return JSON.stringify(audits, null, 2)
      case 'list_daily_summaries': return summaries
      case 'query_messages': return { items: messages, nextCursor: null }
      case 'send_text_batch': return (args.groupIds || []).map((groupId: number) => ({ groupId, success: true, messageId: `SENT-${groupId}`, error: '' }))
      case 'list_members': return { members, reportedCount: 16, resolvedCount: 16, complete: true, completeness: 'complete', completenessReason: '', httpReturnedCount: 16, httpReportedCount: 16, nimReturnedCount: 16, nimReportedCount: 16, authority: 'style-preview', sources: ['style-preview'] }
      case 'get_card_settings': return { prefix: 'DH', autoRename: false, paused: false }
      case 'get_ai_automation_settings': return { enabled: true, reply: true, tasks: true, recall: false, mute: false, remove: false, manualTakeover: false }
      case 'list_card_rename_jobs': return []
      case 'list_rules': return rules
      case 'save_rule': { const next = { ...args.rule, id: args.rule.id || rules.length + 1 }; rules = rules.filter(rule => rule.id !== next.id).concat(next); return next.id }
      case 'delete_rule': rules = rules.filter(rule => rule.id !== args.ruleId); return null
      case 'export_rules': return JSON.stringify({ version: 1, rules }, null, 2)
      case 'import_rules': return 0
      case 'list_knowledge_bases': return bases
      case 'list_knowledge_documents': return documents
      case 'list_knowledge_bindings': return [{ baseId: 1, accountId: 'ACCOUNT', groupId: 101, enabled: true }]
      case 'update_knowledge_base': case 'bind_knowledge_base': case 'save_knowledge_document': return 1
      case 'list_tasks': return tasks
      case 'save_task': { const next = { ...args.task, id: args.task.id || tasks.length + 1 }; tasks = tasks.filter(task => task.id !== next.id).concat(next); return next.id }
      case 'list_schedules': return schedules
      case 'save_schedule': { const next = { ...args.schedule, id: args.schedule.id || schedules.length + 1 }; schedules = schedules.filter(schedule => schedule.id !== next.id).concat(next); return next.id }
      case 'list_schedule_runs': return [{ id: 1, scheduleId: 1, accountId: 'ACCOUNT', groupId: 101, localDate: '2026-07-22', action: 'open', success: true, error: '', attempts: 1, createdAt: '2026-07-22T00:00:00Z' }]
      case 'get_summary_settings': return { accountId: 'ACCOUNT', enabled: true, time: '23:00', groupIds: [101], timezone: 'Asia/Shanghai' }
      case 'test_ai': return { decision: { reply: '你好，我可以协助处理群规、FAQ 和群内任务。', reason: '样式预览', confidence: 0.95 }, elapsedMs: 18, model: 'deepseek-v4-pro' }
      case 'get_wang_profile_status': return { state: '样式预览', scriptHash: 'STYLE-PREVIEW-HASH', backupPath: null, requiresElevation: false, detail: '当前只用于页面样式调整。' }
      case 'get_gateway_capabilities': return { announcement: 'unsupported', sendText: 'supported', mute: 'supported', recall: 'supported', rename: 'supported', removeMember: 'supported', groupMute: 'supported', memberEvents: 'supported' }
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
