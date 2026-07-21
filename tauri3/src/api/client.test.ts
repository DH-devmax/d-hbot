import { describe, expect, it } from 'vitest'
import { matchesMessageFilters, readableError } from './client'
import type { Message } from '../types'

const message: Message = { id: 7, accountId: 'a', groupId: 2, serverMessageId: 'srv-7', sequence: 1, userId: 9, senderName: '小明', kind: 'text', text: '请查看群规', sentAt: '2026-01-01T00:00:00Z', receivedAt: '2026-01-01T00:00:00Z', processedAt: null, acknowledgedAt: null }

describe('typed client helpers', () => {
  it('matches group, kind, state and keyword filters', () => {
    expect(matchesMessageFilters(message, { groupIds: [2], kind: 'text', keyword: '群规', processingState: 'pending' })).toBe(true)
    expect(matchesMessageFilters(message, { groupIds: [3] })).toBe(false)
    expect(matchesMessageFilters(message, { keyword: '不存在' })).toBe(false)
  })

  it('normalizes command errors without leaking object formatting', () => {
    expect(readableError({ message: '权限不足' })).toBe('权限不足')
    expect(readableError('连接断开')).toBe('连接断开')
  })
})
