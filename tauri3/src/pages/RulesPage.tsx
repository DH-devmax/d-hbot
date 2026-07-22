import { useEffect, useState } from 'react'
import { Download, Pencil, Plus, RefreshCw, Save, ShieldCheck, Trash2, Upload, X } from 'lucide-react'
import { api, readableError } from '../api/client'
import { PageFeedback, SectionHeading } from '../components/PageState'
import type { Group, LoadState, Rule } from '../types'

const actionLabels: Record<string, string> = { recall: '撤回', mute: '禁言', remove: '移出', blacklist: '拉黑', notify: '提示' }
const matcherLabels: Record<string, string> = { contains: '包含关键词', exact: '精确匹配', prefix: '前缀匹配', regex: '正则表达式', length: '加权字符数', lines: '消息行数', image_count: '图片次数', blacklist: '黑名单成员', rename_count: '改名次数', semantic: 'AI 语义' }
const patternMatchers = new Set(['contains', 'exact', 'prefix', 'regex', 'semantic'])
const thresholdMatchers = new Set(['length', 'lines'])
const countMatchers = new Set(['image_count', 'rename_count'])

function blankRule(accountId: string): Rule {
  return { id: 0, accountId, groupId: 0, name: '新规则', matcher: 'contains', pattern: '', threshold: 0, count: 0, windowSeconds: 0, cooldownSeconds: 0, priority: 100, mode: 'observe', enabled: false, semanticThreshold: 0.8, exemptRoles: ['owner', 'admin'], exemptUserIds: [], actions: [{ kind: 'recall', durationSeconds: 0, message: '' }] }
}

function editableCopy(rule: Rule): Rule {
  return { ...rule, exemptRoles: [...rule.exemptRoles], exemptUserIds: [...rule.exemptUserIds], actions: rule.actions.map(action => ({ ...action })) }
}

export default function RulesPage({ accountId, groups, onError }: { accountId: string; groups: Group[]; onError: (value: string) => void }) {
  const [rules, setRules] = useState<Rule[]>([])
  const [editing, setEditing] = useState<Rule | null>(null)
  const [state, setState] = useState<LoadState>('idle')
  const [error, setError] = useState('')
  const [saving, setSaving] = useState(false)

  const reload = async () => {
    if (!accountId) { setRules([]); setState('empty'); return }
    setState('loading'); setError('')
    try {
      const result = await api.listRules(accountId)
      setRules(result)
      setState(result.length ? 'ready' : 'empty')
    } catch (reason) {
      setError(readableError(reason))
      setState(rules.length ? 'offlineCached' : 'error')
    }
  }

  useEffect(() => {
    setEditing(null)
    void reload()
  }, [accountId])

  const save = async () => {
    if (!editing || !editing.name.trim() || (patternMatchers.has(editing.matcher) && !editing.pattern.trim())) {
      onError(patternMatchers.has(editing?.matcher || '') ? '规则名称和匹配内容不能为空' : '规则名称不能为空')
      return
    }
    if (editing.matcher === 'regex') {
      try { new RegExp(editing.pattern) } catch { onError('正则表达式格式有误'); return }
    }
    setSaving(true)
    try {
      await api.saveRule(editing)
      setEditing(null)
      await reload()
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setSaving(false)
    }
  }

  const remove = async () => {
    if (!editing?.id || !window.confirm(`确认删除规则“${editing.name}”？`)) return
    try {
      await api.deleteRule(accountId, editing.id)
      setEditing(null)
      await reload()
    } catch (reason) {
      onError(readableError(reason))
    }
  }

  const exportRules = async () => {
    try { download('dh-rules.json', await api.exportRules(accountId, rules)) } catch (reason) { onError(readableError(reason)) }
  }

  const importRules = (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0]
    if (!file) return
    const reader = new FileReader()
    reader.onload = async () => {
      try {
        const content = String(reader.result)
        const parsed = JSON.parse(content)
        if (!Array.isArray(parsed?.rules)) throw new Error('JSON 中没有 rules 数组')
        const automatic = parsed.rules.filter((rule: Rule) => rule.mode === 'automatic' && rule.enabled).length
        if (automatic && !window.confirm(`文件中有 ${automatic} 条已启用自动规则，确认导入？`)) return
        await api.importRules(accountId, content)
        await reload()
        onError(`已导入 ${parsed.rules.length} 条规则`)
      } catch (reason) {
        onError(`导入失败：${readableError(reason)}`)
      }
    }
    reader.readAsText(file)
    event.target.value = ''
  }

  return <div className="page-stack">
    <section className="section">
      <SectionHeading eyebrow="确定性群管" title="规则" meta={`${rules.length} 条`} actions={<div className="button-row">
        <button className="secondary" title="重新读取规则列表" onClick={() => void reload()}><RefreshCw size={15} />刷新</button>
        <button className="secondary" title="导出当前账号的全部规则" onClick={exportRules}><Download size={15} />导出 JSON</button>
        <label className="secondary file-button" title="从 JSON 文件导入规则"><Upload size={15} />导入 JSON<input type="file" accept="application/json" onChange={importRules} /></label>
        <button className="primary" title="打开窗口创建一条规则" onClick={() => setEditing(blankRule(accountId))}><Plus size={15} />新建规则</button>
      </div>} />
      <div className="default-template-note"><strong>默认规则模板</strong><span>全局规则，首次全部停用；确定性规则启用后按规则执行，AI 语义规则保持观察模式。默认动作仅撤回，群主和管理员自动豁免。</span></div>
      <PageFeedback state={state} error={error} retry={() => void reload()} emptyTitle="规则为空" emptyDetail="默认模板正在准备，连接账号后会自动出现；所有默认规则初始停用。可新建规则或导入兼容模板。">
        <div className="rule-list rule-list-grid">{rules.map(rule => <button className="rule-list-item" title={`编辑规则：${rule.name}`} onClick={() => setEditing(editableCopy(rule))} key={`${rule.id}-${rule.name}`}>
          <ShieldCheck size={15} />
          <span><strong>{rule.name}</strong><small>{rule.groupId === 0 ? '全局规则' : groups.find(group => group.groupId === rule.groupId)?.name || `群 ${rule.groupId}`} · {matcherLabels[rule.matcher] || rule.matcher} · {rule.actions.map(action => actionLabels[action.kind] || action.kind).join('、') || '无动作'}</small></span>
          <em className={rule.enabled ? 'enabled' : ''}>{rule.enabled ? '启用' : '停用'}</em>
        </button>)}</div>
      </PageFeedback>
    </section>
    {editing && <RuleEditorDialog rule={editing} groups={groups} onChange={patch => setEditing(current => current ? { ...current, ...patch } : current)} onSave={() => void save()} onDelete={() => void remove()} onClose={() => !saving && setEditing(null)} saving={saving} />}
  </div>
}

