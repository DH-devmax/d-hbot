import { invoke as desktopInvoke } from '@tauri-apps/api/core'
import { listen as desktopListen, type EventCallback, type UnlistenFn } from '@tauri-apps/api/event'
export const isWebMode = () => import.meta.env.MODE === 'web'
export async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isWebMode()) return desktopInvoke<T>(command, args)
  const response = await fetch(`/api/commands/${encodeURIComponent(command)}`, { method: 'POST', credentials: 'same-origin', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(args || {}) })
  if (response.status === 401) { window.dispatchEvent(new Event('dh-session-expired')); throw new Error('管理会话已过期，请重新登录') }
  if (!response.ok) {
    const error = await response.json().catch(() => null)
    throw new Error(typeof error?.message === 'string' ? error.message : response.status === 501 ? '当前运行环境不支持此操作' : '业务请求失败，请检查连接状态')
  }
  return response.json()
}
const subscribers = new Map<string, Set<EventCallback<unknown>>>()
let socket: WebSocket | undefined
let reconnect: ReturnType<typeof setTimeout> | undefined
function connect() {
  if (socket || subscribers.size === 0) return
  clearTimeout(reconnect); reconnect = undefined
  const current = new WebSocket(`${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/api/events`)
  socket = current
  current.onopen = () => { if (socket === current) window.dispatchEvent(new Event('dh-events-reconnected')) }
  current.onmessage = message => {
    if (socket !== current) return
    try { const data = JSON.parse(message.data); if (data.event === 'resync-required') window.dispatchEvent(new Event('dh-events-reconnected')); subscribers.get(data.event)?.forEach(callback => callback({ event: data.event, id: 0, payload: data.payload })) } catch { /* Ignore invalid event frames. */ }
  }
  current.onclose = () => { if (socket !== current) return; socket = undefined; if (subscribers.size) reconnect = setTimeout(connect, 3000) }
}
export async function listen<T>(event: string, callback: EventCallback<T>): Promise<UnlistenFn> {
  if (!isWebMode()) return desktopListen(event, callback)
  const handlers = subscribers.get(event) || new Set<EventCallback<unknown>>()
  handlers.add(callback as EventCallback<unknown>); subscribers.set(event, handlers); connect()
  return () => {
    handlers.delete(callback as EventCallback<unknown>); if (!handlers.size) subscribers.delete(event)
    if (!subscribers.size) { clearTimeout(reconnect); reconnect = undefined; const previous = socket; socket = undefined; previous?.close() }
  }
}
