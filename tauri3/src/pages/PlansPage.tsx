import { useEffect, useState } from 'react'
import {
  CalendarClock,
  CalendarDays,
  Check,
  CheckCircle2,
  Clock3,
  Eye,
  Plus,
  RefreshCw,
  Save,
  Send,
  Sparkles,
  Trash2,
  X,
} from 'lucide-react'
import { api, readableError } from '../api/client'
import { EmptyState, PageFeedback, SectionHeading } from '../components/PageState'
import type {
  Activity,
  ActivityPreview,
  ActivityRun,
  DailySummary,
  Group,
  LoadState,
  Schedule,
  ScheduleRun,
  SummarySettings,
} from '../types'

type Tab = 'activities' | 'schedules' | 'summaries'

const localTimezone = () => Intl.DateTimeFormat().resolvedOptions().timeZone || 'Asia/Shanghai'

function localDate(offsetDays = 0) {
  const value = new Date()
  value.setDate(value.getDate() + offsetDays)
  const pad = (part: number) => String(part).padStart(2, '0')
  return `${value.getFullYear()}-${pad(value.getMonth() + 1)}-${pad(value.getDate())}`
}

const newActivity = (accountId: string, groupId: number): Activity => ({
  id: 0,
  accountId,
  name: '',
  content: '',
  enabled: false,
  aiOptimize: false,
  aiInstructions: '',
  timezone: localTimezone(),
  startDate: localDate(),
  endDate: localDate(7),
  weekdays: [1, 2, 3, 4, 5, 6, 7],
  sendTimes: ['09:00'],
  groupIds: groupId ? [groupId] : [],
  nextRunAt: null,
  sourceKey: '',
  deletedAt: null,
  createdAt: new Date().toISOString(),
  updatedAt: new Date().toISOString(),
})

const newSchedule = (accountId: string): Schedule => ({
  id: 0,
  accountId,
  name: '每日群发言',
  enabled: false,
  openTime: '08:00',
  closeTime: '22:00',
  timezone: localTimezone(),
  groupIds: [],
})

