import { Activity, CircleAlert, RefreshCw, WifiOff } from 'lucide-react'
import type { ReactNode } from 'react'
import type { LoadState } from '../types'

export function EmptyState({ title, detail, action }: { title: string; detail: string; action?: ReactNode }) {
  return <div className="empty"><div className="empty-icon"><Activity size={18} /></div><strong>{title}</strong><p>{detail}</p>{action}</div>
}

export function PageFeedback({ state, error, emptyTitle, emptyDetail, retry, children }: { state: LoadState; error?: string; emptyTitle: string; emptyDetail: string; retry?: () => void; children: ReactNode }) {
  if (state === 'idle' || state === 'loading') return <div className="page-feedback"><RefreshCw className="spin" size={18} /><strong>正在读取</strong><span>数据会在当前页面就绪后显示。</span></div>
  if (state === 'error') return <div className="page-feedback error"><CircleAlert size={18} /><strong>读取失败</strong><span>{error || '请稍后重试。'}</span>{retry && <button className="secondary" onClick={retry}>重新加载</button>}</div>
  if (state === 'empty') return <EmptyState title={emptyTitle} detail={emptyDetail} action={retry && <button className="secondary" onClick={retry}>重新加载</button>} />
  return <>{state === 'offlineCached' && <div className="cached-banner"><WifiOff size={15} />当前展示本地缓存，恢复连接后会自动刷新。</div>}{children}</>
}

export function SectionHeading({ eyebrow, title, meta, actions }: { eyebrow: string; title: string; meta?: ReactNode; actions?: ReactNode }) {
  return <div className="section-head"><div><span className="eyebrow">{eyebrow}</span><h2>{title}</h2></div><div className="section-actions">{meta && <span className="muted">{meta}</span>}{actions}</div></div>
}
