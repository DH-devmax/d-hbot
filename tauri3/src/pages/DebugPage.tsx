import { useEffect, useState } from 'react'
import { Clipboard, FileArchive, RefreshCw, Wrench } from 'lucide-react'
import { invoke } from '@tauri-apps/api/core'
import type { Diagnostic } from '../runtimeTypes'
import type { DatabaseStatus, SupportBundleResult } from '../types'
import { api, readableError } from '../api/client'
import CalibrationControls from '@calibration-controls'

type MaintenanceStatus = {
  state: string
  scriptHash: string
  backupPath?: string | null
  requiresElevation: boolean
  requestId?: string | null
  detail: string
}

type MaintenanceResult = { requestId: string; operation: string; success: boolean; errorCode: string; message: string; completedAt: string }

type CapabilityStatus = 'supported' | 'manualVerification' | 'unavailable' | 'unsupported' | 'unverified'
type GatewayCapability = CapabilityStatus | { status: CapabilityStatus; source: 'zcgContract' | 'wangElectron' | 'nimRuntime' | 'manualReceipt'; manualAllowed: boolean; automaticAllowed: boolean; reason: string; checkedAt: string; fingerprint: string }
type GatewayCapabilities = {
  announcement: GatewayCapability
  sendText: GatewayCapability
  mute: GatewayCapability
  recall: GatewayCapability
  rename: GatewayCapability
  removeMember: GatewayCapability
  groupMute: GatewayCapability
  memberEvents: GatewayCapability
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
  supported: '已检测可用',
  manualVerification: '待首次手工验证',
  unavailable: '当前不可用',
  unverified: '待首次手工验证',
  unsupported: '不支持',
}
const capabilityStatus = (value: GatewayCapability) => typeof value === 'string' ? value : value.status
const capabilityDetail = (value: GatewayCapability) => typeof value === 'string' ? '' : value.reason
const isTemporaryCapabilityFailure = (value: GatewayCapability) => /超时|排队|传输|网络|连接|尚未就绪|NIM|等待登录|快照同步/i.test(capabilityDetail(value))
const capabilityDisplay = (value: GatewayCapability, key: keyof GatewayCapabilities) => {
  const status = capabilityStatus(value)
  if (key === 'memberEvents' && status === 'unavailable') return '快照同步 · 运行中'
  if (status === 'unavailable' && isTemporaryCapabilityFailure(value)) return '连接暂时不可用 · 等待恢复'
  if (status === 'unavailable') return '协议结构已变化 · 暂停自动操作'
  if (status !== 'supported' || typeof value === 'string') return statusLabels[status]
  if (value.source === 'zcgContract') return 'ZCG 契约 · 已检测'
  if (value.source === 'nimRuntime') return 'NIM 监听 · 已启用'
  if (value.source === 'manualReceipt') return '旺商聊协议 · 已验证'
  return '旺商聊协议 · 已检测'
}

