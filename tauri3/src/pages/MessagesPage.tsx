import { useEffect, useState } from 'react'
import { Check, ChevronLeft, ChevronRight, Filter, RefreshCw, Search, Send, Undo2, X } from 'lucide-react'
import { api, readableError } from '../api/client'
import { EmptyState, PageFeedback, SectionHeading } from '../components/PageState'
import SelectField from '../components/SelectField'
import type { Group, LoadState, Message } from '../types'

const kindLabels: Record<string, string> = { text: '文本', image: '图片', card: '名片', other: '其他' }
const stateLabels: Record<string, string> = { processed: '已处理', pending: '待处理', processing: '处理中', failed: '失败', ignored: '历史归档' }

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
  const [cursor, setCursor] = useState<string | undefined>()
  const [nextCursor, setNextCursor] = useState<string | null>(null)
  const [backStack, setBackStack] = useState<Array<string | undefined>>([])
  const [recallTarget, setRecallTarget] = useState<Message | null>(null)
  const [recalling, setRecalling] = useState(false)
  const pageSize = 30

  const load = async (requestedCursor?: string, filterOverride?: { kind?: string; processingState?: string }) => {
    if (!accountId) { setMessages([]); setState('empty'); return }
    setState('loading'); setError('')
    try {
      const result = await api.queryMessages(accountId, { groupIds: selectedGroups, keyword, kind: filterOverride?.kind ?? kind, processingState: filterOverride?.processingState ?? processingState }, requestedCursor, pageSize)
      setMessages(result.items); setCursor(requestedCursor); setNextCursor(result.nextCursor); setState(result.items.length ? 'ready' : 'empty')
      return true
    } catch (reason) { setError(readableError(reason)); setState(messages.length ? 'offlineCached' : 'error'); return false }
  }
  const reload = async (filterOverride?: { kind?: string; processingState?: string }) => {
    if (await load(undefined, filterOverride)) { setPage(0); setBackStack([]) }
  }
  useEffect(() => { void reload() }, [accountId])
  const nextPage = async () => {
    if (!nextCursor) return
    const target = nextCursor
    if (await load(target)) { setBackStack(values => [...values, cursor]); setPage(value => value + 1) }
  }
  const previousPage = async () => {
    if (!backStack.length) return
    const target = backStack[backStack.length - 1]
    if (await load(target)) { setBackStack(values => values.slice(0, -1)); setPage(value => Math.max(0, value - 1)) }
  }
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
  const recall = async () => {
    if (!recallTarget) return
    setRecalling(true); onError('')
    try { await api.recallMessage(recallTarget); setRecallTarget(null); await reload() } catch (reason) { onError(readableError(reason)) }
    finally { setRecalling(false) }
  }

  return <div className="page-stack">
    <section className="section">
      <SectionHeading eyebrow="实时流水线" title="消息台" meta={`${messages.length} 条`} actions={<button className="secondary" onClick={() => void reload()}><RefreshCw size={15} />刷新</button>} />
      <div className="filter-grid">
        <label className="search-field wide"><Search size={15} aria-hidden="true" /><input aria-label="搜索消息" value={keyword} onChange={event => setKeyword(event.target.value)} onKeyDown={event => event.key === 'Enter' && void reload()} placeholder="搜索成员、内容或消息 ID" /></label>
        <SelectField label="消息类型" value={kind} options={[{ value: 'all', label: '全部类型' }, ...Object.entries(kindLabels).map(([value, label]) => ({ value, label }))]} onChange={value => { setKind(value); void reload({ kind: value }) }} />
        <SelectField label="处理状态" value={processingState} options={[{ value: 'all', label: '全部状态' }, ...Object.entries(stateLabels).map(([value, label]) => ({ value, label }))]} onChange={value => { setProcessingState(value); void reload({ processingState: value }) }} />
      </div>
      <div className="group-filter"><span><Filter size={14} />选择群聊</span>{groups.length ? groups.map(group => <label className={`check-chip ${selectedGroups.includes(group.groupId) ? 'selected' : ''}`} key={group.groupId}><input type="checkbox" checked={selectedGroups.includes(group.groupId)} onChange={() => toggleGroup(group.groupId)} /><Check size={13} />{group.name}</label>) : <em>尚未同步群组</em>}</div>
    </section>
    <section className="section">
      <SectionHeading eyebrow="批量发送" title="向所选群发送文本" meta={selectedGroups.length ? `已选 ${selectedGroups.length} 个群` : '先选择群聊'} />
      <div className="compose-row"><textarea value={draft} onChange={event => setDraft(event.target.value)} placeholder="发送内容只会发到当前勾选的群。" /><button className="primary" disabled={sending || !draft.trim() || !selectedGroups.length} onClick={() => void send()}><Send size={15} />{sending ? '发送中' : '发送文本'}</button></div>
    </section>
    <section className="section">
      <SectionHeading eyebrow="本地消息" title="最近消息" meta={messages.length ? `第 ${page + 1} 页 · ${messages.length} 条` : undefined} actions={<div className="button-row"><button className="icon-button" title="上一页" disabled={!backStack.length} onClick={() => void previousPage()}><ChevronLeft size={16} /></button><button className="icon-button" title="下一页" disabled={!nextCursor} onClick={() => void nextPage()}><ChevronRight size={16} /></button></div>} />
      <PageFeedback state={state} error={error} retry={() => void reload()} emptyTitle="暂无消息" emptyDetail={accountId ? '连接并启用群后，消息会先落库再显示。' : '尚未识别当前账号。'}>
        <div className="message-table"><div className="message-row message-header"><span>时间</span><span>群聊 / 成员</span><span>内容</span><span>类型</span><span>状态</span><span /></div>{messages.map(message => <div className="message-row" key={message.id || `${message.groupId}-${message.serverMessageId}`}><time>{formatDate(message.sentAt)}</time><span><strong>{groups.find(group => group.groupId === message.groupId)?.name || `群 ${message.groupId}`}</strong><small>{message.senderName || `用户 ${message.userId}`}</small></span><p title={message.text}>{message.text || '（无文本内容）'}</p><span className="type-pill">{kindLabels[message.kind] || '其他'}</span><span className={`state-pill ${message.processedAt ? 'done' : ''}`}>{stateLabels[message.processingState || (message.processedAt ? 'processed' : 'pending')] || '待处理'}</span><button className="icon-button" title="撤回消息" data-help="撤回这条群消息；执行前会再次显示消息内容和目标群。" onClick={() => setRecallTarget(message)}><Undo2 size={15} /></button></div>)}</div>
      </PageFeedback>
    </section>
    {recallTarget && <div className="dialog-backdrop group-batch-dialog-backdrop" role="presentation">
      <section className="group-batch-dialog compact-confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="recall-confirm-title">
        <header><div><span className="eyebrow">消息操作</span><h2 id="recall-confirm-title">确认撤回消息</h2></div><button className="icon-command" title="关闭" onClick={() => setRecallTarget(null)} disabled={recalling}><X size={16} /></button></header>
        <p className="group-batch-confirm-copy">将从“{groups.find(group => group.groupId === recallTarget.groupId)?.name || `群 ${recallTarget.groupId}`}”撤回这条消息：</p>
        <blockquote className="message-recall-preview">{recallTarget.text || '（无文本内容）'}</blockquote>
        <footer><span /><button className="secondary" onClick={() => setRecallTarget(null)} disabled={recalling}>取消</button><button className="primary" onClick={() => void recall()} disabled={recalling}><Undo2 size={15} />{recalling ? '正在撤回' : '确认撤回'}</button></footer>
      </section>
    </div>}
  </div>
}

function formatDate(value?: string) {
  if (!value) return '-'
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString('zh-CN', { hour12: false })
}
