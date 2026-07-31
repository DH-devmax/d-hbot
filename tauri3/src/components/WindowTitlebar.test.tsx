import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import WindowTitlebar from './WindowTitlebar'

const windowApi = vi.hoisted(() => ({
  isMaximized: vi.fn(),
  minimize: vi.fn(),
  toggleMaximize: vi.fn(),
  close: vi.fn(),
  onResized: vi.fn(),
}))

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => windowApi,
}))

describe('WindowTitlebar', () => {
  beforeEach(() => {
    document.documentElement.classList.remove('window-maximized')
    Object.defineProperty(window, '__TAURI_INTERNALS__', { configurable: true, value: {} })
    windowApi.isMaximized.mockResolvedValue(false)
    windowApi.minimize.mockResolvedValue(undefined)
    windowApi.toggleMaximize.mockResolvedValue(undefined)
    windowApi.close.mockResolvedValue(undefined)
    windowApi.onResized.mockResolvedValue(() => undefined)
  })

  it('keeps the titlebar blank and shows standard window controls', () => {
    render(<WindowTitlebar />)

    expect(screen.queryByText('DH BOT')).not.toBeInTheDocument()
    expect(screen.queryByText('Workspace')).not.toBeInTheDocument()
    expect(screen.getByRole('button', { name: '最小化' })).toBeVisible()
    expect(screen.getByRole('button', { name: '最大化' })).toBeVisible()
    expect(screen.getByRole('button', { name: '关闭' })).toBeVisible()
    expect(document.querySelector('[data-tauri-drag-region]')).toHaveClass('window-titlebar__drag')
  })

  it('routes controls to the Tauri window API', async () => {
    render(<WindowTitlebar />)

    fireEvent.click(screen.getByRole('button', { name: '最小化' }))
    fireEvent.click(screen.getByRole('button', { name: '最大化' }))
    fireEvent.click(screen.getByRole('button', { name: '关闭' }))

    expect(windowApi.minimize).toHaveBeenCalledOnce()
    expect(windowApi.toggleMaximize).toHaveBeenCalledOnce()
    expect(windowApi.close).toHaveBeenCalledOnce()
  })

  it('removes rounded-corner clipping while maximized', async () => {
    windowApi.isMaximized.mockResolvedValue(true)
    render(<WindowTitlebar />)

    await vi.waitFor(() => expect(document.documentElement).toHaveClass('window-maximized'))
  })
})