function RuleEditorDialog({ rule, groups, onChange, onSave, onDelete, onClose, saving }: { rule: Rule; groups: Group[]; onChange: (patch: Partial<Rule>) => void; onSave: () => void; onDelete: () => void; onClose: () => void; saving: boolean }) {
  const [whitelistText, setWhitelistText] = useState(rule.exemptUserIds.join(','))

  useEffect(() => { setWhitelistText(rule.exemptUserIds.join(',')) }, [rule.id])
  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose() }
    window.addEventListener('keydown', closeOnEscape)
    return () => window.removeEventListener('keydown', closeOnEscape)
  }, [onClose])

  const setAction = (kind: string, enabled: boolean) => {
    const actions = enabled ? [...rule.actions, { kind, durationSeconds: kind === 'mute' ? 600 : 0, message: '' }] : rule.actions.filter(action => action.kind !== kind)
    onChange({ actions })
  }
  const setRole = (role: string, enabled: boolean) => onChange({ exemptRoles: enabled ? [...new Set([...rule.exemptRoles, role])] : rule.exemptRoles.filter(value => value !== role) })
  const updateWhitelist = (value: string) => {
    setWhitelistText(value)
    onChange({ exemptUserIds: value.split(/[,，\s]+/).map(item => Number(item)).filter(item => Number.isSafeInteger(item) && item > 0) })
  }
  const changeMatcher = (matcher: string) => onChange({
    matcher,
    count: countMatchers.has(matcher) && rule.count < 1 ? 1 : rule.count,
    windowSeconds: matcher === 'image_count' && rule.windowSeconds < 1 ? 600 : rule.windowSeconds,
    threshold: thresholdMatchers.has(matcher) && rule.threshold < 1 ? 1 : rule.threshold,
  })

  return <div className="dialog-backdrop rule-dialog-backdrop" role="presentation" onMouseDown={event => { if (event.target === event.currentTarget) onClose() }}>
    <section className="rule-dialog" role="dialog" aria-modal="true" aria-labelledby="rule-dialog-title">
      <header className="rule-dialog-header">
        <div><span className="eyebrow">规则编辑</span><h2 id="rule-dialog-title">{rule.name || '未命名规则'}</h2></div>
        <button className="icon-button" title="关闭规则窗口" aria-label="关闭规则窗口" disabled={saving} onClick={onClose}><X size={17} /></button>
      </header>
      <div className="rule-dialog-body">
        <div className="form-grid">
          <label>规则名称<input value={rule.name} onChange={event => onChange({ name: event.target.value })} /></label>
          <label>作用群<select value={rule.groupId} onChange={event => onChange({ groupId: Number(event.target.value) })}><option value={0}>全局规则</option>{groups.map(group => <option value={group.groupId} key={group.groupId}>{group.name}</option>)}</select></label>
          <label>匹配方式<select value={rule.matcher} onChange={event => changeMatcher(event.target.value)}>{Object.entries(matcherLabels).map(([value, label]) => <option value={value} key={value}>{label}</option>)}</select></label>
          <label>优先级<input type="number" min={0} max={999} value={rule.priority} onChange={event => onChange({ priority: Number(event.target.value) })} /></label>
          {patternMatchers.has(rule.matcher) && <label className="wide-field">匹配内容<textarea value={rule.pattern} onChange={event => onChange({ pattern: event.target.value })} placeholder={rule.matcher === 'semantic' ? '例如：advertisement、abuse、scam' : '例如：广告、诈骗链接或正则表达式'} /></label>}
          {thresholdMatchers.has(rule.matcher) && <label>触发阈值<input type="number" min={1} value={rule.threshold || ''} onChange={event => onChange({ threshold: Number(event.target.value) })} /></label>}
          {countMatchers.has(rule.matcher) && <label>触发次数<input type="number" min={1} value={rule.count || ''} onChange={event => onChange({ count: Number(event.target.value) })} /></label>}
          {rule.matcher === 'image_count' && <label>统计窗口（秒）<input type="number" min={1} value={rule.windowSeconds || ''} onChange={event => onChange({ windowSeconds: Number(event.target.value) })} /></label>}
          <label>冷却时间（秒）<input type="number" min={0} value={rule.cooldownSeconds} onChange={event => onChange({ cooldownSeconds: Number(event.target.value) })} /></label>
          {rule.matcher === 'semantic' && <label>语义阈值<input type="number" min={0} max={1} step={0.05} value={rule.semanticThreshold} onChange={event => onChange({ semanticThreshold: Number(event.target.value) })} /></label>}
          <label className="wide-field">成员白名单（旺商号，逗号分隔）<input value={whitelistText} onChange={event => updateWhitelist(event.target.value)} placeholder="例如：10001,10002" /></label>
        </div>
        <div className="editor-section"><span className="eyebrow">豁免角色</span><div className="action-checks">{[['owner', '群主'], ['admin', '管理员'], ['member', '普通群员']].map(([role, label]) => <label key={role}><input type="checkbox" checked={rule.exemptRoles.includes(role)} onChange={event => setRole(role, event.target.checked)} />{label}</label>)}</div></div>
        <div className="editor-section"><span className="eyebrow">执行控制</span><div className="toggle-row"><label className="switch-field"><input type="checkbox" checked={rule.enabled} onChange={event => onChange({ enabled: event.target.checked })} /><span>启用规则</span></label><label className="switch-field"><input type="checkbox" checked={rule.mode === 'automatic'} onChange={event => onChange({ mode: event.target.checked ? 'automatic' : 'observe' })} /><span>自动执行（默认观察）</span></label></div><div className="action-checks">{Object.entries(actionLabels).map(([kind, label]) => <label key={kind}><input type="checkbox" checked={rule.actions.some(action => action.kind === kind)} onChange={event => setAction(kind, event.target.checked)} />{label}{kind === 'mute' && rule.actions.some(action => action.kind === 'mute') && <input className="duration-input" aria-label="禁言时长（秒）" type="number" min={1} value={rule.actions.find(action => action.kind === 'mute')?.durationSeconds || 600} onChange={event => onChange({ actions: rule.actions.map(action => action.kind === 'mute' ? { ...action, durationSeconds: Number(event.target.value) } : action) })} />}</label>)}</div></div>
      </div>
      <footer className="rule-dialog-footer">
        <div>{rule.id > 0 && <button className="secondary" title="删除当前规则" onClick={onDelete} disabled={saving}><Trash2 size={15} />删除</button>}</div>
        <div className="button-row"><button className="secondary" onClick={onClose} disabled={saving}>取消</button><button className="primary" title="保存当前规则配置" onClick={onSave} disabled={saving}><Save size={15} />{saving ? '保存中' : '保存规则'}</button></div>
      </footer>
    </section>
  </div>
}

function download(name: string, content: string) {
  const blob = new Blob([content], { type: 'application/json;charset=utf-8' })
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = name
  link.click()
  URL.revokeObjectURL(url)
}
