import { RefreshCw } from 'lucide-react'
import type { RuntimeControlProps } from './runtimeTypes'
export default function RuntimeControls({ diagnostic, loading, refresh, setError }: RuntimeControlProps) {
  return <div><h2>连接状态</h2><p role="status">{diagnostic?.detail || '正在读取连接状态'}</p>
    <button className="secondary" disabled={loading} onClick={() => void refresh().catch(error => setError(String(error)))}><RefreshCw size={16} />重新检查</button>
  </div>
}
