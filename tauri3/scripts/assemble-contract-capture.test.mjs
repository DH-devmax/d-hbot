import assert from 'node:assert/strict'
import test from 'node:test'

import { assembleContractCapture, selectDevToolsPage } from './assemble-contract-capture.mjs'

function operation() {
  return {
    name: 'mute',
    route: '/v1/group/set-member-mute',
    request: { params: { groupId: 1, userId: 2, min: 10 } },
    transport: { transportCode: 200, errno: 0, requestId: 'request-1' },
    business: { code: 0, errno: 0, msg: 'OK', data: {} },
    normalizedReceipt: { route: '/v1/group/set-member-mute', status: 'succeeded' },
    expectedNormalizedState: { action: 'mute' },
  }
}

test('assembler freezes DevTools metadata and actual protocol layers', () => {
  const capture = assembleContractCapture({
    page: { title: '旺商聊', url: 'http://127.0.0.1:9222/' },
    mainScriptSha256: 'a'.repeat(64),
    operations: [operation()],
    callbacks: [{ sequence: 1, kind: 'message' }],
    expectedNormalizedState: { groups: [] },
    capturedAt: '2026-07-22T00:00:00Z',
  })
  assert.equal(capture.version, 2)
  assert.equal(capture.metadata.mainScriptSha256, 'a'.repeat(64))
  assert.deepEqual(capture.operations[0], operation())
})

test('assembler rejects incomplete envelope traces', () => {
  const incomplete = operation()
  delete incomplete.business.data
  assert.throws(
    () => assembleContractCapture({
      page: { title: '旺商聊', url: 'http://127.0.0.1:9222/' },
      mainScriptSha256: 'a'.repeat(64),
      operations: [incomplete],
      callbacks: [],
      expectedNormalizedState: {},
      capturedAt: '2026-07-22T00:00:00Z',
    }),
    /business 缺少 data/,
  )
})

test('assembler requires a captured callback rather than an empty placeholder', () => {
  assert.throws(
    () => assembleContractCapture({
      page: { title: '旺商聊', url: 'http://127.0.0.1:9222/' },
      mainScriptSha256: 'a'.repeat(64),
      operations: [operation()],
      callbacks: [],
      expectedNormalizedState: {},
      capturedAt: '2026-07-22T00:00:00Z',
    }),
    /至少包含一个 callback/,
  )
})

test('collector selects the marked wangshangliao page from multiple DevTools pages', () => {
  const selected = selectDevToolsPage([
    { type: 'page', title: 'DevTools', url: 'http://127.0.0.1:9222/devtools' },
    { type: 'page', title: '旺商聊', url: 'file:///C:/wangshangliao/index.html' },
    { type: 'service_worker', title: '', url: 'file:///worker.js' },
  ])
  assert.equal(selected.title, '旺商聊')
})

test('collector rejects ambiguous or unrelated DevTools pages', () => {
  assert.throws(() => selectDevToolsPage([
    { type: 'page', title: '旺商聊', url: 'file:///one' },
    { type: 'page', title: 'WangShangLiao', url: 'file:///two' },
  ]), /关闭重复窗口/)
  assert.throws(() => selectDevToolsPage([
    { type: 'page', title: 'Chrome', url: 'https://example.test' },
  ]), /没有找到/)
})
