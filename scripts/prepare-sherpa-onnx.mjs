#!/usr/bin/env node
// 把当前平台对应的 sherpa-onnx 共享库下载、校验并解压到 Tauri 资源目录。
//
// 只随安装包分发推理库，不下载模型。发布矩阵的目标平台通过 `--target` 传入；
// 本地 `tauri build` 不传时按当前 Node 平台推断。下载包与 SHA-256 固定在
// `src-tauri/crates/att-qwen/vendor/sherpa-onnx-libs.json`，升级 sherpa-onnx 时
// 与 `vendor/c-api.h` 一起替换。

import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { copyFileSync, createWriteStream, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { Readable, Transform } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { fileURLToPath } from 'node:url'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
export const MANIFEST_PATH = path.join(ROOT, 'src-tauri/crates/att-qwen/vendor/sherpa-onnx-libs.json')
export const DEFAULT_OUTPUT_DIR = path.join(ROOT, 'src-tauri/resources/sherpa-onnx')
const RELEASE_BASE = 'https://github.com/k2-fsa/sherpa-onnx/releases/download'
const STAMP_FILE = '.att-sherpa-onnx.json'

const MAIN_LIBRARY = {
  'windows-x64': 'sherpa-onnx-c-api.dll',
  'macos-universal2': 'libsherpa-onnx-c-api.dylib',
  'linux-x64': 'libsherpa-onnx-c-api.so'
}
const PRESERVED_FILES = new Set(['.gitignore', '.gitkeep', 'README.md'])

export function assetKeyForTarget(target = '', platform = process.platform, arch = process.arch) {
  const normalized = String(target).trim().toLowerCase()
  if (normalized) {
    if (normalized.includes('windows') && /^x86_64-/.test(normalized)) return 'windows-x64'
    if (normalized.includes('apple-darwin')) return 'macos-universal2'
    if (normalized.includes('linux') && /^x86_64-/.test(normalized)) return 'linux-x64'
    throw new Error(`不支持的 sherpa-onnx 打包目标：${target}`)
  }
  if (platform === 'win32' && arch === 'x64') return 'windows-x64'
  if (platform === 'darwin') return 'macos-universal2'
  if (platform === 'linux' && arch === 'x64') return 'linux-x64'
  throw new Error(`当前构建平台没有对应的 sherpa-onnx 资产：${platform}/${arch}`)
}

/** 显式 --target 优先，其次读取 Tauri beforeBuildCommand 注入的目标三元组。 */
export function targetTripleForBuild(explicitTarget = '', env = process.env) {
  return String(explicitTarget ?? '').trim() || String(env?.TAURI_ENV_TARGET_TRIPLE ?? '').trim()
}

export function isLibraryFile(key, name) {
  const lower = name.toLowerCase()
  if (key === 'windows-x64') return lower.endsWith('.dll')
  if (key === 'macos-universal2') return lower.endsWith('.dylib')
  if (key === 'linux-x64') return lower.includes('.so')
  return false
}

function isOnnxRuntimeFile(key, name) {
  const lower = name.toLowerCase()
  if (key === 'windows-x64') return lower.startsWith('onnxruntime') && lower.endsWith('.dll')
  if (key === 'macos-universal2') return lower.startsWith('libonnxruntime') && lower.endsWith('.dylib')
  if (key === 'linux-x64') return lower.startsWith('libonnxruntime') && lower.includes('.so')
  return false
}

export function validateManifest(manifest) {
  if (!/^v?\d+\.\d+\.\d+$/.test(manifest?.version || '')) throw new Error('sherpa-onnx 清单缺少合法 version')
  if (!/^v\d+\.\d+\.\d+$/.test(manifest?.release || '')) throw new Error('sherpa-onnx 清单缺少合法 release')
  for (const key of Object.keys(MAIN_LIBRARY)) {
    const asset = manifest?.assets?.[key]
    if (!asset?.file || !Number.isInteger(asset.size) || asset.size <= 0) {
      throw new Error(`sherpa-onnx 清单缺少 ${key} 的 file / size`)
    }
    if (!/^[0-9a-f]{64}$/.test(asset.sha256 || '')) throw new Error(`sherpa-onnx 清单的 ${key} 缺少 SHA-256`)
  }
  return manifest
}

export function loadManifest(file = MANIFEST_PATH) {
  return validateManifest(JSON.parse(readFileSync(file, 'utf8')))
}

function assetUrl(manifest, asset) {
  return `${RELEASE_BASE}/${manifest.release}/${asset.file}`
}

function sha256File(file) {
  const hash = createHash('sha256')
  const buffer = readFileSync(file)
  hash.update(buffer)
  return hash.digest('hex')
}

function verifyArchive(file, asset) {
  const size = statSync(file).size
  if (size !== asset.size) throw new Error(`下载包大小不符：期望 ${asset.size}，实际 ${size}`)
  const digest = sha256File(file)
  if (digest !== asset.sha256) throw new Error(`下载包 SHA-256 不符：期望 ${asset.sha256}，实际 ${digest}`)
}

async function downloadVerified(url, destination, asset) {
  let lastError
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    const partial = `${destination}.part-${process.pid}`
    try {
      const response = await fetch(url, {
        redirect: 'follow',
        headers: { 'user-agent': 'att-sherpa-onnx-packager' },
        signal: AbortSignal.timeout(15 * 60 * 1000)
      })
      if (!response.ok || !response.body) throw new Error(`下载失败：HTTP ${response.status}`)
      const hash = createHash('sha256')
      let received = 0
      let lastTenth = -1
      const meter = new Transform({
        transform(chunk, _encoding, callback) {
          hash.update(chunk)
          received += chunk.length
          const tenth = asset.size ? Math.min(10, Math.floor((received / asset.size) * 10)) : 0
          if (tenth !== lastTenth) {
            console.log(`  下载 ${asset.file}：${tenth * 10}%`)
            lastTenth = tenth
          }
          callback(null, chunk)
        }
      })
      await pipeline(Readable.fromWeb(response.body), meter, createWriteStream(partial))
      if (received !== asset.size) throw new Error(`下载包大小不符：期望 ${asset.size}，实际 ${received}`)
      const digest = hash.digest('hex')
      if (digest !== asset.sha256) throw new Error(`下载包 SHA-256 不符：期望 ${asset.sha256}，实际 ${digest}`)
      renameSync(partial, destination)
      return destination
    } catch (error) {
      lastError = error
      rmSync(partial, { force: true })
      if (attempt < 3) {
        console.warn(`下载失败（第 ${attempt} 次）：${error.message}，5 秒后重试`)
        await new Promise(resolve => setTimeout(resolve, 5000))
      }
    }
  }
  throw lastError
}

