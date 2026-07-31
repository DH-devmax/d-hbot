import { useEffect, useMemo, useState } from 'react'
import { CalendarDays, ChevronLeft, ChevronRight, Download, RefreshCw, Search, X } from 'lucide-react'
import { api, readableError } from '../api/client'
import { PageFeedback, SectionHeading } from '../components/PageState'
import SelectField from '../components/SelectField'
import type { Audit, Group, LoadState } from '../types'

const eventLabels: Record<string, string> = {
  message_received: '收到群消息',
  message_processed: '群消息处理完成',
  message_ignored: '群消息未进入处理',
  member_event_fallback: '成员事件转为名单对账',
  rule_matched: '规则命中',
  machine_rule_evaluated: '机器规则检测',
  ai_rule_evaluated: 'AI 规则检测',
  ai_rule_failed: 'AI 规则检测失败',
  action_executed: '执行群管动作',
  effect_dispatched: '协议动作执行结果',
  ai_reply: 'AI 助手回复',
  ai_reply_completed: 'AI 助手回复完成',
  ai_reply_failed: 'AI 助手回复失败',
  member_joined: '成员入群',
  member_left: '成员离群',
  member_updated: '成员资料变更',
  card_renamed: '群名片修改',
  card_rename_job: '群名片任务执行结果',
  card_name_correction_queued: '异常群名片进入纠正队列',
  locked_card_restore_queued: '锁定名片恢复排队',
  blacklisted_member_rejoined: '黑名单成员重新入群',
  schedule_open: '定时开群',
  schedule_close: '定时关群',
  schedule_open_group: '定时开群',
  schedule_close_group: '定时关群',
  daily_summary: '生成每日摘要',
  daily_summary_failed: '每日摘要失败',
  task_reminder_queued: '任务提醒排队',
  semantic_classifier_fallback: '语义分类降级',
  gateway_queue_overflow: '消息队列溢出',
  automation_paused: '自动化已暂停',
  '人工发送群消息': '人工发送群消息',
  '人工撤回消息': '人工撤回消息',
  '人工禁言成员': '人工禁言成员',
  '人工解禁成员': '人工解禁成员',
  '人工修改群名片': '人工修改群名片',
  '人工移出成员': '人工移出成员',
  '人工设置全群发言': '人工设置全群发言',
  '人工更新群公告': '人工发布新群公告',
  '人工发布新群公告': '人工发布新群公告',
  '人工编辑群公告': '人工编辑群公告',
  '人工删除群公告': '人工删除群公告',
  '人工批量发布新群公告': '人工批量发布新群公告',
  '人工批量设置全群发言': '人工批量设置全群发言',
}
const levelLabels: Record<string, string> = { info: '信息', warning: '警告', error: '错误', success: '成功' }
const detailKeyLabels: Record<string, string> = {
  direction: '消息方向', kind: '消息类型', sequence: '接收队列序号', serverMessageId: '旺商聊消息编号',
  senderName: '发送成员名称', contentPreview: '内容摘要', processingState: '处理状态', source: '消息来源',
  flow: '消息流向', result: '处理结果', inserted: '首次入库', duplicate: '是否重复', expected: '预期事件数',
  normalized: '成功识别数', fallback: '后续处理', jobId: '任务编号', originalName: '原群名片', desiredName: '目标群名片',
  attempts: '当前尝试次数', errorCode: '错误代码', errorKind: '错误类型', status: '状态', success: '是否成功',
  effect: '自动动作', error: '错误', receipt: '协议回执', route: '协议路由', transportCode: '传输状态码',
  transportErrno: '传输错误码', businessCode: '业务状态码', businessErrno: '业务错误码', businessMessage: '业务说明',
  requestId: '请求编号', messageId: '本地消息编号', session: '会话', verification: '回读验证', acknowledged: '本次确认数量',
  acknowledgedThrough: '已确认至序号', remaining: '剩余数量', dropped: '丢弃数量', retryable: '可重试', unknown: '结果待确认',
  groupId: '群', userId: '成员', durationSeconds: '时长（秒）', action: '操作', actionKind: '动作', ruleId: '规则编号',
  matchedRuleIds: '命中规则', contributorRuleIds: '动作来源规则', automaticActionRuleIds: '自动动作来源规则', automatic: '自动执行', elapsedMs: '耗时（毫秒）',
  scores: '语义置信度', confidence: '置信度', decision: '执行决定', mode: '执行模式', reason: '原因', muted: '全员禁言',
  noticeId: '公告编号', content: '内容', execution: '执行位置', queueMs: '排队耗时（毫秒）', cacheHit: '问答缓存命中',
  knowledgeCacheHit: '知识缓存命中', knowledgeMs: '知识检索耗时（毫秒）', modelMs: '模型耗时（毫秒）',
  providerAttempts: '模型连接尝试次数', failover: '是否切换备用连接', totalMs: '总耗时（毫秒）',
}
const detailValueLabels: Record<string, string> = {
  incoming: '收到', outgoing: '发出', text: '文本', image: '图片', card: '名片', notice: '通知', other: '其他',
  pending: '待处理', queued: '已入库并排队', processing: '处理中', processed: '已处理', ignored: '已忽略',
  'rules-and-ai-evaluated': '规则与 AI 检查完成', 'persisted-and-acknowledged': '已保存并确认源队列',
  'roster-reconciliation': '由 60 秒成员名单对账补齐',
  succeeded: '成功', failed: '失败', unknown: '待人工确认', verified: '已回读确认', unsupported: '未开放',
  'not-applicable': '无需回读', group_mute: '全群发言控制', automatic: '自动执行', observe: '观察', recall: '撤回',
  mute: '禁言', unmute: '解除禁言', remove: '移出成员', blacklist: '加入黑名单', notify: '提示', send_text: '发送文字',
  background: '后台异步执行', 'ai-service-unavailable': 'AI 服务不可用时发送中文提示',
  'failed-terminal': '永久失败', true: '是', false: '否', OK: '正常',
}

