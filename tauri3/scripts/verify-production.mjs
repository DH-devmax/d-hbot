import { readFile, readdir, stat } from 'node:fs/promises'
import path from 'node:path'

const root = path.resolve(process.argv[2] || 'dist/production')
const forbiddenNames = [/dh-fixture/i, /fixture-gateway/i]
const forbiddenText = [
  'DH-Fixture',
  'start_fixture_host',
  'set_runtime_mode',
  'http://127.0.0.1:9233',
  '/fixture/events/',
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

const rootStat = await stat(root).catch(() => null)
if (!rootStat?.isDirectory()) throw new Error(`生产产物目录不存在：${root}`)

const productionFiles = await files(root)
if (!productionFiles.length) throw new Error('生产产物目录为空')

for (const file of productionFiles) {
  const relative = path.relative(root, file)
  if (forbiddenNames.some(pattern => pattern.test(relative))) {
    throw new Error(`生产产物包含开发文件：${relative}`)
  }
  const content = await readFile(file)
  const ascii = content.toString('latin1')
  const utf16 = content.toString('utf16le')
  for (const token of forbiddenText) {
    if (ascii.includes(token) || utf16.includes(token)) {
      throw new Error(`生产产物 ${relative} 包含开发标记：${token}`)
    }
  }
}

const executable = productionFiles.some(file => /DH-BOT(?:\.exe)?$/i.test(path.basename(file)))
if (!executable) throw new Error('生产目录中没有 DH-BOT 主程序')
console.log(`生产产物隔离检查通过：${productionFiles.length} 个文件`)
