import { useEffect, useState } from 'react'
import { AppWindow, BookOpen, Bot, Check, Copy, FileText, Link2, Plus, RefreshCw, Save, Trash2, Upload, X } from 'lucide-react'
import { api, readableError } from '../api/client'
import { EmptyState, PageFeedback, SectionHeading } from '../components/PageState'
import type { AiSettings, Group, KnowledgeBase, KnowledgeDocument, LoadState } from '../types'
import AiAssistantPanel from './AiAssistantPanel'
import BusinessAppsPanel from './BusinessAppsPanel'

const blankDocument = (baseId: number): KnowledgeDocument => ({
  id: 0,
  baseId,
  baseName: '',
  title: '',
  kind: 'markdown',
  content: '',
  source: 'manual',
  contentHash: '',
  enabled: true,
})

export default function KnowledgePage({ accountId, groups, aiSettings, setAiSettings, refresh, onError }: { accountId: string; groups: Group[]; aiSettings: AiSettings; setAiSettings: (value: AiSettings) => void; refresh: () => Promise<void>; onError: (value: string) => void }) {
  const [activeTab, setActiveTab] = useState<'knowledge' | 'assistant' | 'apps'>('knowledge')
  const [bases, setBases] = useState<KnowledgeBase[]>([])
  const [baseId, setBaseId] = useState<number | null>(null)
  const [documents, setDocuments] = useState<KnowledgeDocument[]>([])
  const [document, setDocument] = useState<KnowledgeDocument | null>(null)
  const [selectedGroups, setSelectedGroups] = useState<number[]>([])
  const [state, setState] = useState<LoadState>('idle')
  const [docState, setDocState] = useState<LoadState>('idle')
  const [saving, setSaving] = useState(false)
  const [removeConfirm, setRemoveConfirm] = useState<KnowledgeBase | null>(null)
  const [removing, setRemoving] = useState(false)

  const reload = async () => {
    if (!accountId) {
      setBases([])
      setState('empty')
      return
    }
    setState('loading')
    try {
      const result = await api.listKnowledgeBases(accountId)
      setBases(result)
      setBaseId(current => current && result.some(base => base.id === current) ? current : result[0]?.id ?? null)
      setState(result.length ? 'ready' : 'empty')
    } catch (reason) {
      setState(bases.length ? 'offlineCached' : 'error')
      onError(readableError(reason))
    }
  }

  const loadDocuments = async (id: number | null) => {
    if (!id) {
      setDocuments([])
      setDocument(null)
      setDocState('empty')
      return
    }
    setDocState('loading')
    try {
      const [result, bindings] = await Promise.all([
        api.listKnowledgeDocuments(id),
        api.listKnowledgeBindings(accountId, id),
      ])
      setDocuments(result)
      setSelectedGroups(bindings.filter(binding => binding.enabled).map(binding => binding.groupId))
      setDocument(current => current && result.some(value => value.id === current.id) ? current : result[0] || blankDocument(id))
      setDocState(result.length ? 'ready' : 'empty')
    } catch (reason) {
      setDocState(documents.length ? 'offlineCached' : 'error')
      onError(readableError(reason))
    }
  }

  useEffect(() => { void reload() }, [accountId])
  useEffect(() => { void loadDocuments(baseId) }, [baseId])

  const currentBase = bases.find(base => base.id === baseId) || null

  const create = async () => {
    const base: KnowledgeBase = { id: 0, accountId, name: '新知识库', description: '', enabled: true, builtIn: false, readOnly: false }
    try {
      const id = await api.createKnowledgeBase(base)
      await reload()
      setBaseId(id)
    } catch (reason) {
      onError(readableError(reason))
    }
  }

  const saveDoc = async () => {
    if (!document || !currentBase || currentBase.readOnly || !document.title.trim()) {
      onError(currentBase?.readOnly ? '内置只读知识库请先复制后编辑' : '文档标题不能为空')
      return
    }
    setSaving(true)
    try {
      const id = await api.saveKnowledgeDocument({ ...document, baseId: currentBase.id, baseName: currentBase.name })
      setDocuments(values => values.map(item => item.id === document.id ? { ...document, id } : item))
      setDocument(value => value ? { ...value, id } : value)
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setSaving(false)
    }
  }

  const bind = async () => {
    if (!currentBase) return
    try {
      await api.bindKnowledgeBase(currentBase.id, accountId, selectedGroups)
      onError(selectedGroups.length ? `已绑定 ${selectedGroups.length} 个群` : '已解除当前知识库的群绑定')
    } catch (reason) {
      onError(readableError(reason))
    }
  }

  const clone = async () => {
    if (!currentBase) return
    try {
      const id = await api.cloneKnowledgeBase(accountId, currentBase.id, `${currentBase.name}（副本）`)
      await reload()
      setBaseId(id)
    } catch (reason) {
      onError(readableError(reason))
    }
  }

  const saveBase = async () => {
    if (!currentBase || currentBase.readOnly) return
    if (!currentBase.name.trim()) {
      onError('知识库名称不能为空')
      return
    }
    try {
      await api.updateKnowledgeBase(currentBase)
      onError('知识库设置已保存')
    } catch (reason) {
      onError(readableError(reason))
    }
  }

  const remove = async () => {
    const target = removeConfirm
    if (!target || target.readOnly) return
    setRemoving(true)
    try {
      await api.deleteKnowledgeBase(accountId, target.id)
      setRemoveConfirm(null)
      await reload()
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setRemoving(false)
    }
  }

  const importDocument = (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0]
    if (!file || !currentBase) return
    const reader = new FileReader()
    reader.onload = () => setDocument({ ...blankDocument(currentBase.id), title: file.name.replace(/\.[^.]+$/, ''), kind: file.name.endsWith('.md') ? 'markdown' : 'text', source: file.name, content: String(reader.result || '') })
    reader.readAsText(file)
    event.target.value = ''
  }

  return <div className="page-stack">
    <section className="section">
      <div className="knowledge-page-tabs" role="tablist" aria-label="知识与 AI 功能"><button className={activeTab === 'knowledge' ? 'active' : ''} data-help="管理群规、FAQ 和业务文档，并选择资料使用群。" onClick={() => setActiveTab('knowledge')}><BookOpen size={17} /><span><strong>知识库</strong><small>群规与业务资料</small></span></button><button className={activeTab === 'assistant' ? 'active' : ''} data-help="设置 AI 服务、查看内置人格并执行离群对话测试。" onClick={() => setActiveTab('assistant')}><Bot size={17} /><span><strong>AI 助手</strong><small>连接与本地测试</small></span></button><button className={activeTab === 'apps' ? 'active' : ''} data-help="查看、启停和测试 DH BOT 内置业务应用。" onClick={() => setActiveTab('apps')}><AppWindow size={17} /><span><strong>业务应用</strong><small>预测与后续能力</small></span></button></div>
      {activeTab === 'knowledge' && <>
      <SectionHeading eyebrow="群专属资料" title="知识库" meta={bases.length ? `${bases.length} 个知识库` : undefined} actions={<div className="button-row"><button className="secondary" data-help="重新读取知识库和绑定状态。" onClick={() => void reload()}><RefreshCw size={15} />刷新</button><button className="primary" data-help="创建一个可编辑知识库，用于群规、FAQ 或业务资料。" onClick={() => void create()}><Plus size={15} />新建知识库</button></div>} />
      <div className="default-template-note"><strong>DH 默认群规与 FAQ</strong><span>连接账号后自动创建，内置只读、默认不绑定群。可直接选择使用群；如需修改内容，先复制为可编辑知识库。</span></div>
      <PageFeedback state={state} error="知识库读取失败" retry={() => void reload()} emptyTitle="知识库为空" emptyDetail="连接账号后会创建 DH 默认群规与 FAQ，默认不绑定群。">
        <div className="knowledge-layout">
          <div className="knowledge-list">{bases.map(base => <button key={base.id} className={`knowledge-item ${base.id === baseId ? 'selected' : ''}`} onClick={() => setBaseId(base.id)}><BookOpen size={15} /><span><strong>{base.name}</strong><small>{base.description || '未填写说明'}</small></span><em className={base.enabled ? 'enabled' : ''}>{base.readOnly ? '内置' : base.enabled ? '启用' : '停用'}</em></button>)}</div>
          {currentBase && <div className="knowledge-editor">
            <div className="editor-title"><div><span className="eyebrow">当前知识库</span><h3>{currentBase.name}</h3><p>{currentBase.readOnly ? '内置只读库，复制后可编辑。' : '绑定群后，AI 只会检索启用的资料。'}</p></div><div className="button-row"><button className="secondary" data-help="复制一份可编辑知识库，内置只读库本身不会被修改。" onClick={() => void clone()}><Copy size={15} />复制为可编辑</button><button className="secondary" data-help="删除当前可编辑知识库及其文档，内置只读库不能删除。" disabled={currentBase.readOnly} onClick={() => setRemoveConfirm(currentBase)}><Trash2 size={15} />删除</button></div></div>
            <div className="knowledge-meta"><label>知识库名称<input value={currentBase.name} readOnly={currentBase.readOnly} onChange={event => setBases(values => values.map(base => base.id === currentBase.id ? { ...base, name: event.target.value } : base))} /></label><label>知识库说明<input value={currentBase.description} readOnly={currentBase.readOnly} onChange={event => setBases(values => values.map(base => base.id === currentBase.id ? { ...base, description: event.target.value } : base))} /></label><label className="switch-field"><input type="checkbox" checked={currentBase.enabled} disabled={currentBase.readOnly} onChange={event => setBases(values => values.map(base => base.id === currentBase.id ? { ...base, enabled: event.target.checked } : base))} /><span>启用该知识库</span></label><div className="button-row"><button className="secondary" data-help="保存当前知识库名称、说明和启用状态。" disabled={currentBase.readOnly} onClick={() => void saveBase()}><Save size={15} />保存设置</button></div><div className="group-filter knowledge-group-filter"><span><Link2 size={14} />使用群</span>{groups.map(group => <label className={`check-chip ${selectedGroups.includes(group.groupId) ? 'selected' : ''}`} key={group.groupId}><input type="checkbox" checked={selectedGroups.includes(group.groupId)} onChange={() => setSelectedGroups(values => values.includes(group.groupId) ? values.filter(id => id !== group.groupId) : [...values, group.groupId])} /><span className="check-chip-indicator"><Check size={11} aria-hidden="true" /></span>{group.name}</label>)}<button className="secondary" data-help="保存选择的群；未选择任何群时，AI 不会使用这个知识库。" onClick={() => void bind()}>保存绑定</button></div></div>
            <div className="document-layout"><div className="document-list"><div className="list-head"><strong>文档</strong><div className="button-row"><label className={`icon-button ${currentBase.readOnly ? 'disabled' : ''}`} title={currentBase.readOnly ? '内置知识库请先复制后导入' : '导入 TXT 或 Markdown'} data-help={currentBase.readOnly ? '内置知识库请先复制后导入。' : '从 TXT 或 Markdown 文件新建一篇文档。'}><Upload size={15} /><input disabled={currentBase.readOnly} type="file" accept=".txt,.md,text/plain,text/markdown" onChange={importDocument} /></label><button className="icon-button" data-help={currentBase.readOnly ? '内置知识库请先复制后新建文档。' : '新建一篇空白文档。'} disabled={currentBase.readOnly} title={currentBase.readOnly ? '内置知识库请先复制后新建' : '新建文档'} onClick={() => setDocument(blankDocument(currentBase.id))}><Plus size={15} /></button></div></div><PageFeedback state={docState} error="文档读取失败" emptyTitle="暂无文档" emptyDetail="导入 TXT/Markdown 或新建一篇资料。"><div>{documents.map(item => <button className={`document-item ${document?.id === item.id ? 'selected' : ''}`} key={item.id} onClick={() => setDocument(item)}><FileText size={14} /><span>{item.title}</span><em className={item.enabled ? 'enabled' : ''}>{item.enabled ? '启用' : '停用'}</em></button>)}</div></PageFeedback></div>{document && <div className="document-editor"><label>标题<input value={document.title} readOnly={currentBase.readOnly} onChange={event => setDocument({ ...document, title: event.target.value })} /></label><label>内容<textarea value={document.content} readOnly={currentBase.readOnly} onChange={event => setDocument({ ...document, content: event.target.value })} placeholder="写入群规、FAQ 或业务资料。" /></label><label className="switch-field"><input type="checkbox" checked={document.enabled} disabled={currentBase.readOnly} onChange={event => setDocument({ ...document, enabled: event.target.checked })} /><span>AI 可以检索此文档</span></label><div className="button-row"><button className="primary" data-help="保存当前文档内容并加入知识检索。" disabled={saving || currentBase.readOnly} onClick={() => void saveDoc()}><Save size={15} />保存文档</button><button className="secondary" data-help="删除当前文档；删除后不会继续注入 AI。" disabled={currentBase.readOnly || !document.id} onClick={async () => { try { await api.deleteKnowledgeDocument(currentBase.id, document.id); await loadDocuments(currentBase.id) } catch (reason) { onError(readableError(reason)) } }}><Trash2 size={15} />删除文档</button></div></div>}</div>
          </div>}
        </div>
      </PageFeedback>
      </>}
      {activeTab === 'assistant' && <AiAssistantPanel accountId={accountId} aiSettings={aiSettings} setAiSettings={setAiSettings} refresh={refresh} onError={onError} />}
      {activeTab === 'apps' && <BusinessAppsPanel accountId={accountId} onError={onError} />}
    </section>
    {removeConfirm && <div className="dialog-backdrop group-batch-dialog-backdrop" role="presentation">
      <section className="group-batch-dialog compact-confirm-dialog" role="alertdialog" aria-modal="true" aria-labelledby="knowledge-delete-title">
        <header><div><span className="eyebrow">知识库操作</span><h2 id="knowledge-delete-title">确认删除知识库</h2></div><button className="icon-command" title="关闭" onClick={() => setRemoveConfirm(null)} disabled={removing}><X size={16} /></button></header>
        <p className="group-batch-confirm-copy">将删除“{removeConfirm.name}”及其中的全部文档。内置知识库和其他知识库不会受影响。</p>
        <footer><span /><button className="secondary" onClick={() => setRemoveConfirm(null)} disabled={removing}>取消</button><button className="primary" onClick={() => void remove()} disabled={removing}><Trash2 size={15} />{removing ? '正在删除' : '确认删除'}</button></footer>
      </section>
    </div>}
  </div>
}
