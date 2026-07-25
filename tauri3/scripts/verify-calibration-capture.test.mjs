import assert from 'node:assert/strict'
import test from 'node:test'

import { sanitizeContractCapture } from './sanitize-contract-capture.mjs'
import { verifyCalibrationCapture } from './verify-calibration-capture.mjs'

function operation(route, action, state = {}) {
  return {
    name: action,
    route,
    request: { kind: 'request', params: {} },
    transport: { transportCode: 200, errno: 0, requestId: `request-${action}` },
    business: { code: 0, errno: 0, msg: 'OK', data: {} },
    normalizedReceipt: { route, status: 'succeeded', verification: 'verified', messageId: action === 'send_text' ? 'message-1' : '' },
    expectedNormalizedState: { action, ...state },
  }
}

function capture() {
  return {
    version: 2,
    metadata: { appFileVersion: '2.7.8', mainScriptSha256: 'a'.repeat(64), pageUrl: 'file:///index.html' },
    operations: [
      operation('/v1/group/get-group-list', 'list_groups'),
      operation('/v1/group/get-group-members', 'list_members'),
      operation('nim.sendCustomMsg', 'send_text', { messageId: 'message-1' }),
      operation('/v1/group/message-rollback', 'recall', { messageId: 'message-1', recalled: true }),
      operation('/v1/group/set-member-mute', 'mute', { groupId: 'g1', userId: 'u1' }),
      operation('/v1/group/member-mute-cancel', 'unmute', { groupId: 'g1', userId: 'u1' }),
      operation('/v1/group/set-member-nickname', 'rename', { groupId: 'g1', userId: 'u1', cardName: 'test' }),
      operation('/v1/group/set-member-nickname', 'rename', { groupId: 'g1', userId: 'u1', cardName: 'original' }),
      operation('/v1/group/notice-opt', 'group_announcement', { groupId: 'g1', content: 'test' }),
      operation('/v1/group/notice-opt', 'group_announcement', { groupId: 'g1', content: 'original' }),
      operation('/v1/group/set-group-mute', 'group_mute', { groupId: 'g1' }),
      operation('/v1/group/set-group-mute', 'group_unmute', { groupId: 'g1' }),
    ],
    callbacks: [{ kind: 'teamMemberUpdated' }],
    expectedNormalizedState: {
      restored: true,
      restorationError: '',
      baselines: [
        { family: 'member-mute', identity: { groupId: 'g1', userId: 'u1' }, state: { muted: false } },
        { family: 'member-rename', identity: { groupId: 'g1', userId: 'u1' }, state: { cardName: 'original' } },
        { family: 'group-notice', identity: { groupId: 'g1' }, state: { noticeId: 'notice-1', content: 'original' } },
        { family: 'group-mute', identity: { groupId: 'g1' }, state: { muteMode: 'MUTE_NO' } },
      ],
      observedStates: [
        { family: 'member-mute', identity: { groupId: 'g1', userId: 'u1' }, state: { muted: false } },
        { family: 'member-rename', identity: { groupId: 'g1', userId: 'u1' }, state: { cardName: 'original' } },
        { family: 'group-notice', identity: { groupId: 'g1' }, state: { noticeId: 'notice-1', content: 'original' } },
        { family: 'group-mute', identity: { groupId: 'g1' }, state: { muteMode: 'MUTE_NO' } },
      ],
      capabilities: ['sendText', 'recall', 'mute', 'rename', 'announcement', 'groupMute', 'memberEvents'],
    },
  }
}

test('verifies paired real calibration evidence', () => {
  const result = verifyCalibrationCapture(capture())
  assert.equal(result.capabilities.groupMute, 'supported')
  assert.equal(result.capabilities.removeMember, 'unverified')
})

test('rejects missing restoration pair', () => {
  const value = capture()
  value.operations = value.operations.filter(operation => operation.expectedNormalizedState.action !== 'unmute')
  assert.throws(() => verifyCalibrationCapture(value), /mute/)
})

test('rejects a final read that differs from the captured baseline', () => {
  const value = capture()
  value.expectedNormalizedState.observedStates.find(state => state.family === 'member-rename').state = { cardName: 'not-restored' }
  assert.throws(() => verifyCalibrationCapture(value), /基线不一致/)
})

test('rejects paired writes without a baseline for that capability', () => {
  const value = capture()
  value.expectedNormalizedState.baselines = value.expectedNormalizedState.baselines.filter(state => state.family !== 'group-notice')
  value.expectedNormalizedState.observedStates = value.expectedNormalizedState.observedStates.filter(state => state.family !== 'group-notice')
  assert.throws(() => verifyCalibrationCapture(value), /announcement.*基线/)
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
  assert.throws(() => verifyCalibrationCapture(value), /Fixture/)
})
