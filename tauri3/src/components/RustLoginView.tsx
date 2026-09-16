import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { LoaderCircle, LogIn, LogOut, RefreshCw } from 'lucide-react'
import './RustLoginView.css'

export type RustLoginStatus = { savedAccount?: string | null; configured: boolean; authenticated: boolean; nimConnected: boolean; captchaId: string; lastError: string | null; message?: string | null; challenge?: { id: string; kind: 'deviceSms'; maskedPhone: string; expiresIn: number; resendAfter: number } | null }
type Captcha = { verify(): void; refresh(): void; destroy?(): void }
type CaptchaInit = (options: { captchaId: string; element: string; mode: string; width: string; apiVersion: number; onVerify: (error: unknown, data?: { validate: string }) => void }, success: (instance: Captcha) => void, failure: () => void) => void
declare global { interface Window { initNECaptcha?: CaptchaInit } }
let captchaLoader: Promise<void> | undefined
function loadCaptcha() {
  if (window.initNECaptcha) return Promise.resolve()
  if (!captchaLoader) captchaLoader = new Promise<void>((resolve, reject) => {
    const script = document.createElement('script')
    script.src = 'https://cstaticdun.126.net/load.min.js'; script.async = true
    script.onload = () => window.initNECaptcha ? resolve() : reject(new Error('captcha'))
    script.onerror = () => { script.remove(); reject(new Error('captcha')) }
    document.head.append(script)
  }).catch(error => { captchaLoader = undefined; throw error })
  return captchaLoader
}
async function request(action: string, body: object = {}): Promise<RustLoginStatus> {
  const response = await fetch(`/api/wang/${action}`, { method: 'POST', credentials: 'same-origin', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body), signal: AbortSignal.timeout(120000) })
  if (response.status === 401) { window.dispatchEvent(new Event('dh-session-expired')); throw new Error('管理会话已过期，请重新登录') }
  if (!response.ok) throw new Error(response.status === 429 ? '尝试过于频繁，请稍后重试' : response.status === 409 ? '正在处理另一项登录操作' : '登录服务暂不可用')
  return response.json()
}
export default function RustLoginView({ initial, accountActionsTarget }: { initial: RustLoginStatus; accountActionsTarget?: HTMLElement | null }) {
  const [status, setStatus] = useState(initial)
  const [account, setAccount] = useState(initial.savedAccount || '')
  const [remember, setRemember] = useState(!!initial.savedAccount)
  const [password, setPassword] = useState('')
  const [smsLogin, setSmsLogin] = useState(false)
  const [code, setCode] = useState('')
  const [resendAfter, setResendAfter] = useState(0)
  const [expiresIn, setExpiresIn] = useState(0)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState(initial.lastError ? initial.message || '登录校验未通过，请重试' : '')
  const captcha = useRef<Captcha | null>(null)
  const attempt = useRef<{ action: string; body: Record<string, string | boolean> } | null>(null)
  const mounted = useRef(true)
  useEffect(() => {
    setResendAfter(status.challenge?.resendAfter ?? 0); setExpiresIn(status.challenge?.expiresIn ?? 0)
    const timer = setInterval(() => { setResendAfter(value => Math.max(0, value - 1)); setExpiresIn(value => Math.max(0, value - 1)) }, 1000)
    return () => clearInterval(timer)
  }, [status.challenge])
  useEffect(() => {
    if (busy) return
    let active = true
    const timer = setInterval(() => {
      void request('status').then(next => { if (active) setStatus(next) }).catch(() => {})
    }, 10000)
    return () => { active = false; clearInterval(timer) }
  }, [busy])
  const complete = (next: RustLoginStatus) => {
    if (!mounted.current) return
    setStatus(next)
    if (next.lastError === 'password_attempts_exceeded' || next.message?.includes('尝试次数过多')) { setSmsLogin(true); setPassword('') }
    setError(next.lastError ? next.message || (next.authenticated ? '账号已登录，即时通信连接失败' : '登录校验未通过，请重试') : '')
    window.dispatchEvent(new Event('dh-events-reconnected'))
  }
  useEffect(() => {
    mounted.current = true
    let active = true
    if (!status.authenticated) void loadCaptcha().then(() => {
      if (!active || !window.initNECaptcha) return
      window.initNECaptcha({ captchaId: status.captchaId, element: '#dh-rust-captcha', mode: 'popup', width: '320px', apiVersion: 2,
        onVerify: (failure, data) => {
          const input = attempt.current
          if (failure || !data?.validate) { if (input) setError('安全验证未通过或加载失败，请重新点击登录'); return }
          if (!input) return
          attempt.current = null; setBusy(true); setError('')
          void request(input.action, { ...input.body, validateStr: data.validate }).then(complete).catch(e => mounted.current && setError(e.message)).finally(() => {
            for (const key of Object.keys(input.body)) input.body[key] = ''; if (mounted.current) { setPassword(''); setCode(''); setBusy(false); captcha.current?.refresh() }
          })
        },
      }, instance => { if (active) captcha.current = instance; else instance.destroy?.() }, () => active && setError('安全验证加载失败，请重新打开登录页'))
    }).catch(() => active && setError('安全验证加载失败，请重新打开登录页'))
    return () => { active = false; mounted.current = false; attempt.current = null; captcha.current?.destroy?.(); captcha.current = null }
  }, [status.authenticated, status.captchaId])
  const command = async (action: string) => {
    attempt.current = null; setCode('')
    setBusy(true); setError('')
    try { complete(await request(action)) } catch (e) { setError(e instanceof Error ? e.message : '操作失败') } finally { setBusy(false) }
  }
  const verify = (action: string, body: Record<string, string | boolean>) => {
    if (action === 'login' && (!String(body.account).trim() || (!body.password && !body.useSavedPassword))) { setError('请先填写完整的账号和密码'); return }
    if (!captcha.current) { setError('安全验证尚未就绪'); return }
    setError(''); attempt.current = { action, body }
    try { captcha.current.verify() } catch { attempt.current = null; setError('安全验证无法打开，请刷新页面后重试') }
  }
  if (status.authenticated && accountActionsTarget) return createPortal(<div className="wang-account-actions" role="group" aria-label="旺商聊账号">
    <span className="wang-account-label" title={status.nimConnected ? '旺商聊账号已连接' : '账号已登录，通信未连接'}>旺商聊</span>
    <button className="secondary" aria-label="续期并重连" title="续期并重连" disabled={busy} onClick={() => void command('refresh')}>{busy ? <LoaderCircle className="spin" size={16} /> : <RefreshCw size={16} />}</button>
    <button className="secondary" aria-label="退出账号" title="退出旺商聊账号" disabled={busy} onClick={() => void command('logout')}><LogOut size={16} /></button>
    {error && <p className="wang-account-error" role="alert">{error}</p>}
  </div>, accountActionsTarget)
  return <section className="rust-login" aria-label="旺商聊登录" style={{ borderBottom: '1px solid var(--border-color, #ddd)', padding: '16px 0', marginBottom: 16 }}>
    <h2 style={{ fontSize: 18, margin: '0 0 12px' }}>旺商聊账号</h2>
    {!status.authenticated && !status.challenge && <div role="group" aria-label="登录方式" style={{ display: 'flex', gap: 8, marginBottom: 12 }}>
      <button className="secondary" type="button" disabled={busy} aria-pressed={!smsLogin} onClick={() => { setSmsLogin(false); setError('') }}>密码登录</button>
      <button className="secondary" type="button" disabled={busy} aria-pressed={smsLogin} onClick={() => { setSmsLogin(true); setPassword(''); setError('') }}>短信验证码登录</button>
    </div>}
    {status.authenticated ? <div style={{ display: 'flex', alignItems: 'center', gap: 12, flexWrap: 'wrap' }}>
      <span role="status">{status.nimConnected ? '已连接' : '账号已登录，通信未连接'}</span>
      <button className="secondary" disabled={busy} onClick={() => void command('refresh')}><RefreshCw size={16} /> 续期并重连</button>
      <button className="secondary" disabled={busy} onClick={() => void command('logout')}><LogOut size={16} /> 退出账号</button>
    </div> : status.challenge ? <form onSubmit={e => { e.preventDefault(); verify('verify-sms', { challengeId: status.challenge!.id, verificationCode: code }) }} style={{ maxWidth: 420, display: 'grid', gap: 10 }}>
      <p role="status">{expiresIn > 0 ? `设备登录验证，验证码已发至 ${status.challenge.maskedPhone.replace(/\D/g, '').length >= 4 ? status.challenge.maskedPhone : '绑定手机号'}` : '验证码验证已过期，请返回账号登录'}</p>
      <label>短信验证码<input required autoComplete="one-time-code" inputMode="numeric" pattern="[0-9]{6}" maxLength={6} value={code} onChange={e => setCode(e.target.value.replace(/\D/g, ''))} disabled={busy || expiresIn === 0} style={{ display: 'block', width: '100%', boxSizing: 'border-box' }} /></label>
      <button className="primary" type="submit" disabled={busy || expiresIn === 0 || code.length !== 6}>{busy ? <LoaderCircle className="spin" size={16} /> : <LogIn size={16} />} 验证并登录</button>
      <div style={{ display: 'flex', gap: 12, flexWrap: 'wrap' }}>
        <button className="secondary" type="button" disabled={busy || resendAfter > 0 || expiresIn === 0} onClick={() => verify('resend-sms', { challengeId: status.challenge!.id })}><RefreshCw size={16} />{resendAfter > 0 ? `${resendAfter} 秒后重发` : '重新发送验证码'}</button>
        <button className="secondary" type="button" disabled={busy} onClick={() => void command('logout')}>返回账号登录</button>
      </div>
    </form> : smsLogin ? <form onSubmit={e => { e.preventDefault(); verify('start-sms', { phone: account }) }} style={{ maxWidth: 420, display: 'grid', gap: 10 }}>
      <label>手机号（+86）<input required inputMode="tel" autoComplete="tel-national" pattern="[0-9]{11}" maxLength={11} value={account} onChange={e => setAccount(e.target.value.replace(/\D/g, ''))} disabled={busy} /></label>
      <p>完成人工验证后发送短信；每次发送至少间隔 60 秒。</p>
      <button className="primary" disabled={busy || !/^[0-9]{11}$/.test(account)} type="submit">{busy ? '正在发送' : '获取短信验证码'}</button>
    </form> : <form onSubmit={e => { e.preventDefault(); verify('login', { account, password, ...(remember ? { rememberPassword: true } : {}), ...(!password && account === status.savedAccount ? { useSavedPassword: true } : {}) }) }} style={{ maxWidth: 420, display: 'grid', gap: 10 }}>
      <label>旺商聊账号或手机号<input required autoComplete="username" maxLength={64} value={account} onChange={e => setAccount(e.target.value.trim())} disabled={busy} style={{ display: 'block', width: '100%', boxSizing: 'border-box' }} /></label>
      <label>密码<input required={account !== status.savedAccount} placeholder={account === status.savedAccount ? '使用已保存密码' : ''} type="password" autoComplete="current-password" maxLength={1024} value={password} onChange={e => setPassword(e.target.value)} disabled={busy} style={{ display: 'block', width: '100%', boxSizing: 'border-box' }} /></label>
      <label><input type="checkbox" checked={remember} disabled={busy} onChange={e => setRemember(e.target.checked)} /> 记住账号密码</label>
      {status.savedAccount && <button className="secondary" type="button" disabled={busy} onClick={() => { setRemember(false); void command('forget-login') }}>删除已保存的账号密码</button>}
      <button className="primary" type="submit" disabled={busy || !account.trim() || (!password && account !== status.savedAccount)}>{busy ? <LoaderCircle className="spin" size={16} /> : <LogIn size={16} />} {busy ? '正在登录' : '登录'}</button>
    </form>}
    <div id="dh-rust-captcha" />
    {error && <p role="alert">{error}</p>}
  </section>
}
