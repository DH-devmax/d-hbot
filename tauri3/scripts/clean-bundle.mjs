import { rm } from 'node:fs/promises'
import path from 'node:path'

const root = path.resolve(import.meta.dirname, '..')
const bundle = path.join(root, 'src-tauri', 'target', 'release', 'bundle')
await rm(bundle, { recursive: true, force: true })
console.log(`已清理旧 Tauri bundle：${path.relative(root, bundle)}`)
