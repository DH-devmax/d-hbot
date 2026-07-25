import { readFile } from 'node:fs/promises'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const capabilityNames = ['sendText', 'recall', 'mute', 'rename', 'announcement', 'groupMute', 'memberEvents']
const outputCapabilityNames = ['announcement', 'sendText', 'mute', 'recall', 'rename', 'removeMember', 'groupMute', 'memberEvents']

function requireObject(value, label) {
  if (!value || Array.isArray(value) || typeof value !== 'object') throw new Error(`${label} 必须是对象`)
}

function verified(operation) {
  return operation?.normalizedReceipt?.status === 'succeeded'
    && ['verified', 'not-applicable'].includes(operation?.normalizedReceipt?.verification)
}

function sameTarget(left, right, fields) {
  return fields.every(field => String(left?.[field] ?? '') === String(right?.[field] ?? ''))
}

function hasRestoredPair(operations, firstAction, secondAction, fields, valueField) {
  const first = operations.filter(operation => operation?.expectedNormalizedState?.action === firstAction && verified(operation))
  const second = operations.filter(operation => operation?.expectedNormalizedState?.action === secondAction && verified(operation))
  return first.some(left => second.some(right => {
    if (!sameTarget(left.expectedNormalizedState, right.expectedNormalizedState, fields)) return false
    if (!valueField) return true
    return String(left.expectedNormalizedState?.[valueField] ?? '') !== String(right.expectedNormalizedState?.[valueField] ?? '')
  }))
}

function capabilityEvidence(capture) {
  const operations = capture.operations
  const callbacks = capture.callbacks
  return {
    sendText: operations.some(operation => operation?.expectedNormalizedState?.action === 'send_text'
      && operation?.normalizedReceipt?.status === 'succeeded'
      && String(operation?.normalizedReceipt?.messageId || operation?.expectedNormalizedState?.messageId || '')),
    recall: operations.some(operation => operation?.expectedNormalizedState?.action === 'recall'
      && operation?.expectedNormalizedState?.recalled === true
      && verified(operation)),
    mute: hasRestoredPair(operations, 'mute', 'unmute', ['groupId', 'userId']),
    rename: hasRestoredPair(operations, 'rename', 'rename', ['groupId', 'userId'], 'cardName'),
    announcement: hasRestoredPair(operations, 'group_announcement', 'group_announcement', ['groupId'], 'content'),
    groupMute: hasRestoredPair(operations, 'group_mute', 'group_unmute', ['groupId']),
    memberEvents: callbacks.some(callback => ['teamMemberJoined', 'teamMemberUpdated', 'teamMemberLeft'].includes(callback?.kind)),
  }
}

export function verifyCalibrationCapture(capture) {
  requireObject(capture, 'Contract v2')
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
  if (capture.expectedNormalizedState.restored !== true) throw new Error('校准证据未确认恢复原状态')
  for (const [index, operation] of capture.operations.entries()) {
    requireObject(operation, `operations[${index}]`)
    for (const field of ['request', 'transport', 'business', 'normalizedReceipt', 'expectedNormalizedState']) {
      requireObject(operation[field], `operations[${index}].${field}`)
    }
    for (const field of ['transportCode', 'errno', 'requestId']) {
      if (!(field in operation.transport)) throw new Error(`operations[${index}].transport 缺少 ${field}`)
    }
    for (const field of ['code', 'errno', 'msg', 'data']) {
      if (!(field in operation.business)) throw new Error(`operations[${index}].business 缺少 ${field}`)
    }
  }
  const routes = new Set(capture.operations.map(operation => operation.route))
  if (!routes.has('/v1/group/get-group-list') || !routes.has('/v1/group/get-group-members')) {
    throw new Error('校准证据缺少群列表或成员列表只读请求')
  }
  const requested = capture.expectedNormalizedState.capabilities
  if (!Array.isArray(requested) || requested.length === 0) throw new Error('校准证据缺少能力清单')
  const unknown = requested.filter(capability => !capabilityNames.includes(capability))
  if (unknown.length) throw new Error(`校准证据包含未知能力：${unknown.join(', ')}`)
  const evidence = capabilityEvidence(capture)
  const missing = requested.filter(capability => !evidence[capability])
  if (missing.length) throw new Error(`以下能力缺少成功、回读或恢复证据：${missing.join(', ')}`)
  return {
    appFileVersion: capture.metadata.appFileVersion,
    mainScriptSha256: capture.metadata.mainScriptSha256,
    verified: true,
    capabilities: Object.fromEntries(outputCapabilityNames.map(capability => [
      capability,
      requested.includes(capability) && evidence[capability] ? 'supported' : 'unverified',
    ])),
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
