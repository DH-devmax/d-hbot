import { useState } from 'react'
import { Bot, Save, Send } from 'lucide-react'
import { invoke } from '@tauri-apps/api/core'
import type { AiSettings } from '../types'
import { readableError } from '../api/client'

export default function AiAssistantPanel({ aiSettings, setAiSettings, refresh, onError }: { aiSettings: AiSettings; setAiSettings: (value: AiSettings) => void; refresh: () => Promise<void>; onError: (value: string) => void }) {
  const [apiKey, setApiKey] = useState('')
  const [question, setQuestion] = useState('')
  const [reply, setReply] = useState('')
  const [includeBuiltInKnowledge, setIncludeBuiltInKnowledge] = useState(false)
  const [busy, setBusy] = useState(false)

  const save = async () => {
    setBusy(true)
    onError('')
    try {
      await invoke('save_ai_settings', { baseUrl: aiSettings.base_url, webhookUrl: aiSettings.webhook_url, model: aiSettings.model, apiKey: apiKey || null })
      setApiKey('')
      onError('AI 设置已保存')
      await refresh()
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
      const result = await invoke<{ decision: { reply: string; reason: string; confidence: number }; elapsedMs: number; model: string; knowledgeSource?: string }>('test_ai', { message: question, recentContext: [], includeBuiltInKnowledge })
      setReply(`${result.decision.reply || 'AI 没有返回文字回复'}\n\n理由：${result.decision.reason || '未说明'}\n置信度：${Math.round(result.decision.confidence * 100)}%\n资料：${result.knowledgeSource || (includeBuiltInKnowledge ? 'DH 默认群规与 FAQ' : '空上下文')}\n模型：${result.model} · ${result.elapsedMs}ms`)
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setBusy(false)
    }
  }

  return <div className="assistant-layout">
    <div className="assistant-intro"><div className="app-icon"><Bot size={22} /></div><div><span className="eyebrow">AI 助手</span><h2>DH BOT 内置业务人格</h2><p>语气自然，优先回答当前群已绑定资料；资料不足时提示管理员确认。普通聊天保持安静，只有明确 <strong>@DH</strong> 才会触发。</p></div></div>
    <div className="assistant-grid">
      <section className="assistant-card"><span className="eyebrow">连接提供商</span><h3>AI 连接设置</h3><label>Base URL<input value={aiSettings.base_url} onChange={event => setAiSettings({ ...aiSettings, base_url: event.target.value })} placeholder="https://example.com/v1" data-help="OpenAI-compatible 服务地址，远程地址使用 HTTPS。" /></label><label>Webhook URL<input value={aiSettings.webhook_url} onChange={event => setAiSettings({ ...aiSettings, webhook_url: event.target.value })} placeholder="可留空" data-help="使用 Webhook 时填写，和 Base URL 二选一。" /></label><label>模型<input value={aiSettings.model} onChange={event => setAiSettings({ ...aiSettings, model: event.target.value })} placeholder="deepseek-v4-pro" data-help="默认使用 deepseek-v4-pro，可按服务商实际模型填写。" /></label><label>API Key<input type="password" value={apiKey} onChange={event => setApiKey(event.target.value)} placeholder={aiSettings.api_key_configured ? '已保存，留空保持原值' : '尚未设置'} data-help="密钥只保存到系统保护存储，日志和界面不会显示原文。" /></label><button className="primary" data-help="保存地址、模型和密钥设置。" onClick={() => void save()} disabled={busy}><Save size={15} />保存 AI 设置</button></section>
      <section className="assistant-card"><span className="eyebrow">使用边界</span><h3>回复规则</h3><dl className="assistant-facts"><dt>触发方式</dt><dd>明确 @DH 或旺商聊提及元数据</dd><dt>资料范围</dt><dd>当前群已启用的知识库</dd><dt>预测应用</dt><dd>先做统计，再由 AI 优化表达</dd><dt>资料不足</dt><dd>当前资料里没有说明，建议联系群管理员确认。</dd></dl><div className="setting-note"><strong>AI 回复、固定规则和人工群控彼此独立</strong><span>AI 返回的动作与任务只作为建议；预测应用只接受文字回复。</span></div></section>
    </div>
    <section className="assistant-card assistant-test"><span className="eyebrow">离群测试</span><h3>测试当前 AI</h3><p className="muted">此处只在本地窗口显示结果，不读取群消息，也不会发送回复或执行群管动作。</p><label className="switch-field"><input type="checkbox" checked={includeBuiltInKnowledge} onChange={event => setIncludeBuiltInKnowledge(event.target.checked)} /><span>加载默认群规与 FAQ，仅用于本地测试</span></label><textarea value={question} onChange={event => setQuestion(event.target.value)} placeholder="输入一条测试问题，例如：@DH 群规是什么？" data-help="输入任意本地测试问题，结果不会进入群聊。" /><button className="primary" data-help="使用当前 AI 设置执行一次离群测试。" onClick={() => void test()} disabled={busy || !question.trim()}><Send size={15} />测试 AI</button>{reply && <pre className="ai-result">{reply}</pre>}</section>
  </div>
}
