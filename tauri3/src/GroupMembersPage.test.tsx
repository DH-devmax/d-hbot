import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import GroupMembersPage from './GroupMembersPage'

const invoke = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }))

const group = {
  accountId: 'ACCOUNT', groupId: 100, name: '项目协作群', enabled: true,
  aiEnabled: true, moderationEnabled: false, manualTakeover: false,
  welcomeMessage: '欢迎 @「[成员]」加入群聊。',
}
const secondGroup = { ...group, groupId: 101, name: '运营通知群', enabled: false, aiEnabled: false }

describe('GroupMembersPage control tabs', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
    invoke.mockReset()
    invoke.mockImplementation((command: string) => {
      if (command === 'list_members') return Promise.resolve({ members: [], reportedCount: 0, resolvedCount: 0, complete: true, sources: [] })
      if (command === 'get_card_settings') return Promise.resolve({ prefix: 'DH', autoRename: false, paused: false })
      if (command === 'get_ai_automation_settings') return Promise.resolve({ enabled: true, reply: true, tasks: true, recall: false, mute: false, remove: false, manualTakeover: false })
      if (command === 'get_group_management_context') return Promise.resolve({ groupId: 100, isManager: true, announcementStatus: '可用' })
      if (command === 'get_group_announcement') return Promise.resolve({ groupId: 100, noticeId: 'NOTICE', content: '群公告内容', mode: 'COMMON_NOTICE', authorUserId: 1 })
      if (command === 'get_gateway_capabilities') return Promise.resolve({ announcement: 'supported', groupMute: 'supported' })
      if (command === 'test_ai') return Promise.resolve({ decision: { reply: '优化后的群公告' }, elapsedMs: 10, model: 'test' })
      if (command === 'list_card_rename_jobs') return Promise.resolve([])
      return Promise.resolve(null)
    })
  })

  it('switches manual, AI, and rule controls without hiding the member list', async () => {
    const user = userEvent.setup()
    render(<GroupMembersPage groups={[group]} selectedGroup={group.groupId} setSelectedGroup={vi.fn()} activeGroup={group} onError={vi.fn()} refresh={vi.fn()} />)

    expect(screen.getByRole('tab', { name: /人工群控/ })).toHaveAttribute('aria-selected', 'true')
    expect(screen.getByRole('button', { name: '全员禁言' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '发布公告' })).toBeDisabled()
    await waitFor(() => expect(screen.getByRole('button', { name: 'AI 优化' })).toBeEnabled())
    await user.click(screen.getByRole('button', { name: 'AI 优化' }))
    await waitFor(() => expect(screen.getByDisplayValue('优化后的群公告')).toBeInTheDocument())

    await user.click(screen.getByRole('tab', { name: /AI 自动化/ }))
    await waitFor(() => expect(screen.getByRole('switch', { name: /AI 总开关/ })).toBeInTheDocument())

    await user.click(screen.getByRole('tab', { name: /规则处理/ }))
    expect(screen.getByRole('button', { name: /群管规则/ })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '保存群设置' })).toBeInTheDocument()
    expect(screen.getByPlaceholderText('搜索名称、旺商号')).toBeInTheDocument()
  })

  it('selects only current search results and reports partial batch announcement failures', async () => {
    const user = userEvent.setup()
    const onError = vi.fn()
    vi.spyOn(window, 'confirm').mockReturnValue(true)
    invoke.mockImplementation((command: string, args?: Record<string, any>) => {
      if (command === 'get_gateway_capabilities') return Promise.resolve({ announcement: 'supported', groupMute: 'supported' })
      if (command === 'execute_group_batch') return Promise.resolve((args?.input?.groupIds || []).map((groupId: number) => ({
        groupId,
        success: groupId === 100,
        status: groupId === 100 ? 'succeeded' : 'failed',
        requestId: `REQUEST-${groupId}`,
        messageId: groupId === 100 ? `NOTICE-${groupId}` : '',
        error: groupId === 100 ? '' : '当前账号缺少群管理权限',
      })))
      if (command === 'test_ai') return Promise.resolve({ decision: { reply: '优化后的批量公告' } })
      return Promise.resolve(null)
    })
    render(<GroupMembersPage groups={[group, secondGroup]} selectedGroup={null} setSelectedGroup={vi.fn()} onError={onError} refresh={vi.fn()} />)

    expect(screen.getByText('共 2 个群')).toBeInTheDocument()
    await user.type(screen.getByPlaceholderText('搜索群名称'), '运营')
    await waitFor(() => expect(screen.getByRole('button', { name: '批量公告' })).toBeEnabled())
    await user.click(screen.getByRole('button', { name: '批量公告' }))
    await user.click(screen.getByRole('button', { name: '全选当前搜索结果' }))
    expect(screen.getByText('已选 1 个群')).toBeInTheDocument()

    await user.clear(screen.getByPlaceholderText('搜索群名称'))
    await user.click(screen.getByRole('checkbox', { name: '选择群 项目协作群' }))
    expect(screen.getByText('已选 2 个群')).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: '编辑公告' }))
    const textarea = screen.getByPlaceholderText('输入要同时发布到所选群的公告内容')
    await user.clear(textarea)
    await user.type(textarea, '测试批量公告')
    await user.click(screen.getByRole('button', { name: 'AI 优化' }))
    await waitFor(() => expect(screen.getByDisplayValue('优化后的批量公告')).toBeInTheDocument())
    await user.click(screen.getByRole('button', { name: '发布到 2 个群' }))

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('execute_group_batch', { input: { action: 'announcement', groupIds: [101, 100], text: '优化后的批量公告' } }))
    expect(screen.queryByText('请到旺商聊确认')).not.toBeInTheDocument()
    expect(screen.getByText('失败')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '重试失败群' })).toBeInTheDocument()
    expect(onError).toHaveBeenCalledWith(expect.stringContaining('成功 1 个，失败 1 个'))
  })

  it('requires manual verification for an unknown batch receipt', async () => {
    const user = userEvent.setup()
    vi.spyOn(window, 'confirm').mockReturnValue(true)
    invoke.mockImplementation((command: string) => {
      if (command === 'get_gateway_capabilities') return Promise.resolve({ announcement: 'supported', groupMute: 'supported' })
      if (command === 'execute_group_batch') return Promise.resolve([{ groupId: 100, success: false, status: 'unknown', requestId: '', messageId: '', error: '网络中断，结果待确认' }])
      return Promise.resolve(null)
    })
    render(<GroupMembersPage groups={[group]} selectedGroup={null} setSelectedGroup={vi.fn()} onError={vi.fn()} refresh={vi.fn()} />)

    await waitFor(() => expect(screen.getByRole('button', { name: '批量公告' })).toBeEnabled())
    await user.click(screen.getByRole('button', { name: '批量公告' }))
    await user.click(screen.getByRole('button', { name: '全选当前搜索结果' }))
    await user.click(screen.getByRole('button', { name: '编辑公告' }))
    await user.type(screen.getByPlaceholderText('输入要同时发布到所选群的公告内容'), '待确认公告')
    await user.click(screen.getByRole('button', { name: '发布到 1 个群' }))

    expect(await screen.findByText('请到旺商聊确认')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: '重试失败群' })).not.toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /发布到/ })).not.toBeInTheDocument()
  })
})
