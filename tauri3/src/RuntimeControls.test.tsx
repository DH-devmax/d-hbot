import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import RuntimeControls from './RuntimeControls.production'

const invoke = vi.hoisted(() => vi.fn())

vi.mock('@tauri-apps/api/core', () => ({ invoke }))

describe('production WangShangLiao startup settings', () => {
  beforeEach(() => {
    window.localStorage.clear()
    invoke.mockReset().mockImplementation((command: string) => {
      if (command === 'get_wang_startup_settings') {
        return Promise.resolve({ path: 'C:\\Apps\\wangshangliao.exe', autoStart: true })
      }
      if (command === 'get_wang_profile_status') {
        return Promise.resolve({ state: 'patched', detail: '固定登录分区已启用', scriptHash: 'HASH', backupPath: 'backup', requiresElevation: false })
      }
      if (command === 'locate_wangshangliao') return Promise.resolve([])
      return Promise.resolve(null)
    })
  })

  it('uses backend settings instead of a stale browser path', async () => {
    window.localStorage.setItem('dh.wangshangliao.path', 'D:\\Stale\\wangshangliao.exe')
    render(<RuntimeControls diagnostic={null} loading={false} refresh={vi.fn()} setError={vi.fn()} />)

    expect(await screen.findByDisplayValue('C:\\Apps\\wangshangliao.exe')).toBeVisible()
    expect(screen.queryByDisplayValue('D:\\Stale\\wangshangliao.exe')).not.toBeInTheDocument()
  })

  it('loads and persists the auto-start preference with the selected path', async () => {
    const user = userEvent.setup()
    render(<RuntimeControls diagnostic={null} loading={false} refresh={vi.fn()} setError={vi.fn()} />)

    expect(await screen.findByDisplayValue('C:\\Apps\\wangshangliao.exe')).toBeVisible()
    const checkbox = screen.getByRole('checkbox', { name: '打开 DH BOT 时自动启动或显示旺商聊' })
    expect(checkbox).toBeChecked()
    expect(await screen.findByText('账号登录复用：已启用')).toBeVisible()
    await user.click(checkbox)
    await user.click(screen.getByRole('button', { name: '保存启动设置' }))

    expect(invoke).toHaveBeenCalledWith('save_wang_startup_settings', {
      settings: { path: 'C:\\Apps\\wangshangliao.exe', autoStart: false },
    })
  })

  it('waits for elevated profile maintenance and resumes the confirmed restart', async () => {
    const user = userEvent.setup()
    let confirmedStarts = 0
    invoke.mockImplementation((command: string, payload?: { confirmRestart?: boolean }) => {
      if (command === 'get_wang_startup_settings') {
        return Promise.resolve({ path: 'C:\\Apps\\wangshangliao.exe', autoStart: true })
      }
      if (command === 'locate_wangshangliao') return Promise.resolve([])
      if (command === 'get_wang_profile_status') {
        return Promise.resolve({ state: 'patched', detail: '固定登录分区已启用', scriptHash: 'HASH', backupPath: 'backup', requiresElevation: false })
      }
      if (command === 'save_wang_startup_settings') return Promise.resolve(null)
      if (command === 'get_wang_maintenance_result') {
        return Promise.resolve({ success: true, errorCode: '', message: '维护完成' })
      }
      if (command === 'start_wangshangliao') {
        if (!payload?.confirmRestart) {
          return Promise.resolve({ needsConfirmation: true, detail: '需要确认' })
        }
        confirmedStarts += 1
        return Promise.resolve(confirmedStarts === 1
          ? { needsConfirmation: false, detail: '等待 UAC', maintenanceRequestId: 'maintenance-1' }
          : { needsConfirmation: false, detail: '已重启', maintenanceRequestId: null })
      }
      return Promise.resolve(null)
    })
    vi.spyOn(window, 'confirm').mockReturnValue(true)
    const refresh = vi.fn().mockResolvedValue(undefined)
    render(<RuntimeControls diagnostic={null} loading={false} refresh={refresh} setError={vi.fn()} />)

    await screen.findByDisplayValue('C:\\Apps\\wangshangliao.exe')
    await user.click(screen.getByRole('button', { name: '启动 / 显示旺商聊' }))

    await waitFor(() => expect(confirmedStarts).toBe(2), { timeout: 2_000 })
    expect(invoke).toHaveBeenCalledWith('get_wang_maintenance_result', { requestId: 'maintenance-1' })
    expect(refresh).toHaveBeenCalled()
  })

  it('applies and reports the persistent login profile from settings', async () => {
    const user = userEvent.setup()
    invoke.mockImplementation((command: string) => {
      if (command === 'get_wang_startup_settings') return Promise.resolve({ path: 'C:\\Apps\\wangshangliao.exe', autoStart: true })
      if (command === 'locate_wangshangliao') return Promise.resolve([])
      if (command === 'save_wang_startup_settings') return Promise.resolve(null)
      if (command === 'get_wang_profile_status') return Promise.resolve({ state: 'maintenance-required', detail: '等待启用', scriptHash: 'OLD', requiresElevation: false })
      if (command === 'apply_wang_profile_patch') return Promise.resolve({ state: 'patched', detail: '固定登录分区已启用', scriptHash: 'NEW', backupPath: 'backup', requiresElevation: false })
      return Promise.resolve(null)
    })
    const setError = vi.fn()
    render(<RuntimeControls diagnostic={null} loading={false} refresh={vi.fn()} setError={setError} />)

    await screen.findByDisplayValue('C:\\Apps\\wangshangliao.exe')
    await user.click(screen.getByRole('button', { name: '启用账号复用' }))

    await waitFor(() => expect(setError).toHaveBeenCalledWith(expect.stringContaining('账号记录复用已启用')))
    expect(invoke).toHaveBeenCalledWith('apply_wang_profile_patch', { path: 'C:\\Apps\\wangshangliao.exe' })
  })
})
