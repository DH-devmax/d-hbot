import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Group } from '../types'
import AuditPage from './AuditPage'
import KnowledgePage from './KnowledgePage'
import PlansPage from './PlansPage'

const mocks = vi.hoisted(() => ({
  listKnowledgeBases: vi.fn(), listKnowledgeDocuments: vi.fn(), listKnowledgeBindings: vi.fn(),
  queryAudit: vi.fn(), exportAudit: vi.fn(),
  listTasks: vi.fn(), listSchedules: vi.fn(), listSummaries: vi.fn(), getSummarySettings: vi.fn(),
  listScheduleRuns: vi.fn(),
}))

vi.mock('../api/client', () => ({
  api: mocks,
  readableError: (reason: unknown) => reason instanceof Error ? reason.message : String(reason),
}))

const groups: Group[] = [{ accountId: 'ACCOUNT', groupId: 101, name: '测试群', enabled: true, aiEnabled: false, moderationEnabled: false, manualTakeover: false }]

describe('production pages retain scoped cached data', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.listKnowledgeBases.mockResolvedValue([{ id: 7, accountId: 'ACCOUNT', name: '群规库', description: '业务资料', enabled: true, builtIn: false, readOnly: false }])
    mocks.listKnowledgeDocuments.mockResolvedValue([])
    mocks.listKnowledgeBindings.mockResolvedValue([])
    mocks.queryAudit.mockResolvedValue({ items: [{ id: 1, accountId: 'ACCOUNT', groupId: 101, userId: 0, actor: 'DH BOT', event: 'daily_summary', level: 'info', details: '已生成', createdAt: '2026-07-21T00:00:00Z' }], nextCursor: null })
    mocks.exportAudit.mockResolvedValue('')
    mocks.listTasks.mockResolvedValue([{ id: 9, accountId: 'ACCOUNT', groupId: 101, title: '联系群员', description: '', status: 'pending', createdAt: '2026-07-21T00:00:00Z', updatedAt: '2026-07-21T00:00:00Z' }])
    mocks.listSchedules.mockResolvedValue([])
    mocks.listSummaries.mockResolvedValue([])
    mocks.getSummarySettings.mockResolvedValue({ accountId: 'ACCOUNT', enabled: false, time: '23:00', groupIds: [], timezone: 'Asia/Shanghai' })
    mocks.listScheduleRuns.mockResolvedValue([])
  })

  it('keeps knowledge bases visible when a refresh goes offline', async () => {
    render(<KnowledgePage accountId="ACCOUNT" groups={groups} onError={vi.fn()} />)
    await screen.findAllByText('群规库')
    mocks.listKnowledgeBases.mockRejectedValueOnce(new Error('连接断开'))
    await userEvent.click(screen.getByRole('button', { name: '刷新' }))
    await screen.findByText(/当前展示本地缓存/)
    expect(screen.getAllByText('群规库').length).toBeGreaterThan(0)
  })

  it('keeps audit rows visible when a refresh goes offline', async () => {
    render(<AuditPage accountId="ACCOUNT" groups={groups} onError={vi.fn()} />)
    await screen.findByText('生成每日摘要')
    mocks.queryAudit.mockRejectedValueOnce(new Error('连接断开'))
    await userEvent.click(screen.getByRole('button', { name: '刷新' }))
    await screen.findByText(/当前展示本地缓存/)
    expect(screen.getByText('生成每日摘要')).toBeInTheDocument()
  })

  it('keeps tasks visible when one combined refresh fails', async () => {
    render(<PlansPage accountId="ACCOUNT" groups={groups} onError={vi.fn()} />)
    await screen.findAllByText('联系群员')
    mocks.listTasks.mockRejectedValueOnce(new Error('连接断开'))
    await userEvent.click(screen.getByRole('button', { name: '刷新' }))
    await waitFor(() => expect(screen.getByText(/当前展示本地缓存/)).toBeInTheDocument())
    expect(screen.getAllByText('联系群员').length).toBeGreaterThan(0)
  })
})
