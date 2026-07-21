import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { spawnSync } from 'node:child_process'

const root = path.resolve(import.meta.dirname, '..')
const verifier = path.join(root, 'scripts', 'verify-production.mjs')
const temporary = await mkdtemp(path.join(os.tmpdir(), 'dh-production-scan-'))

function verify(directory) {
  return spawnSync(process.execPath, [verifier, directory], { encoding: 'utf8' })
}

function fakePe(subsystem) {
  const buffer = Buffer.alloc(0x200)
  buffer.write('MZ', 0, 'ascii')
  buffer.writeUInt32LE(0x80, 0x3c)
  buffer.write('PE\0\0', 0x80, 'ascii')
  buffer.writeUInt16LE(0x20b, 0x98)
  buffer.writeUInt16LE(subsystem, 0x98 + 68)
  return buffer
}

try {
  const clean = path.join(temporary, 'clean')
  await mkdir(clean)
  await writeFile(path.join(clean, 'DH-BOT.exe'), fakePe(2))
  const cleanResult = verify(clean)
  if (cleanResult.status !== 0) throw new Error(cleanResult.stderr || cleanResult.stdout)

  const cases = [
    ['fixture binary', 'DH-Fixture.exe', 'placeholder'],
    ['fixture command', 'DH-BOT.exe', 'start_fixture_host'],
    ['fixture devtools', 'DH-BOT.exe', '127.0.0.1:9233'],
    ['fixture server', 'DH-BOT.exe', '127.0.0.1:51300'],
    ['fixture data path', 'DH-BOT.exe', '%APPDATA%\\DH\\fixture'],
    ['fixture environment', 'DH-BOT.exe', 'DH_RUNTIME_MODE'],
  ]

  for (const [name, filename, content] of cases) {
    const directory = path.join(temporary, name.replaceAll(' ', '-'))
    await mkdir(directory)
    await writeFile(path.join(directory, filename), content)
    const result = verify(directory)
    if (result.status === 0) throw new Error(`扫描器未拦截：${name}`)
  }

  const consoleBuild = path.join(temporary, 'console-build')
  await mkdir(consoleBuild)
  await writeFile(path.join(consoleBuild, 'DH-BOT.exe'), fakePe(3))
  if (verify(consoleBuild).status === 0) throw new Error('扫描器未拦截 CUI 子系统')

  const invalidBuild = path.join(temporary, 'invalid-build')
  await mkdir(invalidBuild)
  await writeFile(path.join(invalidBuild, 'DH-BOT.exe'), 'not a PE executable')
  if (verify(invalidBuild).status === 0) throw new Error('扫描器未拦截无效 PE')

  console.log(`生产隔离扫描自测通过：${cases.length + 3} 个场景`)
} finally {
  await rm(temporary, { recursive: true, force: true })
}
