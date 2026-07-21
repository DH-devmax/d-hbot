import { mkdir, readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const sensitiveKey = /^(api[-_]?key|authorization|cookie|set-cookie|password|passwd|secret|access[-_]?token|refresh[-_]?token)$/i
const sensitiveValues = [
  /\bsk-[a-z0-9_-]{12,}\b/i,
  /\bbearer\s+[a-z0-9._~+\/-]+=*\b/i,
  /(?:^|[;\s])(sessionid|auth_token|access_token|refresh_token)=[^;\s]+/i,
  /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/i,
]

function kindForKey(key) {
  const normalized = key.replace(/[-_]/g, '').toLowerCase()
  if (normalized.includes('mainscriptsha256') || normalized === 'sha256') return null
  if (normalized.includes('requestid') || normalized === 'traceid') return 'REQUEST'
  if (normalized.includes('listenersession') || normalized === 'sessionid') return 'SESSION'
  if (normalized.includes('messageid') || normalized.includes('msgid') || normalized === 'idserver' || normalized === 'idclient') return 'MESSAGE'
  if (normalized === 'groupmemberids' || normalized === 'memberids') return 'USER'
  if (normalized.includes('nimid') || normalized === 'accid') return 'NIM'
  if (normalized === 'account' || normalized.includes('accountid') || normalized.includes('nimaccount')) return 'ACCOUNT'
  if (normalized.endsWith('userid') || normalized === 'senderid' || normalized === 'from') return 'USER'
  if (normalized === 'groupid' || normalized === 'groupcloudid' || normalized === 'teamid' || normalized === 'to') return 'GROUP'
  if (normalized === 'groupname' || normalized === 'nickname' || normalized === 'cardname' || normalized === 'usernick' || normalized === 'groupmembernick') return 'NAME'
  return null
}

function assertNoSecrets(value, location = '$') {
  if (Array.isArray(value)) {
    value.forEach((item, index) => assertNoSecrets(item, `${location}[${index}]`))
    return
  }
  if (value && typeof value === 'object') {
    for (const [key, item] of Object.entries(value)) {
      if (sensitiveKey.test(key)) throw new Error(`捕获包含敏感字段：${location}.${key}`)
      assertNoSecrets(item, `${location}.${key}`)
    }
    return
  }
  if (typeof value === 'string') {
    const trimmed = value.trim()
    if ((trimmed.startsWith('{') && trimmed.endsWith('}')) || (trimmed.startsWith('[') && trimmed.endsWith(']'))) {
      try {
        assertNoSecrets(JSON.parse(trimmed), `${location}<json>`)
      } catch (error) {
        if (String(error.message).includes('捕获包含敏感')) throw error
      }
    }
    if (sensitiveValues.some(pattern => pattern.test(value))) {
      throw new Error(`捕获包含敏感值：${location}`)
    }
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

  function sanitize(value, key = '') {
    const kind = kindForKey(key)
    if (kind && (typeof value === 'string' || typeof value === 'number')) {
      return placeholder(kind, value)
    }
    if (kind && Array.isArray(value)) {
      return value.map(item =>
        typeof item === 'string' || typeof item === 'number' ? placeholder(kind, item) : sanitize(item, key),
      )
    }
    if (Array.isArray(value)) return value.map(item => sanitize(item))
    if (value && typeof value === 'object') {
      return Object.fromEntries(
        Object.keys(value)
          .sort((left, right) => left.localeCompare(right, 'en'))
          .map(childKey => [childKey, sanitize(value[childKey], childKey)]),
      )
    }
    if (typeof value === 'string') {
      const trimmed = value.trim()
      if ((trimmed.startsWith('{') && trimmed.endsWith('}')) || (trimmed.startsWith('[') && trimmed.endsWith(']'))) {
        try {
          return JSON.stringify(sanitize(JSON.parse(trimmed)))
        } catch {
          // Preserve non-JSON protocol strings.
        }
      }
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
}

export function sanitizeContractCapture(value) {
  assertNoSecrets(value)
  validateContract(value)
  const sanitized = createSanitizer()(value)
  assertNoSecrets(sanitized)
  return sanitized
}

async function selfTest() {
  const raw = {
    version: 2,
    metadata: { appFileVersion: 'fixture', mainScriptSha256: 'a'.repeat(64) },
    operations: [{ request: { params: { groupId: 77, userId: 88, msgId: 'm-1' } } }],
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
  const [, , input, output] = process.argv
  if (input === '--self-test') return selfTest()
  if (!input || !output) {
    throw new Error('用法：node scripts/sanitize-contract-capture.mjs INPUT.json OUTPUT.json')
  }
  const raw = JSON.parse(await readFile(input, 'utf8'))
  const sanitized = sanitizeContractCapture(raw)
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
