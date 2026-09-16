import { useEffect, useRef, useState } from 'react'
import RustLoginView, { type RustLoginStatus } from './RustLoginView'
export default function WangLoginView({ officialReady = false, accountActionsTarget }: { officialReady?: boolean; accountActionsTarget?: HTMLElement | null }) {
  const [rust, setRust] = useState<RustLoginStatus | null>(null)
  const [readError, setReadError] = useState('')
  const [mode, setMode] = useState<'loading' | 'rust' | 'cdp'>('loading')
  useEffect(() => {
    const controller = new AbortController()
    let timer: ReturnType<typeof setTimeout>
    let failures = 0
    const read = async () => {
      try {
        const response = await fetch('/api/wang/status', { method: 'POST', credentials: 'same-origin', signal: AbortSignal.any([controller.signal, AbortSignal.timeout(8000)]) })
        if (response.status === 401) { window.dispatchEvent(new Event('dh-session-expired')); setReadError('管理会话已过期，请重新登录'); return }
        if (!response.ok) throw new Error(response.status === 409 ? '连接维护中，暂未读取到登录状态' : '登录状态服务暂不可用')
        const value = await response.json()
        if (value.mode === 'rust' && value.configured) { setRust(value); setMode('rust') }
        else if (value.mode === 'cdp') setMode('cdp')
        else throw new Error('mode')
      } catch (error) { if (!controller.signal.aborted) { failures++; if (failures >= 3) setReadError(error instanceof Error ? error.message : '登录状态读取失败'); timer = setTimeout(() => void read(), 2000) } }
    }
    void read()
    return () => { controller.abort(); clearTimeout(timer) }
  }, [])
  const [open, setOpen] = useState(false)
  const [frame, setFrame] = useState<{ image: string; width: number; height: number } | null>(null)
  const [status, setStatus] = useState('')
  const socket = useRef<WebSocket | null>(null)
  const lastMove = useRef(0)
  const dragging = useRef(false)
  const keyboard = useRef<HTMLInputElement>(null)
  useEffect(() => {
    if (!open) return
    setStatus('正在连接官方登录页…'); setFrame(null)
    const ws = new WebSocket(`${location.protocol === 'https:' ? 'wss:' : 'ws:'}//${location.host}/api/login-view`)
    socket.current = ws
    ws.onmessage = event => {
      try {
        const data = JSON.parse(event.data)
        if (data.type === 'frame') { setFrame(data); setStatus('请在下方官方页面中操作，滑块需你手动拖动') }
        else { setFrame(null); setStatus(data.type === 'complete' ? '登录完成，正在同步连接状态' : data.message || '官方页面已离开登录流程'); window.dispatchEvent(new Event('dh-events-reconnected')) }
      } catch { setStatus('登录画面读取失败') }
    }
    ws.onclose = () => { setFrame(null); setStatus(current => current.includes('登录完成') ? current : '映射已停止。请确认官方客户端位于登录页，然后关闭并重试。') }
    return () => { ws.close(); socket.current = null; dragging.current = false; setFrame(null) }
  }, [open])
  const send = (data: object) => { if (socket.current?.readyState === WebSocket.OPEN) socket.current.send(JSON.stringify(data)) }
  if (rust) return <RustLoginView initial={rust} accountActionsTarget={accountActionsTarget} />
  if (mode !== 'cdp') return <p role="status">{readError || '正在读取登录状态…'}</p>
  if (officialReady) return null
  return <section>
    <button onClick={() => setOpen(value => !value)}>{open ? '关闭登录映射' : '在 Web 中登录旺商聊'}</button>
    {open && <div role="region" aria-label="旺商聊官方登录映射">
      <p role="status">{status}</p>
      <p>此区域连接同机官方旺商聊。密码与验证码输入会转发到官方登录页，不写入 DH 数据库。登录后画面停止。</p>
      {frame && <div style={{ maxWidth: 1000, position: 'relative' }}>
        <img alt="旺商聊官方登录界面" draggable={false} src={`data:image/jpeg;base64,${frame.image}`} style={{ width: '100%', display: 'block', touchAction: 'none' }}
          onPointerDown={e => { e.preventDefault(); dragging.current = true; e.currentTarget.setPointerCapture(e.pointerId); keyboard.current?.focus({ preventScroll: true }); const r = e.currentTarget.getBoundingClientRect(); send({ type: 'pointer', action: 'down', x: (e.clientX-r.left)*frame.width/r.width, y: (e.clientY-r.top)*frame.height/r.height }) }}
          onPointerMove={e => { if (!dragging.current || performance.now()-lastMove.current < 35) return; lastMove.current=performance.now(); const r=e.currentTarget.getBoundingClientRect(); send({ type:'pointer',action:'move',x:Math.max(0,Math.min(frame.width-1,(e.clientX-r.left)*frame.width/r.width)),y:Math.max(0,Math.min(frame.height-1,(e.clientY-r.top)*frame.height/r.height)) }) }}
          onPointerUp={e => { if (!dragging.current) return; dragging.current=false; const r=e.currentTarget.getBoundingClientRect(); send({type:'pointer',action:'up',x:Math.max(0,Math.min(frame.width-1,(e.clientX-r.left)*frame.width/r.width)),y:Math.max(0,Math.min(frame.height-1,(e.clientY-r.top)*frame.height/r.height))}); e.currentTarget.releasePointerCapture(e.pointerId) }}
          onPointerCancel={() => { dragging.current=false; send({type:'pointer',action:'up',x:0,y:0}) }} />
        <input ref={keyboard} type="password" aria-label="官方登录页键盘输入" autoComplete="off" style={{ position:'absolute',width:1,height:1,opacity:0,left:0,bottom:0 }}
          onKeyDown={e => { if (['Backspace','Tab','Enter','Escape','ArrowLeft','ArrowRight','ArrowUp','ArrowDown','Delete'].includes(e.key)) { e.preventDefault(); send({type:'key',key:e.key}) } }}
          onChange={e => { if (!(e.nativeEvent as InputEvent).isComposing && e.target.value) {send({type:'text',text:e.target.value});e.target.value=''} }}
          onCompositionEnd={e => { if (e.currentTarget.value) {send({type:'text',text:e.currentTarget.value});e.currentTarget.value=''} }} />
      </div>}
    </div>}
  </section>
}
