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
    mocks.saveRule.mockReset()
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
})
