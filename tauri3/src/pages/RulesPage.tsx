import { useEffect, useMemo, useState } from 'react'
import { Bot, Check, Download, Plus, RefreshCw, Save, Search, ShieldCheck, Trash2, Upload, X } from 'lucide-react'
import { api, readableError } from '../api/client'
import { PageFeedback, SectionHeading } from '../components/PageState'
import SelectField from '../components/SelectField'
import type { Group, LoadState, Rule, RuleMember } from '../types'

type RuleTab = 'machine' | 'ai'

const actionLabels: Record<string, string> = { recall: '撤回', mute: '禁言', remove: '移出', blacklist: '拉黑', notify: '提示' }
const machineMatchers: Record<string, string> = { contains: '包含关键词', exact: '精确匹配', prefix: '前缀匹配', regex: '正则表达式', length: '加权字符数', lines: '消息行数', image_count: '图片次数', blacklist: '黑名单成员', rename_count: '改名次数' }
const aiMatchers: Record<string, string> = { semantic: 'AI 语义分类' }
const matcherLabels = { ...machineMatchers, ...aiMatchers }
const patternMatchers = new Set(['contains', 'exact', 'prefix', 'regex', 'semantic'])
const thresholdMatchers = new Set(['length', 'lines'])
const countMatchers = new Set(['image_count', 'rename_count'])
const priorityLabels = { low: '1 低', medium: '2 中', high: '3 高' }

function blankRule(accountId: string, ruleType: RuleTab): Rule {
  return { id: 0, accountId, ruleType, scope: 'global', groupIds: [], priorityLevel: 'medium', whitelistUserIds: [], name: ruleType === 'ai' ? '新 AI 规则' : '新机器规则', matcher: ruleType === 'ai' ? 'semantic' : 'contains', pattern: '', threshold: 0, count: 0, windowSeconds: 0, mode: 'observe', enabled: false, semanticThreshold: 0.8, actions: [{ kind: 'recall', durationSeconds: 0, message: '' }] }
}

function editableCopy(rule: Rule): Rule {
  return { ...rule, groupIds: [...rule.groupIds], whitelistUserIds: [...rule.whitelistUserIds], actions: rule.actions.map(action => ({ ...action })) }
}

