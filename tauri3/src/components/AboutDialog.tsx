import { useEffect } from 'react'
import { ExternalLink, Send, X } from 'lucide-react'

type Props = {
  onClose: () => void
}

export default function AboutDialog({ onClose }: Props) {
  useEffect(() => {
    document.dispatchEvent(new CustomEvent('dh-hide-button-help'))
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose() }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [onClose])

  return <div className="about-backdrop" role="presentation" onMouseDown={event => { if (event.target === event.currentTarget) onClose() }}>
    <section className="about-dialog" role="dialog" aria-modal="true" aria-labelledby="about-title">
      <header className="about-header">
        <div><span className="eyebrow">关于应用</span><h2 id="about-title">DH BOT</h2></div>
        <button className="icon-button" data-help="关闭版本和作者信息窗口，回到当前页面。" aria-label="关闭关于窗口" onClick={onClose}><X size={17} /></button>
      </header>
      <div className="about-version"><strong>3.0.0-beta.1</strong><span>Rust + Tauri 桌面版</span></div>
      <p className="about-copy">面向群聊管理、知识库和 AI 协作的本地工作台。数据保存在当前电脑，群管理动作始终受账号权限和页面设置控制。</p>
      <dl className="about-details"><dt>作者</dt><dd>DH BOT 开发团队</dd><dt>联系</dt><dd><a className="telegram-link" href="https://t.me/dh114514" target="_blank" rel="noreferrer"><Send size={15} />Telegram：t.me/dh114514<ExternalLink size={13} /></a></dd><dt>运行方式</dt><dd>桌面应用 · 本地数据 · 旺商聊 DevTools</dd></dl>
      <footer className="about-footer"><span>感谢使用 DH BOT</span><button className="secondary" data-help="关闭关于窗口，继续使用 DH BOT。" onClick={onClose}>知道了</button></footer>
    </section>
  </div>
}
