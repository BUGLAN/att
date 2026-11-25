#!/usr/bin/env node
// Tauri 启动入口：把 ATT_CUDA / ATT_CUDNN 翻译成 `--features cuda[,cuda-cudnn]`。
//
// 默认（未设置环境变量）不加任何 feature，构建行为与以前一致：本地 Whisper 走 CPU + int8。
// 设置 ATT_CUDA=1 启用便携 CUDA 构建；再设 ATT_CUDNN=1（或 ATT_CUDA=cudnn）追加 cuDNN 加速。
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

function envFlag(value) {
  return TRUTHY.has(String(value ?? '').trim().toLowerCase())
}

/** ATT_CUDA=1/true/on/yes（或 cudnn）时启用 CUDA 构建。 */
export function cudaRequested(env = process.env) {
  return envFlag(env.ATT_CUDA) || String(env.ATT_CUDA ?? '').trim().toLowerCase() === 'cudnn'
}

/** ATT_CUDNN=1/true/on/yes（或 ATT_CUDA=cudnn）时在 CUDA 之上追加 cuDNN。 */
export function cudnnRequested(env = process.env) {
  return envFlag(env.ATT_CUDNN) || String(env.ATT_CUDA ?? '').trim().toLowerCase() === 'cudnn'
}

/** 按环境变量给 tauri 参数追加 `--features`；没有开关时原样返回。 */
export function withCudaFeatures(args, env = process.env) {
  const features = []
  if (cudaRequested(env)) {
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
  const result = spawnSync(process.execPath, [tauriCli, ...withCudaFeatures([command, ...rest])], {
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