async function obtainArchive(manifest, key, asset, explicitArchive) {
  if (explicitArchive) {
    const file = path.resolve(explicitArchive)
    verifyArchive(file, asset)
    return file
  }
  const cacheRoot = process.env.ATT_SHERPA_CACHE_DIR || path.join(tmpdir(), 'att-sherpa-onnx-cache', manifest.release)
  mkdirSync(cacheRoot, { recursive: true })
  const cached = path.join(cacheRoot, asset.file)
  if (existsSync(cached)) {
    try {
      verifyArchive(cached, asset)
      console.log(`复用已校验下载包：${cached}`)
      return cached
    } catch (error) {
      console.warn(`缓存包不可用，重新下载：${error.message}`)
      rmSync(cached, { force: true })
    }
  }
  console.log(`下载 sherpa-onnx ${manifest.version}（${key}）：${asset.file}`)
  await downloadVerified(assetUrl(manifest, asset), cached, asset)
  return cached
}

function walkFiles(dir) {
  const files = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name)
    if (entry.isDirectory()) files.push(...walkFiles(full))
    else files.push(full)
  }
  return files
}

function extractArchive(archive, directory) {
  const result = spawnSync('tar', ['-xjf', archive, '-C', directory], { stdio: 'inherit' })
  if (result.error) throw new Error(`解压失败（找不到 tar）：${result.error.message}`)
  if (result.status !== 0) throw new Error(`解压失败：tar 退出码 ${result.status}`)
}

