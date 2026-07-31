import { CircleAlert, Info, TriangleAlert } from 'lucide-react'

export type AppNotice = {
  level: 'error' | 'warning' | 'info'
  title: string
  detail: string
  action: string
}

export function describeAppNotice(message: string): AppNotice {
  const value = message.trim()
  if (/^(已|成功|完成|已保存|已绑定|已解除|已导入)/.test(value)) {
    return { level: 'info', title: '操作完成', detail: value, action: '' }
  }
  if (value.includes('样式预览 IPC 尚未覆盖') || value.includes('get_developer_calibration_status')) {
    return {
      level: 'warning',
      title: '样式预览数据不完整',
      detail: '当前页面缺少开发校准状态的模拟响应，只影响样式预览，不影响生产程序。',
      action: '处理方式：刷新预览；若仍出现，请补充对应的预览数据命令。',
    }
  }
  if (/9222|DevTools|Devtools|json\/list|读取 DevTools/.test(value)) {
    return {
      level: 'error',
      title: '旺商聊 DevTools 连接失败',
      detail: 'DH BOT 暂时读不到旺商聊的本地页面。',
      action: '处理方式：确认旺商聊已登录并开启 9222，再到“调试”页面重试连接。',
    }
  }
  if (/NIM|会话.*就绪|尚未初始化|未就绪/.test(value)) {
    return {
      level: 'warning',
      title: '旺商聊会话尚未就绪',
      detail: 'DevTools 已连接，但旺商聊登录会话还没有完成初始化。',
      action: '处理方式：打开旺商聊完成登录，进入主界面后等待几秒，再点击“立即检查”。',
    }
  }
  if (/权限|管理员|管理权限|permission/i.test(value)) {
    return {
      level: 'warning',
      title: '当前账号权限不足',
      detail: '这项操作需要当前账号具备对应群的管理权限。',
      action: '处理方式：把账号设置为群管理员后刷新群列表，再重试。',
    }
  }
  if (/429|频繁|限流|too many/i.test(value)) {
    return {
      level: 'warning',
      title: '旺商聊请求过于频繁',
      detail: '短时间内的读取或写入请求超过了当前连接的处理速度。',
      action: '处理方式：暂停连续刷新和批量点击，等待片刻后只重试一次。',
    }
  }
  if (/网关请求排队超时|请求队列.*超时|Runtime\.evaluate/i.test(value)) {
    return {
      level: 'warning',
      title: '旺商聊请求队列拥堵',
      detail: '多个同步、消息或群管请求正在等待同一个 DevTools 通道，这不代表协议结构已变化。',
      action: '处理方式：暂停连续刷新，等待当前请求完成；详细页会标明是“同步成员”还是“发送消息”在排队。',
    }
  }
  if (/协议结构.*变化|路由缺失|响应结构.*变化|解码失败/i.test(value)) {
    return {
      level: 'error',
      title: '协议能力已暂停',
      detail: '当前旺商聊页面的路由、NIM 方法或响应结构与已识别协议不一致，只暂停受影响的自动写操作。',
      action: '处理方式：打开“调试”查看具体能力和协议指纹；重新登录旺商聊后再执行一次只读检查。',
    }
  }
  if (/禁止调用此接口|业务码\s*1001|business.*1001/i.test(value)) {
    return {
      level: 'error',
      title: '旺商聊拒绝了当前路由',
      detail: '连接本身正常，但旺商聊业务层不允许这条 HTTP 路由执行当前操作。',
      action: '处理方式：请查看审计中的“协议路由”与“业务说明”；撤回等能力会改用已验证的 NIM 链路，不会对 1001 自动重试。',
    }
  }
  if (/群名片.*(回读|验证).*(不一致|失败)|rename.*verification/i.test(value)) {
    return {
      level: 'warning',
      title: '群名片尚未真正生效',
      detail: 'DH BOT 已发出改名请求，但 NIM 群名片回读与目标名称不一致，因此不记为成功。',
      action: '处理方式：打开“群组与成员”查看后台队列和具体失败成员，等待队列重试或手工重试失败项。',
    }
  }
  if (/401|403|API Key|密钥|Provider|AI 服务/i.test(value)) {
    return {
      level: 'error',
      title: 'AI 服务配置需要检查',
      detail: 'AI 连接地址、模型或密钥没有通过服务端校验。',
      action: '处理方式：到“知识与 AI → AI 助手”检查地址、模型和密钥，再执行测试 AI。',
    }
  }
  if (/超时|timeout/i.test(value)) {
    return {
      level: 'warning',
      title: '操作等待超时',
      detail: '服务没有在规定时间内返回结果，当前状态可能仍在处理中。',
      action: '处理方式：先到“审计”确认结果，再决定是否重试，避免重复操作。',
    }
  }
  if (/SQLite|数据库|database|quick_check/i.test(value)) {
    return {
      level: 'error',
      title: '本地数据需要检查',
      detail: 'DH BOT 读取本地数据库时遇到问题，原数据不会被自动覆盖。',
      action: '处理方式：打开“调试”查看数据库完整性和备份路径，确认后再恢复。',
    }
  }
  return {
    level: 'error',
    title: '操作未完成',
    detail: value || '没有收到具体错误信息。',
    action: '处理方式：打开“调试”页面查看连接状态和最近错误，再重试一次。',
  }
}

export default function ErrorBanner({ message, onClose }: { message: string; onClose: () => void }) {
  const notice = describeAppNotice(message)
  const Icon = notice.level === 'info' ? Info : notice.level === 'warning' ? TriangleAlert : CircleAlert
  return <div className={`error-banner error-banner--${notice.level}`} role="alert">
    <Icon size={17} aria-hidden="true" />
    <div className="error-banner-copy">
      <strong>{notice.title}</strong>
      <span>{notice.detail}</span>
      {notice.action && <small>{notice.action}</small>}
    </div>
    <button type="button" aria-label="关闭提示" onClick={onClose}>关闭</button>
  </div>
}
