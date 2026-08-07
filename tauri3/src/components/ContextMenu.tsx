import { useEffect, useLayoutEffect, useRef, useState } from 'react'

type MenuItem = {
  key: string
  label: string
  hint?: string
  danger?: boolean
  disabled?: boolean
  run: () => void | Promise<void>
}

type MenuState = {
  groups: MenuItem[][]
  x: number
  y: number
  origin: HTMLElement | null
  feedback: string
}

const rowSelector = '.message-row, .audit-row, .member-table tbody tr'
const listItemSelector = '.group-item, .rule-list-item, .knowledge-item, .document-item, .work-item'
const headerClasses = ['message-header', 'audit-header']
const actionSelector = 'button, [role="button"]'
// 只包含真正可编辑文本的输入类型；checkbox、file、date、time 等右键不给编辑菜单。
const textInputTypes = new Set(['text', 'search', 'url', 'tel', 'email', 'number', 'password'])
const shortcutHint = '请改用 Ctrl+C / Ctrl+V'

/**
 * 与 `vite.config.ts` 里 `developer` 的判定保持一致。
 * 必须写成 `import.meta.env.X` 的完整字面形式，Vite 才能在生产构建时把它替换成常量并消除分支；
 * 先赋值给变量再取属性会让这个消除失效。
 */
function isDeveloperChannel() {
  return import.meta.env.VITE_DH_FIXTURE === '1' || import.meta.env.MODE === 'style'
}

function collapse(value: string | null | undefined) {
  return (value || '').replace(/\s+/g, ' ').trim()
}

function isEditableField(node: Element): node is HTMLInputElement | HTMLTextAreaElement {
  if (node instanceof HTMLTextAreaElement) return true
  return node instanceof HTMLInputElement && textInputTypes.has(node.type)
}

/** `selectionStart` 在部分输入类型上会返回 null 或抛错，统一兜成"没有选区"。 */
function selectionRange(field: HTMLInputElement | HTMLTextAreaElement) {
  try {
    const { selectionStart, selectionEnd } = field
    if (selectionStart === null || selectionEnd === null) return null
    return { start: selectionStart, end: selectionEnd }
  } catch {
    return null
  }
}

/**
 * 页面里的输入框都是 React 受控组件，直接改 `field.value` 不会触发 onChange，
 * 值会在下一次渲染被覆盖。这里走原型上的原生 setter 再补一个 input 事件，让 React 收到变更。
 */
function setFieldValue(field: HTMLInputElement | HTMLTextAreaElement, value: string, caret: number) {
  const prototype = field instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype
  const setter = Object.getOwnPropertyDescriptor(prototype, 'value')?.set
  if (setter) setter.call(field, value)
  else field.value = value
  field.dispatchEvent(new Event('input', { bubbles: true }))
  field.focus()
  try {
    field.setSelectionRange(caret, caret)
  } catch {
    /* 少数输入类型不支持设置光标位置，忽略即可 */
  }
}

async function writeClipboard(text: string) {
  if (!navigator.clipboard?.writeText) throw new Error('clipboard write unavailable')
  await navigator.clipboard.writeText(text)
}

function canReadClipboard() {
  return typeof navigator.clipboard?.readText === 'function'
}

/** 取右键落点所在的那一列：从落点往上走到行的直接子元素。 */
function cellOf(row: HTMLElement, target: Element) {
  let node: Element | null = target
  while (node && node !== row && node.parentElement !== row) node = node.parentElement
  return node && node !== row && node instanceof HTMLElement ? node : null
}

/** 整行按列用 \t 连接，跳过没有文本的列（复选框列、操作按钮列），粘进表格软件可直接分列。 */
function rowAsTsv(row: HTMLElement) {
  return Array.from(row.children)
    .map(cell => collapse(cell.textContent))
    .filter(Boolean)
    .join('\t')
}

function itemName(element: HTMLElement) {
  const strong = element.querySelector('strong')
  if (strong) return collapse(strong.textContent)
  const span = element.querySelector('span')
  if (span) return collapse(span.textContent)
  return collapse(element.textContent)
}

function isDisabledAction(action: HTMLElement) {
  if (action instanceof HTMLButtonElement && action.disabled) return true
  return action.hasAttribute('disabled') || action.getAttribute('aria-disabled') === 'true'
}

/**
 * 菜单不新增能力，只镜像行内已有的操作按钮：读它们的 title/aria-label 当菜单文案，
 * 执行时调用 `.click()`。业务逻辑、二次确认、disabled 规则全部沿用原按钮。
 */
function mirrorActions(scope: HTMLElement) {
  return Array.from(scope.querySelectorAll<HTMLElement>(actionSelector)).flatMap((action, index) => {
    const label = collapse(action.getAttribute('title') || action.getAttribute('aria-label'))
    if (!label) return []
    return [{
      key: `mirror:${index}:${label}`,
      label,
      danger: action.classList.contains('danger-text') || /移出|删除|撤回|清空/.test(label),
      disabled: isDisabledAction(action),
      run: () => action.click(),
    }]
  })
}