function readStamp(outputDir) {
  try {
    return JSON.parse(readFileSync(path.join(outputDir, STAMP_FILE), 'utf8'))
  } catch {
    return null
  }
}

function alreadyPrepared(outputDir, manifest, key, asset) {
  const stamp = readStamp(outputDir)
  return stamp?.key === key
    && stamp?.version === manifest.version
    && stamp?.sha256 === asset.sha256
    && Array.isArray(stamp?.files)
    && stamp.files.every(file => file?.name && existsSync(path.join(outputDir, file.name)))
}

function cleanOutput(outputDir) {
  mkdirSync(outputDir, { recursive: true })
  for (const entry of readdirSync(outputDir, { withFileTypes: true })) {
    if (PRESERVED_FILES.has(entry.name)) continue
    rmSync(path.join(outputDir, entry.name), { recursive: true, force: true })
  }
}

function copyLibraries(extracted, outputDir, key) {
  const all = walkFiles(extracted)
  const libraries = all.filter(file => isLibraryFile(key, path.basename(file)))
  if (!libraries.some(file => path.basename(file) === MAIN_LIBRARY[key])) {
    throw new Error(`下载包中没有主推理库 ${MAIN_LIBRARY[key]}`)
  }
  if (!libraries.some(file => isOnnxRuntimeFile(key, path.basename(file)))) {
    throw new Error('下载包中没有 ONNX Runtime 动态库')
  }
  const copied = new Map()
  for (const file of libraries) {
    const name = path.basename(file)
    if (copied.has(name)) {
      throw new Error(`下载包中有重名的动态库：${name}`)
    }
    copied.set(name, file)
  }
  for (const [name, file] of copied) {
    copyFileSync(file, path.join(outputDir, name))
  }
  return [...copied.keys()].sort().map(name => ({ name, bytes: statSync(path.join(outputDir, name)).size }))
}

function parseArgs(argv) {
  const args = { target: '', archive: '', output: DEFAULT_OUTPUT_DIR }
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]
    if (arg === '--target') args.target = argv[++index] || ''
    else if (arg === '--archive') args.archive = argv[++index] || ''
    else if (arg === '--output') args.output = path.resolve(argv[++index] || DEFAULT_OUTPUT_DIR)
    else if (arg === '--help' || arg === '-h') {
      console.log('用法：node scripts/prepare-sherpa-onnx.mjs [--target <rust target>] [--archive <本地 tar.bz2>] [--output <目录>]')
      process.exit(0)
    } else {
      throw new Error(`未知参数：${arg}`)
    }
  }
  return args
}

async function main() {
  const args = parseArgs(process.argv.slice(2))
  const manifest = loadManifest()
  const target = targetTripleForBuild(args.target)
  const key = assetKeyForTarget(target)
  const asset = manifest.assets[key]
  if (alreadyPrepared(args.output, manifest, key, asset)) {
    console.log(`sherpa-onnx ${manifest.version} 已就绪：${args.output}`)
    return
  }
  const archive = await obtainArchive(manifest, key, asset, args.archive)
  const temp = mkdtempSync(path.join(tmpdir(), 'att-sherpa-unpack-'))
  try {
    extractArchive(archive, temp)
    cleanOutput(args.output)
    const files = copyLibraries(temp, args.output, key)
    const bytes = files.reduce((sum, file) => sum + file.bytes, 0)
    writeFileSync(path.join(args.output, STAMP_FILE), JSON.stringify({
      version: manifest.version,
      key,
      sha256: asset.sha256,
      files,
      bytes
    }, null, 2) + '\n')
    const mib = (bytes / 1024 / 1024).toFixed(1)
    console.log(`已准备 sherpa-onnx ${manifest.version}（${key}）：${files.length} 个动态库，解压后 ${mib} MiB`)
    console.log(`资源目录：${args.output}`)
  } finally {
    rmSync(temp, { recursive: true, force: true })
  }
}

const invokedDirectly = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
if (invokedDirectly) {
  main().catch(error => {
    console.error(`准备 sherpa-onnx 失败：${error.message}`)
    process.exit(1)
  })
}
