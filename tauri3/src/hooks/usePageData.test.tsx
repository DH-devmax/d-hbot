import { render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import { usePageData } from './usePageData'

function Harness({ loader, identity }: { loader: () => Promise<string[]>; identity: string }) {
  const page = usePageData(loader, [identity], values => values.length === 0)
  return <div><span data-testid="state">{page.state}</span><span>{page.data?.join(',') || '-'}</span><button onClick={() => void page.reload()}>reload</button></div>
}

describe('usePageData', () => {
  it('moves through loading, ready and offlineCached without dropping data', async () => {
    const loader = vi.fn<() => Promise<string[]>>()
      .mockResolvedValueOnce(['本地缓存'])
      .mockRejectedValueOnce(new Error('连接断开'))
    render(<Harness loader={loader} identity="ACCOUNT-A" />)
    expect(screen.getByTestId('state')).toHaveTextContent(/idle|loading/)
    await screen.findByText('本地缓存')
    expect(screen.getByTestId('state')).toHaveTextContent('ready')
    await userEvent.click(screen.getByRole('button', { name: 'reload' }))
    await waitFor(() => expect(screen.getByTestId('state')).toHaveTextContent('offlineCached'))
    expect(screen.getByText('本地缓存')).toBeInTheDocument()
  })

  it('clears stale data when the account identity changes', async () => {
    let resolveSecond!: (value: string[]) => void
    const loader = vi.fn<() => Promise<string[]>>()
      .mockResolvedValueOnce(['ACCOUNT-A 数据'])
      .mockImplementationOnce(() => new Promise(resolve => { resolveSecond = resolve }))
    const { rerender } = render(<Harness loader={loader} identity="ACCOUNT-A" />)
    await screen.findByText('ACCOUNT-A 数据')
    rerender(<Harness loader={loader} identity="ACCOUNT-B" />)
    await waitFor(() => expect(screen.queryByText('ACCOUNT-A 数据')).not.toBeInTheDocument())
    resolveSecond(['ACCOUNT-B 数据'])
    await screen.findByText('ACCOUNT-B 数据')
  })
})
