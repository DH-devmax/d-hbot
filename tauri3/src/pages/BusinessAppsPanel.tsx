import { useEffect, useState } from 'react'
import { Activity, AppWindow, Check, CircleAlert, RefreshCw, Send, ToggleLeft } from 'lucide-react'
import { api, readableError } from '../api/client'
import type { BusinessAppHealth, BusinessAppRecord, BusinessAppRun } from '../types'
import { PageFeedback } from '../components/PageState'

const statusText: Record<string, string> = { unchecked: '未检查', ready: '数据可用', stale: '只有过期数据', unavailable: '数据源异常', succeeded: '已完成', fallback: '已用模板回复', needs_input: '等待彩种名称', failed: '失败' }

export default function BusinessAppsPanel({ accountId, onError }: { accountId: string; onError: (value: string) => void }) {
  const [apps, setApps] = useState<BusinessAppRecord[]>([])
  const [health, setHealth] = useState<BusinessAppHealth | null>(null)
  const [runs, setRuns] = useState<BusinessAppRun[]>([])
  const [state, setState] = useState<'idle' | 'loading' | 'ready' | 'empty' | 'error'>('idle')
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState('@DH 预测 PC28')
  const [testReply, setTestReply] = useState('')

  const load = async () => {
    if (!accountId) { setApps([]); setState('empty'); return }
    setState('loading')
    try {
      const result = await api.listBusinessApps(accountId)
      setApps(result)
      const prediction = result.find(app => app.appId === 'prediction')
      if (prediction) setRuns(await api.listBusinessAppRuns(accountId, prediction.appId, 30))
      setState(result.length ? 'ready' : 'empty')
    } catch (reason) {
      setState('error')
      onError(readableError(reason))
    }
  }
  useEffect(() => { void load() }, [accountId])

  const prediction = apps.find(app => app.appId === 'prediction')
  const toggle = async () => {
    if (!prediction) return
    setBusy(true)
    try { await api.setBusinessAppEnabled(accountId, prediction.appId, !prediction.enabled); await load() } catch (reason) { onError(readableError(reason)) } finally { setBusy(false) }
  }
  const inspect = async () => {
    setBusy(true)
    try { setHealth(await api.getBusinessAppHealth(accountId, 'prediction')); await load() } catch (reason) { onError(readableError(reason)) } finally { setBusy(false) }
  }
  const test = async () => {
    setBusy(true); setTestReply('')
    try { const result = await api.testBusinessApp(accountId, 'prediction', message); setTestReply(`${result.reply}\n\n状态：${statusText[result.status] || result.status} · 数据：${statusText[result.freshness] || result.freshness}\nAI 润色：${result.aiUsed ? '是' : '否'} · 耗时：${result.elapsedMs}ms${result.error ? `\n说明：${result.error}` : ''}`) } catch (reason) { onError(readableError(reason)) } finally { setBusy(false) }
  }

  return <div className="business-apps"><div className="business-app-toolbar"><div><span className="eyebrow">业务应用</span><h2>内置应用</h2><p className="muted">应用能力随 DH BOT 发布，前台只控制启停和测试，不维护上游协议。</p></div><button className="secondary" data-help="重新读取业务应用状态和最近运行记录。" onClick={() => void load()} disabled={busy}><RefreshCw size={15} className={busy ? 'spin' : ''} />刷新</button></div><PageFeedback state={state} error="业务应用读取失败" retry={() => void load()} emptyTitle="暂无业务应用" emptyDetail="连接账号后会显示 DH BOT 内置业务应用。"><>{prediction && <section className={`business-app-card ${prediction.enabled ? 'enabled' : ''}`}><div className="business-app-title"><div className="app-icon"><AppWindow size={22} /></div><div><span className="eyebrow">{prediction.appId} · v{prediction.version}</span><h3>{prediction.name}</h3><p>{prediction.description}</p></div><span className={`app-status ${prediction.enabled ? 'active' : ''}`}><i />{prediction.enabled ? '已启用' : '已停用'}</span></div><div className="business-app-details"><div><strong>触发方式</strong><span>明确 @DH 预测 彩种名称</span></div><div><strong>适用范围</strong><span>所有已启用管理且开启 AI 回复的群</span></div><div><strong>当前状态</strong><span>{statusText[prediction.status] || prediction.status} · {prediction.statusDetail}</span></div></div><div className="button-row"><button className={prediction.enabled ? 'secondary' : 'primary'} data-help={prediction.enabled ? '关闭后，群聊中的 @DH 预测将停止回复。' : '开启后，所有符合条件的管理群都可以使用 @DH 预测。'} onClick={() => void toggle()} disabled={busy}>{prediction.enabled ? <ToggleLeft size={15} /> : <Check size={15} />}{prediction.enabled ? '停用预测应用' : '启用预测应用'}</button><button className="secondary" data-help="逐个检查内置彩种的数据是否可用，不发送群消息。" onClick={() => void inspect()} disabled={busy}><Activity size={15} />检查数据源</button></div></section>} {health && <section className="health-panel"><div className="health-heading"><div><span className="eyebrow">最近检查 {new Date(health.checkedAt).toLocaleString()}</span><h3>{health.detail}</h3></div><span className={`app-status ${health.status === 'ready' ? 'active' : ''}`}><i />{statusText[health.status] || health.status}</span></div><div className="health-grid">{health.games.map(game => <div key={game.id}><span>{game.name}</span><strong className={`health-${game.status}`}>{game.status === 'ready' ? <Check size={14} /> : <CircleAlert size={14} />}{statusText[game.status] || game.status}</strong><small>{game.detail}</small></div>)}</div></section>}<section className="business-test"><div><span className="eyebrow">离群预览</span><h3>测试预测应用</h3><p className="muted">只在本地窗口显示，结果不会进入群聊、任务或审计。</p></div><div className="compose-row"><input value={message} onChange={event => setMessage(event.target.value)} placeholder="@DH 预测 PC28" data-help="输入明确 @DH 预测 和可选彩种名称。" /><button className="primary" data-help="读取数据并在本地显示应用格式回复。" onClick={() => void test()} disabled={busy || !message.trim()}><Send size={15} />本地测试</button></div>{testReply && <pre className="ai-result">{testReply}</pre>}</section><section className="business-runs"><div className="list-head"><strong>最近运行</strong><span className="muted">{runs.length} 条</span></div>{runs.length ? runs.map(run => <div className="business-run" key={run.id}><span>{new Date(run.createdAt).toLocaleString()}</span><strong>{statusText[run.status] || run.status}</strong><em>{run.aiUsed ? 'AI 润色' : '应用模板'}</em><small>{run.error || run.reply.slice(0, 80)}</small></div>) : <p className="muted">还没有群内运行记录。</p>}</section></></PageFeedback></div>
}
