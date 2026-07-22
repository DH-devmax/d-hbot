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

describe('GroupMembersPage control tabs', () => {
  beforeEach(() => {
    invoke.mockReset()
    invoke.mockImplementation((command: string) => {
      if (command === 'list_members') return Promise.resolve({ members: [], reportedCount: 0, resolvedCount: 0, complete: true, sources: [] })
      if (command === 'get_card_settings') return Promise.resolve({ prefix: 'DH', autoRename: false, paused: false })
      if (command === 'get_ai_automation_settings') return Promise.resolve({ enabled: true, reply: true, tasks: true, recall: false, mute: false, remove: false, manualTakeover: false })
      if (command === 'list_card_rename_jobs') return Promise.resolve([])
      return Promise.resolve(null)
    })
  })

  it('switches manual, AI, and rule controls without hiding the member list', async () => {
    const user = userEvent.setup()
    render(<GroupMembersPage groups={[group]} selectedGroup={group.groupId} setSelectedGroup={vi.fn()} activeGroup={group} onError={vi.fn()} refresh={vi.fn()} />)

    expect(screen.getByRole('tab', { name: /人工群控/ })).toHaveAttribute('aria-selected', 'true')
    expect(screen.getByRole('button', { name: '全员禁言' })).toBeInTheDocument()

    await user.click(screen.getByRole('tab', { name: /AI 自动化/ }))
    await waitFor(() => expect(screen.getByRole('switch', { name: /AI 总开关/ })).toBeInTheDocument())

    await user.click(screen.getByRole('tab', { name: /规则处理/ }))
    expect(screen.getByRole('button', { name: /群管规则/ })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '保存群设置' })).toBeInTheDocument()
    expect(screen.getByPlaceholderText('搜索名称、旺商号')).toBeInTheDocument()
  })
})
