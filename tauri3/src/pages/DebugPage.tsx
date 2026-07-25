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
  requestId?: string | null
  detail: string
}

type MaintenanceResult = { requestId: string; operation: string; success: boolean; errorCode: string; message: string; completedAt: string }

type CapabilityStatus = 'supported' | 'unverified' | 'unsupported'
type GatewayCapabilities = {
  announcement: CapabilityStatus
  sendText: CapabilityStatus
  mute: CapabilityStatus
  recall: CapabilityStatus
  rename: CapabilityStatus
  removeMember: CapabilityStatus
  groupMute: CapabilityStatus
  memberEvents: CapabilityStatus
}

const capabilityLabels: Record<keyof GatewayCapabilities, string> = {
  announcement: '群公告',
  sendText: '发送消息',
  mute: '禁言 / 解禁',
  recall: '撤回消息',
  rename: '修改群名片',
  removeMember: '移出成员',
  groupMute: '全群发言',
  memberEvents: '成员事件',
}

const statusLabels: Record<CapabilityStatus, string> = {
  supported: '已校准',
  unverified: '待校准',
  unsupported: '不支持',
}

export default function DebugPage({ diagnostic, database, refresh, onError }: { diagnostic: Diagnostic | null; database: DatabaseStatus | null; refresh: () => Promise<void>; onError: (value: string) => void }) {
  const [maintenance, setMaintenance] = useState<MaintenanceStatus | null>(null)
  const [capabilities, setCapabilities] = useState<GatewayCapabilities | null>(null)
  const [busy, setBusy] = useState(false)
  const wangPath = async () => {
    const settings = await invoke<{ path: string }>('get_wang_startup_settings')
    return settings.path.trim() || null
  }
  const check = async () => {
    setBusy(true)
    onError('')
    try {
      const tauriRuntime = typeof window !== 'undefined' && Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
      if (tauriRuntime) {
        const path = await wangPath()
        const [maintenanceStatus, gatewayCapabilities] = await Promise.all([
          invoke<MaintenanceStatus>('get_wang_profile_status', { path }),
          invoke<GatewayCapabilities>('get_gateway_capabilities'),
        ])
        setMaintenance(maintenanceStatus)
        setCapabilities(gatewayCapabilities)
      } else {
        setMaintenance({ state: '浏览器预览', scriptHash: '', backupPath: null, requiresElevation: false, requestId: null, detail: '桌面程序会在这里显示真实旺商聊固定登录分区状态。' })
        setCapabilities(null)
      }
      await refresh()
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setBusy(false)
    }
  }
  const maintain = async (operation: 'apply' | 'restore') => {
    setBusy(true)
    onError('')
    try {
      const path = await wangPath()
      let status = await invoke<MaintenanceStatus>(operation === 'apply' ? 'apply_wang_profile_patch' : 'restore_wang_profile_patch', { path })
      setMaintenance(status)
      if (status.requestId) {
        let completed: MaintenanceResult | null = null
        for (let attempt = 0; attempt < 40 && !completed; attempt += 1) {
          await new Promise(resolve => window.setTimeout(resolve, 500))
          completed = await invoke<MaintenanceResult | null>('get_wang_maintenance_result', { requestId: status.requestId })
        }
        if (!completed) throw new Error('管理员维护仍未返回结果，请稍后点“立即检查”')
        if (!completed.success) throw new Error(`管理员维护失败：${completed.message}`)
        status = await invoke<MaintenanceStatus>('get_wang_profile_status', { path })
        setMaintenance(status)
      }
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
      <div className="capability-grid">{capabilities ? Object.entries(capabilities).map(([key, value]) => <div key={key}><span>{capabilityLabels[key as keyof GatewayCapabilities]}</span><strong className={`capability-${value}`}>{statusLabels[value]}</strong></div>) : <p className="muted">连接桌面程序后显示协议能力校准状态。</p>}</div>
    </section>
    <section className="section">
      <div className="section-head"><div><span className="eyebrow">固定登录分区</span><h2>旺商聊脚本维护</h2></div><Wrench size={18} /></div>
      <dl className="runtime-details debug-details"><dt>维护状态</dt><dd>{maintenance?.state || '等待检查'}</dd><dt>脚本哈希</dt><dd>{maintenance?.scriptHash || '-'}</dd><dt>备份位置</dt><dd>{maintenance?.backupPath || '-'}</dd><dt>管理员权限</dt><dd>{maintenance?.requiresElevation ? '应用补丁时会弹出 UAC' : '当前操作不需要提升'}</dd><dt>说明</dt><dd>{maintenance?.detail || '尚未读取'}</dd></dl>
      <div className="button-row"><button className="primary" disabled={busy} onClick={() => void maintain('apply')}>应用固定登录分区</button><button className="secondary" disabled={busy || !maintenance?.backupPath} onClick={() => void maintain('restore')}>恢复旺商聊原文件</button></div>
    </section>
    <section className="section"><span className="eyebrow">本地数据</span><h2>数据库状态</h2><dl className="runtime-details debug-details"><dt>路径</dt><dd>{database?.path || '-'}</dd><dt>Schema</dt><dd>{database?.schemaVersion ?? '-'}</dd><dt>完整性</dt><dd>{database?.integrity || '-'}</dd><dt>群 / 消息</dt><dd>{database ? `${database.groups} / ${database.messages}` : '-'}</dd></dl></section>
  </div>
}
