import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import WebSession from './WebSession'
afterEach(() => { vi.unstubAllGlobals(); vi.unstubAllEnvs() })
it('requires administrator username and password after setup discovery', async () => {
 vi.stubEnv('MODE','web')
 const fetcher=vi.fn().mockImplementation(async (url:string) => url==='/api/setup/status'?{ok:true,json:async()=>({mode:'login'})}:url==='/api/session'?{ok:false}:{ok:true})
 vi.stubGlobal('fetch',fetcher)
 render(<WebSession><div>Authenticated content</div></WebSession>)
 await screen.findByLabelText('管理员账号')
 fireEvent.change(screen.getByLabelText('管理员账号'),{target:{value:'owner'}})
 fireEvent.change(screen.getByLabelText('管理员密码'),{target:{value:'synthetic-password'}})
 fireEvent.click(screen.getByRole('button',{name:'登录',exact:true}))
 await waitFor(()=>expect(screen.getByText('Authenticated content')).toBeInTheDocument())
 expect(JSON.parse(fetcher.mock.calls.find(c=>c[0]==='/api/login')![1].body)).toEqual({username:'owner',password:'synthetic-password'})
})
it('shows activation instead of business content when uninitialized', async()=>{
 vi.stubEnv('MODE','web');vi.stubGlobal('fetch',vi.fn().mockImplementation(async(url:string)=>url==='/api/setup/status'?{ok:true,json:async()=>({mode:'activate'})}:{ok:false}))
 render(<WebSession><div>Business secret</div></WebSession>)
 expect(await screen.findByLabelText('一次性激活码')).toBeInTheDocument()
 expect(screen.queryByText('Business secret')).not.toBeInTheDocument()
 expect(screen.getByLabelText('确认密码')).toBeInTheDocument()
})
