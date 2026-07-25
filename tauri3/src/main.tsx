import React, { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { Activity, BookOpen, Bug, CalendarClock, CircleAlert, LoaderCircle, MessagesSquare, Settings2, ShieldCheck, Users } from 'lucide-react'
import './styles.css'
import './window-titlebar.css'
import './brand.css'
import './runtime.css'
import './pages.css'
import './close-dialog.css'
import './button-help.css'
import './premium.css'
import './about.css'
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
import ButtonHelp from './components/ButtonHelp'
import AboutDialog from './components/AboutDialog'
import WindowTitlebar from './components/WindowTitlebar'
import type { Diagnostic } from './runtimeTypes'
import type { AiSettings, Audit, DailySummary, DatabaseStatus, Group, PageName } from './types'
import { api, readableError } from './api/client'

class AppErrorBoundary extends React.Component<{ children: ReactNode }, { message: string }> {
  state = { message: '' }
  static getDerivedStateFromError(error: unknown) { return { message: error instanceof Error ? error.message : String(error) } }
  render() { if (!this.state.message) return this.props.children; return <main className="fatal-page"><img src="/logo.png" alt="Logo" /><h1>页面加载失败</h1><p>{this.state.message}</p><button className="primary" onClick={() => window.location.reload()}>重新加载</button></main> }
}

const nav: [PageName, typeof Activity][] = [
  ['总览', Activity], ['群组与成员', Users], ['消息台', MessagesSquare], ['规则', ShieldCheck],
  ['知识与 AI', BookOpen], ['任务与计划', CalendarClock], ['审计', Activity], ['设置', Settings2], ['调试', Bug],
]

function isTauriRuntime() { return typeof window !== 'undefined' && Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__) }

type WangStartupEvent = { eventId: string; status: string; detail: string; needsConfirmation: boolean }
type WangStartResult = { status?: string; detail?: string; needsConfirmation?: boolean; maintenanceRequestId?: string | null }
type MaintenanceResult = { success: boolean; errorCode: string; message: string }

const wangInformationalStatuses = new Set(['discovering', 'validating', 'starting', 'started-waiting', 'ready', 'devtools-ready', 'nim-not-ready'])
const wangTransientStatuses = new Set(['discovering', 'validating', 'starting', 'started-waiting'])

