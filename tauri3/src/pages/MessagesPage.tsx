import { useEffect, useMemo, useState } from 'react'
import { Check, ChevronLeft, ChevronRight, Filter, RefreshCw, Search, Send, Undo2 } from 'lucide-react'
import { api, readableError } from '../api/client'
import { EmptyState, PageFeedback, SectionHeading } from '../components/PageState'
import type { Group, LoadState, Message } from '../types'

const kindLabels: Record<string, string> = { text: '文本', image: '图片', card: '名片', other: '其他' }
const stateLabels: Record<string, string> = { processed: '已处理', pending: '待处理', processing: '处理中', failed: '失败' }

export default function MessagesPage({ groups, accountId, onError }: { groups: Group[]; accountId: string; onError: (value: string) => void }) {
  const [selectedGroups, setSelectedGroups] = useState<number[]>([])
  const [keyword, setKeyword] = useState('')
  const [kind, setKind] = useState('all')
  const [processingState, setProcessingState] = useState('all')
  const [messages, setMessages] = useState<Message[]>([])
  const [state, setState] = useState<LoadState>('idle')
  const [error, setError] = useState('')
  const [draft, setDraft] = useState('')
  const [sending, setSending] = useState(false)
  const [page, setPage] = useState(0)
  const pageSize = 30

  const reload = async () => {
    if (!accountId) { setMessages([]); setState('empty'); return }
    setState('loading'); setError('')
    try {
      const result = await api.queryMessages(accountId, { groupIds: selectedGroups, keyword, kind, processingState }, undefined, 500)
      setMessages(result.items); setPage(0); setState(result.items.length ? 'ready' : 'empty')
    } catch (reason) { setError(readableError(reason)); setState(messages.length ? 'offlineCached' : 'error') }
  }
  useEffect(() => { void reload() }, [accountId])
  const visible = useMemo(() => messages.slice(page * pageSize, (page + 1) * pageSize), [messages, page])
  const pageCount = Math.max(1, Math.ceil(messages.length / pageSize))
  const toggleGroup = (id: number) => setSelectedGroups(current => current.includes(id) ? current.filter(value => value !== id) : [...current, id])
  const send = async () => {
    const targets = groups.filter(group => selectedGroups.includes(group.groupId))
    if (!targets.length || !draft.trim()) return
    setSending(true); onError('')
    try {
      const results = await api.sendTextBatch(targets, draft.trim())
      const failed = results.filter(result => !result.success)
      if (failed.length) onError(`已发送 ${results.length - failed.length} 个群，${failed.length} 个群失败：${failed.map(result => result.groupName).join('、')}`)
      setDraft(''); await reload()
    } catch (reason) { onError(readableError(reason)) }
    finally { setSending(false) }
  }
  const recall = async (message: Message) => {
    if (!window.confirm(`确认撤回 ${message.senderName || '该成员'} 的这条消息？`)) return
    try { await api.recallMessage(message); await reload() } catch (reason) { onError(readableError(reason)) }
  }

  return <div className="page-stack">
    <section className="section">
      <SectionHeading eyebrow="实时流水线" title="消息台" meta={`${messages.length} 条`} actions={<button className="secondary" onClick={() => void reload()}><RefreshCw size={15} />刷新</button>} />
      <div className="filter-grid">
        <label className="search-field wide"><Search size={15} /><input value={keyword} onChange={event => setKeyword(event.target.value)} onKeyDown={event => event.key === 'Enter' && void reload()} placeholder="搜索成员、内容或消息 ID" /></label>
        <label><span>消息类型</span><select value={kind} onChange={event => { setKind(event.target.value); void reload() }}><option value="all">全部类型</option>{Object.entries(kindLabels).map(([value, label]) => <option value={value} key={value}>{label}</option>)}</select></label>
        <label><span>处理状态</span><select value={processingState} onChange={event => { setProcessingState(event.target.value); void reload() }}><option value="all">全部状态</option>{Object.entries(stateLabels).map(([value, label]) => <option value={value} key={value}>{label}</option>)}</select></label>
      </div>
      <div className="group-filter"><span><Filter size={14} />选择群聊</span>{groups.length ? groups.map(group => <label className={`check-chip ${selectedGroups.includes(group.groupId) ? 'selected' : ''}`} key={group.groupId}><input type="checkbox" checked={selectedGroups.includes(group.groupId)} onChange={() => toggleGroup(group.groupId)} /><Check size={13} />{group.name}</label>) : <em>尚未同步群组</em>}</div>
    </section>
    <section className="section">
      <SectionHeading eyebrow="批量发送" title="向所选群发送文本" meta={selectedGroups.length ? `已选 ${selectedGroups.length} 个群` : '先选择群聊'} />
      <div className="compose-row"><textarea value={draft} onChange={event => setDraft(event.target.value)} placeholder="发送内容只会发到当前勾选的群。" /><button className="primary" disabled={sending || !draft.trim() || !selectedGroups.length} onClick={() => void send()}><Send size={15} />{sending ? '发送中' : '发送文本'}</button></div>
    </section>
    <section className="section">
      <SectionHeading eyebrow="本地消息" title="最近消息" meta={messages.length ? `${page + 1} / ${pageCount} 页` : undefined} actions={<div className="button-row"><button className="icon-button" title="上一页" disabled={page === 0} onClick={() => setPage(value => Math.max(0, value - 1))}><ChevronLeft size={16} /></button><button className="icon-button" title="下一页" disabled={page >= pageCount - 1} onClick={() => setPage(value => Math.min(pageCount - 1, value + 1))}><ChevronRight size={16} /></button></div>} />
      <PageFeedback state={state} error={error} retry={() => void reload()} emptyTitle="暂无消息" emptyDetail={accountId ? '连接并启用群后，消息会先落库再显示。' : '尚未识别当前账号。'}>
        <div className="message-table"><div className="message-row message-header"><span>时间</span><span>群聊 / 成员</span><span>内容</span><span>类型</span><span>状态</span><span /></div>{visible.map(message => <div className="message-row" key={message.id || `${message.groupId}-${message.serverMessageId}`}><time>{formatDate(message.sentAt)}</time><span><strong>{groups.find(group => group.groupId === message.groupId)?.name || `群 ${message.groupId}`}</strong><small>{message.senderName || `用户 ${message.userId}`}</small></span><p title={message.text}>{message.text || '（无文本内容）'}</p><span className="type-pill">{kindLabels[message.kind] || '其他'}</span><span className={`state-pill ${message.processedAt ? 'done' : ''}`}>{stateLabels[message.processingState || (message.processedAt ? 'processed' : 'pending')] || '待处理'}</span><button className="icon-button" title="撤回消息" onClick={() => void recall(message)}><Undo2 size={15} /></button></div>)}</div>
      </PageFeedback>
    </section>
  </div>
}

function formatDate(value?: string) {
  if (!value) return '-'
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString('zh-CN', { hour12: false })
}