export function auditEventLabel(value: string) { return eventLabels[value] || value }

export default function AuditPage({ accountId, groups, onError }: { accountId: string; groups: Group[]; onError: (value: string) => void }) {
  const [audits, setAudits] = useState<Audit[]>([])
  const [state, setState] = useState<LoadState>('idle')
  const [keyword, setKeyword] = useState('')
  const [groupId, setGroupId] = useState(0)
  const [userId, setUserId] = useState('')
  const [eventName, setEventName] = useState('all')
  const [level, setLevel] = useState('all')
  const [date, setDate] = useState(todayLocalDate)
  const [detail, setDetail] = useState<Audit | null>(null)

  const filters = (selectedDate = date) => ({ groupId: groupId || undefined, userId: Number(userId) || undefined, event: eventName, level, ...localDateBounds(selectedDate) })
  const reload = async (selectedDate = date) => {
    if (!accountId) { setAudits([]); setState('empty'); return }
    setState('loading')
    try {
      const result = await api.queryAudit(accountId, filters(selectedDate))
      setAudits(result.items)
      setState(result.items.length ? 'ready' : 'empty')
    } catch (reason) {
      onError(readableError(reason))
      setState(audits.length ? 'offlineCached' : 'error')
    }
  }

  useEffect(() => { void reload() }, [accountId])

  const filtered = useMemo(() => {
    const term = keyword.trim().toLocaleLowerCase()
    return audits.filter(item => !term || `${auditEventLabel(item.event)} ${formatAuditDetails(item.details)} ${item.actor} ${item.userId}`.toLocaleLowerCase().includes(term))
  }, [audits, keyword])

  const exportData = async (format: 'json' | 'csv') => {
    try {
      const content = await api.exportAudit(accountId, filters(), format)
      const exportContent = content ? localizeExportContent(content) : (format === 'json' ? JSON.stringify(filtered, null, 2) : toCsv(filtered))
      const blob = new Blob([exportContent], { type: format === 'json' ? 'application/json;charset=utf-8' : 'text/csv;charset=utf-8' })
      const url = URL.createObjectURL(blob)
      const link = document.createElement('a')
      link.href = url
      link.download = `dh-audit.${format}`
      link.click()
      URL.revokeObjectURL(url)
    } catch (reason) {
      onError(readableError(reason))
    }
  }

  const selectDate = (value: string) => {
    setDate(value)
    void reload(value)
  }

  const [selectedYear, selectedMonth, selectedDay] = date.split('-').map(Number)
  const currentYear = new Date().getFullYear()
  const yearOptions = Array.from(new Set([selectedYear, ...Array.from({ length: 7 }, (_, index) => currentYear + 1 - index)])).filter(Boolean).sort((left, right) => right - left)
  const dayCount = new Date(selectedYear, selectedMonth, 0).getDate()
  const selectDatePart = (year: number, month: number, day: number) => selectDate(formatLocalDate(year, month, Math.min(day, new Date(year, month, 0).getDate())))
  const moveDate = (days: number) => {
    const value = new Date(selectedYear, selectedMonth - 1, selectedDay + days, 12)
    selectDate(formatLocalDate(value.getFullYear(), value.getMonth() + 1, value.getDate()))
  }

  return <div className="page-stack">
    <section className="section">
      <SectionHeading eyebrow="可追溯记录" title="审计" meta={`${filtered.length} 条`} actions={<div className="button-row"><button className="secondary" title="应用当前筛选并刷新" onClick={() => void reload()}><RefreshCw size={15} />刷新</button><button className="secondary" title="导出当前筛选的 CSV" onClick={() => exportData('csv')}><Download size={15} />CSV</button><button className="secondary" title="导出当前筛选的 JSON" onClick={() => exportData('json')}><Download size={15} />JSON</button></div>} />
      <div className="filter-grid audit-filters">
        <label className="search-field"><Search size={15} aria-hidden="true" /><input aria-label="搜索审计记录" value={keyword} onChange={event => setKeyword(event.target.value)} placeholder="搜索事件、成员或详情" /></label>
        <SelectField label="群聊" value={String(groupId)} options={[{ value: '0', label: '全部群' }, ...groups.map(group => ({ value: String(group.groupId), label: group.name }))]} onChange={value => setGroupId(Number(value))} />
        <label><span>成员旺商号</span><input inputMode="numeric" value={userId} onChange={event => setUserId(event.target.value.replace(/\D/g, ''))} placeholder="全部成员" /></label>
        <SelectField label="事件" value={eventName} options={[{ value: 'all', label: '全部事件' }, ...Object.entries(eventLabels).map(([value, label]) => ({ value, label }))]} onChange={setEventName} />
        <SelectField label="级别" value={level} options={[{ value: 'all', label: '全部级别' }, ...Object.entries(levelLabels).map(([value, label]) => ({ value, label }))]} onChange={setLevel} />
        <div className="date-filter"><span>日期</span><div className="date-select-row" role="group" aria-label="审计日期">
          <SelectField label="年份" value={String(selectedYear)} options={yearOptions.map(year => ({ value: String(year), label: `${year} 年` }))} onChange={value => selectDatePart(Number(value), selectedMonth, selectedDay)} />
          <SelectField label="月份" value={String(selectedMonth)} options={Array.from({ length: 12 }, (_, index) => index + 1).map(month => ({ value: String(month), label: `${month} 月` }))} onChange={value => selectDatePart(selectedYear, Number(value), selectedDay)} />
          <SelectField label="日期" value={String(selectedDay)} options={Array.from({ length: dayCount }, (_, index) => index + 1).map(day => ({ value: String(day), label: `${day} 日` }))} onChange={value => selectDatePart(selectedYear, selectedMonth, Number(value))} />
          <button className="date-nav-button" type="button" aria-label="前一天" title="查看前一天" onClick={() => moveDate(-1)}><ChevronLeft size={15} /></button>
          <button className="date-today-button" type="button" title="回到今天" onClick={() => selectDate(todayLocalDate())}><CalendarDays size={14} />今天</button>
          <button className="date-nav-button" type="button" aria-label="后一天" title="查看后一天" onClick={() => moveDate(1)}><ChevronRight size={15} /></button>
        </div></div>
      </div>
      <PageFeedback state={state} emptyTitle="暂无审计记录" emptyDetail="规则、AI、成员和计划操作会记录在这里。"><div className="audit-table"><div className="audit-row audit-header"><span>时间</span><span>群聊</span><span>事件</span><span>级别</span><span>详情</span></div>{filtered.map(item => { const formatted = formatAuditDetails(item.details); return <button className="audit-row" key={item.id} onClick={() => setDetail(item)}><time>{formatDate(item.createdAt)}</time><span>{groups.find(group => group.groupId === item.groupId)?.name || (item.groupId ? `群 ${item.groupId}` : '全局')}</span><strong>{auditEventLabel(item.event)}</strong><em className={`level ${item.level}`}>{levelLabels[item.level] || item.level}</em><span title={formatted}>{formatted}</span></button> })}</div></PageFeedback>
    </section>
    {detail && <aside className="detail-drawer"><header><div><span className="eyebrow">审计详情</span><h2>{auditEventLabel(detail.event)}</h2></div><button className="icon-button" title="关闭审计详情" onClick={() => setDetail(null)}><X size={17} /></button></header><dl><dt>时间</dt><dd>{formatDate(detail.createdAt)}</dd><dt>群聊</dt><dd>{groups.find(group => group.groupId === detail.groupId)?.name || detail.groupId}</dd><dt>成员旺商号</dt><dd>{detail.userId || '-'}</dd><dt>执行者</dt><dd>{detail.actor || '-'}</dd><dt>级别</dt><dd>{levelLabels[detail.level] || detail.level}</dd><dt>详情</dt><dd>{formatAuditDetails(detail.details)}</dd></dl></aside>}
  </div>
}

