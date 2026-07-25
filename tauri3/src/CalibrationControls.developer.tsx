import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { FileCheck2, Radio, Square } from 'lucide-react'
import { readableError } from './api/client'

type CalibrationStatus = {
  active: boolean
  finishing: boolean
  startedAt: string
  appFileVersion: string
  mainScriptSha256: string
  pageTitle: string
  pageUrl: string
  capabilities: string[]
  operationCount: number
  callbackCount: number
  writeOperationCount: number
  baselineCount: number
  restoredBaselineCount: number
  restorationVerified: boolean
  restorationError: string
}

type CaptureResult = { path: string; status: CalibrationStatus }

const capabilityOptions = [
  ['sendText', '发送消息'],
  ['recall', '撤回消息'],
  ['mute', '禁言 / 解禁'],
  ['rename', '修改群名片'],
  ['announcement', '群公告'],
  ['groupMute', '全群发言'],
  ['memberEvents', '成员事件'],
] as const

const emptyStatus: CalibrationStatus = {
  active: false,
  finishing: false,
  startedAt: '',
  appFileVersion: '',
  mainScriptSha256: '',
  pageTitle: '',
  pageUrl: '',
  capabilities: [],
  operationCount: 0,
  callbackCount: 0,
  writeOperationCount: 0,
  baselineCount: 0,
  restoredBaselineCount: 0,
  restorationVerified: false,
  restorationError: '',
}

export default function CalibrationControls({ onError }: { onError: (value: string) => void }) {
  const [status, setStatus] = useState<CalibrationStatus>(emptyStatus)
  const [selected, setSelected] = useState(() => capabilityOptions.map(([key]) => key as string))
  const [restorationNoted, setRestorationNoted] = useState(false)
  const [busy, setBusy] = useState(false)
  const [capturePath, setCapturePath] = useState('')

  const refresh = async () => {
    const next = await invoke<CalibrationStatus>('get_developer_calibration_status')
    setStatus(next)
  }

  useEffect(() => {
    const tauriRuntime = Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
    if (!tauriRuntime) return
    void refresh().catch(reason => onError(readableError(reason)))
  }, [])

  useEffect(() => {
    if (!status.active) return
    const timer = window.setInterval(() => void refresh().catch(() => undefined), 2_000)
    return () => window.clearInterval(timer)
  }, [status.active])

  const toggle = (key: string) => {
    setSelected(current => current.includes(key) ? current.filter(value => value !== key) : [...current, key])
  }

  const begin = async () => {
    setBusy(true)
    setCapturePath('')
    setRestorationNoted(false)
    onError('')
    try {
      setStatus(await invoke<CalibrationStatus>('begin_developer_calibration', { capabilities: selected }))
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setBusy(false)
    }
  }

  const finish = async () => {
    setBusy(true)
    onError('')
    try {
      const result = await invoke<CaptureResult>('finish_developer_calibration', { restored: restorationNoted })
      setCapturePath(result.path)
      setStatus(emptyStatus)
      setRestorationNoted(false)
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setBusy(false)
    }
  }

  const cancel = async () => {
    setBusy(true)
    onError('')
    try {
      setStatus(await invoke<CalibrationStatus>('cancel_developer_calibration'))
      setRestorationNoted(false)
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setBusy(false)
    }
  }

  return <section className="section calibration-control">
    <div className="section-head"><div><span className="eyebrow">Contract v2</span><h2>真实 9222 能力校准</h2></div>{status.active ? <Radio size={18} className="calibration-live" /> : <Square size={18} />}</div>
    <dl className="runtime-details debug-details"><dt>采集状态</dt><dd>{status.finishing ? '正在最终回读并导出' : status.active ? '正在采集' : '未开始'}</dd><dt>旺商聊版本</dt><dd>{status.appFileVersion || '-'}</dd><dt>主脚本 SHA-256</dt><dd>{status.mainScriptSha256 || '-'}</dd><dt>请求 / 回调</dt><dd>{status.active ? `${status.operationCount} / ${status.callbackCount}` : '-'}</dd><dt>真实写操作</dt><dd>{status.active ? status.writeOperationCount : '-'}</dd><dt>恢复基线</dt><dd>{status.active ? `${status.restoredBaselineCount} / ${status.baselineCount}` : '-'}</dd><dt>恢复回读</dt><dd>{status.active ? (status.restorationVerified ? '已验证' : status.restorationError || '未完成') : '-'}</dd></dl>
    {!status.active && <div className="calibration-capabilities">{capabilityOptions.map(([key, label]) => <label className="check-row" key={key}><input type="checkbox" checked={selected.includes(key)} onChange={() => toggle(key)} />{label}</label>)}</div>}
    {status.active && <label className="check-row calibration-restored"><input type="checkbox" checked={restorationNoted} onChange={event => setRestorationNoted(event.target.checked)} />人工备注：已检查恢复结果（不替代上方真实回读验证）</label>}
    <div className="button-row">{status.active ? <><button className="primary" disabled={busy} onClick={() => void finish()}><FileCheck2 size={15} />完成并导出原始 Contract v2</button><button className="secondary" disabled={busy} onClick={() => void cancel()}>取消采集</button></> : <button className="primary" disabled={busy || selected.length === 0} onClick={() => void begin()}><Radio size={15} />开始真实 9222 采集</button>}</div>
    {capturePath && <dl className="runtime-details debug-details calibration-output"><dt>原始文件</dt><dd>{capturePath}</dd></dl>}
  </section>
}
