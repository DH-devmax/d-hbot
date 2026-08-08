import { expect, test, type Page } from '@playwright/test'
import { mkdir } from 'node:fs/promises'
import path from 'node:path'

type FixtureState = {
  rules: Array<Record<string, unknown>>
  tasks: Array<Record<string, unknown>>
  activities: Array<Record<string, unknown>>
  schedules: Array<Record<string, unknown>>
}

const fixtureState: FixtureState = { rules: [], tasks: [], activities: [], schedules: [] }

async function installDeveloperFixture(page: Page) {
  await page.addInitScript(({ initialState }) => {
    const state = structuredClone(initialState)
    const actionLog: Array<{ command: string; args: Record<string, unknown> }> = []
    let callbackId = 1
    const callbacks = new Map<number, (payload: unknown) => void>()
    const group = { accountId: 'ACCOUNT', groupId: 101, name: '16 人开发测试群', ownerUserId: 1, enabled: true, aiEnabled: true, moderationEnabled: true, manualTakeover: false, welcomeMessage: '欢迎 @「[成员]」' }
    const member = (userId: number, cardName: string, role = 'member') => ({ groupId: 101, userId, nimId: `NIM-${userId}`, nickname: cardName, cardName, role, accountState: 'ACCOUNT_STATE_GOOD', present: true, originalCardName: cardName, managedCardName: '', cardSuffix: '' })
    const messages = [{ id: 1, accountId: 'ACCOUNT', groupId: 101, serverMessageId: 'MESSAGE-1', sequence: 1, userId: 2, senderName: '广州校长', kind: 'text', text: '@DH 请查看群规', sentAt: '2026-07-21T08:00:00Z', receivedAt: '2026-07-21T08:00:00Z', processedAt: '2026-07-21T08:00:01Z', acknowledgedAt: '2026-07-21T08:00:01Z', processingState: 'processed' }]
    const bases = [{ id: 1, accountId: 'ACCOUNT', name: 'DH 群规', description: '开发测试知识库', enabled: true, builtIn: false, readOnly: false }]
    const documents = [{ id: 1, baseId: 1, baseName: 'DH 群规', title: '文明交流', kind: 'markdown', content: '文明交流，资金问题请联系管理员。', source: 'manual', contentHash: 'HASH' }]
    const audits = [{ id: 1, accountId: 'ACCOUNT', groupId: 101, userId: 2, actor: 'rule', event: 'message_received', level: 'info', details: '测试环境收到消息', createdAt: '2026-07-21T08:00:01Z' }]
    const summaries = [{ id: 1, accountId: 'ACCOUNT', groupId: 101, localDate: '2026-07-21', content: '群内交流正常。', source: 'fixture', createdAt: '2026-07-21T12:00:00Z' }]

    const invoke = async (command: string, args: Record<string, any> = {}) => {
      if (!/^(plugin:event\||diagnose$|database_status$|get_|list_|query_|export_)/.test(command)) {
        actionLog.push({ command, args })
      }
      switch (command) {
        case 'plugin:event|listen': return callbackId++
        case 'plugin:event|unlisten': return null
        case 'diagnose': return { status: 'ready', devtoolsUrl: 'http://127.0.0.1:9233', pageTitle: 'DH Fixture', pageUrl: 'http://127.0.0.1:51300', nimAccount: 'ACCOUNT', detail: '开发 Fixture 已就绪' }
        case 'database_status': return { path: '%APPDATA%\\DH\\fixture\\dh.db', schemaVersion: 14, integrity: 'ok', accounts: 1, groups: 1, messages: messages.length }
        case 'get_ai_settings': return { base_url: 'http://127.0.0.1:51300/v1', webhook_url: '', model: 'fixture-model', api_key_configured: true }
        case 'get_wang_startup_settings': return { path: '', autoStart: true }
        case 'save_wang_startup_settings': return null
        case 'list_groups': case 'list_cached_groups': return [group]
        case 'list_audit': return audits
        case 'query_audit': return { items: audits, nextCursor: null }
        case 'export_audit': return JSON.stringify(audits)
        case 'list_daily_summaries': return summaries
        case 'query_messages': return { items: messages, nextCursor: null }
        case 'send_text_batch': return (args.groupIds || []).map((groupId: number) => ({ groupId, success: true, messageId: `SENT-${groupId}`, error: '' }))
        case 'recall_message': return null
        case 'list_members': return { members: [member(1, '群主', 'owner'), member(2, '广校'), ...Array.from({ length: 14 }, (_, index) => member(index + 3, `DH群员${String(index + 1).padStart(4, '0')}`))], reportedCount: 16, resolvedCount: 16, complete: true, sources: ['fixture'] }
        case 'get_card_settings': return { prefix: 'DH', autoRename: false, paused: false }
        case 'get_ai_automation_settings': return { enabled: true, reply: true, tasks: true, recall: false, mute: false, remove: false, manualTakeover: false }
        case 'list_card_rename_jobs': return []
        case 'list_rules': return state.rules
        case 'save_rule': {
          const next = { ...args.rule, id: args.rule.id || state.rules.length + 1 }
          state.rules = state.rules.filter((rule: any) => rule.id !== next.id).concat(next)
          return next.id
        }
        case 'delete_rule': state.rules = state.rules.filter((rule: any) => rule.id !== args.ruleId); return null
        case 'list_knowledge_bases': return bases
        case 'list_knowledge_documents': return documents
        case 'list_knowledge_bindings': return [{ baseId: 1, accountId: 'ACCOUNT', groupId: 101, enabled: true }]
        case 'update_knowledge_base': case 'bind_knowledge_base': case 'save_knowledge_document': return 1
        case 'list_tasks': return state.tasks
        case 'save_task': {
          const next = { ...args.task, id: args.task.id || state.tasks.length + 1 }
          state.tasks = state.tasks.filter((task: any) => task.id !== next.id).concat(next)
          return next.id
        }
        case 'list_activities': return state.activities
        case 'save_activity': {
          const next = { ...args.activity, id: args.activity.id || state.activities.length + 1 }
          state.activities = state.activities.filter((activity: any) => activity.id !== next.id).concat(next)
          return next.id
        }
        case 'list_activity_runs': return []
        case 'preview_activity_text': return { text: args.activity.aiOptimize ? '今晚活动开始啦，欢迎大家参加！' : args.activity.content, source: args.activity.aiOptimize ? 'ai' : 'fixed' }
        case 'publish_activity_now': return args.activityId ? 1 : 0
        case 'list_schedules': return state.schedules
        case 'save_schedule': {
          const next = { ...args.schedule, id: args.schedule.id || state.schedules.length + 1 }
          state.schedules = state.schedules.filter((schedule: any) => schedule.id !== next.id).concat(next)
          return next.id
        }
        case 'list_schedule_runs': return []
        case 'get_summary_settings': return { accountId: 'ACCOUNT', enabled: false, time: '23:00', groupIds: [], timezone: 'Asia/Shanghai' }
        case 'get_runtime_mode': return { mode: 'fixture', dataDir: '%APPDATA%\\DH\\fixture', restartRequired: false }
        case 'test_ai': return { decision: { reply: '你好，我可以回答群规和业务问题。', reason: '开发测试', confidence: 0.95 }, elapsedMs: 12, model: 'fixture-model' }
        case 'get_wang_profile_status': return { state: '开发测试', scriptHash: 'FIXTURE-HASH', backupPath: null, requiresElevation: false, detail: '未读取真实旺商聊目录' }
        case 'get_gateway_capabilities': return { announcement: 'unsupported', sendText: 'supported', mute: 'supported', recall: 'supported', rename: 'supported', removeMember: 'supported', groupMute: 'supported', memberEvents: 'supported' }
        default:
          if (command.startsWith('save_') || command.startsWith('set_') || command.startsWith('delete_')) return null
          throw new Error(`Fixture IPC 未实现: ${command}`)
      }
    }

    Object.assign(window, {
      __DH_E2E_FIXTURE__: true,
      __DH_E2E_ACTIONS__: actionLog,
      __TAURI_INTERNALS__: {
        invoke,
        transformCallback(callback: (payload: unknown) => void, once = false) {
          const id = callbackId++
          callbacks.set(id, once ? payload => { callbacks.delete(id); callback(payload) } : callback)
          return id
        },
        unregisterCallback(id: number) { callbacks.delete(id) },
        runCallback(id: number, payload: unknown) { callbacks.get(id)?.(payload) },
        convertFileSrc(path: string) { return path },
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
      },
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener() {} },
    })
  }, { initialState: fixtureState })
}