export default function PlansPage({
  accountId,
  groups,
  onError,
}: {
  accountId: string
  groups: Group[]
  onError: (value: string) => void
}) {
  const [tab, setTab] = useState<Tab>('activities')
  const [activities, setActivities] = useState<Activity[]>([])
  const [activityRuns, setActivityRuns] = useState<ActivityRun[]>([])
  const [schedules, setSchedules] = useState<Schedule[]>([])
  const [scheduleRuns, setScheduleRuns] = useState<ScheduleRun[]>([])
  const [summaries, setSummaries] = useState<DailySummary[]>([])
  const [summaryGroup, setSummaryGroup] = useState(0)
  const [summaryBusy, setSummaryBusy] = useState(false)
  const [activityBusy, setActivityBusy] = useState(false)
  const [activityPreview, setActivityPreview] = useState<ActivityPreview | null>(null)
  const [summarySettings, setSummarySettings] = useState<SummarySettings>({
    accountId: '',
    enabled: false,
    time: '23:00',
    groupIds: [],
    timezone: localTimezone(),
  })
  const [editingActivity, setEditingActivity] = useState<Activity | null>(null)
  const [editingSchedule, setEditingSchedule] = useState<Schedule | null>(null)
  const [state, setState] = useState<LoadState>('idle')

  const reload = async (selection?: { activityId?: number; scheduleId?: number }) => {
    if (!accountId) {
      setActivities([])
      setSchedules([])
      setSummaries([])
      setState('empty')
      return
    }
    setState('loading')
    try {
      const [nextActivities, nextSchedules, nextSummaries, nextSummarySettings] = await Promise.all([
        api.listActivities(accountId),
        api.listSchedules(accountId),
        api.listSummaries(accountId),
        api.getSummarySettings(accountId),
      ])
      setActivities(nextActivities)
      setSchedules(nextSchedules)
      setSummaries(nextSummaries)
      setSummarySettings(nextSummarySettings)
      setEditingActivity(current => {
        if (selection?.activityId) {
          return nextActivities.find(activity => activity.id === selection.activityId)
            || current
            || newActivity(accountId, groups[0]?.groupId || 0)
        }
        return current || nextActivities[0] || newActivity(accountId, groups[0]?.groupId || 0)
      })
      setEditingSchedule(current => {
        if (selection?.scheduleId) {
          return nextSchedules.find(schedule => schedule.id === selection.scheduleId)
            || current
            || newSchedule(accountId)
        }
        return current || nextSchedules[0] || newSchedule(accountId)
      })
      setState(nextActivities.length || nextSchedules.length || nextSummaries.length ? 'ready' : 'empty')
    } catch (reason) {
      onError(readableError(reason))
      setState(activities.length || schedules.length || summaries.length ? 'offlineCached' : 'error')
    }
  }

  useEffect(() => { void reload() }, [accountId])

  useEffect(() => {
    setActivityPreview(null)
    if (!editingActivity?.id) {
      setActivityRuns([])
      return
    }
    void api.listActivityRuns(accountId, editingActivity.id)
      .then(setActivityRuns)
      .catch(reason => onError(readableError(reason)))
  }, [accountId, editingActivity?.id])

  useEffect(() => {
    if (!editingSchedule?.id) {
      setScheduleRuns([])
      return
    }
    void api.listScheduleRuns(accountId, editingSchedule.id)
      .then(setScheduleRuns)
      .catch(reason => onError(readableError(reason)))
  }, [accountId, editingSchedule?.id])

  const activityValidation = (activity: Activity | null) => {
    if (!activity?.name.trim()) return '活动名称不能为空'
    if (!activity.content.trim()) return '活动文案不能为空'
    if (activity.content.trim().length > 1000) return '活动文案不能超过 1000 个字符'
    if (!activity.groupIds.length) return '请至少选择一个活动群'
    if (!activity.weekdays.length) return '请至少选择一个发送星期'
    if (!activity.sendTimes.length) return '请至少添加一个发送时刻'
    if (activity.endDate < activity.startDate) return '活动结束日期不能早于开始日期'
    return ''
  }

  const saveActivity = async () => {
    const validation = activityValidation(editingActivity)
    if (validation) { onError(validation); return }
    setActivityBusy(true)
    try {
      const id = await api.saveActivity(editingActivity!)
      await reload({ activityId: id })
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setActivityBusy(false)
    }
  }

  const removeActivity = async () => {
    if (!editingActivity?.id) return
    setActivityBusy(true)
    try {
      await api.deleteActivity(accountId, editingActivity.id)
      setEditingActivity(null)
      await reload()
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setActivityBusy(false)
    }
  }

  const previewActivity = async () => {
    const validation = activityValidation(editingActivity)
    if (validation) { onError(validation); return }
    const groupId = editingActivity!.groupIds[0]
    setActivityBusy(true)
    try {
      setActivityPreview(await api.previewActivityText(editingActivity!, groupId))
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setActivityBusy(false)
    }
  }

  const publishActivity = async () => {
    if (!editingActivity?.id) return
    const names = groups
      .filter(group => editingActivity.groupIds.includes(group.groupId))
      .map(group => group.name)
      .join('、')
    if (!window.confirm(`立即向 ${names || editingActivity.groupIds.length + ' 个群'} 发布“${editingActivity.name}”？`)) return
    setActivityBusy(true)
    try {
      await api.publishActivityNow(accountId, editingActivity.id)
      setActivityRuns(await api.listActivityRuns(accountId, editingActivity.id))
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setActivityBusy(false)
    }
  }

  const saveSchedule = async () => {
    if (!editingSchedule?.name.trim() || !editingSchedule.groupIds.length) {
      onError('计划名称和使用群不能为空')
      return
    }
    if (editingSchedule.openTime === editingSchedule.closeTime) {
      onError('开群和关群时间不能相同')
      return
    }
    try {
      const id = await api.saveSchedule(editingSchedule)
      await reload({ scheduleId: id })
    } catch (reason) {
      onError(readableError(reason))
    }
  }

  const removeSchedule = async () => {
    if (!editingSchedule?.id) return
    try {
      await api.deleteSchedule(accountId, editingSchedule.id)
      setEditingSchedule(null)
      await reload()
    } catch (reason) {
      onError(readableError(reason))
    }
  }

  const generateSummary = async () => {
    const groupId = summaryGroup || groups[0]?.groupId
    if (!groupId) { onError('请先选择群聊'); return }
    setSummaryBusy(true)
    try {
      await api.generateSummary(accountId, groupId)
      await reload()
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setSummaryBusy(false)
    }
  }

  const saveSummarySettings = async () => {
    if (summarySettings.enabled && !summarySettings.groupIds.length) {
      onError('启用每日摘要前请至少选择一个群')
      return
    }
    setSummaryBusy(true)
    try {
      await api.saveSummarySettings({ ...summarySettings, accountId })
      await reload()
    } catch (reason) {
      onError(readableError(reason))
    } finally {
      setSummaryBusy(false)
    }
  }

  return <div className="page-stack activity-plans-page"><section className="section">
    <SectionHeading
      eyebrow="群运营"
      title="活动与计划"
      meta={<span className="tabs">
        <button className={tab === 'activities' ? 'active' : ''} onClick={() => setTab('activities')}><CalendarDays size={14} />活动</button>
        <button className={tab === 'schedules' ? 'active' : ''} onClick={() => setTab('schedules')}><CalendarClock size={14} />开关群计划</button>
        <button className={tab === 'summaries' ? 'active' : ''} onClick={() => setTab('summaries')}><CheckCircle2 size={14} />每日摘要</button>
      </span>}
      actions={<button className="secondary" onClick={() => void reload()}><RefreshCw size={15} />刷新</button>}
    />
    <PageFeedback
      state={state === 'empty' ? 'ready' : state}
      error="活动与计划读取失败"
      retry={() => void reload()}
      emptyTitle="暂无活动与计划"
      emptyDetail="当前账号还没有活动配置。"
    >
      {tab === 'activities' && <ActivityPanel
        activities={activities}
        runs={activityRuns}
        editing={editingActivity}
        setEditing={setEditingActivity}
        accountId={accountId}
        groups={groups}
        busy={activityBusy}
        preview={activityPreview}
        onNew={() => { setEditingActivity(newActivity(accountId, groups[0]?.groupId || 0)); setActivityPreview(null) }}
        onSave={() => void saveActivity()}
        onDelete={() => void removeActivity()}
        onPreview={() => void previewActivity()}
        onPublish={() => void publishActivity()}
      />}
      {tab === 'schedules' && <SchedulePanel
        schedules={schedules}
        runs={scheduleRuns}
        editing={editingSchedule}
        setEditing={setEditingSchedule}
        accountId={accountId}
        groups={groups}
        onSave={() => void saveSchedule()}
        onDelete={() => void removeSchedule()}
      />}
      {tab === 'summaries' && <SummaryPanel
        accountId={accountId}
        groups={groups}
        summaries={summaries}
        settings={summarySettings}
        setSettings={setSummarySettings}
        summaryGroup={summaryGroup}
        setSummaryGroup={setSummaryGroup}
        busy={summaryBusy}
        state={state}
        onSave={() => void saveSummarySettings()}
        onGenerate={() => void generateSummary()}
      />}
    </PageFeedback>
  </section></div>
}

