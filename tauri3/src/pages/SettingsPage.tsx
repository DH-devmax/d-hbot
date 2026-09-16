import { useEffect, useState } from 'react'
import { invoke, isWebMode } from '../api/transport'
import RuntimeControls from '@runtime-controls'
import type { Diagnostic } from '../runtimeTypes'
import type { DatabaseStatus } from '../types'
import { readableError } from '../api/client'

export default function SettingsPage({ diagnostic, database, refresh, onError }: { diagnostic: Diagnostic | null; database: DatabaseStatus | null; refresh: () => Promise<void>; onError: (value: string) => void }) {
  const [busy, setBusy] = useState(false)
  const [closeBehavior, setCloseBehavior] = useState('ask')
  useEffect(() => { if (!isWebMode()) void invoke<string>('get_close_behavior').then(setCloseBehavior).catch(() => setCloseBehavior('ask')) }, [])
  const resetCloseBehavior = async () => { setBusy(true); try { await invoke('reset_close_behavior'); setCloseBehavior('ask') } catch (reason) { onError(readableError(reason)) } finally { setBusy(false) } }
  return <div className="page-stack">{isWebMode() && <section className="section"><h2>管理员账号</h2><p>修改账号密码后，需要重新登录管理台。</p><button className="secondary" onClick={() => window.dispatchEvent(new Event('dh-admin-settings'))}>修改管理员账号密码</button></section>}<section className="section"><RuntimeControls diagnostic={diagnostic} loading={busy} refresh={refresh} setError={onError} /></section><section className="section"><span className="eyebrow">运行信息</span><h2>数据库与协议</h2><dl className="runtime-details"><dt>数据库</dt><dd>{database?.path || '未打开'}</dd><dt>Schema</dt><dd>{database?.schemaVersion ?? '-'}</dd><dt>完整性</dt><dd>{database?.integrity || '-'}</dd><dt>当前页面</dt><dd>{diagnostic?.pageTitle || '-'}</dd><dt>页面地址</dt><dd>{diagnostic?.pageUrl || '-'}</dd><dt>NIM 账号</dt><dd>{diagnostic?.nimAccount || '未识别'}</dd>{!isWebMode() && <><dt>关闭按钮</dt><dd>{closeBehavior === 'ask' ? '每次询问' : closeBehavior === 'tray' ? '挂到托盘' : '直接退出'}</dd></>}</dl>{!isWebMode() && <button className="secondary" data-help="清除已记住的关闭行为，下次点击关闭时重新询问。" disabled={busy || closeBehavior === 'ask'} onClick={() => void resetCloseBehavior()}>重置关闭提示</button>}</section></div>
}
