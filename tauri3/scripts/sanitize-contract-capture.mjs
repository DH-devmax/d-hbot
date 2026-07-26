import { mkdir, readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const sensitiveKeySuffixes = ['apikey', 'authorization', 'cookie', 'jwt', 'password', 'passwd', 'privatekey', 'secret', 'signingkey', 'token']
const sensitiveValues = [
  /\bsk-[a-z0-9_-]{12,}\b/i,
  /\bbearer\s+[a-z0-9._~+\/-]+=*\b/i,
  /\beyj[a-z0-9_-]{5,}\.[a-z0-9_-]{5,}\.[a-z0-9_-]{5,}\b/i,
  /(?:^|[;\s])(sessionid|auth_token|access_token|refresh_token)=[^;\s]+/i,
  /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/i,
]
const fixtureEndpoint = /127\.0\.0\.1:(?:9233|51300)|dh fixture/i
const uuidValue = /\b[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\b/i
const teamSessionValue = /\bteam-\d{5,}\b/gi
const urlValue = /^(?:file|https?|wss?):\/\//i
const placeholderValue = /^(?:ACCOUNT|AVATAR|GROUP|MESSAGE|NAME|NIM|NOTICE|OPAQUE|REQUEST|SESSION|TAG|TEXT|URL|USER|UUID)_\d{3,}$/
const twoGroupCapabilities = new Set(['sendText', 'recall', 'mute', 'rename', 'announcement', 'groupMute'])
const protocolLiteralKeys = new Set([
  'action',
  'accountstate',
  'appfileversion',
  'authority',
  'businesscode',
  'businesserrno',
  'capabilities',
  'captureformat',
  'code',
  'errno',
  'family',
  'grouprole',
  'kind',
  'mainscriptsha256',
  'mutemode',
  'requestkind',
  'role',
  'route',
  'source',
  'status',
  'transportcode',
  'transporterrno',
  'verification',
])

function normalizedKey(key) {
  return String(key || '').replace(/[-_]/g, '').toLowerCase()
}

function isSensitiveKey(key) {
  const normalized = normalizedKey(key)
  return sensitiveKeySuffixes.some(suffix => normalized === suffix || normalized.endsWith(suffix))
}

function isProtocolLiteralContext(context) {
  return protocolLiteralKeys.has(normalizedKey(context.key)) || protocolLiteralKeys.has(normalizedKey(context.parentKey))
}

function isNimContext(context) {
  return context.requestKind === 'nim' || /^nim\./i.test(context.route)
}

function genericIdKind(context) {
  const parentKey = normalizedKey(context.parentKey)
  if (['from', 'sender', 'member', 'members', 'memberinfo', 'user', 'users', 'userinfo'].includes(parentKey)) return 'USER'
  if (['to', 'team', 'teams', 'group', 'groups', 'groupinfo'].includes(parentKey)) return 'GROUP'
  if (['message', 'messages', 'msg', 'messageinfo'].includes(parentKey)) return 'MESSAGE'
  if (['groupnotice', 'notice', 'notices', 'noticeinfo'].includes(parentKey)) return 'NOTICE'

  const siblingKeys = new Set(Object.keys(context.parent || {}).map(normalizedKey))
  if (['groupcloudid', 'groupname', 'teamid'].some(key => siblingKeys.has(key))) return 'GROUP'
  if (['nimid', 'accid', 'userid', 'usernick'].some(key => siblingKeys.has(key))) return 'USER'
  if (['idserver', 'idclient', 'messageid', 'msgid'].some(key => siblingKeys.has(key))) return 'MESSAGE'
  if (['noticeid', 'noticecontent', 'noticemode'].some(key => siblingKeys.has(key))) return 'NOTICE'
  if (/notice|announcement/i.test(context.route)) return 'NOTICE'
  if (isNimContext(context)) return 'MESSAGE'
  return null
}

function kindForKey(key, context) {
  const normalized = normalizedKey(key)
  if (normalized.includes('mainscriptsha256') || normalized === 'sha256') return null
  if (normalized.includes('avatar')) return 'AVATAR'
  if (normalized.includes('pageurl') || normalized === 'url' || normalized.endsWith('url')) return 'URL'
  if (normalized.includes('session')) return 'SESSION'
  if (normalized.includes('groupaccount') || normalized === 'teamaccount') return 'GROUP'
  if (normalized.includes('requestid') || normalized === 'traceid') return 'REQUEST'
  if (normalized.includes('messageid') || normalized.includes('msgid') || normalized === 'idserver' || normalized === 'idclient') return 'MESSAGE'
  if (normalized === 'noticeid' || normalized === 'noticeids') return 'NOTICE'
  if (normalized === 'groupmemberids' || normalized === 'memberids') return 'USER'
  if (normalized === 'uid' || normalized.endsWith('uid')) return 'USER'
  if (normalized === 'tag' || normalized.endsWith('tag')) return 'TAG'
  if (normalized.includes('nimid') || normalized === 'accid') return 'NIM'
  if (normalized === 'account' || normalized.includes('accountid') || normalized.includes('nimaccount') || normalized.includes('useraccount') || normalized.includes('memberaccount')) return 'ACCOUNT'
  if (normalized.endsWith('userid') || normalized === 'senderid' || normalized === 'from') return 'USER'
  if (normalized === 'groupid' || normalized === 'groupcloudid' || normalized === 'teamid' || normalized === 'to') return 'GROUP'
  if (normalized === 'target' && isNimContext(context)) return 'GROUP'
  if (normalized === 'id') return genericIdKind(context)
  if (normalized === 'name' || normalized.endsWith('name') || normalized.includes('nick') || normalized === 'alias' || normalized === 'displayname') return 'NAME'
  if (normalized === 'pagetitle' || normalized === 'title' || normalized === 'description' || normalized === 'remark') return 'TEXT'
  if (normalized === 'noticecontent' || normalized === 'text' || normalized === 'content' || normalized === 'data') return 'TEXT'
  return null
}

function isOpaqueBase64(value) {
  const input = value.trim()
  if (input.length < 24 || input.length % 4 === 1 || !/^[a-z0-9+/_-]+={0,2}$/i.test(input)) return false
  if (/^[a-f0-9]{32,}$/i.test(input) || /^\d+$/.test(input) || placeholderValue.test(input)) return false
  const normalized = input.replace(/-/g, '+').replace(/_/g, '/')
  const padded = `${normalized}${'='.repeat((4 - normalized.length % 4) % 4)}`
  let decoded
  try {
    decoded = Buffer.from(padded, 'base64')
  } catch {
    return false
  }
  if (decoded.length < 12 || decoded.toString('base64').replace(/=+$/, '') !== padded.replace(/=+$/, '')) return false
  return true
}

function parsedJson(value) {
  const trimmed = value.trim()
  if (!((trimmed.startsWith('{') && trimmed.endsWith('}')) || (trimmed.startsWith('[') && trimmed.endsWith(']')))) return null
  try {
    return JSON.parse(trimmed)
  } catch {
    return null
  }
}

function assertNoSecrets(value, location = '$') {
  if (Array.isArray(value)) {
    value.forEach((item, index) => assertNoSecrets(item, `${location}[${index}]`))
    return
  }
  if (value && typeof value === 'object') {
    for (const [key, item] of Object.entries(value)) {
      if (isSensitiveKey(key)) throw new Error(`捕获包含敏感字段：${location}.${key}`)
      assertNoSecrets(item, `${location}.${key}`)
    }
    return
  }
  if (typeof value === 'string') {
    const embedded = parsedJson(value)
    if (embedded !== null) assertNoSecrets(embedded, `${location}<json>`)
    if (sensitiveValues.some(pattern => pattern.test(value))) {
      throw new Error(`捕获包含敏感值：${location}`)
    }
  }
}

function redactable(value) {
  return (typeof value === 'string' && value.trim() !== '') || (typeof value === 'number' && value !== 0)
}

function sanitizedPlaceholder(kind, value) {
  return typeof value === 'string' && (value === '' || new RegExp(`^${kind}_\\d{3,}$`).test(value))
}

export function assertSanitizedContract(value, context = {}, location = '$') {
  if (location === '$') assertNoSecrets(value)
  if (typeof value === 'string') {
    const embedded = parsedJson(value)
    if (embedded !== null) return assertSanitizedContract(embedded, context, `${location}<json>`)
    const kind = kindForKey(context.key, context)
    if (kind && !sanitizedPlaceholder(kind, value)) throw new Error(`契约仍含未脱敏字段：${location}`)
    if (!isProtocolLiteralContext(context)
      && (uuidValue.test(value) || urlValue.test(value) || /\bteam-\d{5,}\b/i.test(value) || isOpaqueBase64(value))) {
      throw new Error(`契约仍含未脱敏值：${location}`)
    }
    return
  }
  if (typeof value === 'number') {
    if (kindForKey(context.key, context) && value !== 0) throw new Error(`契约仍含未脱敏字段：${location}`)
    return
  }
  if (Array.isArray(value)) {
    value.forEach((item, index) => assertSanitizedContract(item, { ...context, key: '', parentKey: context.key }, `${location}[${index}]`))
    return
  }
  if (!value || typeof value !== 'object') return
  const route = typeof value.route === 'string' ? value.route : context.route || ''
  const requestKind = typeof value.kind === 'string' ? value.kind : context.requestKind || ''
  for (const [key, item] of Object.entries(value)) {
    assertSanitizedContract(item, {
      key,
      parentKey: context.key || context.parentKey || '',
      parent: value,
      route,
      requestKind,
    }, `${location}.${key}`)
  }
}

function canonicalId(value) {
  if (typeof value === 'string' || typeof value === 'number') return String(value).trim()
  if (!value || Array.isArray(value) || typeof value !== 'object') return ''
  for (const key of ['groupId', 'groupCloudId', 'teamId', 'groupAccount', 'target', 'id']) {
    const result = canonicalId(value[key])
    if (result) return result
  }
  return ''
}

function canonicalUserId(value) {
  if (typeof value === 'string' || typeof value === 'number') return String(value).trim()
  if (!value || Array.isArray(value) || typeof value !== 'object') return ''
  for (const key of ['userId', 'memberId', 'uid', 'id']) {
    const result = canonicalUserId(value[key])
    if (result) return result
  }
  return ''
}

function operationGroupId(operation) {
  const state = operation?.expectedNormalizedState || {}
  const params = operation?.request?.params || {}
  for (const value of [state.groupId, state.groupCloudId, state.teamId, params.groupId, params.groupCloudId, params.teamId, params.target, params.to]) {
    const result = canonicalId(value)
    if (result) return result
  }
  return ''
}

function operationUserId(operation) {
  const state = operation?.expectedNormalizedState || {}
  const params = operation?.request?.params || {}
  for (const value of [state.userId, state.memberId, params.userId, params.memberId, params.uid]) {
    const result = canonicalUserId(value)
    if (result) return result
  }
  return ''
}

function callbackGroupId(callback) {
  const payload = callback?.payload
  if (!payload || Array.isArray(payload) || typeof payload !== 'object') return ''
  for (const value of [payload.groupId, payload.groupCloudId, payload.teamId, payload.target, payload.to]) {
    const result = canonicalId(value)
    if (result) return result
  }
  return ''
}

function compactGroupListOperation(operation, targetGroups) {
  const compacted = structuredClone(operation)
  const data = compacted.business?.data
  if (!data || typeof data !== 'object') return compacted
  const retainedGroups = new Set()
  let foundGroupArrays = false
  for (const key of ['member', 'owner']) {
    if (!Array.isArray(data[key])) continue
    foundGroupArrays = true
    data[key] = data[key].filter(item => {
      const groupId = canonicalId(item)
      if (!targetGroups.has(groupId)) return false
      retainedGroups.add(groupId)
      return true
    })
  }
  if (foundGroupArrays && retainedGroups.size !== targetGroups.size) {
    throw new Error(`精简失败：群列表只包含 ${retainedGroups.size}/${targetGroups.size} 个校准群`)
  }
  return compacted
}

function compactMemberReadback(operation, targetUserId) {
  const compacted = structuredClone(operation)
  const members = compacted.business?.data?.groupMemberInfo
  if (!Array.isArray(members)) throw new Error('精简失败：成员回读缺少 groupMemberInfo')
  const retained = members.filter(member => canonicalUserId(member) === targetUserId)
  if (retained.length !== 1) throw new Error(`精简失败：成员回读目标匹配数为 ${retained.length}`)
  compacted.business.data.groupMemberInfo = retained
  return compacted
}

function requestedActions(capabilities) {
  const actions = new Set()
  if (capabilities.has('sendText')) actions.add('send_text')
  if (capabilities.has('recall')) actions.add('recall')
  if (capabilities.has('mute')) {
    actions.add('mute')
    actions.add('unmute')
  }
  if (capabilities.has('rename')) actions.add('rename')
  if (capabilities.has('announcement')) actions.add('group_announcement')
  if (capabilities.has('groupMute')) {
    actions.add('group_mute')
    actions.add('group_unmute')
  }
  return actions
}

function readbackRoute(action) {
  if (['mute', 'unmute', 'rename'].includes(action)) return '/v1/group/get-group-members'
  if (action === 'group_announcement') return '/v1/group/notice-list'
  if (['group_mute', 'group_unmute'].includes(action)) return '/v1/group/get-group-list'
  return ''
}

export function compactContractCapture(value) {
  validateContract(value)
  const capabilities = new Set(value.expectedNormalizedState?.capabilities || [])
  const requiredGroupCount = [...capabilities].some(capability => twoGroupCapabilities.has(capability)) ? 2 : 1
  const selected = new Set()
  const overrides = new Map()
  const operations = value.operations
  const actions = requestedActions(capabilities)
  const writes = []
  const targetGroups = new Set()
  for (const [index, operation] of operations.entries()) {
    const action = operation.expectedNormalizedState?.action
    if (!actions.has(action)) continue
    selected.add(index)
    writes.push([index, operation])
    const groupId = operationGroupId(operation)
    if (groupId) targetGroups.add(groupId)
  }
  if (targetGroups.size < requiredGroupCount && capabilities.has('memberEvents')) {
    const eventKindsByGroup = new Map()
    for (const callback of value.callbacks) {
      if (!['teamMemberJoined', 'teamMemberUpdated', 'teamMemberLeft'].includes(callback?.kind)) continue
      const groupId = callbackGroupId(callback)
      if (!groupId) continue
      if (!eventKindsByGroup.has(groupId)) eventKindsByGroup.set(groupId, new Set())
      eventKindsByGroup.get(groupId).add(callback.kind)
    }
    for (const [groupId, kinds] of eventKindsByGroup) {
      if (!['teamMemberJoined', 'teamMemberUpdated', 'teamMemberLeft'].every(kind => kinds.has(kind))) continue
      targetGroups.add(groupId)
      if (targetGroups.size >= requiredGroupCount) break
    }
  }
  if (targetGroups.size < requiredGroupCount) throw new Error(`精简失败：写操作只覆盖 ${targetGroups.size} 个群`)

  const groupListIndex = operations.findIndex(operation => operation.route === '/v1/group/get-group-list')
  if (groupListIndex < 0) throw new Error('精简失败：缺少群列表只读请求')
  selected.add(groupListIndex)
  overrides.set(groupListIndex, compactGroupListOperation(operations[groupListIndex], targetGroups))

  for (const groupId of targetGroups) {
    const firstWriteIndex = writes.find(([index, operation]) => operationGroupId(operation) === groupId)?.[0] ?? operations.length
    const preflightIndex = operations.findLastIndex((operation, index) => index < firstWriteIndex
      && operation.route === '/v1/group/get-group-members'
      && operationGroupId(operation) === groupId)
    if (preflightIndex < 0) throw new Error(`精简失败：群 ${groupId} 缺少写操作前成员预检`)
    selected.add(preflightIndex)
  }

  for (const [index, operation] of writes) {
    const route = readbackRoute(operation.expectedNormalizedState?.action)
    if (!route) continue
    const groupId = operationGroupId(operation)
    const readbackIndex = operations.findIndex((candidate, candidateIndex) => candidateIndex > index
      && candidate.route === route
      && (!groupId || operationGroupId(candidate) === groupId))
    if (readbackIndex < 0) throw new Error(`精简失败：${operation.expectedNormalizedState?.action} 缺少后续状态回读`)
    selected.add(readbackIndex)
    if (route === '/v1/group/get-group-members') {
      overrides.set(readbackIndex, compactMemberReadback(operations[readbackIndex], operationUserId(operation)))
    }
  }

  const callbacks = capabilities.has('memberEvents')
    ? value.callbacks.filter(callback => ['teamMemberJoined', 'teamMemberUpdated', 'teamMemberLeft'].includes(callback?.kind)
      && targetGroups.has(callbackGroupId(callback)))
    : value.callbacks.slice(0, 1)
  if (callbacks.length === 0) throw new Error('精简失败：缺少真实回调')
  return {
    ...value,
    callbacks,
    operations: operations.flatMap((operation, index) => selected.has(index) ? [overrides.get(index) || operation] : []),
  }
}

function createSanitizer() {
  const identifiers = new Map()
  const counts = new Map()

  function placeholder(kind, raw) {
    const stableKey = `${kind}:${String(raw)}`
    if (!identifiers.has(stableKey)) {
      const ordinal = (counts.get(kind) ?? 0) + 1
      counts.set(kind, ordinal)
      identifiers.set(stableKey, `${kind}_${String(ordinal).padStart(3, '0')}`)
    }
    return identifiers.get(stableKey)
  }

  function sanitize(value, context = {}) {
    if (typeof value === 'string' && !isProtocolLiteralContext(context)) {
      const embedded = parsedJson(value)
      if (embedded !== null) return JSON.stringify(sanitize(embedded, context))
    }

    const kind = kindForKey(context.key, context)
    if (kind && redactable(value)) {
      return placeholder(kind, value)
    }
    if (kind && Array.isArray(value)) {
      return value.map(item =>
        typeof item === 'string' || typeof item === 'number' ? placeholder(kind, item) : sanitize(item, context),
      )
    }
    if (typeof value === 'string' && !isProtocolLiteralContext(context)) {
      if (uuidValue.test(value)) return placeholder('UUID', value)
      if (urlValue.test(value)) return placeholder('URL', value)
      if (isOpaqueBase64(value)) return placeholder('OPAQUE', value)
      if (/\bteam-\d{5,}\b/i.test(value)) {
        return value.replace(teamSessionValue, match => placeholder('SESSION', match))
      }
    }
    if (Array.isArray(value)) return value.map(item => sanitize(item, { ...context, key: '', parentKey: context.key }))
    if (value && typeof value === 'object') {
      const route = typeof value.route === 'string' ? value.route : context.route || ''
      const requestKind = typeof value.kind === 'string' ? value.kind : context.requestKind || ''
      return Object.fromEntries(
        Object.keys(value)
          .sort((left, right) => left.localeCompare(right, 'en'))
          .map(childKey => [childKey, sanitize(value[childKey], {
            key: childKey,
            parentKey: context.key || context.parentKey || '',
            parent: value,
            route,
            requestKind,
          })]),
      )
    }
    return value
  }

  return sanitize
}

function validateContract(value) {
  if (value?.version !== 2) throw new Error('只接受 Contract v2 捕获')
  const metadata = value.metadata
  if (!metadata || typeof metadata.appFileVersion !== 'string') throw new Error('缺少 metadata.appFileVersion')
  if (!/^[a-f0-9]{64}$/i.test(metadata.mainScriptSha256 ?? '')) throw new Error('mainScriptSha256 必须是 64 位 SHA-256')
  if (!Array.isArray(value.operations) || value.operations.length === 0) throw new Error('捕获缺少 operations')
  if (fixtureEndpoint.test(JSON.stringify(value))) throw new Error('真实校准证据包含 Fixture、9233 或 51300')
}

export function sanitizeContractCapture(value, options = {}) {
  assertNoSecrets(value)
  validateContract(value)
  const source = options.compact ? compactContractCapture(value) : value
  const sanitized = createSanitizer()(source)
  assertNoSecrets(sanitized)
  assertSanitizedContract(sanitized)
  return sanitized
}

async function selfTest() {
  const raw = {
    version: 2,
    metadata: { appFileVersion: 'fixture', mainScriptSha256: 'a'.repeat(64) },
    operations: [{
      route: 'nim.sendCustomMsg',
      request: { kind: 'nim', params: { groupId: 77, target: 77, userId: 88, msgId: 'm-1' } },
    }],
  }
  const first = JSON.stringify(sanitizeContractCapture(raw))
  const second = JSON.stringify(sanitizeContractCapture(raw))
  if (first !== second || !first.includes('GROUP_001') || !first.includes('USER_001') || !first.includes('MESSAGE_001')) {
    throw new Error('确定性脱敏自检失败')
  }
  try {
    sanitizeContractCapture({ ...raw, apiKey: 'placeholder' })
    throw new Error('敏感字段拦截自检失败')
  } catch (error) {
    if (!String(error.message).includes('敏感字段')) throw error
  }
  process.stdout.write('Contract v2 sanitizer self-test passed\n')
}

async function main() {
  const args = process.argv.slice(2)
  const compactIndex = args.indexOf('--compact')
  const compact = compactIndex >= 0
  if (compact) args.splice(compactIndex, 1)
  const [input, output] = args
  if (input === '--self-test') return selfTest()
  if (!input || !output) {
    throw new Error('用法：node scripts/sanitize-contract-capture.mjs INPUT.json OUTPUT.json [--compact]')
  }
  const raw = JSON.parse(await readFile(input, 'utf8'))
  const sanitized = sanitizeContractCapture(raw, { compact })
  await mkdir(path.dirname(output), { recursive: true })
  await writeFile(output, `${JSON.stringify(sanitized, null, 2)}\n`, { flag: 'wx' })
  process.stdout.write(`已生成脱敏契约：${output}\n`)
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main().catch(error => {
    process.stderr.write(`${error.message}\n`)
    process.exitCode = 1
  })
}
