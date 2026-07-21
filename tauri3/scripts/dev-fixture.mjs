import { spawn } from 'node:child_process'

const windows = process.platform === 'win32'
const cargo = windows ? 'cargo.exe' : 'cargo'
const pnpm = windows ? 'pnpm.cmd' : 'pnpm'
const environment = { ...process.env, DH_RUNTIME_MODE: 'fixture' }

const fixture = spawn(cargo, [
  'run',
  '--manifest-path', 'src-tauri/Cargo.toml',
  '--features', 'fixture',
  '--bin', 'dh-fixture',
], { stdio: 'inherit', env: environment })

const app = spawn(pnpm, [
  'tauri', 'dev',
  '--features', 'fixture',
  '--config', 'src-tauri/tauri.fixture.conf.json',
], { stdio: 'inherit', env: environment })

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
