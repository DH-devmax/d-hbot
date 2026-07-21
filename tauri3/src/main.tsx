import React, { useEffect, useMemo, useState, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { Activity, BookOpen, Bug, CalendarClock, CircleAlert, MessagesSquare, Settings2, ShieldCheck } from 'lucide-react'
import './styles.css'
import './brand.css'
import './runtime.css'
import './pages.css'
import GroupMembersPage from './GroupMembersPage'
import OverviewPage, { statusText } from './pages/OverviewPage'
import MessagesPage from './pages/MessagesPage'
import RulesPage from './pages/RulesPage'
import KnowledgePage from './pages/KnowledgePage'
import PlansPage from './pages/PlansPage'
import AuditPage from './pages/AuditPage'
import SettingsPage from './pages/SettingsPage'
import DebugPage from './pages/DebugPage'
import type { Diagnostic } from './runtimeTypes'
import type { AiSettings, Audit, DailySummary, DatabaseStatus, Group, PageName } from './types'
import { api, readableError } from './api/client'

class AppErrorBoundary extends React.Component<{ children: ReactNode }, { message: string }> {
  state = { message: '' }
  static getDerivedStateFromError(error: unknown) { return { message: error instanceof Error ? error.message : String(error) } }
  render() { if (!this.state.message) return this.props.children; return <main className="fatal-page"><img src="/logo.png" alt="Logo" /><h1>页面加载失败</h1><p>{this.state.message}</p><button className="primary" onClick={() => window.location.reload()}>重新加载</button></main> }
}

const nav: [PageName, typeof Activity][] = [
  ['总览', Activity], ['群组与成员', MessagesSquare], ['消息台', MessagesSquare], ['规则', ShieldCheck],
  ['知识与 AI', BookOpen], ['任务与计划', CalendarClock], ['审计', Activity], ['设置', Settings2], ['调试', Bug],
]

function isTauriRuntime() { return typeof window !== 'undefined' && Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) }

