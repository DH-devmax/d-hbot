import { CheckCircle2, CircleAlert, Clock3, ListTodo, LoaderCircle, X } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { createPortal } from 'react-dom'
import type { PageName, RuntimeWorkItem, RuntimeWorkSnapshot } from '../types'

type Props = {
  snapshot: RuntimeWorkSnapshot
  open: boolean
  onToggle: () => void
  onClose: () => void
  onNavigate: (page: PageName) => void
}

const circumference = 2 * Math.PI * 48

const stateLabels: Record<string, string> = {
  running: '处理中', queued: '排队', retrying: '等待重试', succeeded: '已完成', failed: '失败', unknown: '结果未知',
}

function destination(kind: string): PageName {
  if (kind === 'aiReply' || kind === 'aiRule' || kind === 'prediction') return '知识与 AI'
  if (kind === 'schedule' || kind === 'reminder' || kind === 'summary') return '任务与计划'
  if (kind === 'cardRename' || kind === 'sync') return '群组与成员'
  if (kind === 'write') return '审计'
  return '消息台'
}

function WorkRow({ item, onNavigate }: { item: RuntimeWorkItem; onNavigate: (page: PageName) => void }) {
  const Icon = item.state === 'running' ? LoaderCircle : item.state === 'succeeded' ? CheckCircle2 : item.state === 'failed' || item.state === 'unknown' ? CircleAlert : Clock3
  return <button className={`runtime-work-row ${item.state}`} onClick={() => onNavigate(destination(item.kind))}>
    <Icon size={16} className={item.state === 'running' && item.percent == null ? 'spin' : ''} />
    <span><strong>{item.label}</strong><small>{item.scopeLabel || stateLabels[item.state]}{item.queued > 0 ? ` · 后续 ${item.queued}` : ''}</small>{item.error && <em>{item.error}</em>}</span>
    <b>{item.percent != null ? `${item.percent}%` : stateLabels[item.state]}</b>
  </button>
}

export default function RuntimeWorkIndicator({ snapshot, open, onToggle, onClose, onNavigate }: Props) {
  const busy = Boolean(snapshot.active || snapshot.counts.queued || snapshot.counts.retrying)
  const [busyVisible, setBusyVisible] = useState(false)
  useEffect(() => {
    if (!busy) { setBusyVisible(false); return }
    const timer = window.setTimeout(() => setBusyVisible(true), snapshot.counts.queued > 0 ? 0 : 200)
    return () => window.clearTimeout(timer)
  }, [busy, snapshot.counts.queued])
  useEffect(() => {
    if (!open) return
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose()
    }
    document.addEventListener('keydown', closeOnEscape)
    return () => document.removeEventListener('keydown', closeOnEscape)
  }, [open, onClose])

  const active = snapshot.active
  const percent = active?.percent ?? null
  const dashOffset = percent == null ? circumference * 0.72 : circumference * (1 - percent / 100)
  const sections = useMemo(() => [
    ['处理中', snapshot.items.filter(item => item.state === 'running')],
    ['排队', snapshot.items.filter(item => item.state === 'queued')],
    ['等待重试', snapshot.items.filter(item => item.state === 'retrying')],
    ['失败与未知', snapshot.items.filter(item => item.state === 'failed' || item.state === 'unknown')],
    ['最近完成', snapshot.items.filter(item => item.state === 'succeeded')],
  ] as const, [snapshot.items])
  const summaryItem = active || snapshot.items.find(item => item.state === 'queued' || item.state === 'retrying' || item.state === 'failed' || item.state === 'unknown')
  const queued = snapshot.counts.queued + snapshot.counts.retrying
  const toggle = () => {
    document.dispatchEvent(new Event('dh-hide-button-help'))
    onToggle()
  }

  return <>
    <div className="brand">
      <button className={`brand-task-button ${busyVisible ? 'busy' : ''} ${snapshot.counts.failed ? 'has-failure' : ''}`} onClick={toggle} aria-label={busyVisible ? `查看任务队列，${active?.label || '任务处理中'}` : '查看任务队列'}>
        <svg className={`brand-progress ${busyVisible && percent == null ? 'indeterminate' : ''}`} viewBox="0 0 104 104" aria-hidden="true">
          <circle className="brand-progress-track" cx="52" cy="52" r="48" />
          <circle className="brand-progress-value" cx="52" cy="52" r="48" strokeDasharray={circumference} strokeDashoffset={busyVisible ? dashOffset : circumference} />
        </svg>
        <img className="brand-logo" src="/logo.png" alt="DH BOT" />
        {busyVisible && percent != null && <span className="brand-progress-label">{percent}%</span>}
        {snapshot.counts.failed > 0 && <span className="brand-failure-count">{snapshot.counts.failed}</span>}
      </button>
    </div>
    <button className={`runtime-work-summary ${busyVisible || snapshot.counts.failed ? 'visible' : ''}`} onClick={toggle} aria-label="打开正在处理的任务">
      <span>{busyVisible ? <LoaderCircle size={14} className={percent == null ? 'spin' : ''} /> : snapshot.counts.failed ? <CircleAlert size={14} /> : <ListTodo size={14} />}</span>
      <strong>{summaryItem?.label || '当前没有处理任务'}</strong>
      <small>{busyVisible && percent != null ? `进度 ${percent}%` : queued > 0 ? `排队 ${queued} · 并行 ${snapshot.counts.running}` : snapshot.counts.failed ? `${snapshot.counts.failed} 项需要查看` : '任务队列空闲'}</small>
    </button>
    {open && createPortal(<aside className="runtime-work-drawer" role="dialog" aria-label="任务队列" aria-modal="false">
      <header><div><span className="eyebrow">运行状态</span><h2>任务队列</h2></div><button className="icon-button" title="关闭任务队列" onClick={onClose}><X size={18} /></button></header>
      <div className="runtime-work-drawer-summary"><span><strong>{snapshot.counts.running}</strong>处理中</span><span><strong>{queued}</strong>排队</span><span className={snapshot.counts.failed ? 'warning' : ''}><strong>{snapshot.counts.failed}</strong>异常</span></div>
      <div className="runtime-work-list">
        {sections.map(([label, items]) => items.length > 0 && <section key={label}><h3>{label}<small>{items.length}</small></h3>{items.map(item => <WorkRow key={item.id} item={item} onNavigate={page => { onNavigate(page); onClose() }} />)}</section>)}
        {snapshot.items.length === 0 && <div className="runtime-work-empty"><ListTodo size={22} /><strong>当前没有处理任务</strong><span>新任务进入队列后会显示在这里。</span></div>}
      </div>
    </aside>, document.body)}
  </>
}
