import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import RustLoginView from './RustLoginView'
import WangLoginView from './WangLoginView'

const loggedOut = { mode: 'rust', configured: true, authenticated: false, nimConnected: false, captchaId: 'synthetic', lastError: null }
afterEach(() => { vi.unstubAllGlobals(); delete window.initNECaptcha })

it('submits only after the verification callback, then refreshes and logs out', async () => {
  let verify: ((error: unknown, data?: { validate: string }) => void) | undefined
  const open = vi.fn()
  window.initNECaptcha = (options, ready) => {
    verify = options.onVerify
    ready({ verify: open, refresh: vi.fn(), destroy: vi.fn() })
  }
  const connected = { ...loggedOut, authenticated: true, nimConnected: true }
  const fetcher = vi.fn().mockResolvedValueOnce({ ok: true, json: async () => connected })
    .mockResolvedValueOnce({ ok: true, json: async () => connected })
    .mockResolvedValueOnce({ ok: true, json: async () => loggedOut })
  vi.stubGlobal('fetch', fetcher)
  render(<RustLoginView initial={loggedOut} />)
  await waitFor(() => expect(verify).toBeDefined())
  fireEvent.change(screen.getByLabelText('旺商聊账号或手机号'), { target: { value: 'synthetic-account' } })
  fireEvent.change(screen.getByLabelText('密码'), { target: { value: 'synthetic-password' } })
  fireEvent.click(screen.getByRole('button', { name: '登录', exact: true }))
  expect(open).toHaveBeenCalledOnce()
  expect(fetcher).not.toHaveBeenCalled()
  await act(async () => verify?.(null, { validate: 'synthetic-human-result' }))
  expect(JSON.parse(fetcher.mock.calls[0][1].body)).toEqual({ account: 'synthetic-account', password: 'synthetic-password', validateStr: 'synthetic-human-result' })
  expect(screen.getByRole('status')).toHaveTextContent('已连接')
  fireEvent.click(screen.getByRole('button', { name: '续期并重连' }))
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2))
  await waitFor(() => expect(screen.getByRole('button', { name: '退出账号' })).toBeEnabled())
  fireEvent.click(screen.getByRole('button', { name: '退出账号' }))
  await waitFor(() => expect(screen.getByLabelText('密码')).toHaveValue(''))
  expect(fetcher.mock.calls.map(call => call[0])).toEqual(['/api/wang/login', '/api/wang/refresh', '/api/wang/logout'])
})

it('uses saved credentials without rendering or submitting the stored password', async () => {
  let verify: ((error: unknown, data?: { validate: string }) => void) | undefined
  window.initNECaptcha = (options, ready) => { verify = options.onVerify; ready({ verify: vi.fn(), refresh: vi.fn() }) }
  const saved = { ...loggedOut, savedAccount: 'saved-account' }
  const fetcher = vi.fn().mockResolvedValue({ ok: true, json: async () => saved })
  vi.stubGlobal('fetch', fetcher)
  render(<RustLoginView initial={saved} />)
  await waitFor(() => expect(verify).toBeDefined())
  expect(screen.getByLabelText('密码')).toHaveValue('')
  expect(screen.getByRole('button', { name: '删除已保存的账号密码' })).toHaveClass('secondary')
  fireEvent.click(screen.getByRole('button', { name: '登录', exact: true }))
  await act(async () => verify?.(null, { validate: 'human-result' }))
  expect(JSON.parse(fetcher.mock.calls[0][1].body)).toMatchObject({ account: 'saved-account', password: '', useSavedPassword: true, rememberPassword: true })
  fireEvent.change(screen.getByLabelText('旺商聊账号或手机号'), { target: { value: 'other-account' } })
  expect(screen.getByRole('button', { name: '登录', exact: true })).toBeDisabled()
})

it('does not fall back to the desktop bridge when status is busy', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ ok: false, status: 409 }))
  render(<WangLoginView />)
  await act(async () => {})
  expect(screen.getByRole('status')).toHaveTextContent('正在读取登录状态')
  expect(screen.queryByRole('button', { name: '在 Web 中登录旺商聊' })).not.toBeInTheDocument()
})

it('reports captcha failures instead of silently ignoring the click', async () => {
  let verify: ((error: unknown, data?: { validate: string }) => void) | undefined
  window.initNECaptcha = (options, ready) => { verify = options.onVerify; ready({ verify: vi.fn(), refresh: vi.fn() }) }
  const fetcher = vi.fn(); vi.stubGlobal('fetch', fetcher)
  render(<RustLoginView initial={loggedOut} />)
  await waitFor(() => expect(verify).toBeDefined())
  expect(screen.getByRole('button', { name: '登录', exact: true })).toBeDisabled()
  fireEvent.change(screen.getByLabelText('旺商聊账号或手机号'), { target: { value: 'synthetic-account' } })
  fireEvent.change(screen.getByLabelText('密码'), { target: { value: 'synthetic-password' } })
  fireEvent.click(screen.getByRole('button', { name: '登录', exact: true }))
  await act(async () => verify?.(new Error('synthetic load failure')))
  expect(screen.getByRole('alert')).toHaveTextContent('安全验证未通过或加载失败')
  expect(fetcher).not.toHaveBeenCalled()
})

