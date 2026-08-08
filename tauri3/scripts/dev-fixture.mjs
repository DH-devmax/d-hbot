import { spawn, spawnSync } from 'node:child_process'
import path from 'node:path'

const windows = process.platform === 'win32'
const cargo = windows ? 'cargo.exe' : 'cargo'
const pnpm = windows ? 'pnpm.cmd' : 'pnpm'
const environment = { ...process.env, DH_RUNTIME_MODE: 'fixture' }

function spawnPnpm(args) {
  // Node cannot spawn a .cmd shim reliably on every Windows host. When this
  // script is invoked through pnpm, npm_execpath points at the real JS entry;
  // run that entry with the current Node process instead.
  if (windows && process.env.npm_execpath) {
    return spawn(process.execPath, [process.env.npm_execpath, ...args], {
      stdio: 'inherit',
      env: environment,
    })
  }
  return spawn(pnpm, args, { stdio: 'inherit', env: environment, shell: windows })
}

// tauri.fixture.conf.json 声明了 bundle.resources: resources/tools/*，Tauri 的构建脚本在
// dev 下同样会解析这个 glob。该目录是 gitignore 的生成物，全新检出时为空，glob 匹配不到文件
// 会让 dev 直接构建失败。所以先同步产出并 stage 一份 debug 版 DH-Fixture，再启动 dev。
const build = spawnSync(cargo, [
  'build',
  '--manifest-path', 'src-tauri/Cargo.toml',
  '--features', 'fixture',
  '--bin', 'dh-fixture',
], { stdio: 'inherit', env: environment })
if (build.status !== 0) process.exit(build.status ?? 1)

const stage = spawnSync(process.execPath, [
  path.join(import.meta.dirname, 'stage-fixture.mjs'),
  'debug',
], { stdio: 'inherit', env: environment })
if (stage.status !== 0) process.exit(stage.status ?? 1)

const fixture = spawn(cargo, [
  'run',
  '--manifest-path', 'src-tauri/Cargo.toml',
  '--features', 'fixture',
  '--bin', 'dh-fixture',
], { stdio: 'inherit', env: environment })

const app = spawnPnpm([
  'tauri', 'dev',
  '--features', 'fixture',
  '--config', 'src-tauri/tauri.fixture.conf.json',
])

let closing = false
function close(code = 0) {
  if (closing) return
  closing = true
  if (!fixture.killed) fixture.kill()
  if (!app.killed) app.kill()
  setTimeout(() => process.exit(code), 300)
}

fixture.on('exit', code => {
  if (!closing && code !== 0) close(code || 1)
})
app.on('exit', code => close(code || 0))
process.on('SIGINT', () => close(130))
process.on('SIGTERM', () => close(143))
