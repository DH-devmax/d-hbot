import { describe, expect, it } from 'vitest'
import { formatAuditDetails } from './AuditPage'

describe('审计详情汉化', () => {
  it('汉化自动动作回执的全部协议字段', () => {
    const text = formatAuditDetails(JSON.stringify({
      effect: 'group_mute',
      success: true,
      receipt: {
        transportErrno: 0,
        acknowledged: 1,
        acknowledgedThrough: 8,
        remaining: 0,
        dropped: 0,
        verification: 'verified',
      },
    }))

    expect(text).toContain('自动动作：全群发言控制')
    expect(text).toContain('是否成功：是')
    expect(text).toContain('传输错误码：0')
    expect(text).toContain('本次确认数量：1')
    expect(text).toContain('已确认至序号：8')
    expect(text).toContain('剩余数量：0')
    expect(text).toContain('丢弃数量：0')
    expect(text).toContain('回读验证：已回读确认')
    expect(text).not.toMatch(/effect|success|transportErrno|acknowledgedThrough|remaining|dropped/)
  })

  it('汉化旧版纯文本消息类型和成员事件', () => {
    expect(formatAuditDetails('消息类型=other，序号=71')).toBe('消息类型=其他，接收队列序号=71（旧版记录仅保存消息类型和队列序号）')
    expect(formatAuditDetails('消息类型=notice，序号=56')).toBe('消息类型=通知，接收队列序号=56（旧版记录仅保存消息类型和队列序号）')
    expect(formatAuditDetails('收到 NIM 离群事件')).toBe('收到旺商聊成员离群事件')
  })

  it('汉化机器规则和 AI 规则的隔离审计字段', () => {
    const text = formatAuditDetails(JSON.stringify({
      messageId: 88,
      matchedRuleIds: [1, 2],
      contributorRuleIds: [2],
      automaticActionRuleIds: [2],
      automatic: true,
      mode: 'automatic',
      actionKind: 'recall',
      elapsedMs: 31,
      scores: { scam: 0.96 },
    }))
    expect(text).toContain('消息编号：88')
    expect(text).toContain('命中规则：1、2')
    expect(text).toContain('动作来源规则：2')
    expect(text).toContain('自动动作来源规则：2')
    expect(text).toContain('自动执行：是')
    expect(text).toContain('执行模式：自动执行')
    expect(text).toContain('动作：撤回')
    expect(text).toContain('耗时（毫秒）：31')
    expect(text).toContain('语义置信度：scam：0.96')
  })

  it('汉化 AI 回复耗时、备用切换和失败兜底字段', () => {
    const completed = formatAuditDetails(JSON.stringify({
      messageId: 99,
      queueMs: 2,
      cacheHit: true,
      knowledgeCacheHit: false,
      knowledgeMs: 4,
      modelMs: 810,
      providerAttempts: 2,
      failover: true,
      totalMs: 830,
      status: 'succeeded',
    }))
    expect(completed).toContain('问答缓存命中：是')
    expect(completed).toContain('知识缓存命中：否')
    expect(completed).toContain('模型连接尝试次数：2')
    expect(completed).toContain('是否切换备用连接：是')
    expect(completed).toContain('总耗时（毫秒）：830')

    const failed = formatAuditDetails(JSON.stringify({
      messageId: 100,
      status: 'failed',
      error: 'AI 请求超时',
      fallback: 'ai-service-unavailable',
    }))
    expect(failed).toContain('状态：失败')
    expect(failed).toContain('后续处理：AI 服务不可用时发送中文提示')
  })

  it('完整说明收到群消息和处理结果', () => {
    const text = formatAuditDetails(JSON.stringify({
      direction: 'incoming',
      messageId: 91,
      serverMessageId: 'SERVER-91',
      sequence: 1227,
      kind: 'text',
      senderName: '广州校长',
      contentPreview: '规则测试第一行',
      processingState: 'processed',
      result: 'rules-and-ai-evaluated',
    }))
    expect(text).toContain('消息方向：收到')
    expect(text).toContain('本地消息编号：91')
    expect(text).toContain('旺商聊消息编号：SERVER-91')
    expect(text).toContain('接收队列序号：1227')
    expect(text).toContain('消息类型：文本')
    expect(text).toContain('发送成员名称：广州校长')
    expect(text).toContain('内容摘要：规则测试第一行')
    expect(text).toContain('处理状态：已处理')
    expect(text).toContain('处理结果：规则与 AI 检查完成')
  })

  it('说明成员事件已转为名单对账', () => {
    const text = formatAuditDetails(JSON.stringify({
      expected: 1,
      normalized: 0,
      result: 'ignored',
      fallback: 'roster-reconciliation',
    }))
    expect(text).toContain('预期事件数：1')
    expect(text).toContain('成功识别数：0')
    expect(text).toContain('后续处理：由 60 秒成员名单对账补齐')
  })
})