export default function RulesPage({ accountId, groups, onError }: { accountId: string; groups: Group[]; onError: (value: string) => void }) {
  const [rules, setRules] = useState<Rule[]>([])
  const [activeTab, setActiveTab] = useState<RuleTab>('machine')
  const [editing, setEditing] = useState<Rule | null>(null)
  const [state, setState] = useState<LoadState>('idle')
  const [error, setError] = useState('')
  const [saving, setSaving] = useState(false)
  const [featureState, setFeatureState] = useState<Record<number, { machine: boolean; ai: boolean }>>({})
  const [featureSaving, setFeatureSaving] = useState<number | null>(null)

  useEffect(() => {
    setFeatureState(Object.fromEntries(groups.map(group => [group.groupId, { machine: group.machineRulesEnabled ?? group.moderationEnabled, ai: group.aiRulesEnabled ?? false }])))
  }, [groups])

  const reload = async () => {
    if (!accountId) { setRules([]); setState('empty'); return }
    setState('loading'); setError('')
    try {
      const result = await api.listRules(accountId)
      setRules(result)
      setState(result.length ? 'ready' : 'empty')
    } catch (reason) {
      setError(readableError(reason)); setState(rules.length ? 'offlineCached' : 'error')
    }
  }

  useEffect(() => { setEditing(null); void reload() }, [accountId])

  const visibleRules = useMemo(() => rules.filter(rule => rule.ruleType === activeTab), [rules, activeTab])

  const save = async () => {
    if (!editing || !editing.name.trim() || (patternMatchers.has(editing.matcher) && !editing.pattern.trim())) { onError(patternMatchers.has(editing?.matcher || '') ? '规则名称和匹配内容不能为空' : '规则名称不能为空'); return }
    if (editing.scope === 'selected' && !editing.groupIds.length) { onError('请至少选择一个作用群'); return }
    if (editing.matcher === 'regex') { try { new RegExp(editing.pattern) } catch { onError('正则表达式格式有误'); return } }
    setSaving(true)
    try { await api.saveRule(editing); setEditing(null); await reload() } catch (reason) { onError(readableError(reason)) } finally { setSaving(false) }
  }

  const remove = async () => {
    if (!editing?.id || !window.confirm(`确认删除规则“${editing.name}”？`)) return
    try { await api.deleteRule(accountId, editing.id); setEditing(null); await reload() } catch (reason) { onError(readableError(reason)) }
  }

  const updateGroupFeature = async (group: Group, kind: RuleTab, enabled: boolean) => {
    const previous = featureState[group.groupId] || { machine: false, ai: false }
    const next = { ...previous, [kind]: enabled }
    setFeatureState(value => ({ ...value, [group.groupId]: next })); setFeatureSaving(group.groupId)
    try { await api.setGroupRuleFeatures(accountId, group.groupId, next.machine, next.ai) }
    catch (reason) { setFeatureState(value => ({ ...value, [group.groupId]: previous })); onError(readableError(reason)) }
    finally { setFeatureSaving(null) }
  }

  const exportRules = async () => { try { download('dh-rules-v2.json', await api.exportRules(accountId, rules)) } catch (reason) { onError(readableError(reason)) } }
  const importRules = (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0]; if (!file) return
    const reader = new FileReader()
    reader.onload = async () => { try { const content = String(reader.result); const parsed = JSON.parse(content); if (!Array.isArray(parsed?.rules)) throw new Error('JSON 中没有 rules 数组'); const automatic = parsed.rules.filter((rule: Rule) => rule.mode === 'automatic' && rule.enabled).length; if (automatic && !window.confirm(`文件中有 ${automatic} 条已启用自动规则，确认导入？`)) return; await api.importRules(accountId, content); await reload(); onError(`已导入 ${parsed.rules.length} 条规则`) } catch (reason) { onError(`导入失败：${readableError(reason)}`) } }
    reader.readAsText(file); event.target.value = ''
  }

  return <div className="page-stack rules-page">
    <section className="section">
      <SectionHeading eyebrow="自动群管" title="规则" meta={`${rules.length} 条`} actions={<div className="button-row">
        <button className="secondary" data-help="重新读取当前账号的全部规则。" onClick={() => void reload()}><RefreshCw size={15} />刷新</button>
        <button className="secondary" data-help="导出机器规则和 AI 规则的 v2 JSON。" onClick={exportRules}><Download size={15} />导出 JSON</button>
        <label className="secondary file-button" data-help="导入 v1 或 v2 规则 JSON，旧规则会自动升级。"><Upload size={15} />导入 JSON<input type="file" accept="application/json" onChange={importRules} /></label>
        <button className="primary" data-help={`创建一条${activeTab === 'machine' ? '不依赖 AI 的机器规则' : '异步语义判断规则'}。`} onClick={() => setEditing(blankRule(accountId, activeTab))}><Plus size={15} />新建规则</button>
      </div>} />
      <div className="rule-group-switches">
        <div><strong>各群运行开关</strong><span>机器规则和 AI 控制规则互不影响</span></div>
        {groups.map(group => { const value = featureState[group.groupId] || { machine: false, ai: false }; return <div className="rule-group-switch-row" key={group.groupId}><span>{group.name}</span><label><input type="checkbox" checked={value.machine} disabled={featureSaving === group.groupId} onChange={event => void updateGroupFeature(group, 'machine', event.target.checked)} />机器规则</label><label><input type="checkbox" checked={value.ai} disabled={featureSaving === group.groupId} onChange={event => void updateGroupFeature(group, 'ai', event.target.checked)} />AI 规则</label></div> })}
      </div>
      <div className="rule-type-tabs" role="tablist">
        <button className={activeTab === 'machine' ? 'active' : ''} onClick={() => setActiveTab('machine')}><ShieldCheck size={18} /><span><strong>机器规则</strong><small>立即判断，不依赖 AI</small></span></button>
        <button className={activeTab === 'ai' ? 'active' : ''} onClick={() => setActiveTab('ai')}><Bot size={18} /><span><strong>AI 控制规则</strong><small>异步语义分类，默认观察</small></span></button>
      </div>
      <div className="default-template-note"><strong>{activeTab === 'machine' ? '机器规则模板' : 'AI 规则模板'}</strong><span>{activeTab === 'machine' ? '启用后逐条即时判断；没有规则冷却，协议写入仍按 500ms 间隔保护旺商聊。' : '全量异步判断群消息；无需 @DH，首次保持观察模式，高影响动作需单独授权。'}</span></div>
      <PageFeedback state={state} error={error} retry={() => void reload()} emptyTitle="规则为空" emptyDetail="连接账号后会准备默认模板；所有默认规则初始停用。">
        {visibleRules.length ? <div className="rule-list rule-list-grid">{visibleRules.map(rule => <button className="rule-list-item" data-help={`编辑规则“${rule.name}”。`} onClick={() => setEditing(editableCopy(rule))} key={`${rule.id}-${rule.name}`}>
          {rule.ruleType === 'ai' ? <Bot size={15} /> : <ShieldCheck size={15} />}
          <span><strong>{rule.name}</strong><small>{rule.scope === 'global' ? '全局规则' : `${rule.groupIds.length} 个群`} · {matcherLabels[rule.matcher] || rule.matcher} · {priorityLabels[rule.priorityLevel]} · {rule.actions.map(action => actionLabels[action.kind] || action.kind).join('、') || '无动作'}</small></span>
          <em className={rule.enabled ? 'enabled' : ''}>{rule.enabled ? '启用' : '停用'}</em>
        </button>)}</div> : <div className="page-feedback"><strong>{activeTab === 'machine' ? '暂无机器规则' : '暂无 AI 控制规则'}</strong><span>点击“新建规则”开始配置。</span></div>}
      </PageFeedback>
    </section>
    {editing && <RuleEditorDialog accountId={accountId} rule={editing} groups={groups} onChange={patch => setEditing(current => current ? { ...current, ...patch } : current)} onSave={() => void save()} onDelete={() => void remove()} onClose={() => !saving && setEditing(null)} saving={saving} onError={onError} />}
  </div>
}

