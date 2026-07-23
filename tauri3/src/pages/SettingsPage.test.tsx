import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import AiAssistantPanel from './AiAssistantPanel'

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }))
vi.mock('@runtime-controls', () => ({ default: () => <div>连接诊断</div> }))

describe('AI 助手离群测试', () => {
  beforeEach(() => {
    mocks.invoke.mockReset().mockImplementation((command: string) => {
      if (command === 'get_close_behavior') return Promise.resolve('ask')
      if (command === 'test_ai') return Promise.resolve({
        decision: { reply: '请文明交流。', reason: '默认资料命中', confidence: 0.95 },
        elapsedMs: 18,
        model: 'deepseek-v4-pro',
        knowledgeSource: 'DH 默认群规与 FAQ',
      })
      return Promise.resolve(null)
    })
  })

  it('passes the optional built-in knowledge flag and reports the source', async () => {
    render(<AiAssistantPanel
      aiSettings={{ base_url: '', webhook_url: '', model: 'deepseek-v4-pro', api_key_configured: false }}
      setAiSettings={vi.fn()}
      refresh={vi.fn()}
      onError={vi.fn()}
    />)
    await userEvent.click(screen.getByLabelText('加载默认群规与 FAQ，仅用于本地测试'))
    await userEvent.type(screen.getByPlaceholderText('输入一条测试问题，例如：@DH 群规是什么？'), '@DH 群规是什么')
    await userEvent.click(screen.getByRole('button', { name: '测试 AI' }))
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith('test_ai', {
      message: '@DH 群规是什么',
      recentContext: [],
      includeBuiltInKnowledge: true,
    }))
    expect(await screen.findByText(/资料：DH 默认群规与 FAQ/)).toBeInTheDocument()
  })
})
