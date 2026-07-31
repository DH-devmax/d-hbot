import { readdir, rm } from 'node:fs/promises'
import path from 'node:path'

const root = path.resolve(import.meta.dirname, '..')
const release = path.join(root, 'src-tauri', 'target', 'release')
const generated = [
  path.join(release, 'bundle'),
  path.join(release, process.platform === 'win32' ? 'dh-fixture.exe' : 'dh-fixture'),
  path.join(release, 'resources', 'tools'),
  path.join(release, 'target'),
  path.join(root, 'src-tauri', 'resources', 'tools'),
  path.join(root, 'dist', 'production'),
]

for (const target of generated) {
  await rm(target, { recursive: true, force: true })
  console.log(`生产构建清理：${path.relative(root, target)}`)
}

for (const directory of [release, path.join(release, 'deps')]) {
  const entries = await readdir(directory, { withFileTypes: true }).catch(() => [])
  for (const entry of entries) {
    if (!/dh[-_]fixture/i.test(entry.name)) continue
    const target = path.join(directory, entry.name)
    await rm(target, { recursive: true, force: true })
    console.log(`生产构建清理：${path.relative(root, target)}`)
  }
}

const fingerprints = path.join(release, '.fingerprint')
for (const entry of await readdir(fingerprints, { withFileTypes: true }).catch(() => [])) {
  if (!entry.isDirectory()) continue
  const directory = path.join(fingerprints, entry.name)
  const files = await readdir(directory).catch(() => [])
  if (!files.some(file => /dh[-_]fixture/i.test(file))) continue
  await rm(directory, { recursive: true, force: true })
  console.log(`生产构建清理：${path.relative(root, directory)}`)
}
