import assert from 'node:assert/strict'
import test from 'node:test'

import { mergeCalibrationCapabilities } from './merge-calibration-capabilities.mjs'

const hashA = 'a'.repeat(64)
const hashB = 'b'.repeat(64)
const hashC = 'c'.repeat(64)

function capabilities(overrides = {}) {
  return {
    announcement: 'unverified',
    sendText: 'unverified',
    mute: 'unverified',
    recall: 'unverified',
    rename: 'unverified',
    removeMember: 'unsupported',
    groupMute: 'unverified',
    memberEvents: 'unverified',
    ...overrides,
  }
}

function registry() {
  return {
    version: 1,
    calibrations: [
      { appFileVersion: '2.7.8', mainScriptSha256: hashA, verified: true, capabilities: capabilities() },
      { appFileVersion: '2.7.8', mainScriptSha256: hashB, verified: true, capabilities: capabilities({ announcement: 'supported' }) },
    ],
  }
}

function result(hash, overrides = {}) {
  return {
    appFileVersion: '2.7.8',
    mainScriptSha256: hash,
    verified: true,
    capabilities: capabilities(overrides),
  }
}

test('merges multiple verified results only into the exact version and hash', () => {
  const value = registry()
  const merged = mergeCalibrationCapabilities(value, [
    result(hashA, { sendText: 'supported', recall: 'supported' }),
    result(hashA, { mute: 'supported', rename: 'supported', removeMember: 'supported' }),
  ])
  assert.equal(merged.calibrations[0].capabilities.sendText, 'supported')
  assert.equal(merged.calibrations[0].capabilities.recall, 'supported')
  assert.equal(merged.calibrations[0].capabilities.mute, 'supported')
  assert.equal(merged.calibrations[0].capabilities.rename, 'supported')
  assert.equal(merged.calibrations[0].capabilities.removeMember, 'unsupported')
  assert.equal(merged.calibrations[1].capabilities.sendText, 'unverified')
  assert.equal(merged.calibrations[1].capabilities.announcement, 'supported')
  assert.deepEqual(value, registry())
})

test('preserves supported evidence when a later result is unverified', () => {
  const merged = mergeCalibrationCapabilities(registry(), [
    result(hashA, { groupMute: 'supported' }),
    result(hashA, { groupMute: 'unverified', memberEvents: 'supported' }),
  ])
  assert.equal(merged.calibrations[0].capabilities.groupMute, 'supported')
  assert.equal(merged.calibrations[0].capabilities.memberEvents, 'supported')
})

test('rejects an unknown hash instead of creating a calibration entry', () => {
  const value = registry()
  assert.throws(
    () => mergeCalibrationCapabilities(value, [result(hashC, { sendText: 'supported' })]),
    /未知版本或主脚本哈希/,
  )
  assert.deepEqual(value, registry())
})

test('rejects unverified result documents and duplicate registry identities', () => {
  const unverified = result(hashA)
  unverified.verified = false
  assert.throws(() => mergeCalibrationCapabilities(registry(), [unverified]), /未通过严格验证/)

  const duplicate = registry()
  duplicate.calibrations.push(structuredClone(duplicate.calibrations[0]))
  assert.throws(() => mergeCalibrationCapabilities(duplicate, [result(hashA)]), /重复的精确版本和哈希/)
})
