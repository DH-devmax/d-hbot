import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import CloseDialog from './CloseDialog'

describe('CloseDialog', () => {
  it('返回托盘或退出选择', async () => {
    const resolve = vi.fn()
    const cancel = vi.fn()
    const remember = vi.fn()
    const user = userEvent.setup()
    render(<CloseDialog remember={false} onRememberChange={remember} onCancel={cancel} onResolve={resolve} />)

    await user.click(screen.getByRole('checkbox', { name: '记住本次选择' }))
    await user.click(screen.getByRole('button', { name: '挂到托盘' }))
    await user.click(screen.getByRole('button', { name: '退出 DH BOT' }))
    await user.click(screen.getByRole('button', { name: '取消' }))

    expect(remember).toHaveBeenCalledWith(true)
    expect(resolve).toHaveBeenNthCalledWith(1, 'tray')
    expect(resolve).toHaveBeenNthCalledWith(2, 'exit')
    expect(cancel).toHaveBeenCalledOnce()
  })
})
