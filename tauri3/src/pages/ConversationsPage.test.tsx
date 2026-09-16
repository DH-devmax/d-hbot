import { fireEvent, render, screen } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import ConversationsPage from './ConversationsPage'
afterEach(() => vi.unstubAllGlobals())
it('keeps provider reply confirmation separate from receive ACK and surfaces rejected policy changes', async () => {
  vi.stubGlobal('fetch', vi.fn(async (url: string) => ({ ok: true, json: async () => url.endsWith('/list')
    ? { conversations: [{ id: 'local', kind: 'group', name: 'Test group', messageCount: 1, manualTakeover: false }] }
    : url.endsWith('/history') ? { messages: [{ id: 1, senderName: 'Alice', sentAt: 1, text: 'hello', contentKind: 'text', flow: 'in', state: 'processed', reason: '', ackState: 'flushed', aiState: 'succeeded', deliveryState: 'unknown' }] }
      : { ok: false, message: '消息收发验证完成后才能启用自动回复' } })))
  render(<ConversationsPage />)
  fireEvent.click(await screen.findByText('Test group'))
  expect(await screen.findByText(/接收确认已发出/)).toHaveTextContent('回复：发送结果未知')
  expect(screen.queryByText(/服务端已确认/)).not.toBeInTheDocument()
  fireEvent.click(screen.getByLabelText('人工接管'))
  expect(await screen.findByRole('alert')).toHaveTextContent('消息收发验证完成后才能启用自动回复')
})
