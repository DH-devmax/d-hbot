import { readFileSync } from 'node:fs'
import { runInNewContext } from 'node:vm'
import test from 'node:test'
import assert from 'node:assert/strict'

const source = readFileSync(new URL('../src-tauri/src/gateway.rs', import.meta.url), 'utf8')
const template = source.slice(source.indexOf('fn ipc_expression(')).match(/r#"([\s\S]*?)"#/)[1]
const expression = template.replaceAll('{{', '{').replaceAll('}}', '}').replace('{input}', JSON.stringify({ type: 'request', route: '/test', payload: {} }))
for (const preload of [true, false]) {
  test(preload ? 'isolated preload accepts callback and removes listener' : 'legacy Electron remains compatible', async () => {
    let callback, replyChannel, removed = false
    const ipc = {
      once(channel, fn) {
        if (preload) assert.match(channel, /^\d+(\.\d+)?(sss|eee)$/)
        replyChannel = channel
        callback = fn
      },
      send(channel, request) {
        assert.equal(channel, 'xclient')
        assert.equal(request.key, replyChannel)
        callback({}, { code: 200, errno: 0, response: { code: 0 } })
      },
      removeListener(channel, fn) {
        assert.equal(channel, replyChannel)
        assert.equal(fn, callback)
        removed = true
      },
    }
    const context = { setTimeout, clearTimeout, ...(preload ? { electronAPI: ipc } : { require: () => ({ ipcRenderer: ipc }) }) }
    const result = await runInNewContext(expression, context)
    assert.equal(result.transportCode, 200)
    assert.equal(result.errno, 0)
    assert.equal(removed, true)
  })
}
test('missing IPC reports unavailable without throwing', async () => {
  const result = await runInNewContext(expression, { setTimeout, clearTimeout })
  assert.equal(result.transportCode, 503)
})
