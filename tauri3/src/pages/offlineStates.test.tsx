import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Group } from '../types'
import AuditPage from './AuditPage'
import KnowledgePage from './KnowledgePage'
import PlansPage from './PlansPage'

const mocks = vi.hoisted(() => ({
  listKnowledgeBases: vi.fn(), listKnowledgeDocuments: vi.fn(), listKnowledgeBindings: vi.fn(),
  updateKnowledgeBase: vi.fn(), saveKnowledgeDocument: vi.fn(),
  queryAudit: vi.fn(), exportAudit: vi.fn(),
  listActivities: vi.fn(), listSchedules: vi.fn(), listSummaries: vi.fn(), getSummarySettings: vi.fn(),
  listActivityRuns: vi.fn(), listScheduleRuns: vi.fn(),
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
    mocks.listKnowledgeDocuments.mockResolvedValue([{ id: 11, baseId: 7, baseName: '群规库', title: '文明交流', kind: 'markdown', content: '群规内容', source: 'manual', contentHash: 'HASH', enabled: true }])
    mocks.listKnowledgeBindings.mockResolvedValue([])
    mocks.updateKnowledgeBase.mockResolvedValue(undefined)
    mocks.saveKnowledgeDocument.mockResolvedValue(11)
    mocks.queryAudit.mockResolvedValue({ items: [{ id: 1, accountId: 'ACCOUNT', groupId: 101, userId: 0, actor: 'DH BOT', event: 'daily_summary', level: 'info', details: '已生成', createdAt: '2026-07-21T00:00:00Z' }], nextCursor: null })
    mocks.exportAudit.mockResolvedValue('')
    mocks.listActivities.mockResolvedValue([{ id: 9, accountId: 'ACCOUNT', name: '每日互动', content: '今晚 19:30 开始互动', enabled: true, aiOptimize: true, aiInstructions: '', timezone: 'Asia/Shanghai', startDate: '2026-07-21', endDate: '2026-07-31', weekdays: [1, 2, 3, 4, 5], sendTimes: ['19:30'], groupIds: [101], nextRunAt: '2026-07-21T11:30:00Z', sourceKey: '', deletedAt: null, createdAt: '2026-07-21T00:00:00Z', updatedAt: '2026-07-21T00:00:00Z' }])
    mocks.listSchedules.mockResolvedValue([])
    mocks.listSummaries.mockResolvedValue([])
    mocks.getSummarySettings.mockResolvedValue({ accountId: 'ACCOUNT', enabled: false, time: '23:00', groupIds: [], timezone: 'Asia/Shanghai' })
    mocks.listScheduleRuns.mockResolvedValue([])
    mocks.listActivityRuns.mockResolvedValue([])
  })

  it('keeps knowledge bases visible when a refresh goes offline', async () => {
    render(<KnowledgePage accountId="ACCOUNT" groups={groups} onError={vi.fn()} />)
    await screen.findAllByText('群规库')
    mocks.listKnowledgeBases.mockRejectedValueOnce(new Error('连接断开'))
    await userEvent.click(screen.getByRole('button', { name: '刷新' }))
    await screen.findByText(/当前展示本地缓存/)
    expect(screen.getAllByText('群规库').length).toBeGreaterThan(0)
  })

  it('persists knowledge base and document enabled states', async () => {
    render(<KnowledgePage accountId="ACCOUNT" groups={groups} onError={vi.fn()} />)
    await screen.findByDisplayValue('群规库')
    await userEvent.clear(screen.getByLabelText('知识库名称'))
    await userEvent.type(screen.getByLabelText('知识库名称'), '业务资料库')
    await userEvent.click(screen.getByLabelText('启用该知识库'))
    await userEvent.click(screen.getByRole('button', { name: '保存设置' }))
    expect(mocks.updateKnowledgeBase).toHaveBeenCalledWith(expect.objectContaining({ name: '业务资料库', enabled: false }))
    await userEvent.click(screen.getByLabelText('AI 可以检索此文档'))
    await userEvent.click(screen.getByRole('button', { name: '保存文档' }))
    expect(mocks.saveKnowledgeDocument).toHaveBeenCalledWith(expect.objectContaining({ id: 11, enabled: false }))
  })

  it('shows an explicit checked state for bound knowledge groups', async () => {
    mocks.listKnowledgeBindings.mockResolvedValueOnce([{ baseId: 7, accountId: 'ACCOUNT', groupId: 101, enabled: true }])
    render(<KnowledgePage accountId="ACCOUNT" groups={groups} onError={vi.fn()} />)

    const group = await screen.findByRole('checkbox', { name: '测试群' })
    expect(group).toBeChecked()
    expect(group.closest('.check-chip')).toHaveClass('selected')
    expect(group.closest('.check-chip')?.querySelector('.check-chip-indicator')).toBeInTheDocument()
  })

  it('keeps audit rows visible when a refresh goes offline', async () => {
    render(<AuditPage accountId="ACCOUNT" groups={groups} onError={vi.fn()} />)
    await screen.findAllByText('生成每日摘要')
    mocks.queryAudit.mockRejectedValueOnce(new Error('连接断开'))
    await userEvent.click(screen.getByRole('button', { name: '刷新' }))
    await screen.findByText(/当前展示本地缓存/)
    expect(screen.getAllByText('生成每日摘要').length).toBeGreaterThan(0)
  })

  it('passes member and event filters to audit queries', async () => {
    render(<AuditPage accountId="ACCOUNT" groups={groups} onError={vi.fn()} />)
    await screen.findAllByText('生成每日摘要')
    await userEvent.type(screen.getByLabelText('成员旺商号'), '10001')
    await userEvent.click(screen.getByRole('combobox', { name: '事件' }))
    await userEvent.click(screen.getByRole('option', { name: '生成每日摘要' }))
    await userEvent.click(screen.getByRole('button', { name: '刷新' }))
    await waitFor(() => expect(mocks.queryAudit).toHaveBeenLastCalledWith('ACCOUNT', expect.objectContaining({ userId: 10001, event: 'daily_summary' })))
  })

  it('defaults audit filtering to today and reloads a selected date', async () => {
    render(<AuditPage accountId="ACCOUNT" groups={groups} onError={vi.fn()} />)
    const yearInput = await screen.findByRole('combobox', { name: '年份' })
    const monthInput = screen.getByRole('combobox', { name: '月份' })
    const dayInput = screen.getByRole('combobox', { name: '日期' })
    const now = new Date()
    expect(yearInput).toHaveTextContent(`${now.getFullYear()} 年`)
    expect(monthInput).toHaveTextContent(`${now.getMonth() + 1} 月`)
    expect(dayInput).toHaveTextContent(`${now.getDate()} 日`)

    await userEvent.click(yearInput)
    await userEvent.click(screen.getByRole('option', { name: '2026 年' }))
    await userEvent.click(screen.getByRole('combobox', { name: '月份' }))
    await userEvent.click(screen.getByRole('option', { name: '7 月' }))
    await userEvent.click(screen.getByRole('combobox', { name: '日期' }))
    await userEvent.click(screen.getByRole('option', { name: '20 日' }))
    await waitFor(() => expect(mocks.queryAudit).toHaveBeenLastCalledWith('ACCOUNT', expect.objectContaining({
      from: new Date('2026-07-20T00:00:00').toISOString(),
      to: new Date('2026-07-20T23:59:59.999').toISOString(),
    })))
  })

  it('keeps activities visible when one combined refresh fails', async () => {
    render(<PlansPage accountId="ACCOUNT" groups={groups} onError={vi.fn()} />)
    await screen.findAllByText('每日互动')
    mocks.listActivities.mockRejectedValueOnce(new Error('连接断开'))
    await userEvent.click(screen.getByRole('button', { name: '刷新' }))
    await waitFor(() => expect(screen.getByText(/当前展示本地缓存/)).toBeInTheDocument())
    expect(screen.getAllByText('每日互动').length).toBeGreaterThan(0)
  })

  it('shows the persisted activity group and send time', async () => {
    render(<PlansPage accountId="ACCOUNT" groups={groups} onError={vi.fn()} />)
    await screen.findAllByText('每日互动')
    expect(screen.getByRole('checkbox', { name: '测试群' })).toBeChecked()
    expect(screen.getByLabelText('发送时刻 1')).toHaveValue('19:30')
  })
})
