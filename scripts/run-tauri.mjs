#!/usr/bin/env node
// Tauri 启动入口：决定是否给构建追加 `--features cuda[,cuda-cudnn]`。
//
// 默认策略（无需命令行开关）：
// - Windows / Linux：**检测到 CUDA Toolkit（CUDA_PATH / CUDA_HOME）就默认启用 CUDA 构建**，
//   运行时再由设置页「运行设备」在 CPU / GPU 间切换；机器没有 GPU / cuBLAS 时预检后自动回退 CPU + int8。
//   没有 CUDA Toolkit 的构建机自动保持 CPU，不会因为默认 CUDA 而编不过。
// - macOS：始终不启用 CUDA（本地 Whisper 走 Accelerate）。
// - 显式覆盖：ATT_CUDA=1 强制启用；ATT_CUDA=0 / ATT_CPU=1 强制关闭；ATT_CUDNN=1 或 ATT_CUDA=cudnn 追加 cuDNN。
//
// feature 定义见 src-tauri/Cargo.toml → att-core → att-whisper。

import { spawnSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const scriptDir = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(scriptDir, '..')
const tauriCli = path.join(repoRoot, 'node_modules', '@tauri-apps', 'cli', 'tauri.js')

const TRUTHY = new Set(['1', 'true', 'on', 'yes'])
const FALSY = new Set(['0', 'false', 'off', 'no'])

function envFlag(value) {
  return TRUTHY.has(String(value ?? '').trim().toLowerCase())
}

/** 是否检测到 CUDA Toolkit（决定“默认是否启用 CUDA 构建”）。 */
export function cudaToolkitAvailable(env = process.env) {
  return Boolean(String(env.CUDA_PATH ?? '').trim() || String(env.CUDA_HOME ?? '').trim())
}

/**
 * 是否启用 CUDA 构建。
 * 顺序：macOS 否 → ATT_CUDA 真值强制是 → ATT_CUDA=0/off 或 ATT_CPU=1 强制否 → 检测到 CUDA Toolkit 则默认是。
 */
export function cudaRequested(env = process.env, platform = process.platform) {
  if (platform === 'darwin') return false
  const raw = String(env.ATT_CUDA ?? '').trim().toLowerCase()
  if (envFlag(raw) || raw === 'cudnn') return true
  if (envFlag(env.ATT_CPU) || FALSY.has(raw)) return false
  return cudaToolkitAvailable(env)
}

/** ATT_CUDNN=1/true/on/yes（或 ATT_CUDA=cudnn）时在 CUDA 之上追加 cuDNN。 */
export function cudnnRequested(env = process.env) {
  return envFlag(env.ATT_CUDNN) || String(env.ATT_CUDA ?? '').trim().toLowerCase() === 'cudnn'
}

/** 按上面的策略给 tauri 参数追加 `--features`；不需要时原样返回。 */
export function withCudaFeatures(args, env = process.env, platform = process.platform) {
  const features = []
  if (cudaRequested(env, platform)) {
    features.push('cuda')
  }
  if (cudnnRequested(env)) {
    features.push('cuda-cudnn')
  }
  return features.length > 0 ? [...args, '--features', features.join(',')] : [...args]
}

function main(argv) {
  const [command, ...rest] = argv
  if (!command) {
    console.error('用法：node scripts/run-tauri.mjs <dev|build> [tauri 参数…]')
    process.exit(1)
  }
  if ((cudaRequested() || cudnnRequested()) && process.platform === 'darwin') {
    console.error('CUDA 构建仅支持 Windows / Linux；macOS 本地 Whisper 固定走 Accelerate。')
    process.exit(1)
  }
  if (!existsSync(tauriCli)) {
    console.error(`找不到 Tauri CLI：${tauriCli}，请先执行 npm install`)
    process.exit(1)
  }
  // 只对 dev / build 追加 feature，其它子命令原样透传
  const args = command === 'dev' || command === 'build' ? withCudaFeatures([command, ...rest]) : [command, ...rest]
  const result = spawnSync(process.execPath, [tauriCli, ...args], {
    cwd: repoRoot,
    stdio: 'inherit'
  })
  if (result.error) {
    console.error(`启动 Tauri CLI 失败：${result.error.message}`)
    process.exit(1)
  }
  process.exit(result.status ?? 1)
}

const invokedDirectly = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
if (invokedDirectly) {
  main(process.argv.slice(2))
}
