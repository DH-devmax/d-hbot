import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Group } from '../types'
import PlansPage from './PlansPage'

const apiMock = vi.hoisted(() => ({
  listActivities: vi.fn(),
  listActivityRuns: vi.fn(),
  previewActivityText: vi.fn(),
  publishActivityNow: vi.fn(),
  saveActivity: vi.fn(),
  deleteActivity: vi.fn(),
  listSchedules: vi.fn(),
  listSummaries: vi.fn(),
  getSummarySettings: vi.fn(),
  listScheduleRuns: vi.fn(),
  saveSchedule: vi.fn(),
  deleteSchedule: vi.fn(),
  generateSummary: vi.fn(),
  saveSummarySettings: vi.fn(),
}))

vi.mock('../api/client', () => ({
  api: apiMock,
  readableError: (reason: unknown) => reason instanceof Error ? reason.message : String(reason),
}))

const groups: Group[] = [{ accountId: 'ACCOUNT', groupId: 101, name: '测试群', enabled: true, aiEnabled: false, moderationEnabled: false, manualTakeover: false }]

describe('PlansPage activity validation', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    apiMock.listActivities.mockResolvedValue([])
    apiMock.listActivityRuns.mockResolvedValue([])
    apiMock.listSchedules.mockResolvedValue([])
    apiMock.listSummaries.mockResolvedValue([])
    apiMock.getSummarySettings.mockResolvedValue({ accountId: 'ACCOUNT', enabled: false, time: '23:00', groupIds: [], timezone: 'Asia/Shanghai' })
    apiMock.listScheduleRuns.mockResolvedValue([])
    apiMock.previewActivityText.mockResolvedValue({ text: '测试群的活泼活动文案', source: 'ai' })
    apiMock.saveActivity.mockResolvedValue(1)
  })

  it('validates an empty activity and an incomplete group schedule', async () => {
    const onError = vi.fn()
    render(<PlansPage accountId="ACCOUNT" groups={groups} onError={onError} />)
    await screen.findByRole('button', { name: '保存活动' })
    await userEvent.click(screen.getByRole('button', { name: '保存活动' }))
    expect(onError).toHaveBeenCalledWith('活动名称不能为空')
    await userEvent.click(screen.getByRole('button', { name: '开关群计划' }))
    await userEvent.click(screen.getByRole('button', { name: '保存计划' }))
    expect(onError).toHaveBeenLastCalledWith('计划名称和使用群不能为空')
    expect(apiMock.saveActivity).not.toHaveBeenCalled()
    expect(apiMock.saveSchedule).not.toHaveBeenCalled()
  })

  it('previews an AI activity without sending it', async () => {
    render(<PlansPage accountId="ACCOUNT" groups={groups} onError={vi.fn()} />)
    await userEvent.type(await screen.findByLabelText('活动名称'), '每日互动')
    await userEvent.type(screen.getByLabelText('活动原文'), '今晚 19:30 开始互动')
    await userEvent.click(screen.getByText('使用 AI 优化'))
    await userEvent.click(screen.getByRole('button', { name: '预览文案' }))
    expect(await screen.findByText('测试群的活泼活动文案')).toBeVisible()
    expect(apiMock.previewActivityText).toHaveBeenCalledWith(expect.objectContaining({ aiOptimize: true, groupIds: [101] }), 101)
    expect(apiMock.publishActivityNow).not.toHaveBeenCalled()
  })
})
