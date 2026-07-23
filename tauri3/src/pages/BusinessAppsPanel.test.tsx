import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import BusinessAppsPanel from './BusinessAppsPanel'

const mocks = vi.hoisted(() => ({
  listBusinessApps: vi.fn(),
  listBusinessAppRuns: vi.fn(),
  setBusinessAppEnabled: vi.fn(),
  getBusinessAppHealth: vi.fn(),
  testBusinessApp: vi.fn(),
}))

vi.mock('../api/client', () => ({
  api: mocks,
  readableError: (reason: unknown) => String(reason),
}))

describe('业务应用页', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.listBusinessApps.mockResolvedValue([{ accountId: 'A', appId: 'prediction', name: '预测', description: '统计参考', version: '1.0.0', enabled: false, status: 'unchecked', statusDetail: '尚未检查数据源', updatedAt: '2026-07-23T00:00:00Z' }])
    mocks.listBusinessAppRuns.mockResolvedValue([])
    mocks.setBusinessAppEnabled.mockResolvedValue(undefined)
    mocks.getBusinessAppHealth.mockResolvedValue({ appId: 'prediction', status: 'ready', detail: '1 个彩种数据可用', checkedAt: '2026-07-23T00:00:00Z', games: [{ id: 'pc28', name: 'PC28', status: 'ready', detail: '结果与历史数据可用' }] })
    mocks.testBusinessApp.mockResolvedValue({ appId: 'prediction', status: 'succeeded', freshness: 'fresh', reply: 'PC28 第1期\n参考度：低', aiUsed: false, error: '', elapsedMs: 12 })
  })

  it('shows the disabled app, health check and local preview', async () => {
    render(<BusinessAppsPanel accountId="A" onError={vi.fn()} />)
    expect(await screen.findByText('预测')).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: '检查数据源' }))
    await waitFor(() => expect(mocks.getBusinessAppHealth).toHaveBeenCalledWith('A', 'prediction'))
    await userEvent.click(screen.getByRole('button', { name: '本地测试' }))
    await waitFor(() => expect(mocks.testBusinessApp).toHaveBeenCalledWith('A', 'prediction', '@DH 预测 PC28'))
    expect(await screen.findByText(/PC28 第1期/)).toBeInTheDocument()
  })
})
