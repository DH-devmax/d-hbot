import AdminForm from './AdminForm'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { isWebMode } from '../api/transport'
import { ChevronDown, LogOut } from 'lucide-react'
import './WebSession.css'
export default function WebSession({ children }: { children: ReactNode }) {
  const [authenticated, setAuthenticated] = useState(!isWebMode())
  const [setup, setSetup] = useState<'login' | 'activate' | 'migrate' | null>(null)
  const [editingAdmin, setEditingAdmin] = useState(false)
  const [error, setError] = useState('')
  const [barOpen, setBarOpen] = useState(false)
  const barRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!barOpen) return
    const dismiss = (event: PointerEvent) => { if (!barRef.current?.contains(event.target as Node)) setBarOpen(false) }
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') { setBarOpen(false); barRef.current?.querySelector<HTMLButtonElement>('.web-session-handle')?.focus() } }
    document.addEventListener('pointerdown', dismiss)
    document.addEventListener('keydown', escape)
    return () => { document.removeEventListener('pointerdown', dismiss); document.removeEventListener('keydown', escape) }
  }, [barOpen])
  useEffect(() => {
    if (!authenticated || !isWebMode()) return
    let last = 0
    const activity = () => { if (Date.now() - last < 60000) return; last = Date.now(); void fetch('/api/session/activity', { method: 'POST' }).then(r => { if (r.status === 401) window.dispatchEvent(new Event('dh-session-expired')) }).catch(() => {}) }
    window.addEventListener('pointerdown', activity); window.addEventListener('keydown', activity)
    return () => { window.removeEventListener('pointerdown', activity); window.removeEventListener('keydown', activity) }
  }, [authenticated])
  useEffect(() => { const edit = () => setEditingAdmin(true); window.addEventListener('dh-admin-settings', edit); return () => window.removeEventListener('dh-admin-settings', edit) }, [])
  const revision = useRef(0)
  useEffect(() => {
    if (!isWebMode()) return
    let active = true
    void fetch('/api/setup/status').then(r => { if (!r.ok) throw Error(); return r.json() }).then(v => { if (active) setSetup(v.mode) }).catch(() => { if (active) setError('无法读取激活状态') })
    const current = revision.current
    void fetch('/api/session').then(r => { if (active && revision.current === current) setAuthenticated(r.ok) }).catch(() => { if (active && revision.current === current) setError('Web 服务不可用') })
    const expired = () => { revision.current++; setAuthenticated(false); setEditingAdmin(false); setBarOpen(false); setSetup('login') }
    window.addEventListener('dh-session-expired', expired)
    return () => { active = false; window.removeEventListener('dh-session-expired', expired) }
  }, [])
  if (authenticated && !isWebMode()) return <>{children}</>
  if (authenticated && editingAdmin) return <main className="fatal-page admin-page"><AdminForm initialMode="credentials" onComplete={() => { setAuthenticated(false); setEditingAdmin(false); setSetup('login') }} /><button className="admin-auth-back" onClick={() => setEditingAdmin(false)}>返回管理台</button></main>
  if (authenticated) return <div className="web-session"><div ref={barRef} className={`web-session-drawer${barOpen ? ' open' : ''}`}>
    <aside id="web-management-bar" className="web-session-bar" hidden={!barOpen}><div className="web-session-brand"><img src="/logo.png" width="24" height="24" alt="" /><strong>DH BOT</strong><span>管理控制台</span></div><button className="secondary web-session-logout" onClick={() => setEditingAdmin(true)}>管理员账号设置</button><button className="secondary web-session-logout" onClick={async () => { const r = await fetch('/api/logout', { method: 'POST' }); if (r.ok) { setBarOpen(false); setAuthenticated(false) } }}><LogOut size={15} aria-hidden="true" />退出管理</button></aside>
    <button className="web-session-handle" aria-controls="web-management-bar" aria-expanded={barOpen} aria-label={barOpen ? '收起管理栏' : '展开管理栏'} title={barOpen ? '收起管理栏' : '展开管理栏'} onClick={() => setBarOpen(value => !value)}><ChevronDown size={16} /></button>
  </div>{children}</div>
  return <main className="fatal-page admin-page"><img src="/logo.png" alt="DH BOT" />{setup ? <AdminForm initialMode={setup} onComplete={() => { revision.current++; setSetup('login'); setAuthenticated(true) }} /> : <p role="status">{error || '正在读取激活状态'}</p>}</main>
}