test.beforeEach(async ({ page }) => {
  await installDeveloperFixture(page)
  await page.goto('/')
  await expect(page.getByText('开发 Fixture 已就绪')).toBeVisible()
})

test('developer fixture covers navigation, data and management workflows', async ({ page }) => {
  await page.getByRole('button', { name: '群组与成员' }).click()
  await page.getByRole('button', { name: /16 人开发测试群/ }).click()
  await expect(page.getByText('广校', { exact: true })).toBeVisible()
  await expect(page.getByText('群成员').locator('..')).toContainText('16')
  await page.getByPlaceholder('搜索群名片、原名称').fill('广校')
  await expect(page.getByRole('row').filter({ hasText: '广校' })).toBeVisible()
  await expect(page.getByRole('row').filter({ hasText: 'DH群员0001' })).toHaveCount(0)

  await page.getByRole('button', { name: '消息台' }).click()
  await expect(page.getByText('@DH 请查看群规')).toBeVisible()
  await page.locator('label.check-chip').filter({ hasText: '16 人开发测试群' }).click()
  await page.getByPlaceholder('发送内容只会发到当前勾选的群。').fill('开发测试消息')
  await page.getByRole('button', { name: '发送文本' }).click()
  await expect.poll(() => page.evaluate(() => (window as any).__DH_E2E_ACTIONS__.filter((item: any) => item.command === 'send_text_batch').length)).toBe(1)

  await page.getByRole('button', { name: '规则' }).click()
  await page.getByRole('button', { name: '新建规则' }).click()
  await page.getByLabel('匹配内容').fill('测试关键词')
  await page.getByRole('button', { name: '保存规则' }).click()
  await expect(page.getByText('新机器规则').first()).toBeVisible()
  await page.getByRole('button', { name: /AI 控制规则/ }).first().click()
  await expect(page.getByText('暂无 AI 控制规则')).toBeVisible()

  await page.getByRole('button', { name: '知识与 AI' }).click()
  await expect(page.getByRole('heading', { name: 'DH 群规' })).toBeVisible()
  await expect(page.getByLabel('标题')).toHaveValue('文明交流')
  await page.getByLabel('内容').fill('文明交流，不发布恶意链接。')
  await page.getByRole('button', { name: '保存文档' }).click()
  await page.getByRole('button', { name: '保存绑定' }).click()
  await expect.poll(() => page.evaluate(() => (window as any).__DH_E2E_ACTIONS__.map((item: any) => item.command))).toEqual(expect.arrayContaining(['save_knowledge_document', 'bind_knowledge_base']))

  await page.getByRole('button', { name: '活动与计划' }).click()
  await page.getByLabel('活动名称').fill('晚间互动')
  await page.getByLabel('活动原文').fill('今晚活动开始，欢迎参加。')
  await page.getByRole('button', { name: '保存活动' }).click()
  await expect(page.getByText('晚间互动').first()).toBeVisible()
  await page.getByText('使用 AI 优化').click()
  await page.getByRole('button', { name: '预览文案' }).click()
  await expect(page.getByText('今晚活动开始啦，欢迎大家参加！')).toBeVisible()
  await page.getByRole('button', { name: '开关群计划' }).click()
  await page.locator('label.check-chip').filter({ hasText: '16 人开发测试群' }).click()
  await page.getByRole('button', { name: '保存计划' }).click()
  await expect(page.getByText('每日群发言').first()).toBeVisible()

  await page.getByRole('button', { name: '审计' }).click()
  await page.getByText('测试环境收到消息').click()
  await expect(page.getByRole('heading', { name: '收到群消息' })).toBeVisible()

  await page.getByRole('button', { name: '设置' }).click()
  await expect(page.getByText('DH Fixture · 9233')).toBeVisible()
  await page.getByRole('button', { name: '知识与 AI' }).click()
  await page.getByRole('button', { name: /AI 助手/ }).click()
  await page.getByPlaceholder('输入一条测试问题，例如：@DH 群规是什么？').fill('你好')
  await page.getByRole('button', { name: '测试 AI' }).click()
  await expect(page.getByText(/可以回答群规和业务问题/)).toBeVisible()

  await page.getByRole('button', { name: '调试' }).click()
  await expect(page.getByText('http://127.0.0.1:9233')).toBeVisible()
  await expect(page.getByText('FIXTURE-HASH')).toBeVisible()

  const commands = await page.evaluate(() => (window as any).__DH_E2E_ACTIONS__.map((item: any) => item.command))
  expect(commands).toEqual(expect.arrayContaining(['send_text_batch', 'save_rule', 'save_knowledge_document', 'bind_knowledge_base', 'save_activity', 'preview_activity_text', 'save_schedule']))
  expect(await page.locator('body').innerText()).not.toContain('127.0.0.1:9222')
})

