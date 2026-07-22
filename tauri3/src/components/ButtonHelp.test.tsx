import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import ButtonHelp, { buttonHelpText } from './ButtonHelp'

describe('ButtonHelp', () => {
  it('shows a plain-language description supplied by the action', async () => {
    const user = userEvent.setup()
    render(<><button data-help="保存后，这个群才会使用新设置。">保存群设置</button><ButtonHelp /></>)
    await user.hover(screen.getByRole('button', { name: '保存群设置' }))
    expect(screen.getByRole('tooltip')).toHaveTextContent('保存后，这个群才会使用新设置。')
  })

  it('generates descriptions for navigation and ordinary buttons', () => {
    const navigation = document.createElement('button')
    navigation.className = 'nav-item'
    navigation.textContent = '知识与 AI'
    expect(buttonHelpText(navigation)).toContain('打开“知识与 AI”页面')

    const refresh = document.createElement('button')
    refresh.textContent = '刷新'
    expect(buttonHelpText(refresh)).toContain('重新读取最新状态')
  })
})
