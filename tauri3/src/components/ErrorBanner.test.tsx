import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import ErrorBanner, { describeAppNotice } from './ErrorBanner'

describe('ErrorBanner', () => {
  it('turns DevTools transport details into a concrete Chinese diagnosis', () => {
    expect(describeAppNotice('读取 DevTools 页面失败：http://127.0.0.1:9222/json/list')).toEqual(expect.objectContaining({
      title: '旺商聊 DevTools 连接失败',
      action: expect.stringContaining('调试'),
    }))
  })

  it('separates the title, impact and next action', () => {
    const onClose = vi.fn()
    render(<ErrorBanner message="NIM 尚未初始化" onClose={onClose} />)

    expect(screen.getByText('旺商聊会话尚未就绪')).toBeVisible()
    expect(screen.getByText(/DevTools 已连接/)).toBeVisible()
    expect(screen.getByText(/处理方式/)).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: '关闭提示' }))
    expect(onClose).toHaveBeenCalledOnce()
  })

  it('shows successful operations as information instead of an error', () => {
    expect(describeAppNotice('已绑定 2 个群')).toEqual(expect.objectContaining({ level: 'info', title: '操作完成' }))
  })

  it('does not misclassify a queued CDP timeout as a protocol change', () => {
    expect(describeAppNotice('网关请求排队超时：同步群成员名单')).toEqual(expect.objectContaining({
      level: 'warning',
      title: '旺商聊请求队列拥堵',
    }))
  })

  it('gives structural changes and rejected business routes distinct diagnoses', () => {
    expect(describeAppNotice('协议结构变化：路由缺失')).toEqual(expect.objectContaining({ title: '协议能力已暂停' }))
    expect(describeAppNotice('业务码 1001：禁止调用此接口')).toEqual(expect.objectContaining({ title: '旺商聊拒绝了当前路由' }))
  })
})
