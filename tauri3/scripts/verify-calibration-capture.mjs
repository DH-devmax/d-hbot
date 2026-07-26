import { readFile } from 'node:fs/promises'
import path from 'node:path'
import process from 'node:process'
import { isDeepStrictEqual } from 'node:util'
import { fileURLToPath } from 'node:url'

import { assertSanitizedContract } from './sanitize-contract-capture.mjs'

const capabilityNames = ['sendText', 'recall', 'mute', 'rename', 'announcement', 'groupMute', 'memberEvents']
const outputCapabilityNames = ['announcement', 'sendText', 'mute', 'recall', 'rename', 'removeMember', 'groupMute', 'memberEvents']
const twoGroupCapabilities = new Set(['sendText', 'recall', 'mute', 'rename', 'announcement', 'groupMute'])
const memberEventKinds = ['teamMemberJoined', 'teamMemberUpdated', 'teamMemberLeft']
const restorationFamilies = {
  mute: 'member-mute',
  rename: 'member-rename',
  announcement: 'group-notice',
  groupMute: 'group-mute',
}

function requireObject(value, label) {
  if (!value || Array.isArray(value) || typeof value !== 'object') throw new Error(`${label} 必须是对象`)
}

function requireNonEmptyString(value, label) {
  if (typeof value !== 'string' || !value.trim()) throw new Error(`${label} 必须是非空字符串`)
}

function numeric(value, label) {
  if ((typeof value !== 'number' && typeof value !== 'string') || value === '' || !Number.isFinite(Number(value))) {
    throw new Error(`${label} 必须是数字`)
  }
  return Number(value)
}

function isTransportSuccess(value) {
  const code = Number(value)
  return code === 0 || (code >= 200 && code < 300)
}

function isBusinessSuccess(value) {
  const code = Number(value)
  return code === 0 || code === 200
}

function canonicalId(value) {
  if (typeof value === 'string' || typeof value === 'number') {
    const id = String(value).trim()
    return id && id !== '0' ? id : ''
  }
  if (!value || Array.isArray(value) || typeof value !== 'object') return ''
  for (const key of ['groupId', 'groupCloudId', 'teamId', 'target', 'id']) {
    const id = canonicalId(value[key])
    if (id) return id
  }
  return ''
}

function operationGroupId(operation) {
  const state = operation.expectedNormalizedState || {}
  const params = operation.request?.params || {}
  for (const value of [
    state.groupId,
    state.groupCloudId,
    state.teamId,
    params.groupId,
    params.groupCloudId,
    params.teamId,
    params.target,
    params.to,
  ]) {
    const id = canonicalId(value)
    if (id) return id
  }
  return ''
}

function operationUserId(operation) {
  const state = operation.expectedNormalizedState || {}
  const params = operation.request?.params || {}
  for (const value of [state.userId, params.userId, params.memberId]) {
    const id = canonicalId(value)
    if (id) return id
  }
  return ''
}

function operationMessageId(operation) {
  const state = operation.expectedNormalizedState || {}
  const params = operation.request?.params || {}
  const receipt = operation.normalizedReceipt || {}
  const businessData = operation.business?.data || {}
  for (const value of [
    receipt.messageId,
    state.messageId,
    params.messageId,
    params.msgId,
    businessData.messageId,
    businessData.idServer,
  ]) {
    const id = canonicalId(value)
    if (id) return id
  }
  return ''
}

function callbackGroupId(callback) {
  const payload = callback?.payload
  if (!payload || Array.isArray(payload) || typeof payload !== 'object') return ''
  for (const value of [payload.groupId, payload.groupCloudId, payload.teamId, payload.target, payload.to]) {
    const id = canonicalId(value)
    if (id) return id
  }
  return ''
}

