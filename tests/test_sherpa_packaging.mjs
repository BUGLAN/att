import assert from 'node:assert/strict'
import fs from 'node:fs'

import {
  assetKeyForTarget,
  isLibraryFile,
  loadManifest,
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

console.log('[sherpa-packaging] 全部断言通过')
