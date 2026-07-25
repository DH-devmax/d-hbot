import { useEffect, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { Copy, Minus, Square, X } from 'lucide-react'

function isTauriRuntime() {
  return typeof window !== 'undefined' && Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
}

function reportWindowError(action: string, reason: unknown) {
  console.error(`DH BOT window ${action} failed`, reason)
}

export default function WindowTitlebar() {
  const [maximized, setMaximized] = useState(false)

  useEffect(() => {
    if (!isTauriRuntime()) return
    const appWindow = getCurrentWindow()
    let active = true
    const syncMaximized = () => {
      void appWindow.isMaximized().then(value => {
        if (active) setMaximized(value)
      }).catch(reason => reportWindowError('isMaximized', reason))
    }
    syncMaximized()
    let unlisten: (() => void) | undefined
    void appWindow.onResized(syncMaximized).then(cleanup => {
      if (active) unlisten = cleanup
      else cleanup()
    }).catch(reason => reportWindowError('onResized', reason))
    return () => {
      active = false
      unlisten?.()
    }
  }, [])

  const minimize = () => {
    if (isTauriRuntime()) void getCurrentWindow().minimize().catch(reason => reportWindowError('minimize', reason))
  }

  const toggleMaximize = () => {
    if (!isTauriRuntime()) return
    const appWindow = getCurrentWindow()
    void appWindow.toggleMaximize().then(() => appWindow.isMaximized()).then(setMaximized).catch(reason => reportWindowError('toggleMaximize', reason))
  }

  const close = () => {
    if (isTauriRuntime()) void getCurrentWindow().close().catch(reason => reportWindowError('close', reason))
  }

  const toggleFromTitlebar = () => {
    if (!isTauriRuntime()) return
    const appWindow = getCurrentWindow()
    void appWindow.toggleMaximize().then(() => appWindow.isMaximized()).then(setMaximized).catch(reason => reportWindowError('toggleMaximize', reason))
  }

  return <header className="window-titlebar">
    <div className="window-titlebar__drag" data-tauri-drag-region onDoubleClick={toggleFromTitlebar} />
    <div className="window-titlebar__controls">
      <button data-window-control className="window-control" type="button" aria-label="Minimize" title="Minimize" onMouseDown={event => event.stopPropagation()} onClick={minimize}><Minus size={16} strokeWidth={1.8} /></button>
      <button data-window-control className="window-control" type="button" aria-label={maximized ? 'Restore' : 'Maximize'} title={maximized ? 'Restore' : 'Maximize'} onMouseDown={event => event.stopPropagation()} onClick={toggleMaximize}>{maximized ? <Copy size={14} strokeWidth={1.8} /> : <Square size={14} strokeWidth={1.8} />}</button>
      <button data-window-control className="window-control window-control--close" type="button" aria-label="Close" title="Close" onMouseDown={event => event.stopPropagation()} onClick={close}><X size={17} strokeWidth={1.8} /></button>
    </div>
  </header>
}
