import { useEffect, useMemo, useState } from 'react'
import { CalendarDays, ChevronLeft, ChevronRight, Download, RefreshCw, Search, X } from 'lucide-react'
import { api, readableError } from '../api/client'
import { PageFeedback, SectionHeading } from '../components/PageState'
import type { Audit, Group, LoadState } from '../types'

const eventLabels: Record<string, string> = { message_received: '收到消息', message_processed: '消息处理完成', message_ignored: '消息已忽略', rule_matched: '规则命中', action_executed: '执行群管动作', effect_dispatched: '自动动作回执', ai_reply: 'AI 回复', member_joined: '成员入群', member_left: '成员离群', member_updated: '成员资料变更', card_renamed: '群名片修改', card_rename_job: '群名片任务', locked_card_restore_queued: '锁定名片恢复排队', blacklisted_member_rejoined: '黑名单成员重新入群', schedule_open: '定时开群', schedule_close: '定时关群', schedule_open_group: '定时开群', schedule_close_group: '定时关群', daily_summary: '生成每日摘要', daily_summary_failed: '每日摘要失败', task_reminder_queued: '任务提醒排队', semantic_classifier_fallback: '语义分类降级', gateway_queue_overflow: '消息队列溢出', automation_paused: '自动化已暂停' }
const levelLabels: Record<string, string> = { info: '信息', warning: '警告', error: '错误', success: '成功' }

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
    return audits.filter(item => !term || `${eventLabels[item.event] || item.event} ${item.details} ${item.actor} ${item.userId}`.toLocaleLowerCase().includes(term))
  }, [audits, keyword])

  const exportData = async (format: 'json' | 'csv') => {
    try {
      const content = await api.exportAudit(accountId, filters(), format)
      const blob = new Blob([content || (format === 'json' ? JSON.stringify(filtered, null, 2) : toCsv(filtered))], { type: format === 'json' ? 'application/json;charset=utf-8' : 'text/csv;charset=utf-8' })
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
        <label><span>群聊</span><select value={groupId} onChange={event => setGroupId(Number(event.target.value))}><option value={0}>全部群</option>{groups.map(group => <option value={group.groupId} key={group.groupId}>{group.name}</option>)}</select></label>
        <label><span>成员旺商号</span><input inputMode="numeric" value={userId} onChange={event => setUserId(event.target.value.replace(/\D/g, ''))} placeholder="全部成员" /></label>
        <label><span>事件</span><select value={eventName} onChange={event => setEventName(event.target.value)}><option value="all">全部事件</option>{Object.entries(eventLabels).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
        <label><span>级别</span><select value={level} onChange={event => setLevel(event.target.value)}><option value="all">全部级别</option>{Object.entries(levelLabels).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
        <div className="date-filter"><span>日期</span><div className="date-select-row" role="group" aria-label="审计日期">
          <select aria-label="年份" value={selectedYear} onChange={event => selectDatePart(Number(event.target.value), selectedMonth, selectedDay)}>{yearOptions.map(year => <option value={year} key={year}>{year} 年</option>)}</select>
          <select aria-label="月份" value={selectedMonth} onChange={event => selectDatePart(selectedYear, Number(event.target.value), selectedDay)}>{Array.from({ length: 12 }, (_, index) => index + 1).map(month => <option value={month} key={month}>{month} 月</option>)}</select>
          <select aria-label="日期" value={selectedDay} onChange={event => selectDatePart(selectedYear, selectedMonth, Number(event.target.value))}>{Array.from({ length: dayCount }, (_, index) => index + 1).map(day => <option value={day} key={day}>{day} 日</option>)}</select>
          <button className="date-nav-button" type="button" aria-label="前一天" title="查看前一天" onClick={() => moveDate(-1)}><ChevronLeft size={15} /></button>
          <button className="date-today-button" type="button" title="回到今天" onClick={() => selectDate(todayLocalDate())}><CalendarDays size={14} />今天</button>
          <button className="date-nav-button" type="button" aria-label="后一天" title="查看后一天" onClick={() => moveDate(1)}><ChevronRight size={15} /></button>
        </div></div>
      </div>
      <PageFeedback state={state} emptyTitle="暂无审计记录" emptyDetail="规则、AI、成员和计划操作会记录在这里。"><div className="audit-table"><div className="audit-row audit-header"><span>时间</span><span>群聊</span><span>事件</span><span>级别</span><span>详情</span></div>{filtered.map(item => <button className="audit-row" key={item.id} onClick={() => setDetail(item)}><time>{formatDate(item.createdAt)}</time><span>{groups.find(group => group.groupId === item.groupId)?.name || (item.groupId ? `群 ${item.groupId}` : '全局')}</span><strong>{eventLabels[item.event] || item.event}</strong><em className={`level ${item.level}`}>{levelLabels[item.level] || item.level}</em><span>{item.details}</span></button>)}</div></PageFeedback>
    </section>
    {detail && <aside className="detail-drawer"><header><div><span className="eyebrow">审计详情</span><h2>{eventLabels[detail.event] || detail.event}</h2></div><button className="icon-button" title="关闭审计详情" onClick={() => setDetail(null)}><X size={17} /></button></header><dl><dt>时间</dt><dd>{formatDate(detail.createdAt)}</dd><dt>群聊</dt><dd>{groups.find(group => group.groupId === detail.groupId)?.name || detail.groupId}</dd><dt>成员</dt><dd>{detail.userId || '-'}</dd><dt>执行者</dt><dd>{detail.actor || '-'}</dd><dt>级别</dt><dd>{levelLabels[detail.level] || detail.level}</dd><dt>详情</dt><dd>{detail.details}</dd></dl></aside>}
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

function toCsv(items: Audit[]) {
  const escape = (value: unknown) => `"${String(value ?? '').replaceAll('"', '""')}"`
  return ['时间,账号,群ID,成员ID,执行者,事件,级别,详情', ...items.map(item => [item.createdAt, item.accountId, item.groupId, item.userId, item.actor, eventLabels[item.event] || item.event, levelLabels[item.level] || item.level, item.details].map(escape).join(','))].join('\n')
}