function validateOperation(operation, index) {
  const label = `operations[${index}]`
  requireObject(operation, label)
  requireNonEmptyString(operation.name, `${label}.name`)
  requireNonEmptyString(operation.route, `${label}.route`)
  for (const field of ['request', 'transport', 'business', 'normalizedReceipt', 'expectedNormalizedState']) {
    requireObject(operation[field], `${label}.${field}`)
  }

  requireNonEmptyString(operation.request.kind, `${label}.request.kind`)
  if (!Object.hasOwn(operation.request, 'params')) throw new Error(`${label}.request 缺少 params`)
  requireObject(operation.request.params, `${label}.request.params`)

  for (const field of ['transportCode', 'errno', 'requestId']) {
    if (!Object.hasOwn(operation.transport, field)) throw new Error(`${label}.transport 缺少 ${field}`)
  }
  if (!isTransportSuccess(numeric(operation.transport.transportCode, `${label}.transport.transportCode`))) {
    throw new Error(`${label}.transport.transportCode 不是成功码`)
  }
  if (numeric(operation.transport.errno, `${label}.transport.errno`) !== 0) {
    throw new Error(`${label}.transport.errno 不是成功码`)
  }
  requireNonEmptyString(operation.transport.requestId, `${label}.transport.requestId`)

  for (const field of ['code', 'errno', 'msg', 'data']) {
    if (!Object.hasOwn(operation.business, field)) throw new Error(`${label}.business 缺少 ${field}`)
  }
  if (!isBusinessSuccess(numeric(operation.business.code, `${label}.business.code`))) {
    throw new Error(`${label}.business.code 不是成功码`)
  }
  if (numeric(operation.business.errno, `${label}.business.errno`) !== 0) {
    throw new Error(`${label}.business.errno 不是成功码`)
  }
  if (typeof operation.business.msg !== 'string') throw new Error(`${label}.business.msg 必须是字符串`)

  const receipt = operation.normalizedReceipt
  for (const field of [
    'route',
    'status',
    'transportCode',
    'transportErrno',
    'businessCode',
    'businessErrno',
    'businessMessage',
    'requestId',
    'messageId',
  ]) {
    if (!Object.hasOwn(receipt, field)) throw new Error(`${label}.normalizedReceipt 缺少 ${field}`)
  }
  requireNonEmptyString(receipt.route, `${label}.normalizedReceipt.route`)
  if (receipt.route !== operation.route) throw new Error(`${label}.normalizedReceipt.route 与 operation.route 不一致`)
  if (receipt.status !== 'succeeded') throw new Error(`${label}.normalizedReceipt.status 不是 succeeded`)
  if (typeof receipt.businessMessage !== 'string') throw new Error(`${label}.normalizedReceipt.businessMessage 必须是字符串`)
  requireNonEmptyString(receipt.requestId, `${label}.normalizedReceipt.requestId`)
  if (typeof receipt.messageId !== 'string') throw new Error(`${label}.normalizedReceipt.messageId 必须是字符串`)
  if (receipt.verification != null && !['verified', 'not-applicable'].includes(receipt.verification)) {
    throw new Error(`${label}.normalizedReceipt.verification 不是已确认状态`)
  }
  for (const [field, success] of [
    ['transportCode', isTransportSuccess],
    ['transportErrno', value => Number(value) === 0],
    ['businessCode', isBusinessSuccess],
    ['businessErrno', value => Number(value) === 0],
  ]) {
    if (receipt[field] != null && !success(numeric(receipt[field], `${label}.normalizedReceipt.${field}`))) {
      throw new Error(`${label}.normalizedReceipt.${field} 不是成功码`)
    }
  }
  if (Object.keys(operation.expectedNormalizedState).length === 0) {
    throw new Error(`${label}.expectedNormalizedState 不能为空`)
  }
}

function verified(operation, allowNotApplicable = false) {
  const verification = operation?.normalizedReceipt?.verification
  return operation?.normalizedReceipt?.status === 'succeeded'
    && (verification === 'verified' || (allowNotApplicable && verification === 'not-applicable'))
}

function sameTarget(operation, identity, fields) {
  return fields.every(field => {
    const operationValue = field === 'groupId' ? operationGroupId(operation) : operationUserId(operation)
    return operationValue && operationValue === canonicalId(identity?.[field])
  })
}

function restorationEvidence(state, requested) {
  if (state.restored !== true) throw new Error('校准证据未确认恢复原状态')
  if (state.restorationError) throw new Error(`校准恢复回读仍有错误：${state.restorationError}`)
  if (!Array.isArray(state.baselines)) throw new Error('expectedNormalizedState.baselines 必须是数组')
  if (!Array.isArray(state.observedStates)) throw new Error('expectedNormalizedState.observedStates 必须是数组')

  const restored = []
  for (const [index, baseline] of state.baselines.entries()) {
    requireObject(baseline, `expectedNormalizedState.baselines[${index}]`)
    requireNonEmptyString(baseline.family, `expectedNormalizedState.baselines[${index}].family`)
    requireObject(baseline.identity, `expectedNormalizedState.baselines[${index}].identity`)
    requireObject(baseline.state, `expectedNormalizedState.baselines[${index}].state`)
    const matches = state.observedStates.filter(candidate => candidate?.family === baseline.family
      && isDeepStrictEqual(candidate?.identity, baseline.identity))
    if (matches.length !== 1) throw new Error(`校准恢复回读缺少或重复基线目标：${baseline.family}`)
    requireObject(matches[0].state, `expectedNormalizedState.observedStates.${baseline.family}.state`)
    if (!isDeepStrictEqual(matches[0].state, baseline.state)) {
      throw new Error(`校准最终状态与操作前基线不一致：${baseline.family}`)
    }
    restored.push(baseline)
  }

  for (const capability of requested) {
    const family = restorationFamilies[capability]
    if (family && !restored.some(baseline => baseline.family === family)) {
      throw new Error(`能力 ${capability} 缺少操作前基线和最终回读证据`)
    }
  }
  return restored
}

