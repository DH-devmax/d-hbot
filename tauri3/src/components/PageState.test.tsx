import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it } from 'vitest'
import { PageFeedback } from './PageState'

describe('page feedback states', () => {
  it('renders loading, empty, error and cached states independently', () => {
    const loading = renderToStaticMarkup(<PageFeedback state="loading" emptyTitle="空" emptyDetail="空">{null}</PageFeedback>)
    const empty = renderToStaticMarkup(<PageFeedback state="empty" emptyTitle="没有成员" emptyDetail="同步后会显示">{null}</PageFeedback>)
    const error = renderToStaticMarkup(<PageFeedback state="error" error="连接断开" emptyTitle="空" emptyDetail="空">{null}</PageFeedback>)
    const cached = renderToStaticMarkup(<PageFeedback state="offlineCached" emptyTitle="空" emptyDetail="空"><span>缓存内容</span></PageFeedback>)
    expect(loading).toContain('正在读取')
    expect(empty).toContain('没有成员')
    expect(error).toContain('连接断开')
    expect(cached).toContain('当前展示本地缓存')
  })
})