test('captures current Tauri pages for the manual', async ({ page }) => {
  test.skip(process.env.DH_CAPTURE_MANUAL !== '1', 'manual capture is an explicit developer task')
  await page.setViewportSize({ width: 1320, height: 840 })
  const directory = path.resolve('../docs/screenshots/tauri3')
  await mkdir(directory, { recursive: true })
  const capture = async (navigation: string, file: string) => {
    await page.getByRole('button', { name: navigation }).click()
    await page.waitForTimeout(100)
    await page.screenshot({ path: path.join(directory, file), fullPage: false })
  }

  await page.screenshot({ path: path.join(directory, 'overview.png'), fullPage: false })
  await page.getByRole('button', { name: '群组与成员' }).click()
  await page.getByRole('button', { name: /16 人开发测试群/ }).click()
  await page.screenshot({ path: path.join(directory, 'groups.png'), fullPage: false })
  await capture('消息台', 'messages.png')
  await capture('规则', 'rules.png')
  await capture('知识与 AI', 'knowledge.png')
  await capture('活动与计划', 'activities.png')
  await capture('审计', 'audit.png')
  await capture('设置', 'settings.png')
  await page.getByRole('button', { name: '知识与 AI' }).click()
  await page.getByRole('button', { name: /AI 助手/ }).click()
  await page.getByPlaceholder('输入一条测试问题，例如：@DH 群规是什么？').fill('你好，请简单说明群规')
  await page.getByRole('button', { name: '测试 AI' }).click()
  await expect(page.getByText(/可以回答群规和业务问题/)).toBeVisible()
  await page.screenshot({ path: path.join(directory, 'ai-test.png'), fullPage: false })
  await capture('调试', 'debug.png')
})

