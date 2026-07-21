import { readFile } from 'node:fs/promises'
import path from 'node:path'

const root = path.resolve(import.meta.dirname, '..')
const readJson = async relative => JSON.parse(await readFile(path.join(root, relative), 'utf8'))

const packageJson = await readJson('package.json')
const production = await readJson('src-tauri/tauri.conf.json')
const developer = await readJson('src-tauri/tauri.fixture.conf.json')
const scripts = packageJson.scripts || {}

function assert(condition, message) {
  if (!condition) throw new Error(`生产构建边界校验失败：${message}`)
}

const productionBuild = scripts['tauri:build:production'] || ''
const productionRustBuild = scripts['build:rust:production'] || ''
const developerBuild = scripts['tauri:build:developer'] || ''
const resources = production.bundle?.resources || []
const developerResources = developer.bundle?.resources || []

assert(production.productName === 'DH BOT', '生产产品名必须为 DH BOT')
assert(production.identifier === 'cloud.daha6.dhbot', '生产 identifier 不正确')
assert(Array.isArray(resources) && resources.length === 0, '生产 Tauri 配置必须显式使用空 resources')
assert(!JSON.stringify(production).toLowerCase().includes('fixture'), '生产 Tauri 配置含 Fixture 标记')
assert(productionBuild.includes('--no-default-features'), '生产 Tauri 构建未显式关闭默认 feature')
assert(!/--features\s+fixture/.test(productionBuild), '生产 Tauri 构建启用了 fixture feature')
assert(/cargo\s+build/.test(productionRustBuild), '缺少生产 Rust 构建命令')
assert(productionRustBuild.includes('--release'), '生产 Rust 构建未使用 release')
assert(productionRustBuild.includes('--no-default-features'), '生产 Rust 构建未关闭默认 feature')
assert(/--bin\s+dh-bot/.test(productionRustBuild), '生产 Rust 构建未限定 dh-bot 主程序')
assert(!/--features\s+fixture/.test(productionRustBuild), '生产 Rust 构建启用了 fixture feature')
assert(developer.productName === 'DH BOT Dev', '开发产品名必须为 DH BOT Dev')
assert(/--features\s+fixture/.test(developerBuild), '开发 Tauri 构建应启用 fixture feature')
assert(developerResources.some(resource => String(resource).includes('resources/tools')), '开发 Tauri 配置缺少 Fixture 资源')

console.log('生产/开发构建边界校验通过')
