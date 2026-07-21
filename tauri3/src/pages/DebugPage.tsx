import { useEffect, useState } from 'react'
import { RefreshCw, Wrench } from 'lucide-react'
import { invoke } from '@tauri-apps/api/core'
import type { Diagnostic } from '../runtimeTypes'
import type { DatabaseStatus } from '../types'
import { readableError } from '../api/client'

type MaintenanceStatus = {
  state: string
  scriptHash: string
  backupPath?: string | null
  requiresElevation: boolean
  detail: string
}

export default function DebugPage({ diagnostic, database, refresh, onError }: { diagnostic: Diagnostic | null; database: DatabaseStatus | null; refresh: () => Promise<void>; onError: (value: string) => void }) {
  const [maintenance, setMaintenance] = useState<MaintenanceStatus | null>(null)
  const [busy, setBusy] = useState(false)
  const check = async () => {
    setBusy(true)
    onError('')
    try {
      const tauriRuntime = typeof window !== 'undefined' && Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
      setMaintenance(tauriRuntime
        ? await invoke<MaintenanceStatus>('get_wang_profile_status', { path: null })
        : { state: '浏览器预览', scriptHash: '', backupPath: null, requiresElevation: false, detail: '桌面程序会在这里显示真实旺商聊固定登录分区状态。' })
      await refresh()
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setBusy(false)
    }
  }
  useEffect(() => { void check() }, [])
  return <div className="page-stack">
    <section className="section">
      <div className="section-head"><div><span className="eyebrow">连接诊断</span><h2>旺商聊协议状态</h2></div><button className="primary" onClick={() => void check()} disabled={busy}><RefreshCw size={15} className={busy ? 'spin' : ''} />立即检查</button></div>
      <dl className="runtime-details debug-details"><dt>DevTools</dt><dd>{diagnostic?.devtoolsUrl || 'http://127.0.0.1:9222'}</dd><dt>连接状态</dt><dd>{diagnostic?.status || '等待检查'}</dd><dt>当前页面</dt><dd>{diagnostic?.pageTitle || '-'}</dd><dt>页面地址</dt><dd>{diagnostic?.pageUrl || '-'}</dd><dt>NIM 账号</dt><dd>{diagnostic?.nimAccount || '未识别'}</dd><dt>诊断结论</dt><dd>{diagnostic?.detail || '尚未读取'}</dd></dl>
    </section>
    <section className="section">
      <div className="section-head"><div><span className="eyebrow">固定登录分区</span><h2>旺商聊脚本维护</h2></div><Wrench size={18} /></div>
      <dl className="runtime-details debug-details"><dt>维护状态</dt><dd>{maintenance?.state || '等待检查'}</dd><dt>脚本哈希</dt><dd>{maintenance?.scriptHash || '-'}</dd><dt>备份位置</dt><dd>{maintenance?.backupPath || '-'}</dd><dt>管理员权限</dt><dd>{maintenance?.requiresElevation ? '应用补丁时会弹出 UAC' : '当前操作不需要提升'}</dd><dt>说明</dt><dd>{maintenance?.detail || '尚未读取'}</dd></dl>
    </section>
    <section className="section"><span className="eyebrow">本地数据</span><h2>数据库状态</h2><dl className="runtime-details debug-details"><dt>路径</dt><dd>{database?.path || '-'}</dd><dt>Schema</dt><dd>{database?.schemaVersion ?? '-'}</dd><dt>完整性</dt><dd>{database?.integrity || '-'}</dd><dt>群 / 消息</dt><dd>{database ? `${database.groups} / ${database.messages}` : '-'}</dd></dl></section>
  </div>
}