function App() {
  const [page, setPage] = useState<PageName>('总览')
  const [diagnostic, setDiagnostic] = useState<Diagnostic | null>(null)
  const [database, setDatabase] = useState<DatabaseStatus | null>(null)
  const [groups, setGroups] = useState<Group[]>([])
  const [accountId, setAccountId] = useState('')
  const [aiSettings, setAiSettings] = useState<AiSettings>({ base_url: '', webhook_url: '', model: 'deepseek-v4-pro', api_key_configured: false })
  const [overviewAudits, setOverviewAudits] = useState<Audit[]>([])
  const [summaries, setSummaries] = useState<DailySummary[]>([])
  const [selectedGroup, setSelectedGroup] = useState<number | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [automationPaused, setAutomationPaused] = useState(false)
  const [pageEpoch, setPageEpoch] = useState(0)

  const loadAccountData = async (nextAccount: string) => {
    if (!nextAccount) { setOverviewAudits([]); setSummaries([]); return }
    const [auditResult, summaryResult] = await Promise.allSettled([api.listAudit(nextAccount, 20), api.listSummaries(nextAccount, 10)])
    if (auditResult.status === 'fulfilled') setOverviewAudits(auditResult.value)
    if (summaryResult.status === 'fulfilled') setSummaries(summaryResult.value)
  }

  const refresh = async () => {
    setLoading(true)
    const [diagnosticResult, databaseResult, settingsResult] = await Promise.allSettled([
      invoke<Diagnostic>('diagnose'), invoke<DatabaseStatus>('database_status'), invoke<AiSettings>('get_ai_settings'),
    ])
    if (diagnosticResult.status === 'fulfilled') setDiagnostic(diagnosticResult.value)
    else if (!isTauriRuntime()) setDiagnostic({ status: 'unavailable', devtoolsUrl: 'http://127.0.0.1:9222', pageTitle: '', pageUrl: '', nimAccount: '', detail: '浏览器预览模式：请使用桌面程序执行连接检查。' })
    if (databaseResult.status === 'fulfilled') setDatabase(databaseResult.value)
    else if (!isTauriRuntime()) setDatabase({ path: '本地数据库', schemaVersion: 1, integrity: '等待桌面程序', accounts: 0, groups: 0, messages: 0 })
    if (settingsResult.status === 'fulfilled') setAiSettings(settingsResult.value)

    let nextGroups: Group[] = []
    try { nextGroups = diagnosticResult.status === 'fulfilled' && diagnosticResult.value.status === 'ready' ? await api.listGroups() : await api.listCachedGroups() }
    catch (reason) { if (isTauriRuntime()) setError(readableError(reason)) }
    setGroups(nextGroups)
    const nextAccount = diagnosticResult.status === 'fulfilled' ? diagnosticResult.value.nimAccount || nextGroups[0]?.accountId || '' : nextGroups[0]?.accountId || ''
    setAccountId(current => {
      if (current && nextAccount && current !== nextAccount) { setSelectedGroup(null); setPageEpoch(value => value + 1) }
      return nextAccount
    })
    await loadAccountData(nextAccount)
    setLoading(false)
  }

  useEffect(() => {
    void refresh()
    if (!isTauriRuntime()) return
    const listeners: Promise<UnlistenFn>[] = []
    listeners.push(listen<Diagnostic>('connection-status', event => { setDiagnostic(event.payload); if (event.payload.nimAccount) setAccountId(event.payload.nimAccount) }))
    for (const eventName of ['sync-progress', 'message-received', 'task-progress', 'schedule-updated', 'gateway-capabilities']) listeners.push(listen(eventName, () => setPageEpoch(value => value + 1)))
    listeners.push(listen<string>('connection-error', event => setError(readableError(event.payload))))
    listeners.push(listen<{ paused?: boolean } | boolean>('automation-paused', event => setAutomationPaused(typeof event.payload === 'boolean' ? event.payload : Boolean(event.payload.paused))))
    return () => { void Promise.all(listeners).then(values => values.forEach(unlisten => unlisten())) }
  }, [])

  const activeGroup = useMemo(() => groups.find(group => group.groupId === selectedGroup), [groups, selectedGroup])
  const connectionLabel = diagnostic ? statusText(diagnostic.status) : '检查中'

  return <main className="shell">
    <aside className="sidebar"><div className="brand"><img className="brand-logo" src="/logo.png" alt="DH BOT" /></div><nav>{nav.map(([label, Icon]) => <button className={`nav-item ${page === label ? 'active' : ''}`} onClick={() => setPage(label)} key={label}><Icon size={16} />{label}</button>)}</nav><div className="sidebar-foot">DH BOT 3.0</div></aside>
    <section className="workspace">
      <header className="topbar"><div><span className="eyebrow">工作区</span><h1>{page}</h1></div><div className={`connection ${diagnostic?.status === 'ready' ? 'ok' : ''}`}><i />{connectionLabel}</div></header>
      {automationPaused && <div className="pause-banner"><ShieldCheck size={16} />全部自动化已暂停。读取、消息落库和审计仍会继续。</div>}
      {error && <div className="error-banner"><CircleAlert size={16} /><span>{error}</span><button onClick={() => setError('')}>关闭</button></div>}
      {page === '总览' && <OverviewPage diagnostic={diagnostic} database={database} groups={groups} audits={overviewAudits} summaries={summaries} loading={loading} refresh={() => void refresh()} />}
      {page === '群组与成员' && <GroupMembersPage groups={groups} selectedGroup={selectedGroup} setSelectedGroup={setSelectedGroup} activeGroup={activeGroup} onError={setError} refresh={refresh} />}
      {page === '消息台' && <MessagesPage key={`messages-${accountId}-${pageEpoch}`} groups={groups} accountId={accountId} onError={setError} />}
      {page === '规则' && <RulesPage key={`rules-${accountId}-${pageEpoch}`} accountId={accountId} groups={groups} onError={setError} />}
      {page === '知识与 AI' && <KnowledgePage key={`knowledge-${accountId}-${pageEpoch}`} accountId={accountId} groups={groups} onError={setError} />}
      {page === '任务与计划' && <PlansPage key={`plans-${accountId}-${pageEpoch}`} accountId={accountId} groups={groups} onError={setError} />}
      {page === '审计' && <AuditPage key={`audit-${accountId}-${pageEpoch}`} accountId={accountId} groups={groups} onError={setError} />}
      {page === '设置' && <SettingsPage diagnostic={diagnostic} database={database} aiSettings={aiSettings} setAiSettings={setAiSettings} refresh={refresh} onError={setError} />}
      {page === '调试' && <DebugPage diagnostic={diagnostic} database={database} refresh={refresh} onError={setError} />}
    </section>
  </main>
}

createRoot(document.getElementById('root')!).render(<AppErrorBoundary><App /></AppErrorBoundary>)
