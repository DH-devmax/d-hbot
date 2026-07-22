import { render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import AboutDialog from './AboutDialog'

describe('AboutDialog', () => {
  it('shows the exact build, author and Telegram link', () => {
    render(<AboutDialog onClose={vi.fn()} />)
    expect(screen.getByRole('dialog', { name: 'DH BOT' })).toBeInTheDocument()
    expect(screen.getByText('3.0.0-beta.1')).toBeInTheDocument()
    expect(screen.getByText('DH BOT 开发团队')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: /Telegram：t\.me\/dh114514/ })).toHaveAttribute('href', 'https://t.me/dh114514')
  })

  it('closes with Escape', async () => {
    const user = userEvent.setup()
    const onClose = vi.fn()
    render(<AboutDialog onClose={onClose} />)
    await user.keyboard('{Escape}')
    expect(onClose).toHaveBeenCalledOnce()
  })
})
