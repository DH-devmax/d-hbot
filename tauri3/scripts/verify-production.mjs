import { readFile, readdir, stat } from 'node:fs/promises'
import path from 'node:path'

const root = path.resolve(process.argv[2] || 'dist/production')
const requireExecutable = !process.argv.includes('--allow-no-executable')
const forbiddenNames = [
  /dh-fixture/i,
  /fixture-gateway/i,
  /tauri\.fixture\.conf/i,
  /readme-developer/i,
]
const forbiddenText = [
  'DH-Fixture',
  'DH BOT Dev',
  'dh-fixture',
  'start_fixture_host',
  'set_runtime_mode',
  'DH_RUNTIME_MODE',
  'http://127.0.0.1:9233',
  '127.0.0.1:9233',
  'http://127.0.0.1:51300',
  '127.0.0.1:51300',
  '%APPDATA%\\DH\\fixture',
  '\\DH\\fixture',
  '/DH/fixture',
  'runtimeMode":"fixture',
  'runtime_mode":"fixture',
  '/fixture/events/',
  '/fixture/reset',
  '/fixture/state',
  '/fixture/actions',
  '/fixture/faults',
  '开发测试环境',
]

async function files(directory) {
  const result = []
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const current = path.join(directory, entry.name)
    if (entry.isDirectory()) result.push(...await files(current))
    else if (entry.isFile()) result.push(current)
  }
  return result
}

function peSubsystem(buffer) {
  if (buffer.length < 0x40 || buffer.toString('ascii', 0, 2) !== 'MZ') return null
  const peOffset = buffer.readUInt32LE(0x3c)
  if (peOffset + 24 > buffer.length || buffer.toString('ascii', peOffset, peOffset + 4) !== 'PE\0\0') return null
  const optionalOffset = peOffset + 24
  const magic = buffer.readUInt16LE(optionalOffset)
  if (magic !== 0x10b && magic !== 0x20b) return null
  const subsystemOffset = optionalOffset + 68
  return subsystemOffset + 2 <= buffer.length ? buffer.readUInt16LE(subsystemOffset) : null
}

const rootStat = await stat(root).catch(() => null)
if (!rootStat?.isDirectory()) throw new Error(`生产产物目录不存在：${root}`)

const productionFiles = await files(root)
if (!productionFiles.length) throw new Error('生产产物目录为空')

const mainExecutable = productionFiles.find(file => /^DH-BOT\.exe$/i.test(path.basename(file)))
if (mainExecutable) {
  const subsystem = peSubsystem(await readFile(mainExecutable))
  if (subsystem === null) {
    throw new Error('DH-BOT.exe 不是有效的 Windows PE 可执行文件')
  }
  if (subsystem !== 2) {
    throw new Error(`DH-BOT.exe 不是 Windows GUI 子系统（Subsystem=${subsystem}），将显示 CMD 窗口`)
  }
}

for (const file of productionFiles) {
  const relative = path.relative(root, file)
  if (forbiddenNames.some(pattern => pattern.test(relative))) {
    throw new Error(`生产产物包含开发文件：${relative}`)
  }
  const content = await readFile(file)
  const ascii = content.toString('latin1')
  const utf8 = content.toString('utf8')
  const utf16 = content.toString('utf16le')
  for (const token of forbiddenText) {
    if (ascii.includes(token) || utf8.includes(token) || utf16.includes(token)) {
      throw new Error(`生产产物 ${relative} 包含开发标记：${token}`)
    }
  }
}

const executable = productionFiles.some(file => /DH-BOT(?:\.exe)?$/i.test(path.basename(file)))
if (requireExecutable && !executable) throw new Error('生产目录中没有 DH-BOT 主程序')
console.log(`生产产物隔离检查通过：${productionFiles.length} 个文件`)
