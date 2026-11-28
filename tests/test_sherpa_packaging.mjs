import assert from 'node:assert/strict'
import fs from 'node:fs'

import {
  assetKeyForTarget,
  isLibraryFile,
  loadManifest,
  targetTripleForBuild,
  validateManifest
} from '../scripts/prepare-sherpa-onnx.mjs'

const manifest = loadManifest()

assert.equal(manifest.version, '2.0.1')
assert.equal(manifest.release, 'v2.0.1')
assert.deepEqual(Object.keys(manifest.assets).sort(), ['linux-x64', 'macos-universal2', 'windows-x64'])
for (const [key, asset] of Object.entries(manifest.assets)) {
  assert.match(asset.file, new RegExp(`sherpa-onnx-${manifest.release}`), `${key} 的包名要带版本`)
  assert.ok(asset.size > 0, `${key} 缺文件大小`)
  assert.match(asset.sha256, /^[0-9a-f]{64}$/, `${key} 缺 SHA-256`)
}

assert.equal(assetKeyForTarget('aarch64-apple-darwin'), 'macos-universal2')
assert.equal(assetKeyForTarget('x86_64-apple-darwin'), 'macos-universal2')
assert.equal(assetKeyForTarget('x86_64-pc-windows-msvc'), 'windows-x64')
assert.equal(assetKeyForTarget('x86_64-unknown-linux-gnu'), 'linux-x64')
assert.equal(assetKeyForTarget('', 'darwin', 'arm64'), 'macos-universal2')
assert.equal(assetKeyForTarget('', 'win32', 'x64'), 'windows-x64')
assert.throws(() => assetKeyForTarget('aarch64-pc-windows-msvc'), /不支持/)
assert.throws(() => assetKeyForTarget('aarch64-unknown-linux-gnu'), /不支持/)

assert.equal(targetTripleForBuild('', { TAURI_ENV_TARGET_TRIPLE: ' x86_64-pc-windows-msvc ' }), 'x86_64-pc-windows-msvc')
assert.equal(
  assetKeyForTarget(targetTripleForBuild('', { TAURI_ENV_TARGET_TRIPLE: 'x86_64-pc-windows-msvc' }), 'darwin', 'arm64'),
  'windows-x64',
  'Tauri 注入的目标平台优先于宿主平台'
)
assert.equal(
  targetTripleForBuild('aarch64-apple-darwin', { TAURI_ENV_TARGET_TRIPLE: 'x86_64-pc-windows-msvc' }),
  'aarch64-apple-darwin',
  '显式 --target 优先于环境变量'
)
assert.equal(targetTripleForBuild('', {}), '')

assert.equal(isLibraryFile('windows-x64', 'sherpa-onnx-c-api.dll'), true)
assert.equal(isLibraryFile('windows-x64', 'onnxruntime.dll'), true)
assert.equal(isLibraryFile('windows-x64', 'LICENSE'), false)
assert.equal(isLibraryFile('macos-universal2', 'libonnxruntime.1.20.1.dylib'), true)
assert.equal(isLibraryFile('macos-universal2', 'libsherpa-onnx-c-api.dylib'), true)
assert.equal(isLibraryFile('linux-x64', 'libsherpa-onnx-c-api.so'), true)
assert.equal(isLibraryFile('linux-x64', 'libonnxruntime.so.1'), true)
assert.equal(isLibraryFile('linux-x64', 'README.md'), false)

assert.throws(() => validateManifest({ ...manifest, assets: { ...manifest.assets, 'macos-universal2': { ...manifest.assets['macos-universal2'], sha256: 'bad' } } }), /SHA-256/)

const tauriConfig = JSON.parse(fs.readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'))
assert.equal(tauriConfig.bundle.resources['resources/sherpa-onnx/'], 'sherpa-onnx/')
assert.equal(tauriConfig.build.beforeBuildCommand, 'npm run prepare:sherpa && npm run build')

const releaseWorkflow = fs.readFileSync(new URL('../.github/workflows/release.yml', import.meta.url), 'utf8')
assert.match(releaseWorkflow, /ATT_CPU: '1'/, '发布产物必须固定 CPU 变体')
assert.match(releaseWorkflow, /gh release edit[\s\S]*--notes-file/, '已有 Release 要用最终正文更新')
assert.match(releaseWorkflow, /gh release create[\s\S]*--notes-file/, '新建 Release 要直接写入最终正文')
assert.doesNotMatch(releaseWorkflow, /安装包正在构建，完成后会自动写入发布说明/, '不得留下占位正文')

console.log('[sherpa-packaging] 全部断言通过')
