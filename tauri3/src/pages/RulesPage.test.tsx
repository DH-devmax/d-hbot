import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import RulesPage from './RulesPage'

async function chooseOption(label: string, option: string) {
  await userEvent.click(screen.getByRole('combobox', { name: label }))
  await userEvent.click(screen.getByRole('option', { name: option }))
}

const mocks = vi.hoisted(() => ({
  listRules: vi.fn(),
  saveRule: vi.fn(),
  deleteRule: vi.fn(),
  exportRules: vi.fn(),
  importRules: vi.fn(),
  searchRuleMembers: vi.fn(),
  setGroupRuleFeatures: vi.fn(),
}))

vi.mock('../api/client', () => ({
  api: mocks,
  readableError: (reason: unknown) => reason instanceof Error ? reason.message : String(reason),
}))

describe('RulesPage validation', () => {
  beforeEach(() => {
    mocks.listRules.mockReset().mockResolvedValue([])
    mocks.saveRule.mockReset().mockResolvedValue(1)
    mocks.searchRuleMembers.mockReset().mockResolvedValue({ items: [], nextCursor: null })
    mocks.setGroupRuleFeatures.mockReset().mockResolvedValue(undefined)
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

  it('persists multi-group scope, three-level priority and searchable member cards', async () => {
    const groups = [{ accountId: 'ACCOUNT', groupId: 10, name: '测试群', enabled: true, aiEnabled: true, moderationEnabled: true, machineRulesEnabled: true, aiRulesEnabled: false, manualTakeover: false }]
    mocks.searchRuleMembers.mockResolvedValue({ items: [{ accountId: 'ACCOUNT', groupId: 10, userId: 10001, nimId: 'nim-10001', nickname: '老师', cardName: '现名', originalCardName: '原名', managedCardName: 'DH校长' }], nextCursor: null })
    render(<RulesPage accountId="ACCOUNT" groups={groups} onError={vi.fn()} />)
    await screen.findByText('规则为空')
    await userEvent.click(screen.getByRole('button', { name: '新建规则' }))
    await chooseOption('作用范围', '选择多个群')
    await userEvent.click(screen.getByLabelText('测试群'))
    await chooseOption('优先级', '3 高')
    await chooseOption('匹配方式', '图片次数')
    await userEvent.clear(screen.getByLabelText('触发次数'))
    await userEvent.type(screen.getByLabelText('触发次数'), '3')
    await userEvent.clear(screen.getByLabelText('统计窗口（秒）'))
    await userEvent.type(screen.getByLabelText('统计窗口（秒）'), '600')
    await screen.findByText('DH校长')
    await userEvent.click(screen.getByRole('button', { name: /DH校长/ }))
    await userEvent.click(screen.getByRole('button', { name: '保存规则' }))
    expect(mocks.saveRule).toHaveBeenCalledWith(expect.objectContaining({
      ruleType: 'machine',
      scope: 'selected',
      groupIds: [10],
      priorityLevel: 'high',
      matcher: 'image_count',
      count: 3,
      windowSeconds: 600,
      whitelistUserIds: [10001],
    }))
  })

  it('lists members for a global rule with a blank search and settles loading', async () => {
    const groups = [
      { accountId: 'ACCOUNT', groupId: 10, name: '群一', enabled: true, aiEnabled: true, moderationEnabled: true, machineRulesEnabled: true, aiRulesEnabled: false, manualTakeover: false },
      { accountId: 'ACCOUNT', groupId: 20, name: '群二', enabled: false, aiEnabled: false, moderationEnabled: false, machineRulesEnabled: false, aiRulesEnabled: false, manualTakeover: false },
    ]
    mocks.searchRuleMembers.mockResolvedValue({ items: [{ accountId: 'ACCOUNT', groupId: 10, userId: 10001, nimId: 'nim-10001', nickname: '老师', cardName: '广校', originalCardName: '广州校长', managedCardName: '广校' }], nextCursor: null })

    render(<RulesPage accountId="ACCOUNT" groups={groups} onError={vi.fn()} />)
    await screen.findByText('规则为空')
    await userEvent.click(screen.getByRole('button', { name: '新建规则' }))

    expect(await screen.findByRole('button', { name: /广校/ })).toBeInTheDocument()
    expect(mocks.searchRuleMembers).toHaveBeenCalledWith('ACCOUNT', [10, 20], '', undefined, 50)
    expect(screen.queryByText('正在读取成员…')).not.toBeInTheDocument()
  })

  it('opens rule editing in a modal and closes it with Escape', async () => {
    mocks.listRules.mockResolvedValueOnce([{
      id: 7,
      accountId: 'ACCOUNT',
      groupId: 0,
      ruleType: 'machine',
      scope: 'global',
      groupIds: [],
      priorityLevel: 'medium',
      whitelistUserIds: [],
      name: '测试规则',
      matcher: 'contains',
      pattern: '测试',
      threshold: 0,
      count: 0,
      windowSeconds: 0,
      mode: 'observe',
      enabled: false,
      semanticThreshold: 0.8,
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

  it('switches machine and AI rules independently for one group', async () => {
    const groups = [{ accountId: 'ACCOUNT', groupId: 10, name: '测试群', enabled: true, aiEnabled: true, moderationEnabled: true, machineRulesEnabled: true, aiRulesEnabled: false, manualTakeover: false }]
    render(<RulesPage accountId="ACCOUNT" groups={groups} onError={vi.fn()} />)
    await screen.findByText('规则为空')
    await userEvent.click(screen.getByLabelText('AI 规则'))
    expect(mocks.setGroupRuleFeatures).toHaveBeenCalledWith('ACCOUNT', 10, true, true)
  })
})
