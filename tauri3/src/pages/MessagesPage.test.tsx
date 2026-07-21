import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Group, Message } from '../types'
import MessagesPage from './MessagesPage'

const mocks = vi.hoisted(() => ({
  queryMessages: vi.fn(),
  sendTextBatch: vi.fn(),
  recallMessage: vi.fn(),
}))

vi.mock('../api/client', () => ({
  api: mocks,
  readableError: (reason: unknown) => reason instanceof Error ? reason.message : String(reason),
}))

const groups: Group[] = [
  { accountId: 'ACCOUNT-A', groupId: 101, name: '一号群', enabled: true, aiEnabled: false, moderationEnabled: false, manualTakeover: false },
  { accountId: 'ACCOUNT-A', groupId: 102, name: '二号群', enabled: true, aiEnabled: false, moderationEnabled: false, manualTakeover: false },
]

const message = (accountId: string, text: string, id: number): Message => ({
  id, accountId, groupId: 101, serverMessageId: `MESSAGE-${id}`, sequence: id, userId: 7,
  senderName: '小明', kind: 'text', text, sentAt: '2026-07-21T00:00:00Z', receivedAt: '2026-07-21T00:00:00Z',
  processedAt: null, acknowledgedAt: null, processingState: 'pending',
})

describe('MessagesPage', () => {
  beforeEach(() => {
    mocks.queryMessages.mockReset().mockResolvedValue({ items: [message('ACCOUNT-A', 'ACCOUNT-A 消息', 1)], nextCursor: null })
    mocks.sendTextBatch.mockReset()
    mocks.recallMessage.mockReset()
  })

  it('reports partial batch failures with successful and failed counts', async () => {
    const onError = vi.fn()
    mocks.sendTextBatch.mockResolvedValue([
      { groupId: 101, groupName: '一号群', success: true, messageId: 'SENT-1' },
      { groupId: 102, groupName: '二号群', success: false, error: '权限不足' },
    ])
    render(<MessagesPage groups={groups} accountId="ACCOUNT-A" onError={onError} />)
    await screen.findByText('ACCOUNT-A 消息')
    await userEvent.click(screen.getByLabelText('一号群'))
    await userEvent.click(screen.getByLabelText('二号群'))
    await userEvent.type(screen.getByPlaceholderText('发送内容只会发到当前勾选的群。'), '测试消息')
    await userEvent.click(screen.getByRole('button', { name: '发送文本' }))
    await waitFor(() => expect(onError).toHaveBeenCalledWith('已发送 1 个群，1 个群失败：二号群'))
    expect(mocks.sendTextBatch).toHaveBeenCalledWith(groups, '测试消息')
  })

  it('does not expose the previous account while the next account loads', async () => {
    let resolveNext!: (value: { items: Message[]; nextCursor: null }) => void
    mocks.queryMessages
      .mockResolvedValueOnce({ items: [message('ACCOUNT-A', 'ACCOUNT-A 消息', 1)], nextCursor: null })
      .mockImplementationOnce(() => new Promise(resolve => { resolveNext = resolve }))
    const onError = vi.fn()
    const { rerender } = render(<MessagesPage key="ACCOUNT-A" groups={groups} accountId="ACCOUNT-A" onError={onError} />)
    await screen.findByText('ACCOUNT-A 消息')
    rerender(<MessagesPage key="ACCOUNT-B" groups={groups.map(group => ({ ...group, accountId: 'ACCOUNT-B' }))} accountId="ACCOUNT-B" onError={onError} />)
    expect(screen.queryByText('ACCOUNT-A 消息')).not.toBeInTheDocument()
    expect(screen.getByText('正在读取')).toBeInTheDocument()
    resolveNext({ items: [message('ACCOUNT-B', 'ACCOUNT-B 消息', 2)], nextCursor: null })
    await screen.findByText('ACCOUNT-B 消息')
  })

  it('uses backend cursors for the next and previous page', async () => {
    mocks.queryMessages
      .mockResolvedValueOnce({ items: [message('ACCOUNT-A', '第一页', 30)], nextCursor: '30' })
      .mockResolvedValueOnce({ items: [message('ACCOUNT-A', '第二页', 29)], nextCursor: null })
      .mockResolvedValueOnce({ items: [message('ACCOUNT-A', '第一页', 30)], nextCursor: '30' })
    render(<MessagesPage groups={groups} accountId="ACCOUNT-A" onError={vi.fn()} />)
    await screen.findByText('第一页')
    await userEvent.click(screen.getByTitle('下一页'))
    await screen.findByText('第二页')
    expect(mocks.queryMessages).toHaveBeenLastCalledWith('ACCOUNT-A', expect.any(Object), '30', 30)
    await userEvent.click(screen.getByTitle('上一页'))
    await screen.findByText('第一页')
    expect(mocks.queryMessages).toHaveBeenLastCalledWith('ACCOUNT-A', expect.any(Object), undefined, 30)
  })
})
