import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import AdminForm from './AdminForm'
afterEach(() => vi.unstubAllGlobals())
it('clears reset instructions when returning to login', () => {
 render(<AdminForm initialMode="login" onComplete={vi.fn()} />)
 fireEvent.click(screen.getByRole('button', { name: '忘记管理员密码' }))
 expect(screen.getByRole('status')).toHaveTextContent('网页不会自动生成')
 fireEvent.click(screen.getByRole('button', { name: '返回登录' }))
 expect(screen.queryByRole('status')).not.toBeInTheDocument()
})
it('prevents duplicate submission and presents success as status', async () => {
 let finish!: (value: unknown) => void
 const fetcher=vi.fn().mockReturnValue(new Promise(resolve => {finish=resolve}))
 vi.stubGlobal('fetch',fetcher)
 const {container}=render(<AdminForm initialMode="activate" onComplete={vi.fn()} />)
 fireEvent.change(screen.getByLabelText('管理员密码'),{target:{value:'Independent test password!'}})
 fireEvent.change(screen.getByLabelText('确认密码'),{target:{value:'Independent test password!'}})
 fireEvent.submit(container.querySelector('form')!);fireEvent.submit(container.querySelector('form')!)
 expect(fetcher).toHaveBeenCalledTimes(1)
 await act(async()=>finish({ok:true}))
 await waitFor(()=>expect(screen.getByRole('status')).toHaveTextContent('设置成功'))
 expect(screen.queryByRole('alert')).not.toBeInTheDocument()
 expect(screen.getByLabelText('管理员密码')).toHaveValue('')
})
it('uses a valid HTML unicode-sets username pattern',()=>{
 render(<AdminForm initialMode="login" onComplete={vi.fn()} />)
 const input=screen.getByLabelText('管理员账号') as HTMLInputElement
 expect(new RegExp(`^(?:${input.pattern})$`,'v').test('owner-test')).toBe(true)
 expect(new RegExp(`^(?:${input.pattern})$`,'v').test('owner test')).toBe(false)
})
