import React, { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { Activity, BookOpen, Bug, CalendarClock, CircleAlert, MessagesSquare, Settings2, ShieldCheck } from 'lucide-react'
import './styles.css'
import './brand.css'
import './runtime.css'
import './pages.css'
import './close-dialog.css'
import GroupMembersPage from './GroupMembersPage'
import OverviewPage, { statusText } from './pages/OverviewPage'
import MessagesPage from './pages/MessagesPage'
import RulesPage from './pages/RulesPage'
import KnowledgePage from './pages/KnowledgePage'
import PlansPage from './pages/PlansPage'
import AuditPage from './pages/AuditPage'
import SettingsPage from './pages/SettingsPage'
import DebugPage from './pages/DebugPage'
import CloseDialog from './components/CloseDialog'
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

type WangStartupEvent = { eventId: string; status: string; detail: string; needsConfirmation: boolean }
type WangStartResult = { detail?: string; maintenanceRequestId?: string | null }
type MaintenanceResult = { success: boolean; errorCode: string; message: string }

async function finishConfirmedWangRestart(
  start: () => Promise<WangStartResult>,
  initial: WangStartResult,
) {
  let result = initial
  const deadline = Date.now() + 30_000
  while (result.maintenanceRequestId) {
    if (Date.now() >= deadline) throw new Error('等待旺商聊固定登录分区维护完成超时，请在设置页重新检查')
    await new Promise(resolve => window.setTimeout(resolve, 500))
    const maintenance = await invoke<MaintenanceResult | null>('get_wang_maintenance_result', { requestId: result.maintenanceRequestId })
    if (!maintenance) continue
    if (!maintenance.success) throw new Error(maintenance.message || maintenance.errorCode)
    result = await start()
  }
  return result
}

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
  const [closePrompt, setClosePrompt] = useState(false)
  const [rememberCloseChoice, setRememberCloseChoice] = useState(false)
  const activeAccountRef = useRef('')
  const refreshSequenceRef = useRef(0)
  const handledWangStartupRef = useRef(new Set<string>())

  const switchAccount = (nextAccount: string) => {
    if (activeAccountRef.current === nextAccount) return false
    activeAccountRef.current = nextAccount
    setAccountId(nextAccount)
    setGroups([])
    setOverviewAudits([])
    setSummaries([])
    setSelectedGroup(null)
    setPageEpoch(value => value + 1)
    return true
  }

  const loadAccountData = async (nextAccount: string) => {
    setOverviewAudits([])
    setSummaries([])
    if (!nextAccount) return
    const [auditResult, summaryResult] = await Promise.allSettled([api.listAudit(nextAccount, 20), api.listSummaries(nextAccount, 10)])
    if (activeAccountRef.current !== nextAccount) return
    if (auditResult.status === 'fulfilled') setOverviewAudits(auditResult.value)
    if (summaryResult.status === 'fulfilled') setSummaries(summaryResult.value)
  }

  const refresh = async () => {
    const refreshSequence = ++refreshSequenceRef.current
    setLoading(true)
    const [diagnosticResult, databaseResult, settingsResult] = await Promise.allSettled([
      invoke<Diagnostic>('diagnose'), invoke<DatabaseStatus>('database_status'), invoke<AiSettings>('get_ai_settings'),
    ])
    if (refreshSequence !== refreshSequenceRef.current) return
    if (diagnosticResult.status === 'fulfilled') setDiagnostic(diagnosticResult.value)
    else if (!isTauriRuntime()) setDiagnostic({ status: 'unavailable', devtoolsUrl: 'http://127.0.0.1:9222', pageTitle: '', pageUrl: '', nimAccount: '', detail: '浏览器预览模式：请使用桌面程序执行连接检查。' })
    if (databaseResult.status === 'fulfilled') setDatabase(databaseResult.value)
    else if (!isTauriRuntime()) setDatabase({ path: '本地数据库', schemaVersion: 1, integrity: '等待桌面程序', accounts: 0, groups: 0, messages: 0 })
    if (settingsResult.status === 'fulfilled') setAiSettings(settingsResult.value)

    let nextGroups: Group[] = []
    try { nextGroups = diagnosticResult.status === 'fulfilled' && diagnosticResult.value.status === 'ready' ? await api.listGroups() : await api.listCachedGroups() }
    catch (reason) { if (isTauriRuntime()) setError(readableError(reason)) }
    if (refreshSequence !== refreshSequenceRef.current) return
    const nextAccount = diagnosticResult.status === 'fulfilled' ? diagnosticResult.value.nimAccount || nextGroups[0]?.accountId || '' : nextGroups[0]?.accountId || ''
    switchAccount(nextAccount)
    setGroups(nextGroups.filter(group => !nextAccount || group.accountId === nextAccount))
    await loadAccountData(nextAccount)
    setLoading(false)
  }

  useEffect(() => {
    void refresh()
    if (!isTauriRuntime()) return
    const showClosePrompt = () => setClosePrompt(true)
    window.addEventListener('dh-close-requested', showClosePrompt)
    const listeners: Promise<UnlistenFn>[] = []
    const handleWangStartup = (payload: WangStartupEvent) => {
      if (payload.eventId && handledWangStartupRef.current.has(payload.eventId)) return
      if (payload.eventId) handledWangStartupRef.current.add(payload.eventId)
      void (async () => {
        if (payload.detail) setError(payload.detail)
        if (payload.needsConfirmation && window.confirm('检测到旺商聊已运行，但没有开启 9222 DevTools。需要结束该旺商聊进程并重新启动，是否继续？')) {
          try {
            const settings = await invoke<{ path: string }>('get_wang_startup_settings')
            const start = () => invoke<WangStartResult>('start_wangshangliao', {
              path: settings.path.trim() || null,
              devtoolsUrl: 'http://127.0.0.1:9222',
              confirmRestart: true,
            })
            const result = await finishConfirmedWangRestart(start, await start())
            if (result.detail) setError(result.detail)
          } catch (reason) {
            setError(readableError(reason))
          }
        }
        await refresh()
      })()
    }
    listeners.push(listen<Diagnostic>('connection-status', event => {
      setDiagnostic(event.payload)
      const nextAccount = event.payload.nimAccount || ''
      if (nextAccount && switchAccount(nextAccount)) void refresh()
    }))
    for (const eventName of ['sync-progress', 'message-received', 'task-progress', 'schedule-updated', 'gateway-capabilities']) listeners.push(listen(eventName, () => setPageEpoch(value => value + 1)))
    listeners.push(listen<string>('connection-error', event => setError(readableError(event.payload))))
    listeners.push(listen<WangStartupEvent>('wangshangliao-status', event => handleWangStartup(event.payload)))
    void invoke<WangStartupEvent | null>('take_wang_startup_status').then(payload => {
      if (payload) handleWangStartup(payload)
    }).catch(() => undefined)
    listeners.push(listen<{ paused?: boolean } | boolean>('automation-paused', event => setAutomationPaused(typeof event.payload === 'boolean' ? event.payload : Boolean(event.payload.paused))))
    listeners.push(listen('close-requested', () => setClosePrompt(true)))
    return () => {
      window.removeEventListener('dh-close-requested', showClosePrompt)
      void Promise.all(listeners).then(values => values.forEach(unlisten => unlisten()))
    }
  }, [])

  const resolveClose = async (action: 'tray' | 'exit') => {
    try {
      await invoke('resolve_close_action', { action, remember: rememberCloseChoice })
      setClosePrompt(false)
    } catch (reason) {
      setError(readableError(reason))
    }
  }

  const activeGroup = useMemo(() => groups.find(group => group.groupId === selectedGroup), [groups, selectedGroup])
  const connectionLabel = diagnostic ? statusText(diagnostic.status) : '检查中'

  return <><main className="shell">
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
  </main>{closePrompt && <CloseDialog remember={rememberCloseChoice} onRememberChange={setRememberCloseChoice} onCancel={() => setClosePrompt(false)} onResolve={action => void resolveClose(action)} />}</>
}

createRoot(document.getElementById('root')!).render(<AppErrorBoundary><App /></AppErrorBoundary>)