function todayLocalDate() {
  const value = new Date()
  return formatLocalDate(value.getFullYear(), value.getMonth() + 1, value.getDate())
}

function formatLocalDate(year: number, month: number, day: number) {
  return `${year}-${String(month).padStart(2, '0')}-${String(day).padStart(2, '0')}`
}

function formatDate(value: string) {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString('zh-CN', { hour12: false })
}

function localDateBounds(value: string) {
  if (!value) return {}
  const start = new Date(`${value}T00:00:00`)
  const end = new Date(`${value}T23:59:59.999`)
  return { from: start.toISOString(), to: end.toISOString() }
}

export function formatAuditDetails(raw: string) {
  let value: unknown
  try { value = JSON.parse(raw) } catch { return localizePlainAuditDetails(raw) }
  const render = (input: unknown, depth = 0): string => {
    if (Array.isArray(input)) return input.map(item => render(item, depth + 1)).join('、')
    if (input && typeof input === 'object') return Object.entries(input as Record<string, unknown>)
      .filter(([, item]) => item !== '' && item !== null && item !== undefined)
      .map(([key, item]) => `${detailKeyLabels[key] || key}：${render(item, depth + 1)}`)
      .join(depth ? '；' : '　')
    const text = String(input)
    return detailValueLabels[text] || text
  }
  return render(value)
}

