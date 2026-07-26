import { mkdir, readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const capabilityNames = ['announcement', 'sendText', 'mute', 'recall', 'rename', 'removeMember', 'groupMute', 'memberEvents']
const capabilityStatuses = new Set(['supported', 'unverified', 'unsupported'])

function requireObject(value, label) {
  if (!value || Array.isArray(value) || typeof value !== 'object') throw new Error(`${label} 必须是对象`)
}

function exactKey(appFileVersion, mainScriptSha256) {
  return `${String(appFileVersion || '').trim()}\u0000${String(mainScriptSha256 || '').trim().toLowerCase()}`
}

function validateExactIdentity(value, label) {
  if (typeof value.appFileVersion !== 'string' || !value.appFileVersion.trim()) {
    throw new Error(`${label}.appFileVersion 必须是非空字符串`)
  }
  if (!/^[a-f0-9]{64}$/i.test(value.mainScriptSha256 || '')) {
    throw new Error(`${label}.mainScriptSha256 必须是 64 位 SHA-256`)
  }
}

function validateCapabilities(capabilities, label) {
  requireObject(capabilities, label)
  for (const [name, status] of Object.entries(capabilities)) {
    if (!capabilityNames.includes(name)) throw new Error(`${label} 包含未知能力：${name}`)
    if (!capabilityStatuses.has(status)) throw new Error(`${label}.${name} 状态无效：${status}`)
  }
}

export function mergeCalibrationCapabilities(registry, verificationResults) {
  requireObject(registry, '能力表')
  if (registry.version !== 1) throw new Error('只支持 version=1 的能力表')
  if (!Array.isArray(registry.calibrations)) throw new Error('能力表 calibrations 必须是数组')
  if (!Array.isArray(verificationResults) || verificationResults.length === 0) {
    throw new Error('至少需要一个校准验证结果')
  }

  const registryByKey = new Map()
  for (const [index, calibration] of registry.calibrations.entries()) {
    requireObject(calibration, `calibrations[${index}]`)
    validateExactIdentity(calibration, `calibrations[${index}]`)
    validateCapabilities(calibration.capabilities, `calibrations[${index}].capabilities`)
    const key = exactKey(calibration.appFileVersion, calibration.mainScriptSha256)
    if (registryByKey.has(key)) throw new Error(`能力表包含重复的精确版本和哈希：${calibration.appFileVersion}`)
    registryByKey.set(key, index)
  }

  const resultsByKey = new Map()
  for (const [index, result] of verificationResults.entries()) {
    requireObject(result, `验证结果[${index}]`)
    validateExactIdentity(result, `验证结果[${index}]`)
    if (result.verified !== true) throw new Error(`验证结果[${index}] 未通过严格验证`)
    validateCapabilities(result.capabilities, `验证结果[${index}].capabilities`)
    const key = exactKey(result.appFileVersion, result.mainScriptSha256)
    if (!registryByKey.has(key)) {
      throw new Error(`未知版本或主脚本哈希，拒绝自动写入：${result.appFileVersion} ${result.mainScriptSha256}`)
    }
    if (!resultsByKey.has(key)) resultsByKey.set(key, [])
    resultsByKey.get(key).push(result)
  }

  const merged = structuredClone(registry)
  for (const [index, calibration] of merged.calibrations.entries()) {
    const key = exactKey(calibration.appFileVersion, calibration.mainScriptSha256)
    const results = resultsByKey.get(key) || []
    const current = calibration.capabilities
    calibration.mainScriptSha256 = calibration.mainScriptSha256.toLowerCase()
    calibration.capabilities = Object.fromEntries(capabilityNames.map(capability => {
      if (capability === 'removeMember') return [capability, 'unsupported']
      if (current[capability] === 'unsupported') return [capability, 'unsupported']
      const supported = results.some(result => result.capabilities[capability] === 'supported')
      return [capability, supported ? 'supported' : current[capability] || 'unverified']
    }))
    if (results.length > 0) calibration.verified = true
    if (!registryByKey.has(key)) throw new Error(`内部错误：无法定位 calibrations[${index}]`)
  }
  return merged
}

async function main() {
  const args = process.argv.slice(2)
  const outputIndex = args.indexOf('--output')
  const output = outputIndex >= 0 ? args[outputIndex + 1] : ''
  if (outputIndex >= 0) args.splice(outputIndex, 2)
  if (args.length < 2 || (outputIndex >= 0 && !output)) {
    throw new Error('用法：node scripts/merge-calibration-capabilities.mjs REGISTRY.json RESULT.json [RESULT.json ...] [--output OUTPUT.json]')
  }
  const [registryPath, ...resultPaths] = args
  const [registry, ...results] = await Promise.all([
    readFile(registryPath, 'utf8').then(JSON.parse),
    ...resultPaths.map(resultPath => readFile(resultPath, 'utf8').then(JSON.parse)),
  ])
  const merged = mergeCalibrationCapabilities(registry, results)
  const serialized = `${JSON.stringify(merged, null, 2)}\n`
  if (!output) {
    process.stdout.write(serialized)
    return
  }
  await mkdir(path.dirname(output), { recursive: true })
  await writeFile(output, serialized, { flag: 'wx' })
  process.stdout.write(`已生成合并后的能力表：${output}\n`)
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main().catch(error => {
    process.stderr.write(`${error.message}\n`)
    process.exitCode = 1
  })
}
