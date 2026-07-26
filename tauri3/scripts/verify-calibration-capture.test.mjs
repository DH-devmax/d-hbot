import assert from 'node:assert/strict'
import test from 'node:test'

import { sanitizeContractCapture } from './sanitize-contract-capture.mjs'
import { verifyCalibrationCapture } from './verify-calibration-capture.mjs'

function operation(route, action, state = {}, params = {}) {
  const suffix = [action, state.groupId, state.userId, state.messageId, state.content].filter(Boolean).join('-')
  return {
    name: suffix || action,
    route,
    request: { kind: route.startsWith('nim.') ? 'nim' : 'request', params },
    transport: { transportCode: 200, errno: 0, requestId: `request-${suffix || action}` },
    business: { code: 0, errno: 0, msg: 'OK', data: {} },
    normalizedReceipt: {
      route,
      status: 'succeeded',
      transportCode: 200,
      transportErrno: 0,
      businessCode: 0,
      businessErrno: 0,
      businessMessage: 'OK',
      requestId: `request-${suffix || action}`,
      verification: action === 'send_text' ? 'not-applicable' : 'verified',
      messageId: state.messageId || '',
    },
    expectedNormalizedState: { action, ...state },
  }
}

function groupOperations(groupId, userId, ordinal) {
  const messageId = `message-${ordinal}`
  const noticeId = `notice-${ordinal}`
  return [
    operation('/v1/group/get-group-members', 'list_members', { groupId, readOnly: true }, { groupId }),
    operation('nim.sendCustomMsg', 'send_text', { groupId, messageId }, { target: groupId, content: `test-${ordinal}` }),
    operation('/v1/group/message-rollback', 'recall', { groupId, messageId, recalled: true }, { groupCloudId: groupId, msgId: messageId }),
    operation('/v1/group/set-member-mute', 'mute', { groupId, userId, muted: true }, { groupId, userId, min: 1 }),
    operation('/v1/group/member-mute-cancel', 'unmute', { groupId, userId, muted: false }, { groupId, userId }),
    operation('/v1/group/set-member-nickname', 'rename', { groupId, userId, cardName: `temporary-${ordinal}` }, { groupId, userId, nick: `temporary-${ordinal}` }),
    operation('/v1/group/set-member-nickname', 'rename', { groupId, userId, cardName: `original-${ordinal}` }, { groupId, userId, nick: `original-${ordinal}` }),
    operation('/v1/group/notice-opt', 'group_announcement', {
      groupId,
      noticeId,
      content: `temporary-notice-${ordinal}`,
      messageId: `notice-message-${ordinal}-1`,
    }, { groupId, noticeId, noticeContent: `temporary-notice-${ordinal}` }),
    operation('/v1/group/notice-opt', 'group_announcement', {
      groupId,
      noticeId,
      content: `original-notice-${ordinal}`,
      messageId: `notice-message-${ordinal}-2`,
    }, { groupId, noticeId, noticeContent: `original-notice-${ordinal}` }),
    operation('/v1/group/set-group-mute', 'group_mute', { groupId, muted: true }, { groupId, muteMode: 'MUTE_MEMBER' }),
    operation('/v1/group/set-group-mute', 'group_unmute', { groupId, muted: false }, { groupId, muteMode: 'MUTE_NO' }),
  ]
}

function restorationState(groupId, userId, ordinal) {
  return [
    { family: 'member-mute', identity: { groupId, userId }, state: { muted: false } },
    { family: 'member-rename', identity: { groupId, userId }, state: { cardName: `original-${ordinal}` } },
    { family: 'group-notice', identity: { groupId }, state: { noticeId: `notice-${ordinal}`, content: `original-notice-${ordinal}` } },
    { family: 'group-mute', identity: { groupId }, state: { muteMode: 'MUTE_NO' } },
  ]
}

function capture() {
  const baselines = [
    ...restorationState('g1', 'u1', 1),
    ...restorationState('g2', 'u2', 2),
  ]
  return {
    version: 2,
    metadata: { appFileVersion: '2.7.8', mainScriptSha256: 'a'.repeat(64), pageUrl: 'file:///index.html' },
    operations: [
      operation('/v1/group/get-group-list', 'list_groups', { readOnly: true }),
      ...groupOperations('g1', 'u1', 1),
      ...groupOperations('g2', 'u2', 2),
    ],
    callbacks: [
      { kind: 'teamMemberJoined', payload: { teamId: 'g1' } },
      { kind: 'teamMemberUpdated', payload: { teamId: 'g1' } },
      { kind: 'teamMemberLeft', payload: { teamId: 'g1' } },
    ],
    expectedNormalizedState: {
      restored: true,
      restorationError: '',
      baselines,
      observedStates: structuredClone(baselines),
      capabilities: ['sendText', 'recall', 'mute', 'rename', 'announcement', 'groupMute', 'memberEvents'],
    },
  }
}

function verifyCapture(value) {
  return verifyCalibrationCapture(sanitizeContractCapture(value))
}