function WangStatusBanner({ diagnostic, startup }: { diagnostic: Diagnostic | null; startup: WangStartupEvent | null }) {
  if (diagnostic?.status === 'ready') return null
  const status = diagnostic?.status === 'nim-not-ready' || diagnostic?.status === 'devtools-ready'
    ? diagnostic.status
    : startup?.status
  if (status === 'ready') return null
  if (!status && diagnostic) return null
  const loginRequired = status === 'nim-not-ready' || status === 'devtools-ready'
  const discovering = status === 'discovering' || status === 'validating'
  const starting = status === 'starting' || status === 'started-waiting' || !diagnostic && !startup
  const busy = loginRequired || discovering || starting
  const title = loginRequired
    ? '需要您登录账号'
    : status === 'discovering'
      ? '正在查找旺商聊'
      : status === 'validating'
        ? '正在验证旺商聊'
        : status === 'started-waiting'
          ? '正在等待 DevTools'
          : starting
            ? '正在启动旺商聊'
            : status === 'running-without-devtools'
              ? '需要确认重启旺商聊'
              : status === 'permission-denied'
                ? '无法读取部分安装位置'
                : status === 'not-found'
                  ? '没有找到旺商聊'
                  : '正在检查旺商聊连接'
  const detail = loginRequired
    ? '请在旺商聊窗口完成登录，DH BOT 正在实时等待登录。'
    : startup?.detail || '正在检测旺商聊、DevTools 和登录状态。'
  return <div className={`wang-status-banner ${busy ? 'loading' : 'attention'}`} role="status" aria-live="polite">
    <LoaderCircle size={17} className={busy ? 'spin' : ''} />
    <span><strong>{title}</strong><small>{detail}</small></span>
  </div>
}

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
  const [showAbout, setShowAbout] = useState(false)
  const [wangStartup, setWangStartup] = useState<WangStartupEvent | null>(null)
  const activeAccountRef = useRef('')
  const refreshSequenceRef = useRef(0)
  const handledWangStartupRef = useRef(new Set<string>())
  const diagnosticPollRef = useRef(false)
  const wangLoginFocusRequestedRef = useRef(false)
  const wangLoginFocusTimerRef = useRef<number | null>(null)
  const closeActionPendingRef = useRef(false)

  const showClosePrompt = () => {
    if (!closeActionPendingRef.current) setClosePrompt(true)
  }

  const cancelClose = () => setClosePrompt(false)

  const requestWangLoginFocus = () => {
    if (!isTauriRuntime() || wangLoginFocusRequestedRef.current) return
    wangLoginFocusRequestedRef.current = true
    wangLoginFocusTimerRef.current = window.setTimeout(() => {
      wangLoginFocusTimerRef.current = null
      void invoke('focus_wangshangliao').catch(() => {
        wangLoginFocusRequestedRef.current = false
      })
    }, 600)
  }

  const updateWangLoginFocus = (next: Diagnostic) => {
    const loginRequired = next.status === 'nim-not-ready' || next.status === 'devtools-ready'
    if (!loginRequired) {
      if (wangLoginFocusTimerRef.current !== null) window.clearTimeout(wangLoginFocusTimerRef.current)
      wangLoginFocusTimerRef.current = null
      wangLoginFocusRequestedRef.current = false
      return
    }
    requestWangLoginFocus()
  }

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

  const refresh = async (ensureWangRunning = false) => {
    const refreshSequence = ++refreshSequenceRef.current
    setLoading(true)
    const [initialDiagnosticResult, databaseResult, settingsResult] = await Promise.allSettled([
      invoke<Diagnostic>('diagnose'), invoke<DatabaseStatus>('database_status'), invoke<AiSettings>('get_ai_settings'),
    ])
    if (refreshSequence !== refreshSequenceRef.current) return
    let diagnosticResult = initialDiagnosticResult
    if (ensureWangRunning && isTauriRuntime() && initialDiagnosticResult.status === 'fulfilled' && initialDiagnosticResult.value.status === 'unavailable') {
      try {
        const settings = await invoke<{ path: string }>('get_wang_startup_settings')
        const result = await invoke<WangStartResult>('start_wangshangliao', {
          path: settings.path.trim() || null,
          devtoolsUrl: 'http://127.0.0.1:9222',
          confirmRestart: false,
        })
        if (result.status && !wangInformationalStatuses.has(result.status) && !result.needsConfirmation && result.detail) setError(result.detail)
        diagnosticResult = await Promise.resolve(invoke<Diagnostic>('diagnose')).then(
          value => ({ status: 'fulfilled', value } as PromiseFulfilledResult<Diagnostic>),
          reason => ({ status: 'rejected', reason } as PromiseRejectedResult),
        )
      } catch (reason) {
        setError(readableError(reason))
      }
    }
    if (refreshSequence !== refreshSequenceRef.current) return
    if (diagnosticResult.status === 'fulfilled') {
      setDiagnostic(diagnosticResult.value)
      updateWangLoginFocus(diagnosticResult.value)
    }
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
    const listeners: Promise<UnlistenFn>[] = []
    const handleWangStartup = (payload: WangStartupEvent) => {
      if (payload.eventId && handledWangStartupRef.current.has(payload.eventId)) return
      if (payload.eventId) handledWangStartupRef.current.add(payload.eventId)
      setWangStartup(payload)
      if (payload.status === 'nim-not-ready' || payload.status === 'devtools-ready') requestWangLoginFocus()
      if (wangTransientStatuses.has(payload.status)) return
      void (async () => {
        if (payload.needsConfirmation && window.confirm('检测到旺商聊已运行，但没有开启 9222 DevTools。需要结束该旺商聊进程并重新启动，是否继续？')) {
          try {
            const settings = await invoke<{ path: string }>('get_wang_startup_settings')
            const start = () => invoke<WangStartResult>('start_wangshangliao', {
              path: settings.path.trim() || null,
              devtoolsUrl: 'http://127.0.0.1:9222',
              confirmRestart: true,
            })
            const result = await finishConfirmedWangRestart(start, await start())
            if (result.status && !wangInformationalStatuses.has(result.status) && result.detail) setError(result.detail)
          } catch (reason) {
            setError(readableError(reason))
          }
        } else if (payload.status && !wangInformationalStatuses.has(payload.status) && payload.detail) {
          setError(payload.detail)
        }
        await refresh()
      })()
    }
    listeners.push(listen<Diagnostic>('connection-status', event => {
      setDiagnostic(event.payload)
      updateWangLoginFocus(event.payload)
      if (event.payload.status === 'ready') setWangStartup(null)
      const nextAccount = event.payload.nimAccount || ''
      if (nextAccount && switchAccount(nextAccount)) void refresh()
    }))
    for (const eventName of ['sync-progress', 'message-received', 'task-progress', 'schedule-updated', 'gateway-capabilities']) listeners.push(listen(eventName, () => setPageEpoch(value => value + 1)))
    listeners.push(listen<string>('connection-error', event => setError(readableError(event.payload))))
    listeners.push(listen<WangStartupEvent>('wangshangliao-status', event => handleWangStartup(event.payload)))
    void invoke<WangStartupEvent | null>('take_wang_startup_status').then(payload => {
      if (payload) handleWangStartup(payload)
    }).catch(() => undefined)
    const pollDiagnostic = async () => {
      if (diagnosticPollRef.current) return
      diagnosticPollRef.current = true
      try {
        const next = await invoke<Diagnostic>('diagnose')
        setDiagnostic(next)
        updateWangLoginFocus(next)
        if (next.status === 'ready') setWangStartup(null)
        const nextAccount = next.nimAccount || ''
        if (nextAccount && switchAccount(nextAccount)) void refresh()
      } catch {
        // The backend connection-status event remains authoritative during restarts.
      } finally {
        diagnosticPollRef.current = false
      }
    }
    const diagnosticTimer = window.setInterval(() => void pollDiagnostic(), 2_000)
    listeners.push(listen<{ paused?: boolean } | boolean>('automation-paused', event => setAutomationPaused(typeof event.payload === 'boolean' ? event.payload : Boolean(event.payload.paused))))
    listeners.push(listen('close-requested', showClosePrompt))
    return () => {
      window.clearInterval(diagnosticTimer)
      if (wangLoginFocusTimerRef.current !== null) window.clearTimeout(wangLoginFocusTimerRef.current)
      void Promise.all(listeners).then(values => values.forEach(unlisten => unlisten()))
    }
  }, [])

  const resolveClose = async (action: 'tray' | 'exit') => {
    if (closeActionPendingRef.current) return
    closeActionPendingRef.current = true
    setClosePrompt(false)
    try {
      await invoke('resolve_close_action', { action, remember: rememberCloseChoice })
    } catch (reason) {
      setError(readableError(reason))
    } finally {
      closeActionPendingRef.current = false
    }
  }

  const activeGroup = useMemo(() => groups.find(group => group.groupId === selectedGroup), [groups, selectedGroup])
  const connectionLabel = diagnostic ? statusText(diagnostic.status) : '检查中'

  return <div className="app-window"><WindowTitlebar /><main className="shell">
    <aside className="sidebar"><div className="brand"><img className="brand-logo" src="/logo.png" alt="DH BOT" /></div><nav>{nav.map(([label, Icon]) => <button className={`nav-item ${page === label ? 'active' : ''}`} onClick={() => setPage(label)} key={label}><Icon size={16} />{label}</button>)}</nav><button className="sidebar-foot" data-help="打开 DH BOT 版本、技术栈、作者和 Telegram 联系方式。" onClick={() => setShowAbout(true)}><span>DH BOT 3.0</span><small>查看版本与作者</small></button></aside>
    <section className="workspace">
      <header className="topbar"><div><span className="eyebrow">工作区</span><h1>{page}</h1></div><div className={`connection ${diagnostic?.status === 'ready' ? 'ok' : ''}`}><i />{connectionLabel}</div></header>
      {automationPaused && <div className="pause-banner"><ShieldCheck size={16} />全部自动化已暂停。读取、消息落库和审计仍会继续。</div>}
      <WangStatusBanner diagnostic={diagnostic} startup={wangStartup} />
      {error && <div className="error-banner"><CircleAlert size={16} /><span>{error}</span><button onClick={() => setError('')}>关闭</button></div>}
      {page === '总览' && <OverviewPage diagnostic={diagnostic} database={database} groups={groups} audits={overviewAudits} summaries={summaries} loading={loading} refresh={() => void refresh(true)} />}
      {page === '群组与成员' && <GroupMembersPage groups={groups} selectedGroup={selectedGroup} setSelectedGroup={setSelectedGroup} activeGroup={activeGroup} onError={setError} refresh={refresh} />}
      {page === '消息台' && <MessagesPage key={`messages-${accountId}-${pageEpoch}`} groups={groups} accountId={accountId} onError={setError} />}
      {page === '规则' && <RulesPage key={`rules-${accountId}-${pageEpoch}`} accountId={accountId} groups={groups} onError={setError} />}
      {page === '知识与 AI' && <KnowledgePage key={`knowledge-${accountId}-${pageEpoch}`} accountId={accountId} groups={groups} aiSettings={aiSettings} setAiSettings={setAiSettings} refresh={refresh} onError={setError} />}
      {page === '任务与计划' && <PlansPage key={`plans-${accountId}-${pageEpoch}`} accountId={accountId} groups={groups} onError={setError} />}
      {page === '审计' && <AuditPage key={`audit-${accountId}-${pageEpoch}`} accountId={accountId} groups={groups} onError={setError} />}
      {page === '设置' && <SettingsPage diagnostic={diagnostic} database={database} refresh={refresh} onError={setError} />}
      {page === '调试' && <DebugPage diagnostic={diagnostic} database={database} refresh={refresh} onError={setError} />}
    </section>
  </main><ButtonHelp />{showAbout && <AboutDialog onClose={() => setShowAbout(false)} />}{closePrompt && <CloseDialog remember={rememberCloseChoice} onRememberChange={setRememberCloseChoice} onCancel={cancelClose} onResolve={action => void resolveClose(action)} />}</div>
}

async function bootstrap() {
  if (import.meta.env.MODE === 'style') {
    const { installStylePreviewFixture } = await import('./stylePreviewFixture')
    installStylePreviewFixture()
  }
  createRoot(document.getElementById('root')!).render(<AppErrorBoundary><App /></AppErrorBoundary>)
}

void bootstrap()