function editableGroups(field: HTMLInputElement | HTMLTextAreaElement): MenuItem[][] {
  const range = selectionRange(field)
  const selected = range ? field.value.slice(range.start, range.end) : ''
  const writable = !field.disabled && !field.readOnly
  // 密码框（AI API Key）不提供复制/剪切：菜单直接读 value，会把打了码的密钥原文送进剪贴板。
  const masked = field instanceof HTMLInputElement && field.type === 'password'
  const clipboardItems: MenuItem[] = masked ? [] : [
    {
      key: 'cut',
      label: '剪切',
      disabled: !selected || !writable,
      run: async () => {
        await writeClipboard(selected)
        if (!range) return
        setFieldValue(field, field.value.slice(0, range.start) + field.value.slice(range.end), range.start)
      },
    },
    {
      key: 'copy',
      label: '复制',
      disabled: !selected,
      run: () => writeClipboard(selected),
    },
  ]
  return [
    clipboardItems,
    [
      {
        key: 'paste',
        label: '粘贴',
        hint: canReadClipboard() ? undefined : shortcutHint,
        disabled: !writable || !canReadClipboard(),
        run: async () => {
          const text = await navigator.clipboard.readText()
          const start = range?.start ?? field.value.length
          const end = range?.end ?? field.value.length
          setFieldValue(field, field.value.slice(0, start) + text + field.value.slice(end), start + text.length)
        },
      },
      {
        key: 'selectAll',
        label: '全选',
        disabled: !field.value,
        run: () => {
          field.focus()
          try {
            field.select()
          } catch {
            /* 不支持 select() 的输入类型忽略 */
          }
        },
      },
    ],
  ]
}

function groupsFor(target: Element): MenuItem[][] {
  const editable = target.closest<HTMLElement>('input, textarea')
  if (editable && isEditableField(editable)) return editableGroups(editable)

  const link = target.closest<HTMLAnchorElement>('a[href]')
  if (link) {
    return [[{ key: 'copyLink', label: '复制链接地址', run: () => writeClipboard(link.href) }]]
  }

  const row = target.closest<HTMLElement>(rowSelector)
  if (row && !headerClasses.some(name => row.classList.contains(name))) {
    const cell = cellOf(row, target)
    const cellText = collapse(cell?.textContent)
    const tsv = rowAsTsv(row)
    return [
      [
        ...(cellText ? [{ key: 'copyCell', label: '复制这一列', run: () => writeClipboard(cellText) }] : []),
        ...(tsv ? [{ key: 'copyRow', label: '复制整行', run: () => writeClipboard(tsv) }] : []),
      ],
      mirrorActions(row),
    ]
  }

  const listItem = target.closest<HTMLElement>(listItemSelector)
  if (listItem) {
    const name = itemName(listItem)
    return [
      ...(name ? [[{ key: 'copyName', label: '复制名称', run: () => writeClipboard(name) }]] : []),
      mirrorActions(listItem),
    ]
  }

  const titlebar = target.closest<HTMLElement>('.window-titlebar')
  if (titlebar) return [mirrorActions(titlebar)]

  return []
}

