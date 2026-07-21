interface CloseDialogProps {
  remember: boolean
  onRememberChange: (value: boolean) => void
  onCancel: () => void
  onResolve: (action: 'tray' | 'exit') => void
}

export default function CloseDialog({ remember, onRememberChange, onCancel, onResolve }: CloseDialogProps) {
  return <div className="dialog-backdrop" role="presentation">
    <section className="close-dialog" role="dialog" aria-modal="true" aria-labelledby="close-dialog-title">
      <span className="eyebrow">关闭 DH BOT</span>
      <h2 id="close-dialog-title">接下来怎么处理？</h2>
      <p>挂到托盘后群管理仍在后台运行；选择退出会先保存状态并结束后台任务。</p>
      <label className="remember-choice">
        <input type="checkbox" checked={remember} onChange={event => onRememberChange(event.target.checked)} />
        记住本次选择
      </label>
      <div className="dialog-actions">
        <button className="secondary" onClick={onCancel}>取消</button>
        <button className="secondary" onClick={() => onResolve('tray')}>挂到托盘</button>
        <button className="primary danger" onClick={() => onResolve('exit')}>退出 DH BOT</button>
      </div>
    </section>
  </div>
}