function hasOrderedPair(operations, firstPredicate, secondPredicate) {
  return operations.some((operation, index) => firstPredicate(operation)
    && operations.slice(index + 1).some(secondPredicate))
}

function restoredCapabilityGroups(operations, baselines, capability) {
  const groups = new Set()
  const family = restorationFamilies[capability]
  for (const baseline of baselines.filter(item => item.family === family)) {
    const groupId = canonicalId(baseline.identity.groupId)
    if (!groupId) continue
    const targetFields = capability === 'mute' || capability === 'rename' ? ['groupId', 'userId'] : ['groupId']
    const targetOperations = operations.filter(operation => sameTarget(operation, baseline.identity, targetFields))
    let complete = false
    if (capability === 'mute') {
      complete = baseline.state.muted === false && hasOrderedPair(
        targetOperations,
        operation => operation.expectedNormalizedState.action === 'mute'
          && operation.expectedNormalizedState.muted === true
          && verified(operation),
        operation => operation.expectedNormalizedState.action === 'unmute'
          && operation.expectedNormalizedState.muted === false
          && verified(operation),
      )
    } else if (capability === 'rename') {
      const baselineName = String(baseline.state.cardName ?? '')
      complete = hasOrderedPair(
        targetOperations,
        operation => operation.expectedNormalizedState.action === 'rename'
          && String(operation.expectedNormalizedState.cardName ?? '') !== baselineName
          && verified(operation),
        operation => operation.expectedNormalizedState.action === 'rename'
          && String(operation.expectedNormalizedState.cardName ?? '') === baselineName
          && verified(operation),
      )
    } else if (capability === 'announcement') {
      const baselineContent = String(baseline.state.content ?? '')
      const baselineNoticeId = canonicalId(baseline.state.noticeId)
      const matchesNotice = operation => !baselineNoticeId
        || canonicalId(operation.expectedNormalizedState.noticeId) === baselineNoticeId
      complete = hasOrderedPair(
        targetOperations,
        operation => operation.expectedNormalizedState.action === 'group_announcement'
          && String(operation.expectedNormalizedState.content ?? '') !== baselineContent
          && Boolean(operationMessageId(operation))
          && matchesNotice(operation)
          && verified(operation),
        operation => operation.expectedNormalizedState.action === 'group_announcement'
          && String(operation.expectedNormalizedState.content ?? '') === baselineContent
          && Boolean(operationMessageId(operation))
          && matchesNotice(operation)
          && verified(operation),
      )
    } else if (capability === 'groupMute') {
      complete = String(baseline.state.muteMode || '').toUpperCase() === 'MUTE_NO' && hasOrderedPair(
        targetOperations,
        operation => operation.expectedNormalizedState.action === 'group_mute'
          && operation.expectedNormalizedState.muted === true
          && verified(operation),
        operation => operation.expectedNormalizedState.action === 'group_unmute'
          && operation.expectedNormalizedState.muted === false
          && verified(operation),
      )
    }
    if (complete) groups.add(groupId)
  }
  return groups
}

function capabilityEvidence(capture, baselines, authorizedGroups) {
  const operations = capture.operations
  const sentMessages = new Set()
  const sendText = new Set()
  for (const operation of operations) {
    if (operation.expectedNormalizedState.action !== 'send_text' || !verified(operation, true)) continue
    const groupId = operationGroupId(operation)
    const messageId = operationMessageId(operation)
    if (!groupId || !messageId) continue
    sendText.add(groupId)
    sentMessages.add(`${groupId}\u0000${messageId}`)
  }

  const recall = new Set()
  for (const operation of operations) {
    if (operation.expectedNormalizedState.action !== 'recall'
      || operation.expectedNormalizedState.recalled !== true
      || !verified(operation)) continue
    const groupId = operationGroupId(operation)
    const messageId = operationMessageId(operation)
    if (sentMessages.has(`${groupId}\u0000${messageId}`)) recall.add(groupId)
  }

  const memberEvents = new Map()
  for (const callback of capture.callbacks) {
    if (!memberEventKinds.includes(callback?.kind)) continue
    const groupId = callbackGroupId(callback)
    if (!groupId || !authorizedGroups.has(groupId)) continue
    if (!memberEvents.has(groupId)) memberEvents.set(groupId, new Set())
    memberEvents.get(groupId).add(callback.kind)
  }
  const completeMemberEventGroups = new Set(
    [...memberEvents.entries()]
      .filter(([, kinds]) => memberEventKinds.every(kind => kinds.has(kind)))
      .map(([groupId]) => groupId),
  )

  return {
    sendText,
    recall,
    mute: restoredCapabilityGroups(operations, baselines, 'mute'),
    rename: restoredCapabilityGroups(operations, baselines, 'rename'),
    announcement: restoredCapabilityGroups(operations, baselines, 'announcement'),
    groupMute: restoredCapabilityGroups(operations, baselines, 'groupMute'),
    memberEvents: completeMemberEventGroups,
  }
}