export default function ContextMenu() {
  const [menu, setMenu] = useState<MenuState | null>(null)
  const [activeKey, setActiveKey] = useState('')
  const menuRef = useRef<HTMLDivElement>(null)

  const close = (restoreFocus: boolean) => {
    if (restoreFocus) menu?.origin?.focus()
    setMenu(null)
    setActiveKey('')
  }

  useEffect(() => {
    const onContextMenu = (event: MouseEvent) => {
      // 开发通道保留 Shift + 右键透传原生菜单，否则调试时点不到"检查"。生产通道无条件吞掉。
      if (isDeveloperChannel() && event.shiftKey) return
      event.preventDefault()
      const target = event.target instanceof Element ? event.target : null
      if (!target) return
      if (menuRef.current?.contains(target)) return
      const groups = groupsFor(target).filter(group => group.length > 0)
      if (!groups.length) {
        setMenu(null)
        setActiveKey('')
        return
      }
      document.dispatchEvent(new CustomEvent('dh-hide-button-help'))
      const focused = document.activeElement
      setMenu({
        groups,
        x: event.clientX,
        y: event.clientY,
        origin: focused instanceof HTMLElement && focused !== document.body ? focused : null,
        feedback: '',
      })
      setActiveKey('')
    }
    document.addEventListener('contextmenu', onContextMenu, true)
    return () => document.removeEventListener('contextmenu', onContextMenu, true)
  }, [])

  useEffect(() => {
    if (!menu) return
    const onPointerDown = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) close(false)
    }
    /*
     * 打开菜单的那一次右键本身会产生副作用事件：浏览器会把被点中的元素滚进可视区、
     * 把焦点从上一个元素移走。关键在于 scroll 事件是异步派发的——滚动位置早就变了，
     * 事件却要等到下一个渲染时机才送到，因此它会晚于菜单出现。
     * 不隔一帧的话，菜单会被"它自己那次右键引发的滚动"立刻关掉：
     * 真实浏览器实测 24.0ms 菜单出现、28.7ms 收到 scroll、29.8ms 菜单被移除。
     * jsdom 没有布局也不会滚动，这个顺序在单元测试里永远不会出现，只能靠真实浏览器发现。
     *
     * 所以这些"环境类"关闭原因（滚动、缩放、窗口失焦）等一帧再生效；
     * pointerdown 是用户的明确动作，不受影响，保持立刻关闭。
     */
    let settled = false
    const frame = requestAnimationFrame(() => { settled = true })
    const dismissAmbient = () => { if (settled) close(false) }
    document.addEventListener('pointerdown', onPointerDown, true)
    document.addEventListener('scroll', dismissAmbient, true)
    window.addEventListener('resize', dismissAmbient)
    window.addEventListener('blur', dismissAmbient)
    return () => {
      cancelAnimationFrame(frame)
      document.removeEventListener('pointerdown', onPointerDown, true)
      document.removeEventListener('scroll', dismissAmbient, true)
      window.removeEventListener('resize', dismissAmbient)
      window.removeEventListener('blur', dismissAmbient)
    }
  }, [menu])

  useEffect(() => {
    if (!menu) return
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault()
        close(true)
        return
      }
      const items = menu.groups.flat().filter(item => !item.disabled)
      if (!items.length) return
      const index = items.findIndex(item => item.key === activeKey)
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault()
        const step = event.key === 'ArrowDown' ? 1 : -1
        if (index < 0) setActiveKey((step === 1 ? items[0] : items[items.length - 1]).key)
        else setActiveKey(items[(index + step + items.length) % items.length].key)
      } else if (event.key === 'Home') {
        event.preventDefault()
        setActiveKey(items[0].key)
      } else if (event.key === 'End') {
        event.preventDefault()
        setActiveKey(items[items.length - 1].key)
      } else if (event.key === 'Enter' || event.key === ' ') {
        const current = items.find(item => item.key === activeKey)
        if (!current) return
        event.preventDefault()
        void activate(current)
      }
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [menu, activeKey])

  useEffect(() => {
    if (!activeKey || !menuRef.current) return
    const nodes = Array.from(menuRef.current.querySelectorAll<HTMLElement>('[data-menu-key]'))
    nodes.find(node => node.dataset.menuKey === activeKey)?.focus()
  }, [activeKey])

  // 贴边时翻转到落点另一侧。jsdom 下 getBoundingClientRect 全为 0，位置不会变化，不会反复更新。
  useLayoutEffect(() => {
    if (!menu || !menuRef.current) return
    const margin = 8
    const rect = menuRef.current.getBoundingClientRect()
    const x = menu.x + rect.width + margin > window.innerWidth ? Math.max(margin, menu.x - rect.width) : menu.x
    const y = menu.y + rect.height + margin > window.innerHeight ? Math.max(margin, menu.y - rect.height) : menu.y
    if (x !== menu.x || y !== menu.y) setMenu(current => (current ? { ...current, x, y } : null))
  }, [menu])

  const activate = async (item: MenuItem) => {
    if (item.disabled) return
    try {
      await item.run()
      close(false)
    } catch (reason) {
      console.error('DH BOT context menu action failed', reason)
      setMenu(current => (current ? { ...current, feedback: `操作失败，${shortcutHint}` } : null))
    }
  }

  if (!menu) return null
  return <div
    ref={menuRef}
    className="dh-context-menu"
    role="menu"
    aria-label="右键菜单"
    style={{ left: menu.x, top: menu.y }}
  >
    {menu.groups.map((group, groupIndex) => <div key={`group-${groupIndex}`} role="group">
      {groupIndex > 0 && <div className="dh-context-menu__separator" role="separator" />}
      {group.map(item => <div
        key={item.key}
        data-menu-key={item.key}
        role="menuitem"
        tabIndex={-1}
        aria-disabled={item.disabled ? 'true' : undefined}
        className={`dh-context-menu__item ${item.danger ? 'danger' : ''} ${item.key === activeKey ? 'active' : ''}`.replace(/\s+/g, ' ').trim()}
        onClick={() => void activate(item)}
        onPointerEnter={() => { if (!item.disabled) setActiveKey(item.key) }}
      >
        <span>{item.label}</span>
        {item.hint && <small>{item.hint}</small>}
      </div>)}
    </div>)}
    {menu.feedback && <div className="dh-context-menu__feedback" role="status" aria-live="polite">{menu.feedback}</div>}
  </div>
}
