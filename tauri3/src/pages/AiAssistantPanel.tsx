import { useEffect, useMemo, useState } from 'react'
import { Bot, Plus, RefreshCw, Save, Send, Trash2 } from 'lucide-react'
import { invoke } from '@tauri-apps/api/core'
import type { AiProviderEndpoint, AiSettings } from '../types'
import { api, readableError } from '../api/client'

type EndpointDraft = Pick<AiProviderEndpoint, 'id' | 'accountId' | 'name' | 'baseUrl' | 'webhookUrl' | 'apiBackend' | 'model' | 'reasoningEffort' | 'priority' | 'enabled' | 'apiKeyConfigured'>

function blankEndpoint(accountId: string, priority: number): EndpointDraft {
  return { id: 0, accountId, name: '备用连接', baseUrl: '', webhookUrl: '', apiBackend: 'chat_completions', model: 'deepseek-v4-pro', reasoningEffort: 'low', priority, enabled: true, apiKeyConfigured: false }
}

export default function AiAssistantPanel({ accountId = '', aiSettings, setAiSettings, refresh, onError }: { accountId?: string; aiSettings: AiSettings; setAiSettings: (value: AiSettings) => void; refresh: () => Promise<void>; onError: (value: string) => void }) {
  const [endpoints, setEndpoints] = useState<AiProviderEndpoint[]>([])
  const [draft, setDraft] = useState<EndpointDraft>(() => ({ ...blankEndpoint(accountId, 0), name: '主连接', baseUrl: aiSettings.base_url, webhookUrl: aiSettings.webhook_url, apiBackend: aiSettings.api_backend || 'chat_completions', model: aiSettings.model, apiKeyConfigured: aiSettings.api_key_configured }))
  const [apiKey, setApiKey] = useState('')
  const [question, setQuestion] = useState('')
  const [reply, setReply] = useState('')
  const [includeBuiltInKnowledge, setIncludeBuiltInKnowledge] = useState(false)
  const [busy, setBusy] = useState(false)

  const ordered = useMemo(() => [...endpoints].sort((a, b) => a.priority - b.priority || a.id - b.id), [endpoints])

  const loadEndpoints = async () => {
    if (!accountId) return
    try {
      const result = await api.listAiProviderEndpoints(accountId)
      setEndpoints(result)
      const selected = result.find(item => item.id === draft.id) || result[0]
      if (selected) setDraft(selected)
    } catch (reason) {
      onError(readableError(reason))
    }
  }

  useEffect(() => { void loadEndpoints() }, [accountId])

  const save = async () => {
    setBusy(true)
    onError('')
    try {
      if (accountId) {
        const id = await api.saveAiProviderEndpoint({ ...draft, accountId, apiKey: apiKey || null })
        setDraft(value => ({ ...value, id }))
        setApiKey('')
        await loadEndpoints()
      } else {
        await invoke('save_ai_settings', { baseUrl: draft.baseUrl, webhookUrl: draft.webhookUrl, apiBackend: draft.apiBackend, model: draft.model, apiKey: apiKey || null })
        setAiSettings({ base_url: draft.baseUrl, webhook_url: draft.webhookUrl, api_backend: draft.apiBackend, model: draft.model, api_key_configured: Boolean(apiKey) || aiSettings.api_key_configured })
        setApiKey('')
        await refresh()
      }
      onError('AI 连接已保存')
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setBusy(false)
    }
  }

  const remove = async () => {
    if (!accountId || !draft.id) return
    setBusy(true)
    try {
      await api.deleteAiProviderEndpoint(accountId, draft.id)
      await loadEndpoints()
      onError('AI 连接已删除')
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setBusy(false)
    }
  }

  const test = async () => {
    if (!question.trim()) return
    setBusy(true)
    setReply('')
    try {
      const result = accountId && draft.id
        ? await api.testAiProviderEndpoint(accountId, draft.id, question)
        : await invoke<{ decision: { reply: string; reason: string; confidence: number }; elapsedMs: number; model: string; knowledgeSource?: string }>('test_ai', { ...(accountId ? { accountId } : {}), message: question, recentContext: [], includeBuiltInKnowledge })
      setReply(`${result.decision.reply || 'AI 没有返回文字回复'}\n\n理由：${result.decision.reason || '未说明'}\n置信度：${Math.round(result.decision.confidence * 100)}%\n资料：${includeBuiltInKnowledge ? 'DH 默认群规与 FAQ' : '空上下文'}\n模型：${result.model} · ${result.elapsedMs}ms`)
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setBusy(false)
    }
  }

  return <div className="assistant-layout">
    <div className="assistant-intro"><div className="app-icon"><Bot size={22} /></div><div><span className="eyebrow">AI 助手</span><h2>DH BOT 内置业务人格</h2><p>语气自然，优先回答当前群已绑定资料；资料不足时提示管理员确认。普通聊天保持安静，明确 <strong>@DH</strong> 或 <strong>@当前登录账号</strong> 才会触发。</p></div></div>
    <div className="provider-workspace">
      <aside className="provider-list"><div className="list-head"><div><span className="eyebrow">主备连接</span><strong>{ordered.length || 1} 个连接</strong></div><button className="icon-button" title="刷新连接" data-help="重新读取连接、优先级和健康状态。" onClick={() => void loadEndpoints()}><RefreshCw size={15} /></button></div>{ordered.map(endpoint => <button key={endpoint.id} className={`provider-item ${draft.id === endpoint.id ? 'selected' : ''}`} onClick={() => { setDraft(endpoint); setApiKey('') }}><span><strong>{endpoint.name}</strong><small>优先级 {endpoint.priority + 1} · {endpoint.model}</small></span><em className={endpoint.enabled ? 'enabled' : ''}>{endpoint.enabled ? endpoint.healthStatus === 'healthy' ? '正常' : '启用' : '停用'}</em></button>)}<button className="secondary provider-add" data-help="新增一个备用 AI 服务。主连接失败、限流或返回服务错误时会按优先级切换一次。" onClick={() => { setDraft(blankEndpoint(accountId, ordered.length)); setApiKey('') }}><Plus size={15} />新增备用连接</button></aside>
      <section className="assistant-card provider-editor"><div className="editor-title"><div><span className="eyebrow">连接提供商</span><h3>{draft.id ? draft.name : '新连接'}</h3><p>按优先级顺序调用，不会并发消耗多个连接。</p></div>{draft.id > 0 && ordered.length > 1 && <button className="icon-button" title="删除连接" data-help="删除当前连接及其独立密钥；至少会保留一个连接。" onClick={() => void remove()} disabled={busy}><Trash2 size={15} /></button>}</div><div className="provider-fields"><label>连接名称<input value={draft.name} onChange={event => setDraft({ ...draft, name: event.target.value })} placeholder="主连接" /></label><label>优先级<input type="number" min={1} value={draft.priority + 1} onChange={event => setDraft({ ...draft, priority: Math.max(0, Number(event.target.value || 1) - 1) })} data-help="数字越小越优先。主连接冷却时才会切到下一个连接。" /></label><label>接口方式<select value={draft.apiBackend} onChange={event => setDraft({ ...draft, apiBackend: event.target.value as EndpointDraft['apiBackend'] })} data-help="Chat Completions 适用于大多数兼容服务；Responses 适用于提供 /v1/responses 的模型服务。"><option value="chat_completions">Chat Completions</option><option value="responses">Responses</option></select></label><label>模型<input value={draft.model} onChange={event => setDraft({ ...draft, model: event.target.value })} placeholder="deepseek-v4-pro" /></label><label>思考深度<select value={draft.reasoningEffort} onChange={event => setDraft({ ...draft, reasoningEffort: event.target.value as EndpointDraft['reasoningEffort'] })} data-help="仅 Responses 接口使用。xhigh 思考更深，但通常响应更慢。"><option value="low">低</option><option value="medium">中</option><option value="high">高</option><option value="xhigh">超高 (xhigh)</option></select></label><label className="wide">Base URL<input value={draft.baseUrl} onChange={event => setDraft({ ...draft, baseUrl: event.target.value })} placeholder="https://example.com/v1" data-help="OpenAI-compatible 服务地址，远程地址使用 HTTPS。Responses 会自动使用 /v1/responses。" /></label><label className="wide">Webhook URL<input value={draft.webhookUrl} onChange={event => setDraft({ ...draft, webhookUrl: event.target.value })} placeholder="可留空" data-help="使用 Webhook 时填写，和 Base URL 二选一。" /></label><label>API Key<input type="password" value={apiKey} onChange={event => setApiKey(event.target.value)} placeholder={draft.apiKeyConfigured ? '已保存，留空保持原值' : '尚未设置'} data-help="每个连接使用独立密钥引用，原文只进入系统保护存储。" /></label></div><div className="button-row"><label className="switch-field"><input type="checkbox" checked={draft.enabled} onChange={event => setDraft({ ...draft, enabled: event.target.checked })} /><span>启用该连接</span></label><button className="primary" data-help="保存连接。网络客户端会按配置指纹复用，不会每条消息重新握手。" onClick={() => void save()} disabled={busy}><Save size={15} />保存连接</button></div></section>
    </div>
    <section className="assistant-card"><span className="eyebrow">使用边界</span><h3>回复规则</h3><dl className="assistant-facts"><dt>触发方式</dt><dd>明确 @DH 或 @当前登录账号</dd><dt>资料范围</dt><dd>当前群已启用的知识库</dd><dt>主备切换</dt><dd>主连接异常时最多切换一个备用</dd><dt>资料不足</dt><dd>当前资料里没有说明，建议联系群管理员确认。</dd></dl></section>
    <section className="assistant-card assistant-test"><span className="eyebrow">离群测试</span><h3>测试所选 AI 连接</h3><p className="muted">此处只在本地窗口显示结果，不读取群消息，也不会发送回复或执行群管动作。</p><label className="switch-field"><input type="checkbox" checked={includeBuiltInKnowledge} onChange={event => setIncludeBuiltInKnowledge(event.target.checked)} /><span>加载默认群规与 FAQ，仅用于本地测试</span></label><textarea value={question} onChange={event => setQuestion(event.target.value)} placeholder="输入一条测试问题，例如：@DH 群规是什么？" data-help="输入任意本地测试问题，结果不会进入群聊。" /><button className="primary" data-help="只测试当前选中的连接，不触发群内回复。" onClick={() => void test()} disabled={busy || !question.trim()}><Send size={15} />测试 AI</button>{reply && <pre className="ai-result">{reply}</pre>}</section>
  </div>
}
