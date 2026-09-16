import { useEffect, useRef, useState } from 'react'
import { ArrowLeft, MessagesSquare, RefreshCw, UserRound, Users } from 'lucide-react'
import './ConversationsPage.css'

type Conversation = { id: string; kind: 'group' | 'private'; name: string; enabled: boolean; manualTakeover: boolean; updatedAt: number; messageCount: number; pendingAck: number }
type Message = { id: number; senderName: string; sentAt: number; text: string; contentKind: string; flow: string; state: string; reason: string; ackState: string; aiState: string; deliveryState: string }
const states: Record<string, string> = { recalled: '已撤回', pending: '待处理', queued: '已排队', retry: '等待重试', succeeded: '已完成', sent: '服务端已确认', rejected: '已拒绝', ignored: '已忽略', needs_human: '需人工处理', processing: '处理中', processed: '已处理', failed: '处理失败', unknown: '发送结果未知' }
async function request<T>(path: string, body: object = {}): Promise<T> {
  const response = await fetch(`/api/conversations/${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
  if (response.status === 401) { window.dispatchEvent(new Event('dh-session-expired')); throw new Error('管理会话已过期') }
  if (!response.ok) throw new Error(response.status === 409 ? '请先登录旺商聊账号' : response.status === 404 ? '会话服务暂不可用' : '会话读取失败，请重试')
  const result = await response.json()
  if (result && result.ok === false) throw new Error(result.message || '操作未完成')
  return result
}

export default function ConversationsPage() {
  const [items, setItems] = useState<Conversation[]>([])
  const [selected, setSelected] = useState('')
  const [messages, setMessages] = useState<Message[]>([])
  const [filter, setFilter] = useState<'all' | 'group' | 'private'>('all')
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [revision, setRevision] = useState(0)
  const generation = useRef(0)
  const active = items.find(item => item.id === selected)
  useEffect(() => {
    let live = true
    setLoading(true); setError('')
    void request<{ conversations: Conversation[] }>('list').then(result => { if (live) setItems(result.conversations) }).catch(e => { if (live) setError(e.message) }).finally(() => { if (live) setLoading(false) })
    return () => { live = false }
  }, [revision])
  useEffect(() => {
    const current = ++generation.current
    setMessages([])
    if (selected) void request<{ messages: Message[] }>('history', { id: selected }).then(result => { if (generation.current === current) setMessages(result.messages.reverse()) }).catch(e => { if (generation.current === current) setError(e.message) })
    return () => { generation.current++ }
  }, [selected, revision])
  const takeover = async () => {
    if (!active) return
    setSaving(true); setError('')
    try { await request('policy', { id: active.id, enabled: false, manualTakeover: !active.manualTakeover }); setRevision(value => value + 1) }
    catch (e) { setError(e instanceof Error ? e.message : '操作失败') }
    finally { setSaving(false) }
  }
  return <section className="conversation-page" aria-label="会话工作台">
    <header className="conversation-heading"><h2>会话</h2><button className="secondary" title="刷新会话" aria-label="刷新会话" disabled={loading} onClick={() => setRevision(value => value + 1)}><RefreshCw size={16} /></button></header>
    {error && <p role="alert">{error}</p>}
    <div className={`conversation-layout ${active ? 'has-selection' : ''}`}>
      <aside className="conversation-list"><div className="conversation-filters" role="group" aria-label="会话类型">{(['all', 'group', 'private'] as const).map(value => <button key={value} aria-pressed={filter === value} onClick={() => { setFilter(value); setSelected('') }}>{value === 'all' ? '全部' : value === 'group' ? '群聊' : '私信'}</button>)}</div>
        {items.filter(item => filter === 'all' || item.kind === filter).map(item => <button className="conversation-item" aria-current={selected === item.id ? 'true' : undefined} onClick={() => setSelected(item.id)} key={item.id}>{item.kind === 'group' ? <Users size={17} /> : <UserRound size={17} />}<span><strong>{item.name}</strong><small>{item.messageCount} 条消息{item.manualTakeover ? ' · 人工接管' : ''}</small></span></button>)}
        {!items.length && <p className="conversation-empty">{loading ? '正在读取会话…' : error ? '会话暂不可用' : '暂无会话'}</p>}
      </aside>
      <div className="conversation-detail">{active ? <><header><button className="secondary conversation-back" aria-label="返回会话列表" title="返回会话列表" onClick={() => setSelected('')}><ArrowLeft size={16} /></button><h3>{active.name}</h3><label className="conversation-takeover"><input type="checkbox" checked={active.manualTakeover} disabled={saving} onChange={() => void takeover()} />人工接管</label></header>
        <div className="conversation-history">{messages.map(message => <article className="conversation-message" key={message.id}><header><strong>{message.flow === 'out' ? '我' : message.senderName || '成员'}</strong><time>{new Date(message.sentAt).toLocaleString()}</time></header><p>{message.text || (message.contentKind === 'encrypted' ? '内容待解析' : '非文本消息')}</p><small>{states[message.state] || '待确认'} · {message.ackState === 'flushed' ? '接收确认已发出' : '接收确认待发送'}{message.aiState && ` · AI：${states[message.aiState] || '待确认'}`}{message.deliveryState && ` · 回复：${message.deliveryState === 'succeeded' ? '服务端已确认' : states[message.deliveryState] || '待确认'}`}</small>{message.reason && <small>{message.reason}</small>}</article>)}{!messages.length && <p className="conversation-empty">暂无消息</p>}</div>
      </> : <div className="conversation-empty"><MessagesSquare size={28} /><p>未选择会话</p></div>}</div>
    </div>
  </section>
}
