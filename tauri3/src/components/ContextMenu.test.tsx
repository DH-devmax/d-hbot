import { useState } from 'react'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import ContextMenu from './ContextMenu'

const writeText = vi.fn(async () => undefined)
const readText = vi.fn(async () => '')

// jsdom 不实现剪贴板，只在本文件内补齐，不动 src/test/setup.ts（避免影响其他测试文件）。
function installClipboard() {
  Object.defineProperty(navigator, 'clipboard', {
    configurable: true,
    value: { writeText, readText },
  })
}

beforeEach(() => {
  writeText.mockClear()
  readText.mockClear()
  readText.mockResolvedValue('')
  installClipboard()
})

afterEach(() => {
  vi.unstubAllEnvs()
})

/**
 * `userEvent.setup()` 会装上它自己的 `navigator.clipboard` 桩（用来支持 user.copy()/user.paste()），
 * 覆盖掉 beforeEach 里装的 mock。所以必须在 setup() 之后再装一次，
 * 否则组件调用到的是 user-event 的桩：它总是成功 resolve，断言只会看到"spy 从未被调用"，
 * 连 mockRejectedValueOnce 都不会生效。
 */
function setupUser() {
  const user = userEvent.setup()
  installClipboard()
  return user
}

/** 消息台的真实结构：图标按钮内只有 svg，所以整行 TSV 里不出现操作列。 */
function MessageRow({ onRecall = () => undefined }: { onRecall?: () => void }) {
  return <div className="message-table">
    <div className="message-row message-header"><span>时间</span><span>群聊 / 成员</span><span>内容</span></div>
    <div className="message-row">
      <time>2026-08-07 10:00</time>
      <span><strong>测试群</strong><small>张三</small></span>
      <p>你好</p>
      <span className="type-pill">文本</span>
      <span className="state-pill">待处理</span>
      <button className="icon-button" title="撤回消息" onClick={onRecall} />
    </div>
  </div>
}

