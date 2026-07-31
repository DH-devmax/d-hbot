import { useEffect, useState } from 'react'

type TooltipState = {
  target: HTMLElement
  text: string
  left: number
  top: number
  below: boolean
}

const buttonSelector = 'button, [role="button"], label.file-button, label.icon-button'

const helpRules: Array<[RegExp, string]> = [
  [/AI 总开关/, '控制当前群是否使用 AI。关闭后，即使有人 @DH，也不会调用模型或执行任何 AI 动作。'],
  [/自动回复/, '允许 AI 回答群里明确 @DH 或 @当前登录账号的业务问题；普通聊天不会触发。'],
  [/生成任务/, '允许 AI 把对话里明确的待办事项写入 DH BOT 任务列表。'],
  [/AI 规则撤回/, '允许 AI 控制规则在置信度达标后撤回消息；机器规则撤回使用独立开关，不需要这项权限。'],
  [/机器规则/, '机器规则按关键词、行数、长度等确定条件立即判断，不调用 AI，连续违规会逐条处理。'],
  [/允许禁言/, '允许 AI 根据返回动作禁言普通成员；执行前仍会检查账号管理权限。'],
  [/允许移出/, '允许 AI 将普通成员移出群聊，影响较大，建议只在规则充分验证后开启。'],
  [/保存.*(?:设置|计划|任务|规则|文档|绑定)/, '保存当前页面填写的内容。保存成功后，新设置才会正式生效。'],
  [/刷新|重新检查|立即检查/, '重新读取最新状态和数据，不会修改群聊内容。'],
  [/重新同步成员/, '重新读取当前群成员名单，并更新人数、名称和账号状态。'],
  [/新建/, '新建一条内容，填写完成后还需要点击保存。'],
  [/删除/, '删除当前选中的内容；重要数据执行前会再次确认。'],
  [/导入/, '从本机文件读取内容，检查通过后再加入当前页面。'],
  [/导出|CSV|JSON/, '把当前数据整理成文件，方便备份、查看或继续处理。'],
  [/测试 AI/, '使用当前 AI 设置进行本机测试，不读取群消息，也不会执行群管理动作。'],
  [/发送/, '把当前填写的内容发送到已选择的群；发送前请检查群和正文。'],
  [/上一页/, '查看上一页数据，不会改变当前筛选条件。'],
  [/下一页/, '查看下一页数据，不会改变当前筛选条件。'],
  [/关闭/, '关闭当前提示或面板，已保存的数据不会受影响。'],
  [/取消选择/, '清空当前勾选的成员，已执行的操作不会撤销。'],
  [/禁言/, '让选中的普通成员在设定时间内停止发言；执行前会再次确认。'],
  [/解禁/, '解除选中成员的禁言状态，让其恢复发言。'],
  [/黑名单/, '调整选中成员的黑名单状态，后续重新入群时会按设置处理。'],
  [/移出/, '将选中的普通成员移出当前群；执行前会再次确认。'],
]

function cleanLabel(target: HTMLElement) {
  const aria = target.getAttribute('aria-label')?.trim()
  if (aria) return aria
  return (target.innerText || target.textContent || '').replace(/\s+/g, ' ').trim()
}

export function buttonHelpText(target: HTMLElement) {
  const explicit = target.dataset.help?.trim()
  if (explicit) return explicit
  const nativeTitle = target.getAttribute('title')?.trim()
  if (nativeTitle) return nativeTitle
  const label = cleanLabel(target)
  if (target.classList.contains('nav-item')) return `打开“${label}”页面，查看和管理这部分功能。`
  if (target.classList.contains('group-item')) return `选择“${label.slice(0, 28)}”，并打开这个群的成员和管理设置。`
  if (target.classList.contains('work-item')) return `打开“${label.slice(0, 28)}”的详细设置，可以继续查看或编辑。`
  if (target.classList.contains('rule-list-item')) return `选择这条规则，查看匹配条件、执行动作和启用状态。`
  if (target.classList.contains('knowledge-item')) return `选择这个知识库，查看文档、启用状态和绑定群。`
  if (target.classList.contains('document-item')) return `打开这篇文档，查看或编辑 AI 可以检索的内容。`
  if (target.classList.contains('audit-row')) return '打开这条审计记录，查看执行时间、对象、状态和详细原因。'
  for (const [pattern, description] of helpRules) if (pattern.test(label)) return description
  if (target.getAttribute('role') === 'tab') return `切换到“${label.slice(0, 24)}”，只改变当前显示的分类，不会立即执行操作。`
  return label ? `点击“${label.slice(0, 28)}”完成对应操作；按钮变灰时表示当前条件还不满足。` : '点击执行这个操作；鼠标移开后说明会自动收起。'
}

function findButton(node: EventTarget | null) {
  return node instanceof Element ? node.closest<HTMLElement>(buttonSelector) : null
}

function place(target: HTMLElement) {
  const rect = target.getBoundingClientRect()
  return {
    left: Math.min(window.innerWidth - 170, Math.max(170, rect.left + rect.width / 2)),
    top: rect.top > 96 ? rect.top - 10 : rect.bottom + 10,
    below: rect.top <= 96,
  }
}

export default function ButtonHelp() {
  const [tooltip, setTooltip] = useState<TooltipState | null>(null)

  useEffect(() => {
    const show = (target: HTMLElement) => {
      const text = buttonHelpText(target)
      if (!text) return
      const position = place(target)
      target.setAttribute('aria-describedby', 'dh-button-help')
      setTooltip({ target, text, ...position })
    }
    const hide = (target?: HTMLElement | null) => {
      if (target?.getAttribute('aria-describedby') === 'dh-button-help') target.removeAttribute('aria-describedby')
      setTooltip(current => {
        if (current?.target.getAttribute('aria-describedby') === 'dh-button-help') current.target.removeAttribute('aria-describedby')
        return null
      })
    }
    const onPointerOver = (event: PointerEvent) => {
      const target = findButton(event.target)
      if (!target || target.contains(event.relatedTarget as Node | null)) return
      show(target)
    }
    const onPointerOut = (event: PointerEvent) => {
      const target = findButton(event.target)
      if (!target || target.contains(event.relatedTarget as Node | null)) return
      hide(target)
    }
    const onFocusIn = (event: FocusEvent) => { const target = findButton(event.target); if (target) show(target) }
    const onFocusOut = (event: FocusEvent) => { const target = findButton(event.target); if (target) hide(target) }
    const hideRequested = () => hide()
    const reposition = () => setTooltip(current => current ? { ...current, ...place(current.target) } : null)
    document.addEventListener('pointerover', onPointerOver, true)
    document.addEventListener('pointerout', onPointerOut, true)
    document.addEventListener('focusin', onFocusIn, true)
    document.addEventListener('focusout', onFocusOut, true)
    document.addEventListener('dh-hide-button-help', hideRequested)
    window.addEventListener('resize', reposition)
    document.addEventListener('scroll', reposition, true)
    return () => {
      document.removeEventListener('pointerover', onPointerOver, true)
      document.removeEventListener('pointerout', onPointerOut, true)
      document.removeEventListener('focusin', onFocusIn, true)
      document.removeEventListener('focusout', onFocusOut, true)
      document.removeEventListener('dh-hide-button-help', hideRequested)
      window.removeEventListener('resize', reposition)
      document.removeEventListener('scroll', reposition, true)
    }
  }, [])

  if (!tooltip) return null
  return <div id="dh-button-help" role="tooltip" className={`button-help ${tooltip.below ? 'below' : ''}`} style={{ left: tooltip.left, top: tooltip.top }}>{tooltip.text}</div>
}
