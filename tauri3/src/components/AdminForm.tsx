import { useEffect, useRef, useState } from 'react'
import { ArrowLeft, ArrowRight, LoaderCircle, ShieldCheck, KeyRound } from 'lucide-react'
type Mode = 'login' | 'activate' | 'migrate' | 'reset' | 'credentials'
export default function AdminForm({ initialMode, onComplete }: { initialMode: Mode; onComplete: () => void }) {
  const [mode, setMode] = useState(initialMode)
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [confirm, setConfirm] = useState('')
  const [currentPassword, setCurrentPassword] = useState('')
  const [code, setCode] = useState('')
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const submitting = useRef(false)
  const [busy, setBusy] = useState(false)
  const [retryAt, setRetryAt] = useState(0)
  const [clock, setClock] = useState(Date.now())
  useEffect(() => { const timer = setInterval(() => setClock(Date.now()), 1000); return () => clearInterval(timer) }, [])
  const retrySeconds = Math.max(0, Math.ceil((retryAt - clock) / 1000))
  const title = { login: '管理登录', activate: '首次激活', migrate: '升级管理员账号', reset: '重置管理员', credentials: '修改管理员账号密码' }[mode]
  return <section className="admin-auth"><div className="admin-auth-heading"><span className="admin-auth-icon"><ShieldCheck size={22} aria-hidden="true" /></span><span className="admin-auth-eyebrow">DH BOT · 管理控制台</span><h1>{title}</h1><p>{mode === 'login' ? '登录后管理你的工作区' : mode === 'reset' ? '使用服务器生成的一次性重置码设置新凭据' : mode === 'credentials' ? '验证当前密码后更新，所有管理会话将退出' : '创建专属管理员账号，保护你的工作区'}</p></div><form onSubmit={async e => {
    e.preventDefault(); if (submitting.current) return
    if (Date.now() < retryAt) { setError(`请在 ${Math.ceil((retryAt - Date.now()) / 1000)} 秒后重试`); return }
    if (mode !== 'login' && password !== confirm) { setError('两次输入的密码不一致'); return }
    submitting.current = true; setBusy(true); setError(''); setNotice('')
    const path = { login: '/api/login', activate: '/api/setup/activate', migrate: '/api/setup/migrate', reset: '/api/admin/reset', credentials: '/api/admin/credentials' }[mode]
    try {
      const response = await fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(mode === 'login' ? { username, password } : { username, password, currentPassword, code }) })
      if (!response.ok) {
        if (response.status === 429) { const seconds = Number(response.headers.get('Retry-After')) || 60; setRetryAt(Date.now() + seconds * 1000); setError(`尝试过于频繁，请 ${seconds} 秒后重试`) }
        else setError(response.status === 400 ? '账号需为 3–32 位字母、数字、下划线或短横线；密码需为 15–128 字符且不能是常见弱密码' : response.status === 401 ? (mode === 'login' ? '账号或密码错误' : '验证信息错误或已过期') : response.status === 409 ? '激活状态已改变，请刷新页面' : '服务暂不可用，请稍后重试')
        return
      }
      if (mode === 'login') onComplete()
      else { setMode('login'); setNotice('设置成功，请使用新账号密码登录'); if (initialMode === 'credentials') onComplete() }
    } catch { setError('Web 服务不可用') } finally { setPassword(''); setConfirm(''); setCurrentPassword(''); setCode(''); setBusy(false); submitting.current = false }
  }}>
    {(mode === 'activate' || mode === 'reset') && <label>一次性{mode === 'reset' ? '重置' : '激活'}码<input value={code} onChange={e => setCode(e.target.value)} autoComplete="off" required disabled={busy} maxLength={128} /></label>}
    {(mode === 'migrate' || mode === 'credentials') && <label>{mode === 'migrate' ? '原管理员密码' : '当前密码'}<input type="password" value={currentPassword} onChange={e => setCurrentPassword(e.target.value)} autoComplete="current-password" required disabled={busy} /></label>}
    <label>管理员账号<input value={username} onChange={e => setUsername(e.target.value)} autoComplete="username" pattern={'[A-Za-z0-9_\\-]{3,32}'} required disabled={busy} maxLength={32} /></label>
    <label>管理员密码<input type="password" value={password} onChange={e => setPassword(e.target.value)} autoComplete={mode === 'login' ? 'current-password' : 'new-password'} minLength={mode === 'login' ? undefined : 15} maxLength={128} required disabled={busy} /></label>
    {mode !== 'login' && <label>确认密码<input type="password" value={confirm} onChange={e => setConfirm(e.target.value)} autoComplete="new-password" required disabled={busy} maxLength={128} /></label>}
    <button className="admin-auth-primary" type="submit" disabled={busy || retrySeconds > 0}>{busy ? <LoaderCircle size={16} className="spin" aria-hidden="true" /> : <ArrowRight size={16} aria-hidden="true" />}{retrySeconds > 0 ? `请等待 ${retrySeconds} 秒` : busy ? '处理中' : mode === 'login' ? '登录' : '保存并继续'}</button>
  </form>{notice && <p className="admin-auth-notice" role="status">{notice}</p>}{error && <p className="admin-auth-error" role="alert">{error}</p>}
  {mode === 'login' && <button className="admin-auth-link" disabled={busy} type="button" onClick={() => { setMode('reset'); setError(''); setNotice('请通过 SSH 在服务器执行 dh-server reset-admin 数据目录，获取 30 分钟有效的重置码。网页不会自动生成。'); setPassword('') }}><KeyRound size={15} aria-hidden="true" />忘记管理员密码</button>}
  {mode === 'reset' && <button className="admin-auth-link" disabled={busy} type="button" onClick={() => { setMode('login'); setError(''); setNotice(''); setPassword(''); setConfirm(''); setCode('') }}><ArrowLeft size={15} aria-hidden="true" />返回登录</button>}
  </section>
}
