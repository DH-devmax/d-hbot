import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import RuntimeWorkIndicator from './RuntimeWorkIndicator'
import type { RuntimeWorkSnapshot } from '../types'

const snapshot: RuntimeWorkSnapshot = {
  active: { id: 'opaque-active', kind: 'message', label: '处理群消息', scopeLabel: '群消息队列', state: 'running', percent: 42, queued: 3, error: '' },
  items: [
    { id: 'opaque-active', kind: 'message', label: '处理群消息', scopeLabel: '群消息队列', state: 'running', percent: 42, queued: 3, error: '' },
    { id: 'opaque-failure', kind: 'write', label: '发送群消息', scopeLabel: '群操作', state: 'unknown', percent: null, queued: 0, error: '写后回读结果未知' },
  ],
  counts: { running: 1, queued: 3, retrying: 0, failed: 1 },
  updatedAt: '2026-08-06T00:00:00Z',
}

describe('RuntimeWorkIndicator', () => {
  it('shows the current percentage and queue summary', async () => {
    render(<RuntimeWorkIndicator snapshot={snapshot} open={false} onToggle={vi.fn()} onClose={vi.fn()} onNavigate={vi.fn()} />)
    expect(await screen.findByText('42%')).toBeInTheDocument()
    expect(screen.getByText('处理群消息')).toBeInTheDocument()
    expect(screen.getByText('进度 42%')).toBeInTheDocument()
    expect(screen.queryByText('opaque-active')).not.toBeInTheDocument()
  })

  it('uses an indeterminate ring when progress cannot be measured', async () => {
    const indeterminate = { ...snapshot, active: { ...snapshot.active!, percent: null } }
    const { container } = render(<RuntimeWorkIndicator snapshot={indeterminate} open={false} onToggle={vi.fn()} onClose={vi.fn()} onNavigate={vi.fn()} />)
    await waitFor(() => expect(container.querySelector('.brand-progress.indeterminate')).toBeInTheDocument())
  })

  it('groups failures in the drawer and navigates to the relevant page', () => {
    const onNavigate = vi.fn()
    const onClose = vi.fn()
    render(<RuntimeWorkIndicator snapshot={snapshot} open onToggle={vi.fn()} onClose={onClose} onNavigate={onNavigate} />)
    expect(screen.getByRole('dialog', { name: '任务队列' })).toBeInTheDocument()
    expect(screen.getByText('失败与未知')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: /发送群消息/ }))
    expect(onNavigate).toHaveBeenCalledWith('审计')
    expect(onClose).toHaveBeenCalled()
  })

  it('closes the read-only drawer with Escape', () => {
    const onClose = vi.fn()
    render(<RuntimeWorkIndicator snapshot={snapshot} open onToggle={vi.fn()} onClose={onClose} onNavigate={vi.fn()} />)
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(onClose).toHaveBeenCalledTimes(1)
  })
})
