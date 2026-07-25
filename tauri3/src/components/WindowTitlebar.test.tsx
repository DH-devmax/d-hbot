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
    expect(screen.getByRole('button', { name: 'Minimize' })).toBeVisible()
    expect(screen.getByRole('button', { name: 'Maximize' })).toBeVisible()
    expect(screen.getByRole('button', { name: 'Close' })).toBeVisible()
    expect(document.querySelector('[data-tauri-drag-region]')).toHaveClass('window-titlebar__drag')
  })

  it('routes controls to the Tauri window API', async () => {
    render(<WindowTitlebar />)

    fireEvent.click(screen.getByRole('button', { name: 'Minimize' }))
    fireEvent.click(screen.getByRole('button', { name: 'Maximize' }))
    fireEvent.click(screen.getByRole('button', { name: 'Close' }))

    expect(windowApi.minimize).toHaveBeenCalledOnce()
    expect(windowApi.toggleMaximize).toHaveBeenCalledOnce()
    expect(windowApi.close).toHaveBeenCalledOnce()
  })
})
