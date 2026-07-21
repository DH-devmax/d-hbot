import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Group } from '../types'
import PlansPage from './PlansPage'

const apiMock = vi.hoisted(() => ({
  listTasks: vi.fn(), listSchedules: vi.fn(), listSummaries: vi.fn(), getSummarySettings: vi.fn(),
  listScheduleRuns: vi.fn(), saveTask: vi.fn(), saveSchedule: vi.fn(), deleteTask: vi.fn(),
  deleteSchedule: vi.fn(), generateSummary: vi.fn(), saveSummarySettings: vi.fn(),
}))

vi.mock('../api/client', () => ({
  api: apiMock,
  readableError: (reason: unknown) => reason instanceof Error ? reason.message : String(reason),
}))

const groups: Group[] = [{ accountId: 'ACCOUNT', groupId: 101, name: '测试群', enabled: true, aiEnabled: false, moderationEnabled: false, manualTakeover: false }]

describe('PlansPage validation', () => {
  beforeEach(() => {
    apiMock.listTasks.mockResolvedValue([])
    apiMock.listSchedules.mockResolvedValue([])
    apiMock.listSummaries.mockResolvedValue([])
    apiMock.getSummarySettings.mockResolvedValue({ accountId: 'ACCOUNT', enabled: false, time: '23:00', groupIds: [], timezone: 'Asia/Shanghai' })
    apiMock.listScheduleRuns.mockResolvedValue([])
  })

  it('validates empty task title and incomplete group schedules', async () => {
    const onError = vi.fn()
    render(<PlansPage accountId="ACCOUNT" groups={groups} onError={onError} />)
    await screen.findByRole('button', { name: '保存任务' })
    await userEvent.click(screen.getByRole('button', { name: '保存任务' }))
    expect(onError).toHaveBeenCalledWith('任务标题不能为空')
    await userEvent.click(screen.getByRole('button', { name: '开关群计划' }))
    await userEvent.click(screen.getByRole('button', { name: '保存计划' }))
    expect(onError).toHaveBeenLastCalledWith('计划名称和使用群不能为空')
    expect(apiMock.saveTask).not.toHaveBeenCalled()
    expect(apiMock.saveSchedule).not.toHaveBeenCalled()
  })
})
