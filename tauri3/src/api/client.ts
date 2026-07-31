import { invoke } from '@tauri-apps/api/core'
import type { AiProviderEndpoint, Audit, AuditFilters, BusinessAppHealth, BusinessAppRecord, BusinessAppRun, BusinessAppTestResult, DailySummary, Group, GroupBatchAction, GroupBatchResult, KnowledgeBase, KnowledgeDocument, Message, MessageFilters, PageResult, Rule, RuleMember, Schedule, ScheduleRun, SendResult, SummarySettings, SupportBundleResult, TaskItem } from '../types'

export function readableError(reason: unknown) {
  const raw = typeof reason === 'string'
    ? reason
    : reason && typeof reason === 'object' && 'message' in reason
      ? String(reason.message)
      : reason && typeof reason === 'object' && 'detail' in reason
        ? String(reason.detail)
        : String(reason)
  if (/timed? ?out|timeout|请求超时/i.test(raw)) return '请求超时，请稍后重试'
  if (/error sending request|network request failed|network error/i.test(raw)) return '网络请求失败，请检查网络或服务地址'
  if (/connection refused|连接被拒绝/i.test(raw)) return '连接被拒绝，请确认服务已启动'
  if (/AI 请求失败|AI 服务连接失败|AI 网络请求失败/i.test(raw) && /https?:\/\//i.test(raw)) return 'AI 服务连接失败，请检查网络、代理或服务地址'
  if (typeof reason === 'string') return reason
  if (reason && typeof reason === 'object') {
    if ('message' in reason) return String(reason.message)
    if ('detail' in reason) return String(reason.detail)
  }
  return String(reason)
}

function missingCommand(reason: unknown) {
  const message = readableError(reason).toLowerCase()
  return message.includes('unknown command') || message.includes('not found') || message.includes('command') && message.includes('missing')
}

export function matchesMessageFilters(message: Message, filters: MessageFilters) {
  const keyword = filters.keyword?.trim().toLocaleLowerCase()
  return (!filters.groupIds?.length || filters.groupIds.includes(message.groupId)) &&
    (!filters.kind || filters.kind === 'all' || message.kind === filters.kind) &&
    (!filters.processingState || filters.processingState === 'all' || (message.processingState || (message.processedAt ? 'processed' : 'pending')) === filters.processingState) &&
    (!keyword || `${message.senderName} ${message.text} ${message.serverMessageId}`.toLocaleLowerCase().includes(keyword))
}

export const api = {
  invoke,
  exportSupportBundle: () => invoke<SupportBundleResult>('export_support_bundle'),
  listGroups: () => invoke<Group[]>('list_groups'),
  listCachedGroups: () => invoke<Group[]>('list_cached_groups'),
  recentMessages: (accountId: string, groupId?: number, limit = 500) => invoke<Message[]>('recent_messages', { accountId, groupId: groupId ?? null, limit }),
  async queryMessages(accountId: string, filters: MessageFilters, cursor?: string, limit = 100): Promise<PageResult<Message>> {
    try {
      return await invoke<PageResult<Message>>('query_messages', { query: { accountId, groupIds: filters.groupIds || [], keyword: filters.keyword || null, kind: filters.kind === 'all' ? null : filters.kind || null, processingState: filters.processingState === 'all' ? null : filters.processingState || null, cursor: cursor ?? null, limit } })
    } catch (reason) {
      if (!missingCommand(reason)) throw reason
      const messages = await api.recentMessages(accountId, undefined, Math.max(limit, 500))
      const filtered = messages.filter(message => matchesMessageFilters(message, filters))
      return { items: filtered.slice(0, limit), nextCursor: null }
    }
  },
  async sendTextBatch(groups: Group[], text: string): Promise<SendResult[]> {
    try {
      const result = await invoke<Array<{ groupId: number; success: boolean; messageId: string; error: string }>>('send_text_batch', { groupIds: groups.map(group => group.groupId), text })
      return result.map(item => ({ ...item, groupName: groups.find(group => group.groupId === item.groupId)?.name || `群 ${item.groupId}` }))
    } catch (reason) {
      if (!missingCommand(reason)) throw reason
      return Promise.all(groups.map(async group => {
        try {
          const messageId = await invoke<string>('send_text', { groupId: group.groupId, text })
          return { groupId: group.groupId, groupName: group.name, success: true, messageId }
        } catch (sendError) {
          return { groupId: group.groupId, groupName: group.name, success: false, error: readableError(sendError) }
        }
      }))
    }
  },
  executeGroupBatch: (action: GroupBatchAction, groupIds: number[], text?: string) => invoke<GroupBatchResult[]>('execute_group_batch', { input: { action, groupIds, text: text ?? null } }),
  recallMessage: (message: Message) => invoke<void>('recall_message', { groupId: message.groupId, senderUserId: message.userId, messageId: message.serverMessageId }),
  listRules: (accountId: string, groupId?: number) => invoke<Rule[]>('list_rules', { accountId, groupId: groupId ?? null }),
  saveRule: (rule: Rule) => invoke<number>('save_rule', { rule }),
  setGroupRuleFeatures: (accountId: string, groupId: number, machineEnabled: boolean, aiEnabled: boolean) => invoke<void>('set_group_rule_features', { accountId, groupId, machineEnabled, aiEnabled }),
  searchRuleMembers: (accountId: string, groupIds: number[], keyword: string, cursor?: string, limit = 50) => invoke<PageResult<RuleMember>>('search_rule_members', { accountId, groupIds, keyword, cursor: cursor ?? null, limit }),
  deleteRule: (accountId: string, ruleId: number) => invoke<void>('delete_rule', { accountId, ruleId }),
  async exportRules(accountId: string, rules: Rule[]) {
    try { return await invoke<string>('export_rules', { accountId }) }
    catch (reason) {
      if (!missingCommand(reason)) throw reason
      return JSON.stringify({ version: 2, exportedAt: new Date().toISOString(), rules }, null, 2)
    }
  },
  async importRules(accountId: string, json: string) {
    return invoke<number>('import_rules', { accountId, json })
  },
  listKnowledgeBases: (accountId: string) => invoke<KnowledgeBase[]>('list_knowledge_bases', { accountId }),
  createKnowledgeBase: (base: KnowledgeBase) => invoke<number>('create_knowledge_base', { base }),
  updateKnowledgeBase: (base: KnowledgeBase) => invoke<void>('update_knowledge_base', { base }),
  cloneKnowledgeBase: (accountId: string, baseId: number, name: string) => invoke<number>('clone_knowledge_base', { accountId, baseId, name }),
  deleteKnowledgeBase: (accountId: string, baseId: number) => invoke<void>('delete_knowledge_base', { accountId, baseId }),
  listKnowledgeDocuments: (baseId: number) => invoke<KnowledgeDocument[]>('list_knowledge_documents', { baseId }),
  saveKnowledgeDocument: (document: KnowledgeDocument) => invoke<number>('save_knowledge_document', { document }),
  deleteKnowledgeDocument: (baseId: number, documentId: number) => invoke<void>('delete_knowledge_document', { baseId, documentId }),
  bindKnowledgeBase: (baseId: number, accountId: string, groupIds: number[]) => invoke<void>('bind_knowledge_base', { baseId, accountId, groupIds }),
  listKnowledgeBindings: (accountId: string, baseId?: number) => invoke<Array<{ baseId: number; accountId: string; groupId: number; enabled: boolean }>>('list_knowledge_bindings', { accountId, baseId: baseId ?? null }),
  listAiProviderEndpoints: (accountId: string) => invoke<AiProviderEndpoint[]>('list_ai_provider_endpoints', { accountId }),
  saveAiProviderEndpoint: (input: Pick<AiProviderEndpoint, 'id' | 'accountId' | 'name' | 'baseUrl' | 'webhookUrl' | 'apiBackend' | 'model' | 'reasoningEffort' | 'priority' | 'enabled'> & { apiKey?: string | null }) => invoke<number>('save_ai_provider_endpoint', { input }),
  deleteAiProviderEndpoint: (accountId: string, endpointId: number) => invoke<void>('delete_ai_provider_endpoint', { accountId, endpointId }),
  testAiProviderEndpoint: (accountId: string, endpointId: number, message: string) => invoke<{ decision: { reply: string; reason: string; confidence: number }; elapsedMs: number; model: string }>('test_ai_provider_endpoint', { accountId, endpointId, message }),
  listBusinessApps: (accountId: string) => invoke<BusinessAppRecord[]>('list_business_apps', { accountId }),
  setBusinessAppEnabled: (accountId: string, appId: string, enabled: boolean) => invoke<void>('set_business_app_enabled', { accountId, appId, enabled }),
  getBusinessAppHealth: (accountId: string, appId: string) => invoke<BusinessAppHealth>('get_business_app_health', { accountId, appId }),
  listBusinessAppRuns: (accountId: string, appId: string, limit = 50) => invoke<BusinessAppRun[]>('list_business_app_runs', { accountId, appId, limit }),
  testBusinessApp: (accountId: string, appId: string, message: string) => invoke<BusinessAppTestResult>('test_business_app', { input: { accountId, appId, message } }),
  listTasks: (accountId: string, groupId?: number) => invoke<TaskItem[]>('list_tasks', { accountId, groupId: groupId ?? null }),
  saveTask: (task: TaskItem) => invoke<number>('save_task', { task }),
  deleteTask: (accountId: string, taskId: number) => invoke<void>('delete_task', { accountId, taskId }),
  listSchedules: (accountId: string) => invoke<Schedule[]>('list_schedules', { accountId }),
  saveSchedule: (schedule: Schedule) => invoke<number>('save_schedule', { schedule }),
  deleteSchedule: (accountId: string, scheduleId: number) => invoke<void>('delete_schedule', { accountId, scheduleId }),
  listScheduleRuns: async (accountId: string, scheduleId?: number): Promise<ScheduleRun[]> => {
    try { return await invoke<ScheduleRun[]>('list_schedule_runs', { accountId, scheduleId: scheduleId ?? null, limit: 100 }) }
    catch (reason) { if (missingCommand(reason)) return []; throw reason }
  },
  listSummaries: (accountId: string, limit = 60) => invoke<DailySummary[]>('list_daily_summaries', { accountId, limit }),
  getSummarySettings: (accountId: string) => invoke<SummarySettings>('get_summary_settings', { accountId }),
  saveSummarySettings: (settings: SummarySettings) => invoke<void>('save_summary_settings', { settings }),
  generateSummary: (accountId: string, groupId: number) => invoke<DailySummary>('generate_daily_summary', { accountId, groupId }),
  listAudit: (accountId: string, limit = 1000) => invoke<Audit[]>('list_audit', { accountId, limit }),
  async queryAudit(accountId: string, filters: AuditFilters, limit = 500): Promise<PageResult<Audit>> {
    const query = { accountId, groupId: filters.groupId || null, userId: filters.userId || null, event: filters.event && filters.event !== 'all' ? filters.event : null, level: filters.level && filters.level !== 'all' ? filters.level : null, from: filters.from || null, to: filters.to || null, cursor: filters.cursor || null, limit }
    try { return await invoke<PageResult<Audit>>('query_audit', { query }) }
    catch (reason) { if (!missingCommand(reason)) throw reason; return { items: await api.listAudit(accountId, limit), nextCursor: null } }
  },
  async exportAudit(accountId: string, filters: AuditFilters, format: 'json' | 'csv') {
    try { return await invoke<string>('export_audit', { query: { accountId, groupId: filters.groupId || null, userId: filters.userId || null, event: filters.event && filters.event !== 'all' ? filters.event : null, level: filters.level && filters.level !== 'all' ? filters.level : null, from: filters.from || null, to: filters.to || null, cursor: null, limit: 1000 }, format }) }
    catch (reason) { if (!missingCommand(reason)) throw reason; return '' }
  },
}