function ActivityPanel({
  activities,
  runs,
  editing,
  setEditing,
  accountId,
  groups,
  busy,
  preview,
  onNew,
  onSave,
  onDelete,
  onPreview,
  onPublish,
}: {
  activities: Activity[]
  runs: ActivityRun[]
  editing: Activity | null
  setEditing: (activity: Activity) => void
  accountId: string
  groups: Group[]
  busy: boolean
  preview: ActivityPreview | null
  onNew: () => void
  onSave: () => void
  onDelete: () => void
  onPreview: () => void
  onPublish: () => void
}) {
  const weekdays = [[1, '一'], [2, '二'], [3, '三'], [4, '四'], [5, '五'], [6, '六'], [7, '日']] as const
  const update = (patch: Partial<Activity>) => editing && setEditing({ ...editing, ...patch })
  const toggleGroup = (groupId: number) => update({
    groupIds: editing?.groupIds.includes(groupId)
      ? editing.groupIds.filter(id => id !== groupId)
      : [...(editing?.groupIds || []), groupId],
  })
  const toggleWeekday = (weekday: number) => update({
    weekdays: editing?.weekdays.includes(weekday)
      ? editing.weekdays.filter(value => value !== weekday)
      : [...(editing?.weekdays || []), weekday].sort(),
  })
  const updateTime = (index: number, value: string) => update({
    sendTimes: editing?.sendTimes.map((time, current) => current === index ? value : time),
  })

  return <div className="work-layout activity-layout">
    <div className="work-list activity-list">
      {activities.map(activity => <button
        className={`work-item ${editing?.id === activity.id ? 'selected' : ''}`}
        key={activity.id}
        onClick={() => setEditing(activity)}
      >
        {activity.aiOptimize ? <Sparkles size={15} /> : <CalendarDays size={15} />}
        <span>
          <strong>{activity.name}</strong>
          <small>{activity.startDate} 至 {activity.endDate} · {activity.sendTimes.join(' / ')}</small>
        </span>
        <em className={activity.enabled ? 'enabled' : ''}>{activity.enabled ? '运行中' : '停用'}</em>
      </button>)}
      <button className="new-line" onClick={onNew}><Plus size={15} />新建活动</button>
    </div>
    {editing ? <div className="editor-panel activity-editor">
      <div className="editor-title">
        <div><span className="eyebrow">活动配置</span><h3>{editing.name || '未命名活动'}</h3></div>
        <div className="button-row">
          {editing.id > 0 && <button className="secondary" disabled={busy} onClick={onDelete}><Trash2 size={15} />删除</button>}
          {editing.id > 0 && <button className="secondary" disabled={busy} onClick={onPublish}><Send size={15} />立即发布</button>}
          <button className="secondary" disabled={busy} onClick={onPreview}><Eye size={15} />预览文案</button>
          <button className="primary" disabled={busy} onClick={onSave}><Save size={15} />保存活动</button>
        </div>
      </div>

      <div className="activity-switches">
        <label className={`activity-toggle ${editing.enabled ? 'checked' : ''}`}>
          <input type="checkbox" checked={editing.enabled} onChange={event => update({ enabled: event.target.checked })} />
          <span><strong>启用活动</strong><small>{editing.nextRunAt ? `下次 ${new Date(editing.nextRunAt).toLocaleString('zh-CN')}` : '当前未排期'}</small></span>
          <i aria-hidden="true"><b /></i>
        </label>
        <label className={`activity-toggle ${editing.aiOptimize ? 'checked' : ''}`}>
          <input type="checkbox" checked={editing.aiOptimize} onChange={event => update({ aiOptimize: event.target.checked })} />
          <span><strong>使用 AI 优化</strong><small>{editing.aiOptimize ? '每群每次独立生成' : '按原文发布'}</small></span>
          <i aria-hidden="true"><b /></i>
        </label>
      </div>

      <div className="form-grid activity-form">
        <label>活动名称<input value={editing.name} maxLength={100} onChange={event => update({ name: event.target.value })} /></label>
        <label>时区<input value={editing.timezone} onChange={event => update({ timezone: event.target.value })} /></label>
        <label>开始日期<input type="date" value={editing.startDate} onChange={event => update({ startDate: event.target.value })} /></label>
        <label>结束日期<input type="date" value={editing.endDate} onChange={event => update({ endDate: event.target.value })} /></label>
        <label className="wide-field">活动原文<textarea value={editing.content} maxLength={1000} onChange={event => update({ content: event.target.value })} /></label>
        {editing.aiOptimize && <label className="wide-field">AI 风格要求<input value={editing.aiInstructions} maxLength={200} onChange={event => update({ aiInstructions: event.target.value })} /></label>}

        <fieldset className="activity-fieldset wide-field">
          <legend>发送星期</legend>
          <div className="weekday-picker">{weekdays.map(([value, label]) => <label className={editing.weekdays.includes(value) ? 'selected' : ''} key={value}>
            <input type="checkbox" checked={editing.weekdays.includes(value)} onChange={() => toggleWeekday(value)} />周{label}
          </label>)}</div>
        </fieldset>

        <fieldset className="activity-fieldset wide-field">
          <legend>发送时刻</legend>
          <div className="activity-time-list">{editing.sendTimes.map((time, index) => <div key={`${index}-${time}`}>
            <Clock3 size={14} />
            <input aria-label={`发送时刻 ${index + 1}`} type="time" value={time} onChange={event => updateTime(index, event.target.value)} />
            <button className="icon-button" title="移除时刻" disabled={editing.sendTimes.length === 1} onClick={() => update({ sendTimes: editing.sendTimes.filter((_, current) => current !== index) })}><X size={14} /></button>
          </div>)}<button className="secondary" onClick={() => update({ sendTimes: [...editing.sendTimes, '12:00'] })}><Plus size={14} />添加时刻</button></div>
        </fieldset>

        <fieldset className="activity-fieldset wide-field">
          <legend>发布群</legend>
          <div className="group-filter activity-group-picker">{groups.map(group => <label className={`check-chip ${editing.groupIds.includes(group.groupId) ? 'selected' : ''}`} key={group.groupId}>
            <input type="checkbox" checked={editing.groupIds.includes(group.groupId)} onChange={() => toggleGroup(group.groupId)} />
            <Check size={13} />{group.name}
          </label>)}</div>
        </fieldset>
      </div>

      {preview && <div className="activity-preview">
        <div><span className="eyebrow">发送预览</span><em>{preview.source === 'ai' ? 'AI 文案' : preview.source === 'ai-fallback' ? '原文回退' : '规则原文'}</em></div>
        <p>{preview.text}</p>
      </div>}

      <ActivityHistory runs={runs} groups={groups} />
    </div> : <EmptyState title="选择活动" detail={`当前账号 ${accountId ? '可新建活动' : '尚未连接'}`} />}
  </div>
}