describe('ContextMenu', () => {
  it('吞掉默认菜单，空白处不弹自定义菜单', () => {
    render(<><div data-testid="blank">空白</div><ContextMenu /></>)
    expect(fireEvent.contextMenu(screen.getByTestId('blank'))).toBe(false)
    expect(screen.queryByRole('menu')).not.toBeInTheDocument()
  })

  it('开发通道用 Shift + 右键透传原生菜单', () => {
    vi.stubEnv('VITE_DH_FIXTURE', '1')
    render(<><MessageRow /><ContextMenu /></>)
    expect(fireEvent.contextMenu(screen.getByText('你好'), { shiftKey: true })).toBe(true)
    expect(screen.queryByRole('menu')).not.toBeInTheDocument()
  })

  it('生产通道即使按住 Shift 也吞掉默认菜单', () => {
    render(<><MessageRow /><ContextMenu /></>)
    expect(fireEvent.contextMenu(screen.getByText('你好'), { shiftKey: true })).toBe(false)
    expect(screen.getByRole('menu')).toBeInTheDocument()
  })

  it('表头行不弹菜单', () => {
    render(<><MessageRow /><ContextMenu /></>)
    fireEvent.contextMenu(screen.getByText('群聊 / 成员'))
    expect(screen.queryByRole('menu')).not.toBeInTheDocument()
  })

  it('数据行复制落点所在列', async () => {
    const user = setupUser()
    render(<><MessageRow /><ContextMenu /></>)
    fireEvent.contextMenu(screen.getByText('你好'))
    await user.click(screen.getByRole('menuitem', { name: '复制这一列' }))
    expect(writeText).toHaveBeenCalledWith('你好')
  })

  it('数据行复制整行为 TSV，跳过无文本的操作列', async () => {
    const user = setupUser()
    render(<><MessageRow /><ContextMenu /></>)
    fireEvent.contextMenu(screen.getByText('你好'))
    await user.click(screen.getByRole('menuitem', { name: '复制整行' }))
    expect(writeText).toHaveBeenCalledWith('2026-08-07 10:00\t测试群张三\t你好\t文本\t待处理')
  })

  it('镜像行内按钮，点击菜单项真的触发原按钮', async () => {
    const user = setupUser()
    const onRecall = vi.fn()
    render(<><MessageRow onRecall={onRecall} /><ContextMenu /></>)
    fireEvent.contextMenu(screen.getByText('你好'))
    await user.click(screen.getByRole('menuitem', { name: '撤回消息' }))
    expect(onRecall).toHaveBeenCalledTimes(1)
  })

  it('原按钮 disabled 时菜单项不可点', async () => {
    const user = setupUser()
    const onRemove = vi.fn()
    render(<>
      <table className="member-table"><tbody><tr>
        <td><input type="checkbox" aria-label="选择张三" /></td>
        <td><strong>张三</strong></td>
        <td><div className="row-actions"><button title="移出群聊" disabled onClick={onRemove} /></div></td>
      </tr></tbody></table>
      <ContextMenu />
    </>)
    fireEvent.contextMenu(screen.getByText('张三'))
    const item = screen.getByRole('menuitem', { name: '移出群聊' })
    expect(item).toHaveAttribute('aria-disabled', 'true')
    await user.click(item)
    expect(onRemove).not.toHaveBeenCalled()
  })

  it('输入框给出剪切/复制/粘贴/全选，并把粘贴内容写回受控组件', async () => {
    const user = setupUser()
    function Controlled() {
      const [value, setValue] = useState('ab')
      return <input aria-label="备注" value={value} onChange={event => setValue(event.target.value)} />
    }
    render(<><Controlled /><ContextMenu /></>)
    const field = screen.getByLabelText('备注') as HTMLInputElement
    field.focus()
    field.setSelectionRange(0, 2)
    readText.mockResolvedValue('XY')

    fireEvent.contextMenu(field)
    expect(screen.getAllByRole('menuitem').map(node => node.textContent)).toEqual(['剪切', '复制', '粘贴', '全选'])

    await user.click(screen.getByRole('menuitem', { name: '粘贴' }))
    await waitFor(() => expect(field.value).toBe('XY'))
  })

  it('密码框不提供复制和剪切', () => {
    render(<><input type="password" aria-label="API Key" defaultValue="secret" /><ContextMenu /></>)
    const field = screen.getByLabelText('API Key')
    field.focus()
    fireEvent.contextMenu(field)
    const labels = screen.getAllByRole('menuitem').map(node => node.textContent)
    expect(labels).toEqual(['粘贴', '全选'])
  })

  it('链接给出复制链接地址', async () => {
    const user = setupUser()
    render(<><a href="https://t.me/dh114514">Telegram</a><ContextMenu /></>)
    fireEvent.contextMenu(screen.getByText('Telegram'))
    await user.click(screen.getByRole('menuitem', { name: '复制链接地址' }))
    expect(writeText).toHaveBeenCalledWith('https://t.me/dh114514')
  })

  it('方向键移动焦点，Escape 关闭并把焦点还给右键前的元素', async () => {
    const user = setupUser()
    render(<><input aria-label="备注" defaultValue="ab" /><ContextMenu /></>)
    const field = screen.getByLabelText('备注')
    field.focus()
    fireEvent.contextMenu(field)

    // 没有选区时"剪切/复制"是 disabled，方向键只在可用项之间移动，所以第一下落在"粘贴"。
    await user.keyboard('{ArrowDown}')
    expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: '粘贴' }))

    await user.keyboard('{ArrowDown}')
    expect(document.activeElement).toBe(screen.getByRole('menuitem', { name: '全选' }))

    await user.keyboard('{Escape}')
    expect(screen.queryByRole('menu')).not.toBeInTheDocument()
    expect(document.activeElement).toBe(field)
  })

  it('复制失败时保留菜单并给出快捷键提示', async () => {
    const user = setupUser()
    writeText.mockRejectedValueOnce(new Error('denied'))
    vi.spyOn(console, 'error').mockImplementation(() => undefined)
    render(<><MessageRow /><ContextMenu /></>)
    fireEvent.contextMenu(screen.getByText('你好'))
    await user.click(screen.getByRole('menuitem', { name: '复制这一列' }))
    expect(await screen.findByRole('status')).toHaveTextContent('请改用 Ctrl+C / Ctrl+V')
    expect(screen.getByRole('menu')).toBeInTheDocument()
  })

  it('点击菜单外部关闭菜单', async () => {
    const user = setupUser()
    render(<><MessageRow /><ContextMenu /></>)
    fireEvent.contextMenu(screen.getByText('你好'))
    expect(screen.getByRole('menu')).toBeInTheDocument()
    await user.click(document.body)
    expect(screen.queryByRole('menu')).not.toBeInTheDocument()
  })
})
