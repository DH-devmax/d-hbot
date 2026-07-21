import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import RulesPage from './RulesPage'

const mocks = vi.hoisted(() => ({
  listRules: vi.fn(),
  saveRule: vi.fn(),
  deleteRule: vi.fn(),
  exportRules: vi.fn(),
  importRules: vi.fn(),
}))

vi.mock('../api/client', () => ({
  api: mocks,
  readableError: (reason: unknown) => reason instanceof Error ? reason.message : String(reason),
}))

describe('RulesPage validation', () => {
  beforeEach(() => {
    mocks.listRules.mockReset().mockResolvedValue([])
    mocks.saveRule.mockReset().mockResolvedValue(1)
  })

  it('blocks saving a rule without match content', async () => {
    const onError = vi.fn()
    render(<RulesPage accountId="ACCOUNT" groups={[]} onError={onError} />)
    await screen.findByText('规则为空')
    await userEvent.click(screen.getByRole('button', { name: '新建规则' }))
    await userEvent.click(screen.getByRole('button', { name: '保存规则' }))
    expect(onError).toHaveBeenCalledWith('规则名称和匹配内容不能为空')
    expect(mocks.saveRule).not.toHaveBeenCalled()
  })

  it('persists count, window, role exemptions and member whitelist', async () => {
    render(<RulesPage accountId="ACCOUNT" groups={[]} onError={vi.fn()} />)
    await screen.findByText('规则为空')
    await userEvent.click(screen.getByRole('button', { name: '新建规则' }))
    await userEvent.selectOptions(screen.getByLabelText('匹配方式'), 'image_count')
    await userEvent.clear(screen.getByLabelText('触发次数'))
    await userEvent.type(screen.getByLabelText('触发次数'), '3')
    await userEvent.clear(screen.getByLabelText('统计窗口（秒）'))
    await userEvent.type(screen.getByLabelText('统计窗口（秒）'), '600')
    await userEvent.type(screen.getByLabelText('成员白名单（旺商号，逗号分隔）'), '10001, 10002')
    await userEvent.click(screen.getByLabelText('普通群员'))
    await userEvent.click(screen.getByRole('button', { name: '保存规则' }))
    expect(mocks.saveRule).toHaveBeenCalledWith(expect.objectContaining({
      matcher: 'image_count',
      count: 3,
      windowSeconds: 600,
      exemptRoles: expect.arrayContaining(['owner', 'admin', 'member']),
      exemptUserIds: [10001, 10002],
    }))
  })
})