function ActivityHistory({ runs, groups }: { runs: ActivityRun[]; groups: Group[] }) {
  if (!runs.length) return null
  const stateLabels: Record<string, string> = {
    pending: '待处理', preparing: '生成中', queued: '待发送', succeeded: '成功',
    failed: '失败', unknown: '待确认', missed: '已错过', retry: '重试中',
  }
  return <div className="activity-history">
    <span className="eyebrow">最近发布</span>
    <div className="activity-run-table">{runs.slice(0, 12).map(run => <div key={run.id}>
      <time>{new Date(run.scheduledFor).toLocaleString('zh-CN')}</time>
      <span>{groups.find(group => group.groupId === run.groupId)?.name || `群 ${run.groupId}`}</span>
      <small>{run.contentSource === 'ai' ? 'AI' : run.contentSource === 'ai-fallback' ? '原文回退' : '原文'}</small>
      <em className={run.state === 'succeeded' ? 'enabled' : run.state === 'failed' || run.state === 'unknown' ? 'failed' : ''}>{stateLabels[run.state] || run.state}</em>
    </div>)}</div>
  </div>
}

function SchedulePanel({
  schedules, runs, editing, setEditing, accountId, groups, onSave, onDelete,
}: {
  schedules: Schedule[]
  runs: ScheduleRun[]
  editing: Schedule | null
  setEditing: (value: Schedule) => void
  accountId: string
  groups: Group[]
  onSave: () => void
  onDelete: () => void
}) {
  const update = (patch: Partial<Schedule>) => editing && setEditing({ ...editing, ...patch })
  return <div className="work-layout">
    <div className="work-list">{schedules.map(schedule => <button className={`work-item ${editing?.id === schedule.id ? 'selected' : ''}`} key={schedule.id} onClick={() => setEditing(schedule)}>
      <CalendarClock size={15} /><span><strong>{schedule.name}</strong><small>每日 {schedule.openTime} 开群 / {schedule.closeTime} 关群</small></span><em className={schedule.enabled ? 'enabled' : ''}>{schedule.enabled ? '启用' : '停用'}</em>
    </button>)}<button className="new-line" onClick={() => setEditing(newSchedule(accountId))}><Plus size={15} />新建计划</button></div>
    {editing ? <div className="editor-panel">
      <div className="editor-title"><div><span className="eyebrow">群发言计划</span><h3>{editing.name || '未命名计划'}</h3></div><div className="button-row">
        {editing.id > 0 && <button className="secondary" onClick={onDelete}><Trash2 size={15} />删除</button>}
        <button className="primary" onClick={onSave}><Save size={15} />保存计划</button>
      </div></div>
      <div className="form-grid">
        <label>计划名称<input value={editing.name} onChange={event => update({ name: event.target.value })} /></label>
        <label>时区<input value={editing.timezone} onChange={event => update({ timezone: event.target.value })} /></label>
        <label>每日开群<input type="time" value={editing.openTime} onChange={event => update({ openTime: event.target.value })} /></label>
        <label>每日关群<input type="time" value={editing.closeTime} onChange={event => update({ closeTime: event.target.value })} /></label>
        <label className="switch-field"><input type="checkbox" checked={editing.enabled} onChange={event => update({ enabled: event.target.checked })} /><span>启用计划</span></label>
        <div className="group-filter wide-field"><span>绑定群</span>{groups.map(group => <label className={`check-chip ${editing.groupIds.includes(group.groupId) ? 'selected' : ''}`} key={group.groupId}>
          <input type="checkbox" checked={editing.groupIds.includes(group.groupId)} onChange={() => update({ groupIds: editing.groupIds.includes(group.groupId) ? editing.groupIds.filter(id => id !== group.groupId) : [...editing.groupIds, group.groupId] })} />{group.name}
        </label>)}</div>
      </div>
      {runs.length > 0 && <div className="schedule-runs"><span className="eyebrow">最近执行</span>{runs.slice(0, 6).map(run => <div className="schedule-run" key={run.id}><span>{run.localDate} · {run.action === 'open' ? '开群' : '关群'}</span><em className={run.success ? 'enabled' : ''}>{run.success ? '成功' : `失败${run.error ? `：${run.error}` : ''}`}</em></div>)}</div>}
    </div> : <EmptyState title="选择计划" detail="当前没有开关群计划。" />}
  </div>
}

