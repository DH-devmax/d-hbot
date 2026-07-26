import assert from 'node:assert/strict'
import test from 'node:test'

import { assertSanitizedContract, compactContractCapture, sanitizeContractCapture } from './sanitize-contract-capture.mjs'

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
        route: '/v1/group/get-group-members',
        request: {
          kind: 'request',
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
  assert.match(first.operations[0].request.params.noticeContent, /^TEXT_\d{3}$/)
  assert.match(first.operations[0].request.params.content.data, /^TEXT_\d{3}$/)
  assert.equal(first.operations[0].request.params.groupRole, 'member')
  assert.equal(first.operations[0].request.params.accountState, 'ACCOUNT_STATE_GOOD')
  assert.equal(first.metadata.mainScriptSha256, 'a'.repeat(64))
})

test('repeated identifiers and embedded JSON use the same stable placeholders', () => {
  const value = capture({
    operations: [
      { route: '/v1/group/get-group-members', request: { kind: 'request', params: { groupId: 77, userId: 88 } } },
      { route: '/v1/group/get-group-members', request: { kind: 'request', params: { groupId: 77, response: JSON.stringify({ userId: 88, idServer: 'm-1' }) } } },
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

test('sanitizes NIM targets and identifiers inside nested protocol JSON', () => {
  const protocol = JSON.stringify({
    id: 'message-real-2',
    from: { id: 7001, name: 'sender' },
    to: { id: 8001, groupCloudId: 'cloud-group-1', groupName: 'group' },
    groupNotice: { id: 'notice-real-1', noticeId: 'notice-real-1', content: 'announcement' },
    idServer: 'message-real-1',
  })
  const sanitized = sanitizeContractCapture(capture({
    operations: [{
      route: 'nim.sendCustomMsg',
      request: { kind: 'nim', params: { target: 'cloud-group-1', content: protocol } },
    }],
  }))
  const nested = JSON.parse(sanitized.operations[0].request.params.content)
  assert.equal(sanitized.operations[0].request.params.target, nested.to.groupCloudId)
  assert.match(sanitized.operations[0].request.params.target, /^GROUP_\d{3}$/)
  assert.match(nested.from.id, /^USER_\d{3}$/)
  assert.match(nested.to.id, /^GROUP_\d{3}$/)
  assert.equal(nested.groupNotice.id, nested.groupNotice.noticeId)
  assert.match(nested.groupNotice.noticeId, /^NOTICE_\d{3}$/)
  assert.match(nested.id, /^MESSAGE_\d{3}$/)
  assert.match(nested.idServer, /^MESSAGE_\d{3}$/)
  assert.equal(JSON.stringify(sanitized).includes('cloud-group-1'), false)
  assert.equal(JSON.stringify(sanitized).includes('notice-real-1'), false)
  assert.equal(JSON.stringify(sanitized).includes('message-real-2'), false)
})

test('sanitizes generic announcement ids without changing ordinary business ids', () => {
  const sanitized = sanitizeContractCapture(capture({
    operations: [
      {
        route: '/v1/group/notice-opt',
        request: { kind: 'request', params: { groupId: 1, noticeId: 'notice-7' } },
        business: { code: 0, data: { id: 'notice-7', noticeContent: 'test' } },
      },
      {
        route: '/v1/business/query',
        request: { kind: 'request', params: { id: 42 } },
        business: { code: 0, data: { id: 'ordinary-id', label: 'keep' } },
      },
    ],
  }))
  assert.equal(sanitized.operations[0].request.params.noticeId, 'NOTICE_001')
  assert.equal(sanitized.operations[0].business.data.id, 'NOTICE_001')
  assert.equal(sanitized.operations[1].request.params.id, 42)
  assert.equal(sanitized.operations[1].business.data.id, 'ordinary-id')
})

test('sanitizes identity fields, sessions, URLs and opaque Base64 payloads', () => {
  const opaque = Buffer.from(Uint8Array.from([8, 1, 18, 20, ...Buffer.from('team-27928953961:123456789012345678')])).toString('base64')
  const sanitized = sanitizeContractCapture(capture({
    metadata: {
      appFileVersion: '2.7.8',
      mainScriptSha256: 'a'.repeat(64),
      pageTitle: '真实群名称',
      pageUrl: 'file:///D:/app/index.html#/messages?sessionId=team-27928953961',
    },
    operations: [{
      name: '真实成员改名',
      route: '/v1/group/set-member-nickname',
      request: {
        kind: 'request',
        params: {
          groupAccount: '27928953961',
          userAvatar: '1234567890123456789',
          avatar: '9876543210987654321',
          name: '真实姓名',
          nick: '真实昵称',
          fromNick: '发送者昵称',
          uid: 'member-uid-real',
          tag: 'member-tag-real',
          session: '52fdfc07-2182-454f-963f-5f0f9a621d72',
          b: opaque,
        },
      },
    }],
  }))
  const serialized = JSON.stringify(sanitized)
  for (const leaked of [
    '27928953961',
    '1234567890123456789',
    '9876543210987654321',
    '真实姓名',
    '真实昵称',
    '发送者昵称',
    'member-uid-real',
    'member-tag-real',
    '52fdfc07-2182-454f-963f-5f0f9a621d72',
    'team-27928953961',
    opaque,
  ]) assert.equal(serialized.includes(leaked), false, `仍包含 ${leaked}`)
  assert.match(sanitized.metadata.pageUrl, /^URL_\d{3}$/)
  assert.match(sanitized.operations[0].request.params.groupAccount, /^GROUP_\d{3}$/)
  assert.match(sanitized.operations[0].request.params.userAvatar, /^AVATAR_\d{3}$/)
  assert.match(sanitized.operations[0].request.params.session, /^SESSION_\d{3}$/)
  assert.match(sanitized.operations[0].request.params.b, /^OPAQUE_\d{3}$/)
  assert.doesNotThrow(() => assertSanitizedContract(sanitized))
})

test('sanitizes group lists, 1000-member rosters and NIM callback ext fields', () => {
  const roster = Array.from({ length: 1000 }, (_, index) => ({
    userId: `member-sentinel-${index}`,
    userAvatar: `900000000000000${String(index).padStart(3, '0')}`,
    name: `member-name-sentinel-${index}`,
    nick: `member-nick-sentinel-${index}`,
  }))
  const callbackWire = Buffer.from(Uint8Array.from([8, 2, 18, 18, ...Buffer.from('wire-group-sentinel')])).toString('base64')
  const sanitized = sanitizeContractCapture(capture({
    operations: [
      {
        route: '/v1/group/get-group-list',
        business: {
          data: {
            member: [{ groupAccount: 'group-account-sentinel', groupName: 'group-name-sentinel', avatar: 'group-avatar-sentinel' }],
            owner: [],
          },
        },
      },
      {
        route: '/v1/group/get-group-members',
        business: { data: { groupAccount: 'group-account-sentinel', groupMemberInfo: roster } },
      },
    ],
    callbacks: [{
      kind: 'teamMemberUpdated',
      payload: {
        teamId: 'group-account-sentinel',
        listenerSession: '5e55aa48-17f7-4ce1-bb9f-18e56676985d',
        ext: JSON.stringify({
          fromNick: 'callback-name-sentinel',
          tag: 'callback-tag-sentinel',
          uid: 'callback-uid-sentinel',
        }),
        wire: callbackWire,
      },
    }],
  }))
  const serialized = JSON.stringify(sanitized)
  for (const sentinel of [
    'group-account-sentinel',
    'group-name-sentinel',
    'group-avatar-sentinel',
    'member-sentinel-',
    'member-name-sentinel-',
    'member-nick-sentinel-',
    'callback-name-sentinel',
    'callback-tag-sentinel',
    'callback-uid-sentinel',
    '5e55aa48-17f7-4ce1-bb9f-18e56676985d',
    callbackWire,
  ]) assert.equal(serialized.includes(sentinel), false, `仍包含 ${sentinel}`)
  const members = sanitized.operations[1].business.data.groupMemberInfo
  assert.equal(members.length, 1000)
  assert.equal(members.every(member => /^USER_\d{3,}$/.test(member.userId)), true)
  assert.equal(members.every(member => /^AVATAR_\d{3,}$/.test(member.userAvatar)), true)
  assert.equal(members.every(member => /^NAME_\d{3,}$/.test(member.name) && /^NAME_\d{3,}$/.test(member.nick)), true)
  const ext = JSON.parse(sanitized.callbacks[0].payload.ext)
  assert.match(ext.fromNick, /^NAME_\d{3,}$/)
  assert.match(ext.tag, /^TAG_\d{3,}$/)
  assert.match(ext.uid, /^USER_\d{3,}$/)
  assert.match(sanitized.callbacks[0].payload.wire, /^OPAQUE_\d{3,}$/)
})

test('strict sanitized-contract audit rejects recognizable raw identifiers', () => {
  assert.throws(() => assertSanitizedContract(capture()), /未脱敏/)
  assert.throws(() => assertSanitizedContract({ payload: { b: Buffer.from('raw protocol body with team-27928953961').toString('base64') } }), /未脱敏/)
})

test('compact capture keeps two-group preflight, writes and their readbacks', () => {
  const makeOperation = (route, action, groupId, userId = 'u1') => ({
    route,
    request: { params: { groupId, userId } },
    business: { data: route === '/v1/group/get-group-members' ? { groupMemberInfo: [{ userId }, { userId: 'other' }] } : {} },
    expectedNormalizedState: { action, groupId, userId },
  })
  const value = capture({
    operations: [
      {
        ...makeOperation('/v1/group/get-group-list', 'list_groups'),
        business: { data: { member: [{ groupId: 'g1' }, { groupId: 'unrelated' }], owner: [{ groupId: 'g2' }] } },
      },
      makeOperation('/v1/group/get-group-list', 'list_groups'),
      makeOperation('/v1/group/get-group-members', 'list_members', 'g1'),
      makeOperation('/v1/group/get-group-members', 'list_members', 'g1'),
      makeOperation('/v1/group/get-group-members', 'list_members', 'g2', 'u2'),
      makeOperation('/v1/group/set-member-nickname', 'rename', 'g1'),
      makeOperation('/v1/group/get-group-members', 'list_members', 'g1'),
      makeOperation('/v1/group/set-member-nickname', 'rename', 'g2', 'u2'),
      makeOperation('/v1/group/get-group-members', 'list_members', 'g2', 'u2'),
      makeOperation('nim.getTeamMembers', 'list_members', 'g1'),
    ],
    callbacks: [{ kind: 'message', payload: {} }, { kind: 'message', payload: {} }],
    expectedNormalizedState: { capabilities: ['rename'] },
  })
  const compact = compactContractCapture(value)
  assert.deepEqual(compact.operations.map(operation => operation.route), [
    '/v1/group/get-group-list',
    '/v1/group/get-group-members',
    '/v1/group/get-group-members',
    '/v1/group/set-member-nickname',
    '/v1/group/get-group-members',
    '/v1/group/set-member-nickname',
    '/v1/group/get-group-members',
  ])
  assert.equal(compact.callbacks.length, 1)
  assert.deepEqual(compact.operations[0].business.data, { member: [{ groupId: 'g1' }], owner: [{ groupId: 'g2' }] })
  assert.equal(compact.operations.at(-1).business.data.groupMemberInfo.length, 1)
})

test('compact capture supports passive member-events-only evidence', () => {
  const groupList = {
    route: '/v1/group/get-group-list',
    business: { data: { member: [{ groupId: 'g1' }], owner: [] } },
  }
  const memberList = {
    route: '/v1/group/get-group-members',
    request: { params: { groupId: 'g1' } },
    expectedNormalizedState: { groupId: 'g1' },
  }
  const callbacks = ['teamMemberJoined', 'teamMemberUpdated', 'teamMemberLeft']
    .map(kind => ({ kind, payload: { teamId: 'g1' } }))
  const compact = compactContractCapture(capture({
    operations: [groupList, memberList],
    callbacks,
    expectedNormalizedState: { capabilities: ['memberEvents'] },
  }))
  assert.equal(compact.operations.length, 2)
  assert.equal(compact.callbacks.length, 3)
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
  for (const key of ['token', 'nimToken', 'jwtToken', 'groupToken', 'sigToken']) {
    assert.throws(
      () => sanitizeContractCapture(capture({
        operations: [{ route: '/v1/login', business: { data: { [key]: 'login-credential-sentinel' } } }],
      })),
      new RegExp(`敏感字段.*${key}`, 'i'),
    )
  }
  for (const key of ['X-Token', 'X-jwt', 'X-Group-Token']) {
    assert.throws(
      () => sanitizeContractCapture(capture({
        operations: [{ route: '/v1/group/get-group-list', request: { headers: { [key]: 'header-credential-sentinel' } } }],
      })),
      new RegExp(`敏感字段.*${key}`, 'i'),
    )
  }
  assert.throws(
    () => sanitizeContractCapture(capture({ note: 'eyJhbGciOiJIUzI1NiJ9.cGF5bG9hZC1zZW50aW5lbA.c2lnbmF0dXJlLXNlbnRpbmVs' })),
    /敏感值/,
  )
  assert.throws(
    () => assertSanitizedContract({ transport: { headers: { 'X-Group-Token': 'credential' } } }),
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
