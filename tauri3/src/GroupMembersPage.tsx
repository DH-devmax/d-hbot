import React, { useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { Ban, Bot, Check, ChevronLeft, ChevronRight, Hand, Megaphone, Pencil, RefreshCw, Search, Shield, Sparkles, UserMinus, Users, Volume2, VolumeX, X } from 'lucide-react'
import './members.css'

type GroupSummary = {
  accountId: string
  groupId: number
  name: string
  enabled: boolean
  aiEnabled: boolean
  moderationEnabled: boolean
  manualTakeover: boolean
  welcomeMessage?: string
}

type Member = {
  groupId: number
  userId: number
  nimId: string
  nickname: string
  cardName: string
  role: string
  accountState: string
  blacklisted: boolean
  present: boolean
  originalCardName: string
  managedCardName: string
  cardSuffix: string
}

type MemberRoster = {
  members: Member[]
  reportedCount: number
  resolvedCount: number
  complete: boolean
  status?: 'loading' | 'cached' | 'partial' | 'rate-limited' | 'retrying' | 'ready' | 'error' | string
  completenessReason?: string
  sources: string[]
  sourceErrors?: Array<{ source: string; route: string; page: number; cursor?: string; reason: string }>
  retryAt?: string
  canonicalCount?: number
  syntheticUserIds?: number[]
  httpPages?: number
  nimPages?: number
}

type MemberRosterStatus = Pick<MemberRoster, 'status' | 'reportedCount' | 'resolvedCount' | 'complete' | 'completenessReason' | 'retryAt' | 'canonicalCount' | 'sourceErrors'> & {
  accountId?: string
  groupId: number
}

type CardRenameJob = {
  id: number
  userId: number
  desiredName: string
  state: 'queued' | 'processing' | 'retry' | 'failed' | 'succeeded'
  attempts: number
  nextAttemptAt?: string
  lastError: string
  welcomePending: boolean
}

type AiAutomationSettings = {
  enabled: boolean
  reply: boolean
  tasks: boolean
  recall: boolean
  mute: boolean
  remove: boolean
  manualTakeover: boolean
}

type MemberBatchResult = {
  userId?: number
  nimId?: string
  success: boolean
  error: string
}

type GroupManagementContext = {
  groupId: number
  isManager: boolean
  announcementStatus: string
}

type GroupAnnouncement = {
  groupId: number
  noticeId: string
  content: string
  mode: string
  authorUserId: number
}

type GroupBatchMode = 'announcement' | 'mute' | 'unmute' | null

type GroupBatchResult = {
  groupId: number
  success: boolean
  status: string
  requestId: string
  messageId: string
  error: string
}

type GatewayCapabilities = {
  announcement?: string
  groupMute?: string
}

type Props = {
  groups: GroupSummary[]
  selectedGroup: number | null
  setSelectedGroup: (value: number) => void
  activeGroup?: GroupSummary
  onError: (message: string) => void
  refresh: () => Promise<void>
}

const roleLabels: Record<string, string> = {
  owner: '群主',
  admin: '管理员',
  member: '群员',
}

const stateLabels: Record<string, string> = {
  ACCOUNT_STATE_GOOD: '正常',
  ACCOUNT_STATE_BAN: '已封禁',
  ACCOUNT_STATE_CANCEL: '已注销',
  ACCOUNT_STATE_CANCELED: '已注销',
}

function displayName(member: Member) {
  return member.cardName || member.nickname || '未获得名称'
}

function actionError(reason: unknown) {
  if (typeof reason === 'string') return reason
  if (reason && typeof reason === 'object' && 'message' in reason) return String(reason.message)
  return String(reason)
}

function actionCode(reason: unknown) {
  return reason && typeof reason === 'object' && 'code' in reason ? String(reason.code) : ''
}

function AutomationToggle({ title, detail, checked, disabled, danger, onChange }: { title: string; detail: string; checked: boolean; disabled: boolean; danger?: boolean; onChange: () => void }) {
  return <button data-help={`${title}：${detail}。点击后${checked ? '关闭' : '开启'}这项权限。`} className={`automation-toggle ${checked ? 'checked' : ''} ${danger ? 'danger' : ''}`} role="switch" aria-checked={checked} disabled={disabled} onClick={onChange}><span><strong>{title}</strong><small>{detail}</small></span><i aria-hidden="true"><b /></i></button>
}

export default function GroupMembersPage({ groups, selectedGroup, setSelectedGroup, activeGroup, onError, refresh }: Props) {
  const [groupSearch, setGroupSearch] = useState('')
  const [memberSearch, setMemberSearch] = useState('')
  const [roleFilter, setRoleFilter] = useState('all')
  const [memberPage, setMemberPage] = useState(0)
  const [muteSeconds, setMuteSeconds] = useState(600)
  const [roster, setRoster] = useState<MemberRoster | null>(null)
  const [memberLoading, setMemberLoading] = useState(false)
  const [activeAction, setActiveAction] = useState('')
  const [selected, setSelected] = useState<Set<number>>(new Set())
  const [viewMode, setViewMode] = useState<'groups' | 'members'>('groups')
  const [cardPrefix, setCardPrefix] = useState('DH')
  const [autoRename, setAutoRename] = useState(false)
  const [cardsPaused, setCardsPaused] = useState(false)
  const [cardPreview, setCardPreview] = useState<any | null>(null)
  const [cardJobs, setCardJobs] = useState<CardRenameJob[]>([])
  const [controlTab, setControlTab] = useState<'manual' | 'ai' | 'rules'>('manual')
  const [aiAutomation, setAiAutomation] = useState<AiAutomationSettings>({ enabled: false, reply: false, tasks: true, recall: false, mute: false, remove: false, manualTakeover: false })
  const [welcomeMessage, setWelcomeMessage] = useState('欢迎 @「[成员]」加入群聊，请先查看群规。')
  const [announcementText, setAnnouncementText] = useState('')
  const [announcementStatus, setAnnouncementStatus] = useState('正在检测')
  const [groupBatchMode, setGroupBatchMode] = useState<GroupBatchMode>(null)
  const [selectedGroupIds, setSelectedGroupIds] = useState<Set<number>>(new Set())
  const [batchAnnouncementText, setBatchAnnouncementText] = useState('')
  const [groupBatchResults, setGroupBatchResults] = useState<GroupBatchResult[] | null>(null)
  const [groupBatchDialogOpen, setGroupBatchDialogOpen] = useState(false)
  const [gatewayCapabilities, setGatewayCapabilities] = useState<GatewayCapabilities | null>(null)
  const memberRequestGeneration = useRef(0)

  const filteredGroups = useMemo(() => {
    const query = groupSearch.trim().toLowerCase()
    return query ? groups.filter(group => group.name.toLowerCase().includes(query)) : groups
  }, [groupSearch, groups])

  const visibleMembers = useMemo(() => {
    const query = memberSearch.trim().toLowerCase()
    return (roster?.members ?? []).filter(member => {
      if (roleFilter !== 'all' && member.role !== roleFilter) return false
      if (!query) return true
      return [member.cardName, member.nickname]
        .some(value => value.toLowerCase().includes(query))
    })
  }, [memberSearch, roleFilter, roster])

  const memberPageSize = 50
  const memberPageCount = Math.max(1, Math.ceil(visibleMembers.length / memberPageSize))
  useEffect(() => { setMemberPage(0) }, [memberSearch, roleFilter, roster])
  useEffect(() => { if (memberPage > memberPageCount - 1) setMemberPage(memberPageCount - 1) }, [memberPage, memberPageCount])
  useEffect(() => {
    void invoke<GatewayCapabilities>('get_gateway_capabilities').then(setGatewayCapabilities).catch(() => setGatewayCapabilities(null))
  }, [])

  useEffect(() => {
    if (!selectedGroup) return
    let disposed = false
    const unlisten = listen<MemberRosterStatus>('member-roster-status', event => {
      const payload = event.payload
      if (disposed || payload.groupId !== selectedGroup || (payload.accountId && payload.accountId !== activeGroup?.accountId)) return
      setRoster(previous => ({
        ...(previous || {
          members: [],
          reportedCount: payload.reportedCount ?? 0,
          resolvedCount: payload.resolvedCount ?? 0,
          complete: Boolean(payload.complete),
          sources: [],
        }),
        ...payload,
      }))
    }).catch(() => undefined)
    return () => {
      disposed = true
      void unlisten.then(cleanup => cleanup?.())
    }
  }, [selectedGroup, activeGroup?.accountId])

  const pagedMembers = useMemo(
    () => visibleMembers.slice(memberPage * memberPageSize, memberPage * memberPageSize + memberPageSize),
    [visibleMembers, memberPage],
  )

  const loadMembers = async (groupId = selectedGroup) => {
    if (!groupId) return
    const generation = ++memberRequestGeneration.current
    onError('')
    setRoster(previous => previous ? { ...previous, status: 'loading', completenessReason: '正在获取旺商聊当前成员' } : {
      members: [],
      reportedCount: 0,
      resolvedCount: 0,
      complete: false,
      status: 'loading',
      completenessReason: '正在获取旺商聊当前成员',
      sources: [],
      canonicalCount: 0,
      syntheticUserIds: [],
    })
    let hasCachedMembers = false
    if (activeGroup?.accountId) {
      try {
        const cached = await invoke<Member[]>('local_members', { accountId: activeGroup.accountId, groupId })
        if (cached.length) {
          hasCachedMembers = true
          if (generation !== memberRequestGeneration.current) return
          setRoster({ members: cached, reportedCount: cached.length, resolvedCount: cached.length, complete: false, status: 'cached', completenessReason: '已显示上次名单，正在获取当前成员', sources: ['local-cache'], canonicalCount: cached.filter(member => member.userId > 0).length, syntheticUserIds: [] })
          setSelected(new Set())
        }
      } catch (reason) {
        if (!hasCachedMembers) onError(actionError(reason))
      }
    }
    setMemberLoading(true)
    try {
      const next = await invoke<MemberRoster>('list_members', { groupId, refresh: true })
      if (generation !== memberRequestGeneration.current) return
      setRoster(next)
      setSelected(new Set())
    } catch (reason) {
      const detail = actionError(reason)
      const status = actionCode(reason) === 'gateway_rate_limited' ? 'rate-limited' : 'error'
      if (generation !== memberRequestGeneration.current) return
      setRoster(previous => previous ? { ...previous, status, completenessReason: detail } : {
        members: [],
        reportedCount: 0,
        resolvedCount: 0,
        complete: false,
        status,
        completenessReason: detail,
        sources: [],
        canonicalCount: 0,
        syntheticUserIds: [],
      })
      if (!hasCachedMembers) onError(detail)
    } finally {
      if (generation === memberRequestGeneration.current) setMemberLoading(false)
    }
  }

  const loadCardJobs = async (groupId = selectedGroup) => {
    if (!groupId || !activeGroup?.accountId) return
    try {
      const jobs = await invoke<CardRenameJob[]>('list_card_rename_jobs', { accountId: activeGroup.accountId, groupId, limit: 200 })
      setCardJobs(jobs)
    } catch (reason) {
      onError(actionError(reason))
    }
  }

  useEffect(() => {
    setRoster(null)
    setMemberSearch('')
    setRoleFilter('all')
    setSelected(new Set())
    setAnnouncementStatus('正在检测')
    setAnnouncementText('')
    if (selectedGroup) {
      void loadMembers(selectedGroup)
      void (async () => {
        try {
          const [cardSettings, aiSettings] = await Promise.all([
            invoke<{ prefix: string; autoRename: boolean; paused: boolean }>('get_card_settings', { accountId: activeGroup?.accountId || '', groupId: selectedGroup }),
            invoke<AiAutomationSettings>('get_ai_automation_settings', { accountId: activeGroup?.accountId || '', groupId: selectedGroup }),
          ])
          setCardPrefix(cardSettings.prefix || 'DH')
          setAutoRename(cardSettings.autoRename)
          setCardsPaused(cardSettings.paused)
          setAiAutomation(aiSettings)
          try {
            const management = await invoke<GroupManagementContext>('get_group_management_context', { groupId: selectedGroup })
            setAnnouncementStatus(management.announcementStatus)
          } catch {
            setAnnouncementStatus('当前版本待校准')
          }
          try {
            const announcement = await invoke<GroupAnnouncement | null>('get_group_announcement', { groupId: selectedGroup })
            setAnnouncementText(announcement?.content || '')
          } catch {
            setAnnouncementText('')
          }
        } catch (reason) {
          onError(actionError(reason))
        }
      })()
      setWelcomeMessage(activeGroup?.welcomeMessage || '欢迎 @「[成员]」加入群聊，请先查看群规。')
    }
  }, [selectedGroup])

  useEffect(() => {
    if (!selectedGroup || !activeGroup?.accountId) return
    void loadCardJobs(selectedGroup)
    const timer = window.setInterval(() => void loadCardJobs(selectedGroup), 3000)
    return () => window.clearInterval(timer)
  }, [selectedGroup, activeGroup?.accountId])

  const toggleModeration = async () => {
    if (!activeGroup) return
    const next = { ...activeGroup, moderationEnabled: !activeGroup.moderationEnabled }
    setActiveAction('feature:moderation')
    onError('')
    try {
      await invoke('set_group_features', { accountId: next.accountId, groupId: next.groupId, enabled: next.enabled, aiEnabled: next.aiEnabled, moderationEnabled: next.moderationEnabled, manualTakeover: next.manualTakeover })
      await refresh()
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }

  const saveAiAutomation = async (patch: Partial<AiAutomationSettings>) => {
    if (!activeGroup) return
    const next = { ...aiAutomation, ...patch }
    if (patch.remove === true && !window.confirm('开启后，AI 返回“移出成员”动作时会直接执行。确认允许 AI 使用此权限？')) return
    setActiveAction('ai-automation')
    onError('')
    try {
      await invoke('save_ai_automation_settings', { settings: {
        accountId: activeGroup.accountId,
        groupId: activeGroup.groupId,
        ...next,
      } })
      setAiAutomation(next)
      await refresh()
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }

  const toggleGroupMute = async (muted: boolean) => {
    if (!activeGroup || !window.confirm(muted ? '确认开启当前群全员禁言？' : '确认解除当前群全员禁言？')) return
    setActiveAction('group-mute')
    onError('')
    try {
      await invoke('set_group_mute', { groupId: activeGroup.groupId, muted })
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }

  const saveAnnouncement = async () => {
    if (!activeGroup || !announcementText.trim()) return
    if (!window.confirm('确认发布或更新当前群公告？保存后群内会出现一条新的公告消息。')) return
    setActiveAction('announcement')
    onError('')
    try {
      await invoke('set_group_announcement', { groupId: activeGroup.groupId, text: announcementText.trim() })
      onError('群公告已保存并发送到当前群')
      const latest = await invoke<GroupAnnouncement | null>('get_group_announcement', { groupId: activeGroup.groupId })
      setAnnouncementText(latest?.content || announcementText.trim())
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }

  const optimizeAnnouncement = async () => {
    const source = announcementText.trim()
    if (!source) return
    setActiveAction('announcement:ai')
    onError('')
    try {
      const result = await invoke<{ decision: { reply?: string } }>('test_ai', {
        message: `请优化下面这段群公告。保留原有事实、日期、金额、规则和链接，不补充未经提供的信息；语气自然、清楚、简洁，适合直接发布到群里。只把优化后的完整公告写入 reply。\n\n原公告：\n${source}`,
        recentContext: [],
      })
      const optimized = result.decision.reply?.trim()
      if (!optimized) throw new Error('AI 没有返回可用的公告内容')
      setAnnouncementText(optimized)
      onError('AI 已优化公告，请确认内容后再发布')
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }

  const groupBatchCapability = groupBatchMode === 'announcement'
    ? gatewayCapabilities?.announcement
    : groupBatchMode
      ? gatewayCapabilities?.groupMute
      : undefined
  const groupBatchBlocked = Boolean(groupBatchMode && groupBatchCapability && groupBatchCapability !== 'supported')
  const groupBatchCapabilityLabel = groupBatchCapability === 'unverified' ? '当前旺商聊版本尚未完成能力校准' : groupBatchCapability === 'unsupported' ? '当前旺商聊版本不支持此功能' : ''
  const enterGroupBatch = (mode: Exclude<GroupBatchMode, null>) => {
    setGroupBatchMode(mode)
    setSelectedGroupIds(new Set())
    setGroupBatchResults(null)
    setGroupBatchDialogOpen(false)
    if (mode === 'announcement') setBatchAnnouncementText(announcementText)
  }
  const toggleGroupSelection = (groupId: number) => {
    setSelectedGroupIds(previous => {
      const next = new Set(previous)
      if (next.has(groupId)) next.delete(groupId)
      else next.add(groupId)
      return next
    })
  }
  const allFilteredGroupsSelected = filteredGroups.length > 0 && filteredGroups.every(group => selectedGroupIds.has(group.groupId))
  const toggleAllFilteredGroups = () => {
    setSelectedGroupIds(previous => {
      const next = new Set(previous)
      if (allFilteredGroupsSelected) filteredGroups.forEach(group => next.delete(group.groupId))
      else filteredGroups.forEach(group => next.add(group.groupId))
      return next
    })
  }
  const closeGroupBatch = () => {
    setGroupBatchMode(null)
    setSelectedGroupIds(new Set())
    setGroupBatchResults(null)
    setGroupBatchDialogOpen(false)
  }
  const runGroupBatch = async (retryIds?: number[]) => {
    if (!groupBatchMode) return
    const ids = retryIds?.length ? retryIds : Array.from(selectedGroupIds)
    if (!ids.length) {
      onError('请先选择至少一个群')
      return
    }
    const text = batchAnnouncementText.trim()
    if (groupBatchMode === 'announcement' && !text) {
      onError('请先填写公告内容')
      return
    }
    if (groupBatchBlocked) {
      onError(groupBatchCapabilityLabel || '当前能力尚未校准')
      return
    }
    const names = ids.map(id => groups.find(group => group.groupId === id)?.name || `群 ${id}`).join('、')
    const label = groupBatchMode === 'announcement' ? '发布公告' : groupBatchMode === 'mute' ? '开启全员禁言' : '解除全禁'
    if (!window.confirm(`确认对以下 ${ids.length} 个群${label}？\n${names}`)) return
    setActiveAction('group-batch')
    onError('')
    try {
      const results = await invoke<GroupBatchResult[]>('execute_group_batch', { input: { action: groupBatchMode, groupIds: ids, text: groupBatchMode === 'announcement' ? text : null } })
      setGroupBatchResults(results)
      const succeeded = results.filter(result => result.success).length
      const failed = results.length - succeeded
      const failedIds = new Set(results.filter(result => !result.success && result.status !== 'unknown').map(result => result.groupId))
      setSelectedGroupIds(failedIds)
      onError(failed ? `${label}完成：成功 ${succeeded} 个，失败 ${failed} 个。失败群已保留，可检查后重试。` : `${label}完成：${succeeded} 个群全部成功`)
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }
  const optimizeBatchAnnouncement = async () => {
    const source = batchAnnouncementText.trim()
    if (!source) return
    setActiveAction('group-batch:ai')
    onError('')
    try {
      const result = await invoke<{ decision: { reply?: string } }>('test_ai', {
        message: `请优化下面这段群公告。保留原有事实、日期、金额、规则和链接，不补充未经提供的信息；语气自然、清楚、简洁，适合同时发布到多个群。只把优化后的完整公告写入 reply。\n\n原公告：\n${source}`,
        recentContext: [],
      })
      const optimized = result.decision.reply?.trim()
      if (!optimized) throw new Error('AI 没有返回可用的公告内容')
      setBatchAnnouncementText(optimized)
      onError('AI 已优化公告草稿，请确认内容后再发布')
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }

  const saveWelcome = async () => {
    if (!activeGroup) return
    setActiveAction('welcome')
    onError('')
    try {
      await invoke('save_group_welcome', { accountId: activeGroup.accountId, groupId: activeGroup.groupId, welcomeMessage })
      await refresh()
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }

  const saveCards = async (mode: 'auto' | 'pause' | 'prefix') => {
    if (!activeGroup) return
    let nextAuto = autoRename
    let nextPaused = cardsPaused
    if (mode === 'auto') nextAuto = !nextAuto
    if (mode === 'pause') nextPaused = !nextPaused
    if (mode === 'prefix' && !cardPrefix.trim()) {
      onError('前缀不能为空')
      return
    }
    if (mode === 'prefix' && cardPreview?.items?.some((item: any) => item.suffix) && !window.confirm('当前已有固定编号成员，确认同时更新原 DH 前缀吗？四位后缀会保留。')) return
    setActiveAction(`cards:${mode}`)
    onError('')
    try {
      await invoke('save_card_settings', { accountId: activeGroup.accountId, groupId: activeGroup.groupId, prefix: cardPrefix, autoRename: nextAuto, paused: nextPaused })
      setAutoRename(nextAuto)
      setCardsPaused(nextPaused)
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }

  const generateCardPreview = async () => {
    if (!activeGroup) return
    setActiveAction('cards:preview')
    onError('')
    try {
      await loadMembers(selectedGroup)
      const next = await invoke<any>('preview_card_names', { accountId: activeGroup.accountId, groupId: activeGroup.groupId, prefix: cardPrefix })
      setCardPreview(next)
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }

  const applyCardPreview = async () => {
    if (!activeGroup || !cardPreview?.items?.length) return
    if (!window.confirm(`确认执行当前建议？将处理 ${cardPreview.willRename} 名群员。`)) return
    setActiveAction('cards:apply')
    onError('')
    try {
      await invoke('apply_card_names', { groupId: activeGroup.groupId, plans: cardPreview.items })
      await loadCardJobs(selectedGroup)
      await loadMembers(selectedGroup)
      setCardPreview(null)
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }

  const retryFailedCards = async () => {
    if (!activeGroup) return
    setActiveAction('cards:retry')
    onError('')
    try {
      await invoke('retry_card_rename_jobs', { accountId: activeGroup.accountId, groupId: activeGroup.groupId })
      await loadCardJobs(selectedGroup)
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }

  const cleanupInactive = async () => {
    if (!activeGroup || !roster) return
    const candidates = roster.members.filter(member => member.role === 'member' && (member.accountState.includes('BAN') || member.accountState.includes('CANCEL') || ['已封禁用户', '该用户已注销'].includes(displayName(member))))
    if (!candidates.length) {
      onError('当前群没有需要清理的注销或封禁普通成员')
      return
    }
    if (!window.confirm(`发现 ${candidates.length} 名失效普通成员，确认逐个移出？`)) return
    setActiveAction('cleanup')
    onError('')
    try {
      const results = await invoke<MemberBatchResult[]>('execute_member_batch', { input: { groupId: activeGroup.groupId, action: 'remove', members: candidates.map(member => ({ userId: member.userId > 0 ? member.userId : null, nimId: member.nimId || null })) } })
      reportBatchResults(results, '清理')
      await loadMembers(selectedGroup)
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }

  const restoreNames = async () => {
    if (!activeGroup || !roster) return
    const candidates = roster.members.filter(member => member.role === 'member' && (selected.size ? selected.has(member.userId) : true) && member.originalCardName && member.originalCardName !== member.cardName)
    if (!candidates.length) {
      onError('当前群没有可恢复的普通成员名称')
      return
    }
    if (!window.confirm(`确认恢复 ${candidates.length} 名群员的原名称？`)) return
    setActiveAction('restore')
    onError('')
    try {
      for (const member of candidates) await invoke('rename_member', { groupId: activeGroup.groupId, member: { userId: member.userId > 0 ? member.userId : null, nimId: member.nimId || null }, nickname: member.originalCardName })
      await loadMembers(selectedGroup)
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }

  const invokeMemberAction = async (member: Member, kind: 'mute' | 'unmute' | 'rename' | 'remove') => {
    if (!selectedGroup) return
    const name = displayName(member)
    let command = ''
    let payload: Record<string, unknown> = { groupId: selectedGroup, userId: member.userId }
    if (kind === 'mute') {
      if (!window.confirm(`确认禁言“${name}”${Math.round(muteSeconds / 60)}分钟？`)) return
      command = 'mute_member'
      payload.durationSeconds = muteSeconds
    } else if (kind === 'unmute') {
      command = 'unmute_member'
    } else if (kind === 'rename') {
      const nickname = window.prompt(`修改“${name}”的群名片`, name)?.trim()
      if (!nickname || nickname === name) return
      command = 'rename_member'
      payload = { groupId: selectedGroup, member: { userId: member.userId > 0 ? member.userId : null, nimId: member.nimId || null }, nickname }
    } else {
      if (!window.confirm(`确认将“${name}”移出当前群？此操作会立即生效。`)) return
      command = 'remove_member'
    }
    const key = `${kind}:${member.userId}`
    setActiveAction(key)
    onError('')
    try {
      await invoke(command, payload)
      await loadMembers(selectedGroup)
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }

  const reportBatchResults = (results: MemberBatchResult[], label: string) => {
    const failed = results.filter(result => !result.success)
    if (failed.length) onError(`${label}完成：成功 ${results.length - failed.length} 人，失败 ${failed.length} 人。${failed.slice(0, 3).map(result => result.error).filter(Boolean).join('；')}`)
    else onError(`${label}完成：成功 ${results.length} 人`)
  }

  const runBulk = async (kind: 'mute' | 'unmute' | 'remove' | 'blacklist' | 'unblacklist') => {
    const members = (roster?.members ?? []).filter(member => selected.has(member.userId))
    if (!selectedGroup || !members.length) return
    const label = kind === 'mute' ? `禁言 ${Math.round(muteSeconds / 60)} 分钟` : kind === 'unmute' ? '解除禁言' : kind === 'remove' ? '移出群聊' : kind === 'blacklist' ? '加入黑名单' : '移出黑名单'
    if (!window.confirm(`确认对已选择的 ${members.length} 位群员执行“${label}”？`)) return
    setActiveAction(`bulk:${kind}`)
    onError('')
    try {
      const results = await invoke<MemberBatchResult[]>('execute_member_batch', { input: { groupId: selectedGroup, action: kind, durationSeconds: kind === 'mute' ? muteSeconds : null, members: members.map(member => ({ userId: member.userId > 0 ? member.userId : null, nimId: member.nimId || null })) } })
      reportBatchResults(results, label)
      await loadMembers(selectedGroup)
    } catch (reason) {
      onError(actionError(reason))
    } finally {
      setActiveAction('')
    }
  }

  const allVisibleSelected = visibleMembers.length > 0 && visibleMembers.every(member => selected.has(member.userId))
  const toggleAll = () => {
    setSelected(previous => {
      const next = new Set(previous)
      if (allVisibleSelected) visibleMembers.forEach(member => next.delete(member.userId))
      else visibleMembers.forEach(member => next.add(member.userId))
      return next
    })
  }

  return <div className="page-stack group-member-workspace">
    <div className="view-tabs" role="tablist"><button className={viewMode === 'groups' ? 'active' : ''} onClick={() => setViewMode('groups')} role="tab">群组</button><button className={viewMode === 'members' ? 'active' : ''} onClick={() => setViewMode('members')} role="tab" disabled={!activeGroup}>成员</button><span>{activeGroup ? `当前群：${activeGroup.name}` : '请选择群'}</span></div>
    <section className={`group-pane section ${viewMode !== 'groups' ? 'pane-hidden' : ''}`}>
      <div className="section-head"><div><span className="eyebrow">选择管理群</span><h2>群组</h2></div><span className="muted">共 {groups.length} 个群{groupSearch.trim() ? ` · 当前显示 ${filteredGroups.length} 个` : ''}</span></div>
      <label className="search-field"><Search size={15} aria-hidden="true" /><input value={groupSearch} onChange={event => setGroupSearch(event.target.value)} placeholder="搜索群名称" /></label>
      <div className="group-batch-actions" aria-label="批量群控">
        <button className="secondary" data-help={gatewayCapabilities?.announcement === 'unverified' ? '当前旺商聊版本尚未完成群公告协议校准。' : '选择多个群，使用同一份公告内容逐群发布；失败群会单独列出。'} onClick={() => enterGroupBatch('announcement')} disabled={Boolean(activeAction) || gatewayCapabilities?.announcement !== 'supported'}><Megaphone size={14} />批量公告</button>
        <button className="secondary" data-help={gatewayCapabilities?.groupMute === 'unverified' ? '当前旺商聊版本尚未完成全员禁言协议校准。' : '选择多个群，按顺序开启全员禁言；每个群都会单独记录结果。'} onClick={() => enterGroupBatch('mute')} disabled={Boolean(activeAction) || gatewayCapabilities?.groupMute !== 'supported'}><VolumeX size={14} />批量全员禁言</button>
        <button className="secondary" data-help={gatewayCapabilities?.groupMute === 'unverified' ? '当前旺商聊版本尚未完成解除全禁协议校准。' : '选择多个群，按顺序解除全员禁言；每个群都会单独记录结果。'} onClick={() => enterGroupBatch('unmute')} disabled={Boolean(activeAction) || gatewayCapabilities?.groupMute !== 'supported'}><Volume2 size={14} />批量解除全禁</button>
      </div>
      {groupBatchMode && <div className="group-batch-selection">
        <div><strong>{groupBatchMode === 'announcement' ? '批量公告' : groupBatchMode === 'mute' ? '批量全员禁言' : '批量解除全禁'}</strong><span>只会处理当前选中的群，搜索后“全选”只作用于当前结果。</span></div>
        <div className="group-batch-selection-actions"><button className="secondary" data-help="选中当前搜索结果中的全部群，不会选中被搜索条件隐藏的群。" onClick={toggleAllFilteredGroups} disabled={!filteredGroups.length}>{allFilteredGroupsSelected ? '取消全选当前结果' : '全选当前搜索结果'}</button><button className="secondary" data-help="清空当前批量操作的群选择，避免误操作。" onClick={() => setSelectedGroupIds(new Set())}>清空选择</button><span>已选 {selectedGroupIds.size} 个群</span><button className="primary" data-help="打开确认窗口，查看群名称和操作内容；确认前不会执行。" onClick={() => { setGroupBatchResults(null); setGroupBatchDialogOpen(true) }} disabled={!selectedGroupIds.size || groupBatchBlocked}>{groupBatchMode === 'announcement' ? '编辑公告' : '核对并执行'}</button><button className="secondary" data-help="退出批量模式并清空本次选择。" onClick={closeGroupBatch}>取消批量</button></div>
        {groupBatchBlocked && <small className="group-batch-warning">{groupBatchCapabilityLabel}</small>}
      </div>}
      {filteredGroups.length ? <div className="group-stack">{filteredGroups.map(group => groupBatchMode ? <label className={`group-item group-item-selectable ${selectedGroupIds.has(group.groupId) ? 'selected' : ''}`} key={group.groupId}><input type="checkbox" checked={selectedGroupIds.has(group.groupId)} onChange={() => toggleGroupSelection(group.groupId)} aria-label={`选择群 ${group.name || group.groupId}`} /><span><strong>{group.name || '未命名群'}</strong><small>{group.enabled ? 'AI 自动化已开启' : 'AI 自动化未开启'}</small></span><em className={group.enabled ? 'enabled' : ''}>{group.enabled ? 'AI 开启' : 'AI 关闭'}</em></label> : <button className={`group-item ${selectedGroup === group.groupId ? 'selected' : ''}`} key={group.groupId} onClick={() => { setSelectedGroup(group.groupId); setViewMode('members') }}><span><strong>{group.name || '未命名群'}</strong><small>{group.enabled ? 'AI 自动化已开启' : 'AI 自动化未开启'}</small></span><em className={group.enabled ? 'enabled' : ''}>{group.enabled ? 'AI 开启' : 'AI 关闭'}</em></button>)}</div> : <div className="compact-empty">没有匹配的群</div>}
    </section>

    <section className={`member-pane section ${viewMode !== 'members' ? 'pane-hidden' : ''}`}>
      <div className="section-head member-title"><div><span className="eyebrow">成员读取与控制</span><h2>{activeGroup?.name || '成员'}</h2></div></div>
      {!activeGroup ? <div className="member-empty"><Users size={22} /><strong>先选择一个群</strong><span>成员会在右侧读取，不需要填写群 ID。</span></div> : <>
        <div className="control-mode-tabs" role="tablist" aria-label="群管理方式">
          <button className={controlTab === 'manual' ? 'active' : ''} role="tab" aria-selected={controlTab === 'manual'} data-help="打开人工群控。这里的操作只在你点击确认后执行，不受 AI 和自动规则开关影响。" onClick={() => setControlTab('manual')}><Hand size={16} /><span><strong>人工群控</strong><small>手动执行</small></span></button>
          <button className={controlTab === 'ai' ? 'active' : ''} role="tab" aria-selected={controlTab === 'ai'} data-help="打开 AI 自动化设置。只有群成员明确 @DH 时才会调用 AI，各项权限可以分别开关。" onClick={() => setControlTab('ai')}><Bot size={16} /><span><strong>AI 自动化</strong><small>{aiAutomation.enabled ? '已开启' : '已关闭'}</small></span></button>
          <button className={controlTab === 'rules' ? 'active' : ''} role="tab" aria-selected={controlTab === 'rules'} data-help="打开规则处理设置。欢迎语、群名片和确定性规则都在这里配置，不依赖 AI。" onClick={() => setControlTab('rules')}><Shield size={16} /><span><strong>规则处理</strong><small>{activeGroup.moderationEnabled ? '已开启' : '已关闭'}</small></span></button>
        </div>
        <div className="control-mode-panel" role="tabpanel">
          {controlTab === 'manual' && <><div className="manual-control-bar"><div><Hand size={16} /><span><strong>人工群控</strong><small>仅在点击并确认后执行，不受 AI 自动化开关影响</small></span></div><div><button data-help="立即禁止当前群所有普通成员发言；执行前会再次向你确认。" onClick={() => void toggleGroupMute(true)} disabled={Boolean(activeAction)}><VolumeX size={14} />全员禁言</button><button data-help="解除当前群的全员禁言，让普通成员恢复发言。" onClick={() => void toggleGroupMute(false)} disabled={Boolean(activeAction)}><Volume2 size={14} />解除全禁</button><button data-help="找出已注销或已封禁的普通成员，确认后逐个移出当前群。" onClick={cleanupInactive} disabled={Boolean(activeAction)}><UserMinus size={14} />清理封禁成员</button><button data-help={selected.size ? `把已选择的 ${selected.size} 位群员恢复为 DH 记录的原名称。` : '把当前群内由 DH 修改过的普通成员恢复为原名称。'} onClick={restoreNames} disabled={Boolean(activeAction)}><RefreshCw size={14} />{selected.size ? `恢复名称（${selected.size}）` : '恢复所有名称'}</button></div></div><div className="manual-announcement"><div><Megaphone size={16} /><span><strong>群公告</strong><small>{announcementStatus === '可用' ? '已通过当前旺商聊版本的真实协议校准' : announcementStatus}</small></span></div><textarea value={announcementText} onChange={event => setAnnouncementText(event.target.value)} maxLength={1000} placeholder="输入要发布的群公告；保存后会同步为群内公告消息。" disabled={announcementStatus !== '可用' || Boolean(activeAction)} /><div className="announcement-actions"><button className="secondary" data-help="调用设置页中的 AI，把当前公告改得更清楚、自然、简洁；只更新输入框，确认前不会发布。" onClick={() => void optimizeAnnouncement()} disabled={!announcementText.trim() || Boolean(activeAction)}><Sparkles size={14} />{activeAction === 'announcement:ai' ? '优化中' : 'AI 优化'}</button><button className="primary" data-help={announcementStatus === '可用' ? '保存公告内容，并按照旺商聊原生协议向当前群发送公告消息。' : `当前群公告状态：${announcementStatus}。完成版本协议校准后按钮会自动开放。`} onClick={() => void saveAnnouncement()} disabled={announcementStatus !== '可用' || !announcementText.trim() || Boolean(activeAction)}><Megaphone size={14} />发布公告</button></div></div></>}
          {controlTab === 'ai' && <div className="ai-automation-panel"><div className="ai-panel-intro"><span><Bot size={18} /></span><div><strong>AI 自动化权限</strong><p>只有群里明确 @DH 才会调用 AI。高影响动作默认关闭，可以逐项授权。</p></div></div><div className="automation-toggle-grid"><AutomationToggle title="AI 总开关" detail="控制该群全部 AI 处理" checked={aiAutomation.enabled} disabled={Boolean(activeAction)} onChange={() => void saveAiAutomation({ enabled: !aiAutomation.enabled })} /><AutomationToggle title="自动回复" detail="回答 @DH 的业务问题" checked={aiAutomation.reply} disabled={Boolean(activeAction)} onChange={() => void saveAiAutomation({ reply: !aiAutomation.reply })} /><AutomationToggle title="生成任务" detail="将明确事项写入任务列表" checked={aiAutomation.tasks} disabled={Boolean(activeAction)} onChange={() => void saveAiAutomation({ tasks: !aiAutomation.tasks })} /><AutomationToggle title="允许撤回" detail="AI 可撤回当前触发消息" checked={aiAutomation.recall} disabled={Boolean(activeAction)} onChange={() => void saveAiAutomation({ recall: !aiAutomation.recall })} /><AutomationToggle title="允许禁言" detail="AI 可禁言或解除禁言" checked={aiAutomation.mute} disabled={Boolean(activeAction)} onChange={() => void saveAiAutomation({ mute: !aiAutomation.mute })} /><AutomationToggle title="允许移出" detail="AI 可将普通成员移出群" checked={aiAutomation.remove} danger disabled={Boolean(activeAction)} onChange={() => void saveAiAutomation({ remove: !aiAutomation.remove })} /></div><div className="takeover-row"><div><Hand size={15} /><span><strong>人工接管</strong><small>临时暂停 AI 回复和 AI 动作，固定规则继续运行</small></span></div><button data-help="临时暂停或恢复本群的 AI 回复和 AI 动作；确定性群管规则仍会继续执行。" className={aiAutomation.manualTakeover ? 'active' : ''} onClick={() => void saveAiAutomation({ manualTakeover: !aiAutomation.manualTakeover })} disabled={Boolean(activeAction)}>{aiAutomation.manualTakeover ? '人工接管中' : '由 AI 处理'}</button></div></div>}
          {controlTab === 'rules' && <div className="rule-control-panel"><div className="automatic-workflow-bar"><div><Sparkles size={16} /><span><strong>固定自动处理</strong><small>不依赖 AI，按照确定规则执行</small></span></div><button data-help="开启后，当前群会执行规则页中已启用的确定性规则；关闭后只记录消息，不自动处理。" className={activeGroup.moderationEnabled ? 'feature-on' : ''} onClick={() => void toggleModeration()} disabled={Boolean(activeAction)}><Shield size={14} />群管规则 {activeGroup.moderationEnabled ? '已开启' : '已关闭'}</button></div>
            <div className="group-welcome"><label>欢迎语 <span>[成员] 会替换为新群名片</span><input value={welcomeMessage} onChange={event => setWelcomeMessage(event.target.value)} placeholder="欢迎 @「[成员]」加入群聊，请先查看群规。" /></label><button className="secondary" data-help="保存当前群的欢迎语；新成员完成入群识别和改名后才会发送。" onClick={() => void saveWelcome()} disabled={Boolean(activeAction)}>保存群设置</button></div>
            <div className="card-tools"><div className="card-tool-title"><strong>群名片自动化</strong><span className="muted">独立于 AI，固定后缀只读，默认前缀 DH</span></div><label>前缀<input value={cardPrefix} onChange={event => setCardPrefix(event.target.value)} maxLength={20} /></label><button data-help="控制以后识别到的新普通成员是否自动排队修改群名片。" className={autoRename ? 'feature-on' : ''} onClick={() => void saveCards('auto')} disabled={Boolean(activeAction)}>{autoRename ? '自动改名：开' : '自动改名：关'}</button><button data-help="根据当前成员名称、冲突情况和固定编号生成改名预览，不会立即修改。" onClick={() => void generateCardPreview()} disabled={Boolean(activeAction)}><RefreshCw size={14} />生成建议</button><button className="primary" data-help="确认后把预览中的改名项目加入后台队列，并逐个验证结果。" onClick={() => void applyCardPreview()} disabled={Boolean(activeAction) || !cardPreview}><Check size={14} />一键执行</button><button data-help={cardsPaused ? '恢复后台改名队列，继续处理尚未完成的任务。' : '暂停后台改名队列，保留现有任务和进度。'} onClick={() => void saveCards('pause')} disabled={Boolean(activeAction)}>{cardsPaused ? '恢复队列' : '暂停队列'}</button></div>
            {cardPreview && <div className="card-preview"><div className="card-preview-head"><span>建议：将修改 {cardPreview.willRename}，已规范 {cardPreview.alreadyManaged}，角色排除 {cardPreview.excluded}，身份缺失 {cardPreview.missingIdentity}</span><button title="关闭预览" onClick={() => setCardPreview(null)}><X size={15} /></button></div><div className="card-preview-list">{cardPreview.items.filter((item: any) => item.status === 'planned').slice(0, 100).map((item: any) => <label key={item.member.userId}><span>{item.originalName || displayName(item.member)}</span><input value={item.suggestedName} readOnly={Boolean(item.suffix)} maxLength={20} onChange={event => setCardPreview({ ...cardPreview, items: cardPreview.items.map((candidate: any) => candidate.member.userId === item.member.userId ? { ...candidate, suggestedName: event.target.value } : candidate) })} /><small>{item.suffix ? `固定后缀 ${item.suffix}` : '简称可编辑'}</small></label>)}</div></div>}
            <div className="card-queue-status"><div><strong>后台队列</strong><span>待处理 {cardJobs.filter(job => ['queued', 'processing', 'retry'].includes(job.state)).length} · 已完成 {cardJobs.filter(job => job.state === 'succeeded').length} · 失败 {cardJobs.filter(job => job.state === 'failed').length}</span></div>{cardJobs.some(job => job.state === 'failed') && <button className="secondary" data-help="把当前群中处理失败的改名任务重新放回后台队列。" onClick={() => void retryFailedCards()} disabled={Boolean(activeAction)}>重试失败</button>}<div className="card-job-list">{cardJobs.filter(job => ['processing', 'queued', 'retry', 'failed'].includes(job.state)).slice(0, 8).map(job => <span key={job.id} className={`card-job card-job-${job.state}`}><b>{job.desiredName}</b><small>{job.state === 'processing' ? '处理中' : job.state === 'queued' ? '排队中' : job.state === 'retry' ? `等待重试（第 ${job.attempts} 次）` : `失败：${job.lastError || '待重试'}`}</small></span>)}</div></div>
          </div>}
        </div>
        {roster && <div className={`member-sync-status member-sync-${roster.status || 'loading'}`} role="status" aria-live="polite">{memberLoading ? '正在获取旺商聊当前成员…' : roster.status === 'cached' ? '已显示上次名单，正在获取当前成员' : roster.status === 'rate-limited' ? '旺商聊请求过于频繁，稍后自动重试' : roster.status === 'error' ? (roster.completenessReason || '成员名单刷新失败，保留当前名单') : '成员名单已更新'}</div>}
        <div className="roster-stats"><span>群成员 <strong>{roster?.members.length ?? '-'}</strong></span><div className="roster-pager"><button className="icon-command" title="上一页" onClick={() => setMemberPage(page => Math.max(0, page - 1))} disabled={memberPage <= 0}><ChevronLeft size={15} /></button><span>{memberPage + 1} / {memberPageCount}</span><button className="icon-command" title="下一页" onClick={() => setMemberPage(page => Math.min(memberPageCount - 1, page + 1))} disabled={memberPage >= memberPageCount - 1}><ChevronRight size={15} /></button></div></div>
        <div className="member-toolbar">
          <label className="search-field member-search"><Search size={15} /><input value={memberSearch} onChange={event => setMemberSearch(event.target.value)} placeholder="搜索群名片、原名称" /></label>
          <select value={roleFilter} onChange={event => setRoleFilter(event.target.value)} aria-label="角色筛选"><option value="all">全部角色</option><option value="owner">群主</option><option value="admin">管理员</option><option value="member">群员</option></select>
          <select value={muteSeconds} onChange={event => setMuteSeconds(Number(event.target.value))} aria-label="禁言时长"><option value={600}>禁言 10 分钟</option><option value={3600}>禁言 1 小时</option><option value={86400}>禁言 24 小时</option><option value={604800}>禁言 7 天</option></select>
          <button className="icon-command" title="重新同步成员" onClick={() => void loadMembers()} disabled={memberLoading}><RefreshCw size={16} className={memberLoading ? 'spin' : ''} /></button>
        </div>
        {selected.size > 0 && <div className="bulk-bar"><strong>已选择 {selected.size} 人</strong><button onClick={() => void runBulk('mute')} disabled={Boolean(activeAction)}><VolumeX size={14} />禁言</button><button onClick={() => void runBulk('unmute')} disabled={Boolean(activeAction)}><Volume2 size={14} />解禁</button><button onClick={() => void runBulk('blacklist')} disabled={Boolean(activeAction)}><Ban size={14} />加入黑名单</button><button onClick={() => void runBulk('unblacklist')} disabled={Boolean(activeAction)}><Shield size={14} />移出黑名单</button><button className="danger-text" onClick={() => void runBulk('remove')} disabled={Boolean(activeAction)}><UserMinus size={14} />移出</button><button onClick={() => setSelected(new Set())}>取消选择</button></div>}
        <div className="member-table-wrap">
          <table className="member-table"><thead><tr><th><input type="checkbox" checked={allVisibleSelected} onChange={toggleAll} aria-label="选择当前列表" /></th><th>序号</th><th>群员</th><th>角色</th><th>账号状态</th><th>操作</th></tr></thead><tbody>{pagedMembers.map((member, index) => <tr key={`${member.userId}:${member.nimId}`}><td><input type="checkbox" checked={selected.has(member.userId)} onChange={() => setSelected(previous => { const next = new Set(previous); if (next.has(member.userId)) next.delete(member.userId); else next.add(member.userId); return next })} aria-label={`选择${displayName(member)}`} /></td><td>{memberPage * memberPageSize + index + 1}</td><td><strong>{displayName(member)}</strong><small>{member.nickname ? `原名称：${member.nickname}` : ''}</small></td><td><span className={`role role-${member.role}`}>{roleLabels[member.role] || '群员'}</span></td><td><span className={member.accountState.includes('BAN') || member.accountState.includes('CANCEL') ? 'state-bad' : ''}>{member.blacklisted ? '黑名单 · ' : ''}{stateLabels[member.accountState] || '未知'}</span></td><td><div className="row-actions"><button title="修改群名片" onClick={() => void invokeMemberAction(member, 'rename')} disabled={Boolean(activeAction)}><Pencil size={15} /></button><button title={`禁言 ${Math.round(muteSeconds / 60)} 分钟`} onClick={() => void invokeMemberAction(member, 'mute')} disabled={Boolean(activeAction) || !member.userId || member.userId <= 0 || member.role === 'owner'}><VolumeX size={15} /></button><button title="解除禁言" onClick={() => void invokeMemberAction(member, 'unmute')} disabled={Boolean(activeAction) || member.userId <= 0}><Volume2 size={15} /></button><button className="danger-text" title="移出群聊" onClick={() => void invokeMemberAction(member, 'remove')} disabled={Boolean(activeAction) || member.userId <= 0 || member.role === 'owner'}><UserMinus size={15} /></button></div></td></tr>)}</tbody></table>
          {memberLoading && <div className="member-loading"><RefreshCw size={18} className="spin" />正在同步成员</div>}
          {!memberLoading && roster && !visibleMembers.length && <div className="compact-empty">没有匹配的群员</div>}
        </div>
      </>}
    </section>
    {groupBatchMode && groupBatchDialogOpen && <div className="dialog-backdrop group-batch-dialog-backdrop" role="presentation" onMouseDown={event => { if (event.currentTarget === event.target && !activeAction) setGroupBatchDialogOpen(false) }}>
      <section className="group-batch-dialog" role="dialog" aria-modal="true" aria-labelledby="group-batch-title">
        <header><div><span className="eyebrow">批量群控</span><h2 id="group-batch-title">{groupBatchMode === 'announcement' ? '批量公告' : groupBatchMode === 'mute' ? '批量全员禁言' : '批量解除全禁'}</h2></div><button className="icon-command" data-help="关闭确认窗口，保留当前选中的群。" title="关闭" onClick={() => setGroupBatchDialogOpen(false)} disabled={Boolean(activeAction)}><X size={16} /></button></header>
        <div className="group-batch-targets"><strong>所选群（{selectedGroupIds.size}）</strong><div>{Array.from(selectedGroupIds).map(groupId => <span key={groupId}>{groups.find(group => group.groupId === groupId)?.name || `群 ${groupId}`}</span>)}</div></div>
        {groupBatchMode === 'announcement' ? <label className="group-batch-copy">公告内容 <span>{batchAnnouncementText.length} / 1000</span><textarea value={batchAnnouncementText} onChange={event => setBatchAnnouncementText(event.target.value)} maxLength={1000} placeholder="输入要同时发布到所选群的公告内容" disabled={Boolean(activeAction)} /></label> : <p className="group-batch-confirm-copy">{groupBatchMode === 'mute' ? '执行后，所选群的普通成员会停止发言。请核对群名称，操作结果将逐群记录。' : '执行后，所选群会恢复普通成员发言。请核对群名称，操作结果将逐群记录。'}</p>}
        {groupBatchResults && <div className="group-batch-results" aria-live="polite"><strong>执行结果</strong>{groupBatchResults.map(result => <div key={result.groupId} className={result.success ? 'succeeded' : result.status === 'unknown' ? 'unknown' : 'failed'}><span>{groups.find(group => group.groupId === result.groupId)?.name || `群 ${result.groupId}`}</span><b>{result.success ? '成功' : result.status === 'unknown' ? '请到旺商聊确认' : '失败'}</b><small>{result.error || result.requestId || result.messageId}</small></div>)}</div>}
        <footer>
          {groupBatchMode === 'announcement' && <button className="secondary" data-help="调用当前 AI 配置优化公告草稿，只更新输入框，不会直接发布。" onClick={() => void optimizeBatchAnnouncement()} disabled={!batchAnnouncementText.trim() || Boolean(activeAction)}><Sparkles size={14} />{activeAction === 'group-batch:ai' ? '优化中' : 'AI 优化'}</button>}
          <span />
          <button className="secondary" data-help="关闭窗口并保留当前选择，稍后还可继续操作。" onClick={() => setGroupBatchDialogOpen(false)} disabled={Boolean(activeAction)}>取消</button>
          {groupBatchResults?.some(result => !result.success && result.status !== 'unknown') && <button className="secondary" data-help="只重新执行明确失败的群；回执状态未知的群需要先到旺商聊核对。" onClick={() => void runGroupBatch(groupBatchResults.filter(result => !result.success && result.status !== 'unknown').map(result => result.groupId))} disabled={Boolean(activeAction)}>重试失败群</button>}
          {!groupBatchResults && <button className="primary" data-help="执行前还会显示系统确认；每个群独立处理并保存回执和审计。" onClick={() => void runGroupBatch()} disabled={!selectedGroupIds.size || groupBatchBlocked || Boolean(activeAction) || (groupBatchMode === 'announcement' && !batchAnnouncementText.trim())}>{activeAction === 'group-batch' ? '处理中' : groupBatchMode === 'announcement' ? `发布到 ${selectedGroupIds.size} 个群` : groupBatchMode === 'mute' ? `禁言 ${selectedGroupIds.size} 个群` : `解除 ${selectedGroupIds.size} 个群`}</button>}
        </footer>
      </section>
    </div>}
  </div>
}
