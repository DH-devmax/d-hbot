import { afterEach, expect, test, vi } from 'vitest'
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }))
afterEach(() => { vi.unstubAllEnvs(); vi.unstubAllGlobals(); vi.resetModules() })
test('a delayed close from an old subscription cannot disconnect its replacement', async () => {
  vi.stubEnv('MODE', 'web')
  class Socket {
    static instances: Socket[] = []
    onclose?: () => void
    onmessage?: (event: { data: string }) => void
    close = vi.fn()
    constructor() { Socket.instances.push(this) }
  }
  vi.stubGlobal('WebSocket', Socket)
  const { listen } = await import('./transport')
  const stop = await listen('event', vi.fn()); stop()
  const callback = vi.fn(); const stopNew = await listen('event', callback)
  expect(Socket.instances).toHaveLength(2)
  Socket.instances[0].onclose?.()
  Socket.instances[0].onmessage?.({data: JSON.stringify({event:'event',payload:'stale'})})
  Socket.instances[1].onmessage?.({data: JSON.stringify({event:'event',payload:'new'})})
  expect(callback).toHaveBeenCalledOnce()
  stopNew(); expect(Socket.instances[1].close).toHaveBeenCalledOnce()
})
test('web RPC preserves actionable backend errors', async () => {
  vi.stubEnv('MODE', 'web')
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false, status: 400, json: async () => ({ code: 'account_mismatch', message: '当前登录账号与操作数据不一致，请刷新后重试' }) }))
  const { invoke } = await import('./transport')
  await expect(invoke('save_rule')).rejects.toThrow('当前登录账号与操作数据不一致，请刷新后重试')
})
test('web RPC uses same origin and expires UI session on 401', async () => {
  vi.stubEnv('MODE', 'web')
  const request = vi.fn().mockResolvedValueOnce({ ok: true, json: async () => ({ items: [], nextCursor: null }) }).mockResolvedValueOnce({ ok: false, status: 401 })
  vi.stubGlobal('fetch', request)
  const expired = vi.fn(); window.addEventListener('dh-session-expired', expired)
  const { invoke } = await import('./transport')
  expect(await invoke('query_audit', { query: { accountId: 'synthetic' } })).toEqual({ items: [], nextCursor: null })
  expect(request.mock.calls[0][0]).toBe('/api/commands/query_audit')
  expect(request.mock.calls[0][1].credentials).toBe('same-origin')
  await expect(invoke('health')).rejects.toThrow('管理会话已过期')
  expect(expired).toHaveBeenCalledOnce(); window.removeEventListener('dh-session-expired', expired)
})
test('page subscribers share one event connection and unsubscribe cleanly', async () => {
  vi.stubEnv('MODE', 'web')
  class FakeSocket {
    static instances: FakeSocket[] = []
    onopen?: () => void
    onmessage?: (event: { data: string }) => void
    onclose?: () => void
    close = vi.fn(() => this.onclose?.())
    constructor() { FakeSocket.instances.push(this) }
  }
  vi.stubGlobal('WebSocket', FakeSocket)
  const { listen } = await import('./transport')
  const first = vi.fn(), second = vi.fn(), resync = vi.fn()
  window.addEventListener('dh-events-reconnected', resync)
  const stopFirst = await listen('connection-status', first)
  const stopSecond = await listen('connection-status', second)
  expect(FakeSocket.instances).toHaveLength(1)
  const socket = FakeSocket.instances[0]; socket.onopen?.()
  expect(resync).toHaveBeenCalledOnce()
  socket.onmessage?.({ data: JSON.stringify({ event: 'connection-status', payload: { status: 'ready' } }) })
  expect(first).toHaveBeenCalledOnce(); expect(second).toHaveBeenCalledOnce()
  stopFirst(); expect(socket.close).not.toHaveBeenCalled()
  stopSecond(); expect(socket.close).toHaveBeenCalledOnce()
  window.removeEventListener('dh-events-reconnected', resync)
})
