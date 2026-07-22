export type PageName = '总览' | '群组与成员' | '消息台' | '规则' | '知识与 AI' | '任务与计划' | '审计' | '设置' | '调试'

export type LoadState = 'idle' | 'loading' | 'empty' | 'ready' | 'error' | 'offlineCached'

export type Group = {
  accountId: string
  groupId: number
  name: string
  ownerUserId?: number
  enabled: boolean
  aiEnabled: boolean
  moderationEnabled: boolean
  manualTakeover: boolean
  welcomeMessage?: string
  updatedAt?: string
}

export type DatabaseStatus = {
  path: string
  schemaVersion: number
  integrity: string
  accounts: number
  groups: number
  messages: number
}

export type Message = {
  id: number
  accountId: string
  groupId: number
  serverMessageId: string
  sequence: number
  userId: number
  senderName: string
  kind: string
  text: string
  sentAt: string
  receivedAt: string
  processedAt?: string | null
  acknowledgedAt?: string | null
  processingState?: string
  attempts?: number
  lastError?: string
}

export type RuleAction = { kind: string; durationSeconds: number; message: string }

export type Rule = {
  id: number
  accountId: string
  groupId: number
  name: string
  matcher: string
  pattern: string
  threshold: number
  count: number
  windowSeconds: number
  cooldownSeconds: number
  priority: number
  mode: 'observe' | 'automatic' | string
  enabled: boolean
  semanticThreshold: number
  exemptRoles: string[]
  exemptUserIds: number[]
  actions: RuleAction[]
}

export type KnowledgeBase = {
  id: number
  accountId: string
  name: string
  description: string
  enabled: boolean
  builtIn: boolean
  readOnly: boolean
}

export type KnowledgeDocument = {
  id: number
  baseId: number
  baseName: string
  title: string
  kind: string
  content: string
  source: string
  contentHash: string
  enabled: boolean
}

export type TaskItem = {
  id: number
  accountId: string
  groupId: number
  title: string
  description: string
  status: string
  dueAt?: string | null
  assigneeId?: number | null
  createdBy?: number | null
  reminderAt?: string | null
  reminderSentAt?: string | null
  createdAt: string
  updatedAt: string
}

export type Schedule = {
  id: number
  accountId: string
  name: string
  enabled: boolean
  openTime: string
  closeTime: string
  timezone: string
  groupIds: number[]
}

export type ScheduleRun = {
  id: number
  scheduleId: number
  accountId?: string
  groupId: number
  localDate: string
  action: string
  runKey?: string
  success: boolean
  error: string
  attempts: number
  nextRetryAt?: string | null
  createdAt: string
}

export type DailySummary = {
  id: number
  accountId: string
  groupId: number
  localDate: string
  content: string
  source: string
  createdAt: string
}

export type SummarySettings = {
  accountId: string
  enabled: boolean
  time: string
  groupIds: number[]
  timezone: string
}

export type Audit = {
  id: number
  accountId: string
  groupId: number
  userId: number
  actor: string
  event: string
  level: string
  details: string
  createdAt: string
}

export type AuditFilters = {
  groupId?: number
  userId?: number
  event?: string
  level?: string
  from?: string
  to?: string
  cursor?: string
}

export type AiSettings = { base_url: string; webhook_url: string; model: string; api_key_configured: boolean }

export type PageResult<T> = { items: T[]; nextCursor?: string | null }

export type SendResult = { groupId: number; groupName: string; success: boolean; messageId?: string; error?: string }

export type GroupBatchAction = 'announcement' | 'mute' | 'unmute'

export type GroupBatchResult = {
  groupId: number
  success: boolean
  status: string
  requestId: string
  messageId: string
  error: string
}

export type MessageFilters = {
  accountId?: string
  groupIds?: number[]
  keyword?: string
  kind?: string
  processingState?: string
}
