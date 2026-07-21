import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { RefreshCw } from 'lucide-react'
import type { RuntimeControlProps } from './runtimeTypes'

type RuntimeMode = { mode: 'real' | 'fixture'; dataDir: string; restartRequired: boolean }

export default function RuntimeControls({ diagnostic, loading, refresh, setError }: RuntimeControlProps) {
  const [runtimeMode, setRuntimeMode] = useState<RuntimeMode>({ mode: 'real', dataDir: '', restartRequired: false })
  const [starting, setStarting] = useState(false)

  useEffect(() => {
    void invoke<RuntimeMode>('get_runtime_mode').then(setRuntimeMode).catch(reason => setError(String(reason)))
  }, [])

  const changeRuntimeMode = async (mode: 'real' | 'fixture') => {
    if (mode === runtimeMode.mode) return
    setError('')
    try {
      const result = await invoke<{ mode: 'real' | 'fixture'; restartRequired: boolean }>('set_runtime_mode', { mode })
      setRuntimeMode({ ...runtimeMode, ...result })
      setError(mode === 'fixture' ? '开发测试环境已保存，重新启动 DH BOT Dev 后连接 9233。' : '真实旺商聊环境已保存，重新启动 DH BOT Dev 后连接 9222。')
    } catch (reason) {
      setError(String(reason))
    }
  }

  const startRuntime = async () => {
    setStarting(true)
    setError('')
    try {
      if (runtimeMode.mode === 'fixture') {
        const path = await invoke<string>('start_fixture_host')
        setError(`DH Fixture 已启动：${path}`)
        window.setTimeout(() => void refresh(), 800)
      } else {
        const devtoolsUrl = diagnostic?.devtoolsUrl || 'http://127.0.0.1:9222'
        let result = await invoke<{ needsConfirmation: boolean; detail: string }>('start_wangshangliao', { path: null, devtoolsUrl, confirmRestart: false })
        if (result.needsConfirmation && window.confirm('检测到旺商聊已运行但未开启 DevTools。确认重启旺商聊？')) {
          result = await invoke('start_wangshangliao', { path: null, devtoolsUrl, confirmRestart: true })
        }
        if (result.detail) setError(result.detail)
        await refresh()
      }
    } catch (reason) {
      setError(String(reason))
    } finally {
      setStarting(false)
    }
  }

  return <div>
    <div className="developer-badge">开发测试环境</div>
    <span className="eyebrow">协议会话</span><h2>连接诊断</h2>
    <p className="muted">{diagnostic?.detail || '尚未执行检查。'}</p>
    <dl><dt>DevTools</dt><dd>{diagnostic?.devtoolsUrl || (runtimeMode.mode === 'fixture' ? 'http://127.0.0.1:9233' : 'http://127.0.0.1:9222')}</dd><dt>NIM 账号</dt><dd>{diagnostic?.nimAccount || '未识别'}</dd></dl>
    <div className="button-row"><button className="primary" onClick={() => void startRuntime()} disabled={loading || starting}>{runtimeMode.mode === 'fixture' ? '启动 / 显示 DH Fixture' : '启动 / 显示旺商聊'}</button><button className="secondary" onClick={() => void refresh()} disabled={loading || starting}><RefreshCw size={15} className={loading ? 'spin' : ''} />重新检查</button></div>
    <div className="runtime-mode-control"><strong>运行环境</strong><span>开发版数据与生产版数据完全分开，切换后重启生效。</span><div role="tablist"><button className={runtimeMode.mode === 'real' ? 'active' : ''} onClick={() => void changeRuntimeMode('real')} disabled={loading || starting}>真实旺商聊 · 9222</button><button className={runtimeMode.mode === 'fixture' ? 'active' : ''} onClick={() => void changeRuntimeMode('fixture')} disabled={loading || starting}>DH Fixture · 9233</button></div><small>{runtimeMode.dataDir}</small></div>
  </div>
}
