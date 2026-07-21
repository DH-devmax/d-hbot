import { copyFile, mkdir, readdir, rm } from 'node:fs/promises'
import path from 'node:path'

const root = path.resolve(import.meta.dirname, '..')
const target = path.join(root, 'src-tauri', 'target', 'release')
const resources = path.join(root, 'src-tauri', 'resources', 'tools')
const windows = process.platform === 'win32'
const source = path.join(target, windows ? 'dh-fixture.exe' : 'dh-fixture')
const destination = path.join(resources, windows ? 'DH-Fixture.exe' : 'DH-Fixture')

await rm(resources, { recursive: true, force: true })
await mkdir(resources, { recursive: true })
await copyFile(source, destination)

const staged = await readdir(resources)
if (staged.length !== 1) throw new Error(`Fixture 资源 staging 异常：${staged.join(', ')}`)
console.log(`Fixture 开发资源已准备：${path.relative(root, destination)}`)