function RuleEditorDialog({ accountId, rule, groups, onChange, onSave, onDelete, onClose, saving, onError }: { accountId: string; rule: Rule; groups: Group[]; onChange: (patch: Partial<Rule>) => void; onSave: () => void; onDelete: () => void; onClose: () => void; saving: boolean; onError: (value: string) => void }) {
  const [memberQuery, setMemberQuery] = useState('')
  const [memberResults, setMemberResults] = useState<RuleMember[]>([])
  const [memberCache, setMemberCache] = useState<Record<number, RuleMember>>({})
  const [memberLoading, setMemberLoading] = useState(false)
  const [memberSearchError, setMemberSearchError] = useState('')
  const searchGroups = rule.scope === 'global' ? groups.map(group => group.groupId) : rule.groupIds
  const searchGroupKey = searchGroups.join(',')

  useEffect(() => { const closeOnEscape = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose() }; window.addEventListener('keydown', closeOnEscape); return () => window.removeEventListener('keydown', closeOnEscape) }, [onClose])
  useEffect(() => {
    let cancelled = false
    setMemberSearchError('')
    if (!accountId || !searchGroups.length) {
      setMemberLoading(false)
      setMemberResults([])
      return () => { cancelled = true }
    }
    setMemberLoading(true)
    const run = async () => {
      try {
        const page = await api.searchRuleMembers(accountId, searchGroups, memberQuery.trim(), undefined, 50)
        if (cancelled) return
        setMemberResults(page.items)
        setMemberCache(value => ({ ...value, ...Object.fromEntries(page.items.map(member => [member.userId, member])) }))
      } catch (reason) {
        if (cancelled) return
        const message = readableError(reason)
        setMemberResults([])
        setMemberSearchError(message)
        onError(`成员白名单读取失败：${message}`)
      } finally {
        if (!cancelled) setMemberLoading(false)
      }
    }
    const timer = window.setTimeout(() => void run(), memberQuery.trim() ? 180 : 0)
    return () => { cancelled = true; window.clearTimeout(timer) }
  }, [accountId, memberQuery, searchGroupKey])

  const setAction = (kind: string, enabled: boolean) => onChange({ actions: enabled ? [...rule.actions, { kind, durationSeconds: kind === 'mute' ? 600 : 0, message: '' }] : rule.actions.filter(action => action.kind !== kind) })
  const toggleGroup = (groupId: number) => onChange({ groupIds: rule.groupIds.includes(groupId) ? rule.groupIds.filter(value => value !== groupId) : [...rule.groupIds, groupId] })
  const toggleMember = (userId: number) => onChange({ whitelistUserIds: rule.whitelistUserIds.includes(userId) ? rule.whitelistUserIds.filter(value => value !== userId) : [...rule.whitelistUserIds, userId] })
  const changeMatcher = (matcher: string) => onChange({ matcher, count: countMatchers.has(matcher) && rule.count < 1 ? 1 : rule.count, windowSeconds: matcher === 'image_count' && rule.windowSeconds < 1 ? 600 : rule.windowSeconds, threshold: thresholdMatchers.has(matcher) && rule.threshold < 1 ? 1 : rule.threshold })
  const availableMatchers = rule.ruleType === 'ai' ? aiMatchers : machineMatchers

  return <div className="dialog-backdrop rule-dialog-backdrop" role="presentation" onMouseDown={event => { if (event.target === event.currentTarget) onClose() }}>
    <section className="rule-dialog" role="dialog" aria-modal="true" aria-labelledby="rule-dialog-title">
      <header className="rule-dialog-header"><div><span className="eyebrow">{rule.ruleType === 'ai' ? 'AI 控制规则' : '机器规则'}</span><h2 id="rule-dialog-title">{rule.name || '未命名规则'}</h2></div><button className="icon-button" title="关闭规则窗口" aria-label="关闭规则窗口" disabled={saving} onClick={onClose}><X size={17} /></button></header>
      <div className="rule-dialog-body">
        <div className="form-grid">
          <label>规则名称<input value={rule.name} onChange={event => onChange({ name: event.target.value })} /></label>
          <SelectField label="优先级" value={rule.priorityLevel} options={Object.entries(priorityLabels).map(([value, label]) => ({ value, label }))} onChange={value => onChange({ priorityLevel: value as Rule['priorityLevel'] })} />
          <SelectField label="作用范围" value={rule.scope} options={[{ value: 'global', label: '全部管理群' }, { value: 'selected', label: '选择多个群' }]} onChange={value => onChange({ scope: value as Rule['scope'], groupIds: value === 'global' ? [] : rule.groupIds })} />
          <SelectField label="匹配方式" value={rule.matcher} options={Object.entries(availableMatchers).map(([value, label]) => ({ value, label }))} onChange={changeMatcher} />
          {rule.scope === 'selected' && <div className="wide-field rule-group-picker"><span>作用群（可多选）</span><div>{groups.map(group => <label className={`check-chip ${rule.groupIds.includes(group.groupId) ? 'selected' : ''}`} key={group.groupId}><input type="checkbox" checked={rule.groupIds.includes(group.groupId)} onChange={() => toggleGroup(group.groupId)} /><Check size={13} />{group.name}</label>)}</div></div>}
          {patternMatchers.has(rule.matcher) && <label className="wide-field">{rule.ruleType === 'ai' ? '语义类别' : '匹配内容'}<textarea value={rule.pattern} onChange={event => onChange({ pattern: event.target.value })} placeholder={rule.ruleType === 'ai' ? '例如：advertisement、abuse、scam' : '填写关键词、前缀或正则表达式'} /></label>}
          {thresholdMatchers.has(rule.matcher) && <label>触发阈值<input type="number" min={1} value={rule.threshold || ''} onChange={event => onChange({ threshold: Number(event.target.value) })} /></label>}
          {countMatchers.has(rule.matcher) && <label>触发次数<input type="number" min={1} value={rule.count || ''} onChange={event => onChange({ count: Number(event.target.value) })} /></label>}
          {rule.matcher === 'image_count' && <label>统计窗口（秒）<input type="number" min={1} value={rule.windowSeconds || ''} onChange={event => onChange({ windowSeconds: Number(event.target.value) })} /></label>}
          {rule.ruleType === 'ai' && <label>置信度阈值<input type="number" min={0} max={1} step={0.05} value={rule.semanticThreshold} onChange={event => onChange({ semanticThreshold: Number(event.target.value) })} /></label>}
        </div>
        <div className="editor-section rule-whitelist">
          <span className="eyebrow">成员白名单</span>
          <p>从作用群搜索当前名称、原名称、DH 名称、旺商号或内部 ID，点击名片即可多选。留空时显示当前作用范围的成员。</p>
          <label className="search-field"><Search size={15} /><input value={memberQuery} onChange={event => setMemberQuery(event.target.value)} placeholder="搜索名称、旺商号或内部 ID" /></label>
          {rule.whitelistUserIds.length > 0 && <div className="selected-member-chips">{rule.whitelistUserIds.map(userId => { const member = memberCache[userId]; return <button key={userId} onClick={() => toggleMember(userId)}>{member?.managedCardName || member?.cardName || member?.originalCardName || `用户 ${userId}`}<X size={12} /></button> })}</div>}
          <div className="member-search-results">
            {memberLoading ? <span className="member-search-state">正在读取成员…</span>
              : memberSearchError ? <span className="member-search-state error">成员读取失败，请稍后重试。</span>
                : !searchGroups.length ? <span className="member-search-state">请先选择作用群。</span>
                  : !memberResults.length ? <span className="member-search-state">{memberQuery.trim() ? '没有找到匹配成员。' : '当前作用范围暂无可选成员。'}</span>
                    : memberResults.map(member => <button className={rule.whitelistUserIds.includes(member.userId) ? 'selected' : ''} key={member.userId} onClick={() => toggleMember(member.userId)}><span><strong>{member.managedCardName || member.cardName || member.nickname || `用户 ${member.userId}`}</strong><small>{member.originalCardName && member.originalCardName !== member.cardName ? `原名 ${member.originalCardName} · ` : ''}{member.nimId || member.userId}</small></span>{rule.whitelistUserIds.includes(member.userId) && <Check size={15} />}</button>)}
          </div>
        </div>
        <div className="editor-section"><span className="eyebrow">执行控制</span><div className="toggle-row"><label className="switch-field"><input type="checkbox" checked={rule.enabled} onChange={event => onChange({ enabled: event.target.checked })} /><span>启用规则</span></label><label className="switch-field"><input type="checkbox" checked={rule.mode === 'automatic'} onChange={event => onChange({ mode: event.target.checked ? 'automatic' : 'observe' })} /><span>{rule.ruleType === 'ai' ? '自动执行（默认观察）' : '命中后自动执行'}</span></label></div><div className="action-checks">{Object.entries(actionLabels).map(([kind, label]) => <label key={kind}><input type="checkbox" checked={rule.actions.some(action => action.kind === kind)} onChange={event => setAction(kind, event.target.checked)} />{label}{kind === 'mute' && rule.actions.some(action => action.kind === 'mute') && <input className="duration-input" aria-label="禁言时长（秒）" type="number" min={1} value={rule.actions.find(action => action.kind === 'mute')?.durationSeconds || 600} onChange={event => onChange({ actions: rule.actions.map(action => action.kind === 'mute' ? { ...action, durationSeconds: Number(event.target.value) } : action) })} />}</label>)}</div></div>
      </div>
      <footer className="rule-dialog-footer"><div>{rule.id > 0 && <button className="secondary" title="删除当前规则" onClick={onDelete} disabled={saving}><Trash2 size={15} />删除</button>}</div><div className="button-row"><button className="secondary" onClick={onClose} disabled={saving}>取消</button><button className="primary" title="保存当前规则配置" onClick={onSave} disabled={saving}><Save size={15} />{saving ? '保存中' : '保存规则'}</button></div></footer>
    </section>
  </div>
}

function download(name: string, content: string) { const blob = new Blob([content], { type: 'application/json;charset=utf-8' }); const url = URL.createObjectURL(blob); const link = document.createElement('a'); link.href = url; link.download = name; link.click(); URL.revokeObjectURL(url) }