function SummaryPanel({
  accountId, groups, summaries, settings, setSettings, summaryGroup, setSummaryGroup, busy, state, onSave, onGenerate,
}: {
  accountId: string
  groups: Group[]
  summaries: DailySummary[]
  settings: SummarySettings
  setSettings: (settings: SummarySettings) => void
  summaryGroup: number
  setSummaryGroup: (groupId: number) => void
  busy: boolean
  state: LoadState
  onSave: () => void
  onGenerate: () => void
}) {
  return <div>
    <div className="summary-settings">
      <label className="switch-field"><input type="checkbox" checked={settings.enabled} onChange={event => setSettings({ ...settings, enabled: event.target.checked })} /><span>启用每日自动摘要</span></label>
      <label>生成时间<input type="time" value={settings.time} onChange={event => setSettings({ ...settings, time: event.target.value })} /></label>
      <div className="group-filter"><span>摘要群</span>{groups.map(group => <label className={`check-chip ${settings.groupIds.includes(group.groupId) ? 'selected' : ''}`} key={group.groupId}>
        <input type="checkbox" checked={settings.groupIds.includes(group.groupId)} onChange={() => setSettings({ ...settings, accountId, groupIds: settings.groupIds.includes(group.groupId) ? settings.groupIds.filter(id => id !== group.groupId) : [...settings.groupIds, group.groupId] })} />{group.name}
      </label>)}</div>
      <button className="secondary" disabled={busy} onClick={onSave}><Save size={15} />保存摘要计划</button>
    </div>
    <div className="summary-toolbar">
      <label>手动选择群<select value={summaryGroup || groups[0]?.groupId || 0} onChange={event => setSummaryGroup(Number(event.target.value))}>{groups.map(group => <option value={group.groupId} key={group.groupId}>{group.name}</option>)}</select></label>
      <button className="primary" disabled={busy || !groups.length} onClick={onGenerate}>{busy ? '生成中' : '生成今日摘要'}</button>
    </div>
    <PageFeedback state={state === 'loading' ? 'loading' : summaries.length ? 'ready' : 'empty'} emptyTitle="暂无每日摘要" emptyDetail="当前日期还没有摘要。">
      <div className="summary-list">{summaries.map(summary => <article className="summary-item" key={summary.id}><div><strong>{groups.find(group => group.groupId === summary.groupId)?.name || `群 ${summary.groupId}`}</strong><time>{summary.localDate}</time></div><p>{summary.content}</p></article>)}</div>
    </PageFeedback>
  </div>
}
