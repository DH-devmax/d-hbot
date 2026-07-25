import assert from 'node:assert/strict'
import test from 'node:test'

import { sanitizeContractCapture } from './sanitize-contract-capture.mjs'

function capture(overrides = {}) {
  return {
    version: 2,
    metadata: {
      appFileVersion: '3.0.0-test',
      mainScriptSha256: 'a'.repeat(64),
      pageTitle: '测试页',
      pageUrl: 'http://127.0.0.1:9222/',
    },
    operations: [
      {
        request: {
          params: {
            groupId: 1143980,
            userId: 10006,
            nimId: 'nim-real-6',
            msgId: 'message-real-1',
            noticeContent: '真实公告内容',
            content: { data: '真实测试消息' },
            groupRole: 'member',
            accountState: 'ACCOUNT_STATE_GOOD',
          },
        },
      },
    ],
    ...overrides,
  }
}

test('sanitizer is deterministic and preserves non-identifier business fields', () => {
  const first = sanitizeContractCapture(capture())
  const second = sanitizeContractCapture(capture())
  assert.deepEqual(first, second)
  assert.equal(first.operations[0].request.params.groupId, 'GROUP_001')
  assert.equal(first.operations[0].request.params.userId, 'USER_001')
  assert.equal(first.operations[0].request.params.nimId, 'NIM_001')
  assert.equal(first.operations[0].request.params.msgId, 'MESSAGE_001')
  assert.equal(first.operations[0].request.params.noticeContent, 'TEXT_002')
  assert.equal(first.operations[0].request.params.content.data, 'TEXT_001')
  assert.equal(first.operations[0].request.params.groupRole, 'member')
  assert.equal(first.operations[0].request.params.accountState, 'ACCOUNT_STATE_GOOD')
  assert.equal(first.metadata.mainScriptSha256, 'a'.repeat(64))
})

test('repeated identifiers and embedded JSON use the same stable placeholders', () => {
  const value = capture({
    operations: [
      { request: { params: { groupId: 77, userId: 88 } } },
      { request: { params: { groupId: 77, response: JSON.stringify({ userId: 88, idServer: 'm-1' }) } } },
    ],
  })
  const sanitized = sanitizeContractCapture(value)
  assert.equal(sanitized.operations[0].request.params.groupId, 'GROUP_001')
  assert.equal(sanitized.operations[1].request.params.groupId, 'GROUP_001')
  assert.deepEqual(JSON.parse(sanitized.operations[1].request.params.response), {
    idServer: 'MESSAGE_001',
    userId: 'USER_001',
  })
})

test('sensitive keys and values stop contract generation', () => {
  assert.throws(
    () => sanitizeContractCapture(capture({ authorization: 'redacted' })),
    /敏感字段/,
  )
  assert.throws(
    () => sanitizeContractCapture(capture({ note: 'Bearer abcdefghijklmnopqrstuvwxyz' })),
    /敏感值/,
  )
  assert.throws(
    () => sanitizeContractCapture(capture({ response: JSON.stringify({ accessToken: 'hidden' }) })),
    /敏感字段/,
  )
})

test('version and main script hash are mandatory', () => {
  assert.throws(() => sanitizeContractCapture({ ...capture(), version: 1 }), /Contract v2/)
  assert.throws(
    () => sanitizeContractCapture(capture({ metadata: { appFileVersion: 'test', mainScriptSha256: 'bad' } })),
    /SHA-256/,
  )
})
