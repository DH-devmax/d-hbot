import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({ exportSupportBundle: vi.fn() }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@calibration-controls', () => ({ default: () => null }))
vi.mock('../api/client', () => ({
  api: { exportSupportBundle: mocks.exportSupportBundle },
  readableError: (value: unknown) => String(value),
}))

import DebugPage from './DebugPage'

describe('调试页诊断包', () => {
  afterEach(() => { vi.unstubAllEnvs(); vi.unstubAllGlobals() })
  beforeEach(() => {
    mocks.exportSupportBundle.mockReset().mockResolvedValue({
      path: 'C:\\Users\\***\\AppData\\Roaming\\DH\\3.0\\support-bundles\\DH-BOT-support-20260730-120000.zip',
      sha256: 'a'.repeat(64),
      includedFiles: 5,
      generatedAt: '2026-07-30T12:00:00Z',
    })
  })

  it('generates a local support bundle and displays the returned checksum', async () => {
    render(<DebugPage
      diagnostic={null}
      database={{ path: 'C:\\Users\\***\\AppData\\Roaming\\DH\\3.0\\dh.db', schemaVersion: 10, integrity: 'ok', accounts: 1, groups: 2, messages: 3 }}
      refresh={vi.fn(async () => undefined)}
      onError={vi.fn()}
    />)

    await userEvent.click(screen.getByRole('button', { name: '生成诊断包' }))

    await waitFor(() => expect(mocks.exportSupportBundle).toHaveBeenCalledOnce())
    expect(await screen.findByText('诊断包已生成')).toBeInTheDocument()
    expect(screen.getByText(/DH-BOT-support-20260730-120000\.zip/)).toBeInTheDocument()
    expect(screen.getByText(/SHA-256：a{64}/)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '复制路径' })).toBeEnabled()
  })

  it('loads Web capabilities and offers a download without desktop maintenance', async () => {
    vi.stubEnv('MODE', 'web')
    const fetch = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ sendText: { status: 'unsupported', reason: '协议尚未接入', source: 'nimRuntime' } }) })
    vi.stubGlobal('fetch', fetch)
    mocks.exportSupportBundle.mockResolvedValue({ path: 'dh-support.zip', downloadUrl: '/api/downloads/synthetic-id', sha256: 'b'.repeat(64), includedFiles: 5, generatedAt: '2026-09-16T00:00:00Z' })
    render(<DebugPage diagnostic={null} database={null} refresh={vi.fn(async () => undefined)} onError={vi.fn()} />)
    await waitFor(() => expect(fetch).toHaveBeenCalledWith('/api/commands/get_gateway_capabilities', expect.anything()))
    expect(screen.queryByText('旺商聊脚本维护')).not.toBeInTheDocument()
    expect(screen.queryByText('DevTools')).not.toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: '生成诊断包' }))
    expect(await screen.findByRole('link', { name: '下载诊断包' })).toHaveAttribute('href', '/api/downloads/synthetic-id')
    expect(screen.queryByRole('button', { name: '复制路径' })).not.toBeInTheDocument()
  })
})
