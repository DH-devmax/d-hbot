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

  it('opens rule editing in a modal and closes it with Escape', async () => {
    mocks.listRules.mockResolvedValueOnce([{
      id: 7,
      accountId: 'ACCOUNT',
      groupId: 0,
      name: '测试规则',
      matcher: 'contains',
      pattern: '测试',
      threshold: 0,
      count: 0,
      windowSeconds: 0,
      cooldownSeconds: 0,
      priority: 100,
      mode: 'observe',
      enabled: false,
      semanticThreshold: 0.8,
      exemptRoles: ['owner', 'admin'],
      exemptUserIds: [],
      actions: [{ kind: 'recall', durationSeconds: 0, message: '' }],
    }])
    render(<RulesPage accountId="ACCOUNT" groups={[]} onError={vi.fn()} />)
    const listItem = (await screen.findByText('测试规则')).closest('button')
    expect(screen.queryByRole('dialog', { name: '测试规则' })).not.toBeInTheDocument()
    await userEvent.click(listItem!)
    expect(screen.getByRole('dialog', { name: '测试规则' })).toBeInTheDocument()
    await userEvent.keyboard('{Escape}')
    expect(screen.queryByRole('dialog', { name: '测试规则' })).not.toBeInTheDocument()
  })
})