export default function DebugPage({ diagnostic, database, refresh, onError }: { diagnostic: Diagnostic | null; database: DatabaseStatus | null; refresh: () => Promise<void>; onError: (value: string) => void }) {
  const [maintenance, setMaintenance] = useState<MaintenanceStatus | null>(null)
  const [capabilities, setCapabilities] = useState<GatewayCapabilities | null>(null)
  const [busy, setBusy] = useState(false)
  const [supportBusy, setSupportBusy] = useState(false)
  const [supportBundle, setSupportBundle] = useState<SupportBundleResult | null>(null)
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
  const exportSupportBundle = async () => {
    setSupportBusy(true)
    onError('')
    try {
      setSupportBundle(await api.exportSupportBundle())
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setSupportBusy(false)
    }
  }
  const copySupportPath = async () => {
    if (!supportBundle?.path) return
    try {
      await navigator.clipboard.writeText(supportBundle.path)
    } catch {
      onError('诊断包路径复制失败，请在下方手动选中路径')
    }
  }
  useEffect(() => { void check() }, [])
  return <div className="page-stack">
    <section className="section">
      <div className="section-head"><div><span className="eyebrow">连接诊断</span><h2>旺商聊协议状态</h2></div><button className="primary" onClick={() => void check()} disabled={busy}><RefreshCw size={15} className={busy ? 'spin' : ''} />立即检查</button></div>
      <dl className="runtime-details debug-details"><dt>DevTools</dt><dd>{diagnostic?.devtoolsUrl || 'http://127.0.0.1:9222'}</dd><dt>连接状态</dt><dd>{diagnostic?.status || '等待检查'}</dd><dt>当前页面</dt><dd>{diagnostic?.pageTitle || '-'}</dd><dt>页面地址</dt><dd>{diagnostic?.pageUrl || '-'}</dd><dt>NIM 账号</dt><dd>{diagnostic?.nimAccount || '未识别'}</dd><dt>诊断结论</dt><dd>{diagnostic?.detail || '尚未读取'}</dd></dl>
      <div className="capability-grid">{capabilities ? Object.entries(capabilities).map(([key, value]) => { const typedKey = key as keyof GatewayCapabilities; const status = capabilityStatus(value); return <div key={key} title={capabilityDetail(value)}><span>{capabilityLabels[typedKey]}</span><strong className={`capability-${status}`}>{capabilityDisplay(value, typedKey)}</strong></div> }) : <p className="muted">连接桌面程序后显示协议能力探测状态。</p>}</div>
    </section>
    <CalibrationControls onError={onError} />
    <section className="section">
      <div className="section-head"><div><span className="eyebrow">固定登录分区</span><h2>旺商聊脚本维护</h2></div><Wrench size={18} /></div>
      <dl className="runtime-details debug-details"><dt>维护状态</dt><dd>{maintenance?.state || '等待检查'}</dd><dt>脚本哈希</dt><dd>{maintenance?.scriptHash || '-'}</dd><dt>备份位置</dt><dd>{maintenance?.backupPath || '-'}</dd><dt>管理员权限</dt><dd>{maintenance?.requiresElevation ? '应用补丁时会弹出 UAC' : '当前操作不需要提升'}</dd><dt>说明</dt><dd>{maintenance?.detail || '尚未读取'}</dd></dl>
      <div className="button-row"><button className="primary" disabled={busy} onClick={() => void maintain('apply')}>应用固定登录分区</button><button className="secondary" disabled={busy || !maintenance?.backupPath} onClick={() => void maintain('restore')}>恢复旺商聊原文件</button></div>
    </section>
    <section className="section"><span className="eyebrow">本地数据</span><h2>数据库状态</h2><dl className="runtime-details debug-details"><dt>路径</dt><dd>{database?.path || '-'}</dd><dt>Schema</dt><dd>{database?.schemaVersion ?? '-'}</dd><dt>完整性</dt><dd>{database?.integrity || '-'}</dd><dt>群 / 消息</dt><dd>{database ? `${database.groups} / ${database.messages}` : '-'}</dd></dl></section>
    <section className="section support-bundle-section"><div className="section-head"><div><span className="eyebrow">协助排查</span><h2>生成诊断包</h2></div><FileArchive size={18} /></div><p className="muted">生成 ZIP 后可直接发给维护者。内含脱敏连接状态、协议能力、审计摘要和最近日志；不会包含数据库、旺商聊登录信息、Cookie、密钥或原始群消息。</p><div className="button-row"><button className="primary" data-help="生成一个可分享的本地诊断 ZIP，用于排查连接、协议、规则或任务异常。" disabled={supportBusy} onClick={() => void exportSupportBundle()}><FileArchive size={15} className={supportBusy ? 'spin' : ''} />{supportBusy ? '正在生成' : '生成诊断包'}</button>{supportBundle && <button className="secondary" data-help="把诊断包的本地路径复制到剪贴板，方便在资源管理器中找到后发送。" onClick={() => void copySupportPath()}><Clipboard size={15} />复制路径</button>}</div>{supportBundle && <div className="support-bundle-result" role="status"><strong>诊断包已生成</strong><span>{supportBundle.path}</span><small>SHA-256：{supportBundle.sha256} · 共 {supportBundle.includedFiles} 个文件</small></div>}</section>
  </div>
}