test('verifies repeated routes with complete evidence from two groups', () => {
  const result = verifyCapture(capture())
  for (const capability of ['sendText', 'recall', 'mute', 'rename', 'announcement', 'groupMute', 'memberEvents']) {
    assert.equal(result.capabilities[capability], 'supported')
  }
  assert.equal(result.capabilities.removeMember, 'unsupported')
  assert.equal(result.groupEvidenceCounts.rename, 2)
  assert.equal(result.groupEvidenceCounts.memberEvents, 1)
})

test('memberEvents remains unverified until all three callbacks exist in one group', () => {
  const value = capture()
  value.callbacks = value.callbacks.filter(callback => callback.kind !== 'teamMemberLeft')
  const result = verifyCapture(value)
  assert.equal(result.capabilities.memberEvents, 'unverified')
  assert.equal(result.capabilities.rename, 'supported')

  value.callbacks.push({ kind: 'teamMemberLeft', payload: { teamId: 'g2' } })
  assert.equal(verifyCapture(value).capabilities.memberEvents, 'unverified')
})

test('a memberEvents-only capture needs one group and all callback kinds', () => {
  const value = capture()
  value.operations = value.operations.filter(operation => operation.route === '/v1/group/get-group-list'
    || (operation.route === '/v1/group/get-group-members' && operation.expectedNormalizedState.groupId === 'g1'))
  value.expectedNormalizedState.capabilities = ['memberEvents']
  value.expectedNormalizedState.baselines = []
  value.expectedNormalizedState.observedStates = []
  const result = verifyCapture(value)
  assert.equal(result.capabilities.memberEvents, 'supported')
})

test('memberEvents ignores callbacks from a group absent from the read-only preflight', () => {
  const value = capture()
  value.callbacks = value.callbacks.map(callback => ({ ...callback, payload: { teamId: 'g3' } }))
  assert.equal(verifyCapture(value).capabilities.memberEvents, 'unverified')
})

test('rejects a requested capability with evidence from only one group', () => {
  const value = capture()
  value.operations = value.operations.filter(operation => !(operation.expectedNormalizedState.action === 'send_text'
    && operation.expectedNormalizedState.groupId === 'g2'))
  assert.throws(() => verifyCapture(value), /sendText.*两个不同 groupId/)
})

test('rejects missing request ids and unsuccessful business envelopes', () => {
  const missingRequest = capture()
  missingRequest.operations[0].transport.requestId = ''
  assert.throws(() => verifyCapture(missingRequest), /requestId.*非空/)

  const missingReceiptRequest = capture()
  missingReceiptRequest.operations[0].normalizedReceipt.requestId = ''
  assert.throws(() => verifyCapture(missingReceiptRequest), /normalizedReceipt\.requestId.*非空/)

  const failedBusiness = capture()
  failedBusiness.operations[3].business.code = 503
  assert.throws(() => verifyCapture(failedBusiness), /business\.code 不是成功码/)
})

test('accepts nullable normalized codes emitted by GatewayReceipt', () => {
  const value = capture()
  const receipt = value.operations.find(operation => operation.expectedNormalizedState.action === 'send_text').normalizedReceipt
  receipt.transportCode = null
  receipt.transportErrno = null
  assert.equal(verifyCapture(value).capabilities.sendText, 'supported')
})

test('rejects a normalized receipt for a different route', () => {
  const value = capture()
  value.operations[2].normalizedReceipt.route = '/wrong-route'
  assert.throws(() => verifyCapture(value), /route 与 operation\.route 不一致/)
})

test('rejects a final read that differs from the captured baseline', () => {
  const value = capture()
  value.expectedNormalizedState.observedStates.find(state => state.family === 'member-rename').state = { cardName: 'not-restored' }
  assert.throws(() => verifyCapture(value), /基线不一致/)
})

test('rejects paired writes without a baseline for every requested group', () => {
  const value = capture()
  value.expectedNormalizedState.baselines = value.expectedNormalizedState.baselines.filter(state => !(state.family === 'group-notice' && state.identity.groupId === 'g2'))
  value.expectedNormalizedState.observedStates = value.expectedNormalizedState.observedStates.filter(state => !(state.family === 'group-notice' && state.identity.groupId === 'g2'))
  assert.throws(() => verifyCapture(value), /announcement.*两个不同 groupId/)
})

test('sanitizes restoration identities before verification', () => {
  const sanitized = sanitizeContractCapture(capture())
  const serialized = JSON.stringify(sanitized)
  assert.equal(serialized.includes('"g1"'), false)
  assert.equal(serialized.includes('"u1"'), false)
  assert.equal(verifyCalibrationCapture(sanitized).verified, true)
})

test('rejects fixture endpoints', () => {
  const value = capture()
  value.metadata.pageUrl = 'http://127.0.0.1:51300/'
  assert.throws(() => verifyCapture(value), /Fixture/)
})

test('strict verifier rejects unsanitized captures', () => {
  assert.throws(() => verifyCalibrationCapture(capture()), /未脱敏/)
})
