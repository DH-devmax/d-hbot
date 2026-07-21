import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { RefreshCw } from 'lucide-react'
import type { RuntimeControlProps } from './runtimeTypes'

export default function RuntimeControls({ diagnostic, loading, refresh, setError }: RuntimeControlProps) {
  const [starting, setStarting] = useState(false)

  const startWangShangLiao = async () => {
    setStarting(true)
    setError('')
    try {
      const devtoolsUrl = diagnostic?.devtoolsUrl || 'http://127.0.0.1:9222'
      let result = await invoke<{ needsConfirmation: boolean; detail: string }>('start_wangshangliao', {
        path: null,
        devtoolsUrl,
        confirmRestart: false,
      })
      if (result.needsConfirmation && window.confirm('检测到旺商聊已运行，但没有开启 9222 DevTools。需要结束该旺商聊进程并重新启动，是否继续？')) {
        result = await invoke('start_wangshangliao', {
          path: null,
          devtoolsUrl,
          confirmRestart: true,
        })
      }
      if (result.detail) setError(result.detail)
      await refresh()
    } catch (reason) {
      setError(String(reason))
    } finally {
      setStarting(false)
    }
  }

  return <div>
    <span className="eyebrow">旺商聊会话</span><h2>连接诊断</h2>
    <p className="muted">{diagnostic?.detail || '尚未执行检查。'}</p>
    <dl><dt>DevTools</dt><dd>{diagnostic?.devtoolsUrl || 'http://127.0.0.1:9222'}</dd><dt>NIM 账号</dt><dd>{diagnostic?.nimAccount || '未识别'}</dd></dl>
    <div className="button-row"><button className="primary" onClick={() => void startWangShangLiao()} disabled={loading || starting}>启动 / 显示旺商聊</button><button className="secondary" onClick={() => void refresh()} disabled={loading || starting}><RefreshCw size={15} className={loading ? 'spin' : ''} />重新检查</button></div>
  </div>
}
