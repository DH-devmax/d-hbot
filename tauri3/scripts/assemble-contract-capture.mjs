import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

function argumentMap(args) {
  const values = new Map()
  for (let index = 0; index < args.length; index += 2) {
    const key = args[index]
    const value = args[index + 1]
    if (!key?.startsWith('--') || !value) throw new Error('参数必须使用 --名称 值')
    values.set(key.slice(2), value)
  }
  return values
}

function requireObject(value, pathName) {
  if (!value || Array.isArray(value) || typeof value !== 'object') {
    throw new Error(`${pathName} 必须是对象`)
  }
}

function requireOperation(operation, index) {
  requireObject(operation, `operations[${index}]`)
  for (const field of ['name', 'route', 'request', 'transport', 'business', 'normalizedReceipt', 'expectedNormalizedState']) {
    if (!(field in operation)) throw new Error(`operations[${index}] 缺少 ${field}`)
  }
  requireObject(operation.request, `operations[${index}].request`)
  requireObject(operation.request.params, `operations[${index}].request.params`)
  requireObject(operation.transport, `operations[${index}].transport`)
  requireObject(operation.business, `operations[${index}].business`)
  requireObject(operation.normalizedReceipt, `operations[${index}].normalizedReceipt`)
  requireObject(operation.expectedNormalizedState, `operations[${index}].expectedNormalizedState`)
  for (const field of ['transportCode', 'errno', 'requestId']) {
    if (!(field in operation.transport)) throw new Error(`operations[${index}].transport 缺少 ${field}`)
  }
  for (const field of ['code', 'errno', 'msg', 'data']) {
    if (!(field in operation.business)) throw new Error(`operations[${index}].business 缺少 ${field}`)
  }
}

export function assembleContractCapture({ page, mainScriptSha256, operations, callbacks, expectedNormalizedState, capturedAt }) {
  requireObject(page, 'page')
  if (typeof page.title !== 'string' || typeof page.url !== 'string') throw new Error('DevTools 页面缺少 title 或 url')
  if (!/^[a-f0-9]{64}$/i.test(mainScriptSha256)) throw new Error('main script 必须是 SHA-256')
  if (!Array.isArray(operations) || operations.length === 0) throw new Error('实际采集必须至少包含一个操作')
  operations.forEach(requireOperation)
  if (!Array.isArray(callbacks) || callbacks.length === 0) {
    throw new Error('实际采集必须至少包含一个 callback')
  }
  requireObject(expectedNormalizedState, 'expectedNormalizedState')
  return {
    version: 2,
    metadata: {
      captureFormat: 'dh-contract-v2',
      capturedAt,
      appFileVersion: page.appFileVersion || 'unknown',
      pageTitle: page.title,
      pageUrl: page.url,
      mainScriptSha256,
    },
    operations,
    callbacks,
    expectedNormalizedState,
  }
}

export function selectDevToolsPage(pages) {
  if (!Array.isArray(pages)) throw new Error('DevTools 页面列表格式无效')
  const candidates = pages.filter(page => page?.type === 'page' && typeof page?.url === 'string')
  const marked = candidates.filter(page => {
    const identity = `${page?.title || ''} ${page.url}`.toLowerCase()
    return identity.includes('旺商聊') || identity.includes('wangshangliao')
  })
  if (marked.length === 1) return marked[0]
  if (marked.length > 1) {
    throw new Error(`检测到 ${marked.length} 个旺商聊页面，请关闭重复窗口后重试`)
  }
  throw new Error(`没有找到明确的旺商聊页面，当前 DevTools 共有 ${candidates.length} 个页面`)
}

async function getDevToolsPage(baseUrl) {
  const url = new URL('/json/list', baseUrl)
  if (!['127.0.0.1', '::1', 'localhost'].includes(url.hostname)) {
    throw new Error('采集器只连接本机 DevTools')
  }
  const response = await fetch(url)
  if (!response.ok) throw new Error(`DevTools 返回 HTTP ${response.status}`)
  const pages = await response.json()
  return selectDevToolsPage(pages)
}

async function readJson(file, label) {
  const value = JSON.parse(await readFile(file, 'utf8'))
  if (label === 'operations') return Array.isArray(value) ? value : value.operations
  if (label === 'callbacks') return Array.isArray(value) ? value : value.callbacks
  if (label === 'state') return value.expectedNormalizedState ?? value
  return value
}

async function main() {
  const args = argumentMap(process.argv.slice(2))
  const required = ['devtools', 'main-script', 'operations', 'callbacks', 'state', 'output']
  for (const key of required) {
    if (!args.has(key)) throw new Error(`缺少 --${key}`)
  }
  const output = path.resolve(args.get('output'))
  const rawDirectory = `${path.sep}contracts${path.sep}raw${path.sep}`
  if (!output.includes(rawDirectory)) {
    throw new Error('原始采集只能写入 contracts/raw/，该目录不会提交到 Git')
  }
  const [page, script, operations, callbacks, expectedNormalizedState] = await Promise.all([
    getDevToolsPage(args.get('devtools')),
    readFile(args.get('main-script')),
    readJson(args.get('operations'), 'operations'),
    readJson(args.get('callbacks'), 'callbacks'),
    readJson(args.get('state'), 'state'),
  ])
  const capture = assembleContractCapture({
    page: { ...page, appFileVersion: args.get('app-version') || 'unknown' },
    mainScriptSha256: createHash('sha256').update(script).digest('hex'),
    operations,
    callbacks,
    expectedNormalizedState,
    capturedAt: new Date().toISOString(),
  })
  await mkdir(path.dirname(output), { recursive: true })
  await writeFile(output, `${JSON.stringify(capture, null, 2)}\n`, { flag: 'wx' })
  process.stdout.write(`已写入本机原始 Contract v2：${output}\n`)
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main().catch(error => {
    process.stderr.write(`${error.message}\n`)
    process.exitCode = 1
  })
}