function localizePlainAuditDetails(raw: string) {
  const localized = raw
    .replaceAll('消息类型=text', '消息类型=文本')
    .replaceAll('消息类型=image', '消息类型=图片')
    .replaceAll('消息类型=card', '消息类型=名片')
    .replaceAll('消息类型=notice', '消息类型=通知')
    .replaceAll('消息类型=other', '消息类型=其他')
    .replaceAll('收到 NIM 入群事件', '收到旺商聊成员入群事件')
    .replaceAll('收到 NIM 离群事件', '收到旺商聊成员离群事件')
    .replaceAll('收到 NIM 成员资料更新事件', '收到旺商聊成员资料更新事件')
  if (/消息类型=.+，序号=\d+/.test(localized)) {
    return `${localized.replace('，序号=', '，接收队列序号=')}（旧版记录仅保存消息类型和队列序号）`
  }
  return localized
}

function localizeExportContent(raw: string) {
  return localizePlainAuditDetails(raw)
}

function toCsv(items: Audit[]) {
  const escape = (value: unknown) => `"${String(value ?? '').replaceAll('"', '""')}"`
  return ['时间,账号,群ID,成员ID,执行者,事件,级别,详情', ...items.map(item => [item.createdAt, item.accountId, item.groupId, item.userId, item.actor, auditEventLabel(item.event), levelLabels[item.level] || item.level, formatAuditDetails(item.details)].map(escape).join(','))].join('\n')
}