it('continues device verification through SMS and retains the form after a wrong code', async () => {
  let verify: ((error: unknown, data?: { validate: string }) => void) | undefined
  window.initNECaptcha = (options, ready) => { verify = options.onVerify; ready({ verify: vi.fn(), refresh: vi.fn() }) }
  const sms = { ...loggedOut, challenge: { id: 'synthetic-challenge', kind: 'deviceSms', maskedPhone: '****1234', expiresIn: 600, resendAfter: 60 } }
  const fetcher = vi.fn().mockResolvedValueOnce({ ok: true, json: async () => sms })
    .mockResolvedValueOnce({ ok: true, json: async () => ({ ...sms, lastError: 'sms_failed', message: '验证码校验未通过，请检查验证码后重试' }) })
    .mockResolvedValueOnce({ ok: true, json: async () => ({ ...loggedOut, authenticated: true, nimConnected: true }) })
  vi.stubGlobal('fetch', fetcher)
  render(<RustLoginView initial={loggedOut} />)
  await waitFor(() => expect(verify).toBeDefined())
  fireEvent.change(screen.getByLabelText('旺商聊账号或手机号'), { target: { value: 'synthetic-account' } })
  fireEvent.change(screen.getByLabelText('密码'), { target: { value: 'synthetic-password' } })
  fireEvent.click(screen.getByRole('button', { name: '登录', exact: true }))
  await act(async () => verify?.(null, { validate: 'synthetic-validation' }))
  expect(screen.getByRole('status')).toHaveTextContent('****1234')
  expect(screen.queryByLabelText('密码')).not.toBeInTheDocument()
  expect(screen.getByRole('button', { name: /秒后重发/ })).toBeDisabled()
  for (const code of ['123456', '654321']) {
    fireEvent.change(screen.getByLabelText('短信验证码'), { target: { value: code } })
    fireEvent.click(screen.getByRole('button', { name: '验证并登录' }))
    await act(async () => verify?.(null, { validate: 'synthetic-validation' }))
    if (code === '123456') expect(screen.getByRole('alert')).toHaveTextContent('验证码校验未通过')
  }
  expect(fetcher.mock.calls[1][0]).toBe('/api/wang/verify-sms')
  expect(JSON.parse(fetcher.mock.calls[1][1].body)).toEqual({ challengeId: 'synthetic-challenge', verificationCode: '123456', validateStr: 'synthetic-validation' })
  expect(screen.getByRole('status')).toHaveTextContent('已连接')
})

it('resends device SMS with a new human validation result and restores the cooldown', async () => {
  let verify: ((error: unknown, data?: { validate: string }) => void) | undefined
  window.initNECaptcha = (options, ready) => { verify = options.onVerify; ready({ verify: vi.fn(), refresh: vi.fn() }) }
  const initial = { ...loggedOut, challenge: { id: 'synthetic-challenge', kind: 'deviceSms' as const, maskedPhone: '****1234', expiresIn: 600, resendAfter: 0 } }
  const fetcher = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ ...initial, challenge: { ...initial.challenge, resendAfter: 60 } }) })
  vi.stubGlobal('fetch', fetcher)
  render(<RustLoginView initial={initial} />)
  await waitFor(() => expect(verify).toBeDefined())
  fireEvent.click(screen.getByRole('button', { name: '重新发送验证码' }))
  expect(fetcher).not.toHaveBeenCalled()
  await act(async () => verify?.(null, { validate: 'synthetic-new-validation' }))
  expect(fetcher.mock.calls[0][0]).toBe('/api/wang/resend-sms')
  expect(JSON.parse(fetcher.mock.calls[0][1].body)).toEqual({ challengeId: 'synthetic-challenge', validateStr: 'synthetic-new-validation' })
  expect(screen.getByRole('button', { name: /秒后重发/ })).toBeDisabled()
})

it('starts independent SMS login only after human verification', async () => {
  let verify: ((error: unknown, data?: { validate: string }) => void) | undefined
  window.initNECaptcha = (options, ready) => { verify = options.onVerify; ready({ verify: vi.fn(), refresh: vi.fn() }) }
  const fetcher = vi.fn().mockResolvedValue({ ok: true, json: async () => ({ ...loggedOut, challenge: { id: 'sms', kind: 'deviceSms', maskedPhone: '****1234', expiresIn: 600, resendAfter: 60 } }) })
  vi.stubGlobal('fetch', fetcher)
  render(<RustLoginView initial={loggedOut} />)
  await waitFor(() => expect(verify).toBeDefined())
  fireEvent.click(screen.getByRole('button', { name: '短信验证码登录' }))
  expect(screen.queryByLabelText('密码')).not.toBeInTheDocument()
  fireEvent.change(screen.getByLabelText('手机号（+86）'), { target: { value: '15555551234' } })
  fireEvent.click(screen.getByRole('button', { name: '获取短信验证码' }))
  expect(fetcher).not.toHaveBeenCalled()
  await act(async () => verify?.(null, { validate: 'human-result' }))
  expect(fetcher.mock.calls[0][0]).toBe('/api/wang/start-sms')
  expect(JSON.parse(fetcher.mock.calls[0][1].body)).toEqual({ phone: '15555551234', validateStr: 'human-result' })
  expect(screen.getByLabelText('短信验证码')).toBeInTheDocument()
})
