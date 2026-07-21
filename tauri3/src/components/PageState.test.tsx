import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { PageFeedback } from './PageState'

describe('page feedback states', () => {
  it.each(['idle', 'loading'] as const)('renders %s as a stable loading state', state => {
    render(<PageFeedback state={state} emptyTitle="空" emptyDetail="空">{null}</PageFeedback>)
    expect(screen.getByText('正在读取')).toBeInTheDocument()
  })

  it('renders the empty state', () => {
    render(<PageFeedback state="empty" emptyTitle="没有成员" emptyDetail="同步后会显示">{null}</PageFeedback>)
    expect(screen.getByText('没有成员')).toBeInTheDocument()
  })

  it('renders an isolated error state', () => {
    render(<PageFeedback state="error" error="连接断开" emptyTitle="空" emptyDetail="空">{null}</PageFeedback>)
    expect(screen.getByText('连接断开')).toBeInTheDocument()
  })

  it('renders ready content without a cache warning', () => {
    render(<PageFeedback state="ready" emptyTitle="空" emptyDetail="空"><span>实时内容</span></PageFeedback>)
    expect(screen.getByText('实时内容')).toBeInTheDocument()
    expect(screen.queryByText(/当前展示本地缓存/)).not.toBeInTheDocument()
  })

  it('keeps cached content visible when refresh fails', () => {
    render(<PageFeedback state="offlineCached" emptyTitle="空" emptyDetail="空"><span>缓存内容</span></PageFeedback>)
    expect(screen.getByText('缓存内容')).toBeInTheDocument()
    expect(screen.getByText(/当前展示本地缓存/)).toBeInTheDocument()
  })
})