/**
 * 右键菜单的这三条只能在真实浏览器里验：
 * jsdom 下 getBoundingClientRect 恒为 0（验不到贴边翻转和层级），
 * navigator.clipboard 是本地 mock（验不到真的写进系统剪贴板）。
 */
test('右键菜单替换 WebView 默认菜单，复制进真实剪贴板，镜像项触达真实 handler', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'])
  await page.getByRole('button', { name: '消息台' }).click()
  const row = page.locator('.message-row').filter({ hasText: '@DH 请查看群规' })
  await expect(row).toBeVisible()

  // 组件的监听在 capture 阶段，这里在冒泡阶段读 defaultPrevented，能确认默认菜单真被吞掉。
  await page.evaluate(() => {
    ;(window as unknown as Record<string, unknown>).__DH_MENU_PREVENTED__ = null
    document.addEventListener('contextmenu', event => {
      ;(window as unknown as Record<string, unknown>).__DH_MENU_PREVENTED__ = event.defaultPrevented
    })
  })

  await row.getByText('@DH 请查看群规').click({ button: 'right' })
  const menu = page.getByRole('menu', { name: '右键菜单' })
  await expect(menu).toBeVisible()
  expect(await page.evaluate(() => (window as unknown as Record<string, unknown>).__DH_MENU_PREVENTED__)).toBe(true)

  const box = (await menu.boundingBox())!
  expect(box.width).toBeGreaterThan(80)
  expect(box.height).toBeGreaterThan(40)
  expect(await menu.evaluate(node => getComputedStyle(node).zIndex)).toBe('1300')

  await menu.getByRole('menuitem', { name: '复制整行' }).click()
  await expect(menu).toBeHidden()
  const clipboard = await page.evaluate(() => navigator.clipboard.readText())
  expect(clipboard).toContain('@DH 请查看群规')
  expect(clipboard).toContain('\t')

  // 镜像项必须打开消息台自己的二次确认，而不是菜单另写一套撤回逻辑。
  await row.getByText('@DH 请查看群规').click({ button: 'right' })
  await menu.getByRole('menuitem', { name: '撤回消息' }).click()
  await expect(page.getByText(/撤回这条消息/)).toBeVisible()
  await expect(page.locator('.message-recall-preview')).toHaveText('@DH 请查看群规')
})

