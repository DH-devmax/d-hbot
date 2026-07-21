import { rm } from 'node:fs/promises'
import path from 'node:path'

const root = path.resolve(import.meta.dirname, '..')
const release = path.join(root, 'src-tauri', 'target', 'release')
const generated = [
  path.join(release, 'bundle'),
  path.join(release, process.platform === 'win32' ? 'dh-fixture.exe' : 'dh-fixture'),
  path.join(root, 'src-tauri', 'resources', 'tools'),
  path.join(root, 'dist', 'production'),
]

for (const target of generated) {
  await rm(target, { recursive: true, force: true })
  console.log(`生产构建清理：${path.relative(root, target)}`)
}
