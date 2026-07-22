import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { RefreshCw } from 'lucide-react'
import type { RuntimeControlProps } from './runtimeTypes'

type WangStartResult = { needsConfirmation: boolean; detail: string; maintenanceRequestId?: string | null }
type MaintenanceResult = { success: boolean; errorCode: string; message: string }
type MaintenanceStatus = {
  state: string
  detail: string
  scriptHash: string
  backupPath?: string | null
  requiresElevation: boolean
  requestId?: string | null
}

async function finishMaintenance(
  start: (confirmRestart: boolean) => Promise<WangStartResult>,
  confirmRestart: boolean,
  initial: WangStartResult,
) {
  let result = initial
  const deadline = Date.now() + 30_000
  while (result.maintenanceRequestId) {
    if (Date.now() >= deadline) throw new Error('等待旺商聊固定登录分区维护完成超时，请重新检查')
    await new Promise(resolve => window.setTimeout(resolve, 500))
    const maintenance = await invoke<MaintenanceResult | null>('get_wang_maintenance_result', { requestId: result.maintenanceRequestId })
    if (!maintenance) continue
    if (!maintenance.success) throw new Error(maintenance.message || maintenance.errorCode)
    result = await start(confirmRestart)
  }
  return result
}

export default function RuntimeControls({ diagnostic, loading, refresh, setError }: RuntimeControlProps) {
  const [starting, setStarting] = useState(false)
  const [candidates, setCandidates] = useState<{ path: string; source: string }[]>([])
  const [wangPath, setWangPath] = useState(() => window.localStorage.getItem('dh.wangshangliao.path') || '')
  const [autoStart, setAutoStart] = useState(true)
  const [profileStatus, setProfileStatus] = useState<MaintenanceStatus | null>(null)

  useEffect(() => {
    void invoke<{ path: string; autoStart: boolean }>('get_wang_startup_settings').then(settings => {
      setAutoStart(settings.autoStart)
      if (settings.path) selectPath(settings.path)
      void checkProfileStatus(settings.path)
    }).catch(() => undefined)
    void invoke<{ path: string; source: string }[]>('locate_wangshangliao').then(found => {
      setCandidates(found)
      setWangPath(current => {
        if (current || found.length !== 1) return current
        window.localStorage.setItem('dh.wangshangliao.path', found[0].path)
        return found[0].path
      })
    }).catch(() => undefined)
  }, [])

  const selectPath = (path: string) => {
    setWangPath(path)
    if (path.trim()) window.localStorage.setItem('dh.wangshangliao.path', path.trim())
    else window.localStorage.removeItem('dh.wangshangliao.path')
  }

  const checkProfileStatus = async (path = wangPath) => {
    const status = await invoke<MaintenanceStatus>('get_wang_profile_status', { path: path.trim() || null })
    setProfileStatus(status)
    return status
  }

  const waitForMaintenance = async (requestId: string) => {
    const deadline = Date.now() + 30_000
    while (Date.now() < deadline) {
      await new Promise(resolve => window.setTimeout(resolve, 500))
      const result = await invoke<MaintenanceResult | null>('get_wang_maintenance_result', { requestId })
      if (!result) continue
      if (!result.success) throw new Error(result.message || result.errorCode)
      return
    }
    throw new Error('等待旺商聊账号登录复用维护完成超时，请重新检查')
  }

  const enableLoginReuse = async () => {
    setStarting(true)
    setError('')
    try {
      await invoke('save_wang_startup_settings', { settings: { path: wangPath.trim(), autoStart } })
      let status = await invoke<MaintenanceStatus>('apply_wang_profile_patch', { path: wangPath.trim() || null })
      if (status.requestId) {
        await waitForMaintenance(status.requestId)
        status = await checkProfileStatus()
      } else {
        setProfileStatus(status)
      }
      setError(status.state === 'patched'
        ? '账号记录复用已启用。请用密码登录一次并勾选“记住密码”；重启后会自动填入，旺商聊要求时仍需点击“安全登录”完成验证。'
        : status.detail)
    } catch (reason) {
      setError(String(reason))
    } finally {
      setStarting(false)
    }
  }

  const startWangShangLiao = async () => {
    setStarting(true)
    setError('')
    try {
      await invoke('save_wang_startup_settings', { settings: { path: wangPath.trim(), autoStart } })
      const devtoolsUrl = diagnostic?.devtoolsUrl || 'http://127.0.0.1:9222'
      const start = (confirmRestart: boolean) => invoke<WangStartResult>('start_wangshangliao', {
        path: wangPath.trim() || null,
        devtoolsUrl,
        confirmRestart,
      })
      let result = await finishMaintenance(start, false, await start(false))
      if (result.needsConfirmation && window.confirm('检测到旺商聊已运行，但没有开启 9222 DevTools。需要结束该旺商聊进程并重新启动，是否继续？')) {
        result = await finishMaintenance(start, true, await start(true))
      }
      if (result.detail) setError(result.detail)
      await checkProfileStatus()
      await refresh()
    } catch (reason) {
      setError(String(reason))
    } finally {
      setStarting(false)
    }
  }

  const saveStartupSettings = async () => {
    setError('')
    try {
      await invoke('save_wang_startup_settings', { settings: { path: wangPath.trim(), autoStart } })
      setError('旺商聊启动设置已保存。')
    } catch (reason) {
      setError(String(reason))
    }
  }

  return <div>
    <span className="eyebrow">旺商聊会话</span><h2>连接诊断</h2>
    <p className="muted">{diagnostic?.detail || '尚未执行检查。'}</p>
    <label>旺商聊程序路径<input value={wangPath} onChange={event => selectPath(event.target.value)} placeholder="自动查找，或粘贴旺商聊 EXE 路径" /></label>
    {candidates.length > 0 && <label>已找到的安装位置<select value={wangPath} onChange={event => selectPath(event.target.value)}><option value="">请选择</option>{candidates.map(candidate => <option value={candidate.path} key={candidate.path}>{candidate.source} · {candidate.path}</option>)}</select></label>}
    {candidates.length > 1 && !wangPath && <p className="field-hint warning">检测到多个安装位置，请选择后再启动，DH BOT 不会自行猜测。</p>}
    <label className="check-row"><input type="checkbox" checked={autoStart} onChange={event => setAutoStart(event.target.checked)} />打开 DH BOT 时自动启动或显示旺商聊</label>
    <div className="setting-note login-reuse-status">
      <strong>账号登录复用：{profileStatus?.state === 'patched' ? '已启用' : profileStatus?.state === 'elevation-requested' ? '等待管理员确认' : '等待启用'}</strong>
      <span>{profileStatus?.detail || '固定使用旺商聊自己的 dh-primary 登录分区；密码登录并勾选“记住密码”后，后续启动会自动填入账号密码。安全验证仍由旺商聊完成。'}</span>
      <div className="button-row"><button className="secondary" onClick={() => void enableLoginReuse()} disabled={loading || starting}>启用账号复用</button><button className="secondary" onClick={() => void checkProfileStatus().catch(reason => setError(String(reason)))} disabled={loading || starting}><RefreshCw size={15} />检查复用状态</button></div>
    </div>
    <dl><dt>DevTools</dt><dd>{diagnostic?.devtoolsUrl || 'http://127.0.0.1:9222'}</dd><dt>NIM 账号</dt><dd>{diagnostic?.nimAccount || '未识别'}</dd></dl>
    <div className="button-row"><button className="primary" onClick={() => void startWangShangLiao()} disabled={loading || starting}>启动 / 显示旺商聊</button><button className="secondary" onClick={() => void saveStartupSettings()} disabled={loading || starting}>保存启动设置</button><button className="secondary" onClick={() => void refresh()} disabled={loading || starting}><RefreshCw size={15} className={loading ? 'spin' : ''} />重新检查</button></div>
  </div>
}