function verifyReadOnlyPreflight(capture, requested) {
  if (!capture.operations.some(operation => operation.route === '/v1/group/get-group-list')) {
    throw new Error('校准证据缺少群列表只读请求')
  }
  const memberGroups = new Set(capture.operations
    .filter(operation => operation.route === '/v1/group/get-group-members')
    .map(operationGroupId)
    .filter(Boolean))
  const requiredCount = requested.some(capability => twoGroupCapabilities.has(capability)) ? 2 : 1
  if (memberGroups.size < requiredCount) {
    throw new Error(`校准证据需要 ${requiredCount} 个不同 groupId 的成员列表只读请求（当前 ${memberGroups.size}）`)
  }
  return memberGroups
}

export function verifyCalibrationCapture(capture) {
  requireObject(capture, 'Contract v2')
  assertSanitizedContract(capture)
  if (capture.version !== 2) throw new Error('只接受 Contract v2')
  requireObject(capture.metadata, 'metadata')
  if (!/^[a-f0-9]{64}$/i.test(capture.metadata.mainScriptSha256 || '')) throw new Error('主脚本 SHA-256 无效')
  if (!capture.metadata.appFileVersion || /fixture/i.test(capture.metadata.appFileVersion)) throw new Error('真实校准版本无效')
  const serialized = JSON.stringify(capture).toLowerCase()
  if (serialized.includes('127.0.0.1:9233') || serialized.includes('127.0.0.1:51300') || serialized.includes('dh fixture')) {
    throw new Error('真实校准证据包含 Fixture、9233 或 51300')
  }
  if (!Array.isArray(capture.operations) || capture.operations.length === 0) throw new Error('校准证据缺少 operations')
  if (!Array.isArray(capture.callbacks) || capture.callbacks.length === 0) throw new Error('校准证据缺少 callbacks')
  requireObject(capture.expectedNormalizedState, 'expectedNormalizedState')
  capture.operations.forEach(validateOperation)

  const requested = capture.expectedNormalizedState.capabilities
  if (!Array.isArray(requested) || requested.length === 0) throw new Error('校准证据缺少能力清单')
  const unknown = requested.filter(capability => !capabilityNames.includes(capability))
  if (unknown.length) throw new Error(`校准证据包含未知能力：${unknown.join(', ')}`)
  if (new Set(requested).size !== requested.length) throw new Error('校准能力清单包含重复项')

  const authorizedGroups = verifyReadOnlyPreflight(capture, requested)
  const baselines = restorationEvidence(capture.expectedNormalizedState, requested)
  const evidence = capabilityEvidence(capture, baselines, authorizedGroups)
  const missing = requested.filter(capability => capability !== 'memberEvents' && evidence[capability].size < 2)
  if (missing.length) {
    throw new Error(`能力 ${missing.join(', ')} 缺少两个不同 groupId 的完整证据`)
  }

  return {
    appFileVersion: capture.metadata.appFileVersion,
    mainScriptSha256: capture.metadata.mainScriptSha256.toLowerCase(),
    verified: true,
    capabilities: Object.fromEntries(outputCapabilityNames.map(capability => {
      if (capability === 'removeMember') return [capability, 'unsupported']
      const requiredGroups = capability === 'memberEvents' ? 1 : 2
      return [capability, requested.includes(capability) && evidence[capability].size >= requiredGroups ? 'supported' : 'unverified']
    })),
    groupEvidenceCounts: Object.fromEntries(capabilityNames.map(capability => [capability, evidence[capability].size])),
    operationCount: capture.operations.length,
    callbackCount: capture.callbacks.length,
  }
}

async function main() {
  const input = process.argv[2]
  if (!input) throw new Error('用法：node scripts/verify-calibration-capture.mjs CONTRACT.sanitized.json')
  const capture = JSON.parse(await readFile(input, 'utf8'))
  process.stdout.write(`${JSON.stringify(verifyCalibrationCapture(capture), null, 2)}\n`)
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main().catch(error => {
    process.stderr.write(`${error.message}\n`)
    process.exitCode = 1
  })
}