test('右键菜单贴近视口右缘时翻转，不被裁掉', async ({ page }) => {
  await page.getByRole('button', { name: '消息台' }).click()
  const row = page.locator('.message-row').filter({ hasText: '@DH 请查看群规' })
  await expect(row).toBeVisible()
  // page.mouse.click 不会自动滚动，行若在折叠线以下，落点会掉到视口外命中 <html>。
  // 先滚进来并等滚动稳定，再量坐标。
  await row.scrollIntoViewIfNeeded()
  await page.waitForTimeout(150)
  const rowBox = (await row.boundingBox())!
  const viewport = page.viewportSize()!
  expect(rowBox.y).toBeGreaterThanOrEqual(0)
  expect(rowBox.y + rowBox.height).toBeLessThanOrEqual(viewport.height)

  const clickX = Math.round(rowBox.x + rowBox.width - 4)
  const clickY = Math.round(rowBox.y + rowBox.height / 2)
  await page.mouse.click(clickX, clickY, { button: 'right' })

  const menu = page.getByRole('menu', { name: '右键菜单' })
  await expect(menu).toBeVisible()
  const box = (await menu.boundingBox())!

  // 前提：落点右侧确实放不下整个菜单。前提不成立就该报错，而不是让翻转断言变成空转。
  expect(viewport.width - clickX).toBeLessThan(box.width)
  expect(box.x).toBeLessThan(clickX)
  expect(box.x).toBeGreaterThanOrEqual(0)
  expect(box.x + box.width).toBeLessThanOrEqual(viewport.width)
  expect(box.y + box.height).toBeLessThanOrEqual(viewport.height)
})

test('密码框右键不提供复制和剪切', async ({ page }) => {
  await page.getByRole('button', { name: '知识与 AI' }).click()
  await page.getByRole('button', { name: /AI 助手/ }).click()
  const apiKey = page.getByLabel('API Key')
  await expect(apiKey).toBeVisible()
  await apiKey.fill('占位密钥仅用于测试')

  await apiKey.click({ button: 'right' })
  const menu = page.getByRole('menu', { name: '右键菜单' })
  await expect(menu).toBeVisible()
  await expect(menu.getByRole('menuitem', { name: '复制', exact: true })).toHaveCount(0)
  await expect(menu.getByRole('menuitem', { name: '剪切', exact: true })).toHaveCount(0)
  await expect(menu.getByRole('menuitem', { name: '全选', exact: true })).toBeVisible()
})
