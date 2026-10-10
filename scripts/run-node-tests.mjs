#!/usr/bin/env node
// Node 测试统一执行器（任务 T3：质量门禁）
//
// 背景：tests/ 下有两个手写 main() 的用例脚本（test_unified_overlay_manager.js、
// test_selection_toolbar_manager.js）会在结束时主动调用 process.exit(0)，
// 直接使用 `node --test tests/*.js` 会被它们提前截断测试收集与汇总输出。
// 因此这里改为「逐个文件串行 spawn 一个 node 进程」，最后一个文件跑完再统一汇总，
// 只要有任一文件失败就以非 0 退出码结束，可直接作为 CI 门禁。
//
// 用法：
//   node scripts/run-node-tests.mjs                 # 跑 tests/ 下全部 *.js 与 *.mjs
//   node scripts/run-node-tests.mjs overlay         # 只跑文件名包含 overlay 的用例
//   NODE_TEST_TIMEOUT_MS=60000 node scripts/...     # 自定义单文件超时（默认 120 秒）

import { spawn } from 'node:child_process'
import { readdir } from 'node:fs/promises'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const scriptDir = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(scriptDir, '..')
const testsDir = path.join(repoRoot, 'tests')

// 单文件超时时间：默认 120 秒，防止用例挂死把 CI 卡到全局超时
const timeoutMs = Number.parseInt(process.env.NODE_TEST_TIMEOUT_MS ?? '', 10) || 120_000

// 位置参数作为文件名子串过滤，方便本地只跑关心的用例
const filters = process.argv.slice(2).filter((arg) => !arg.startsWith('-'))

const TEST_FILE_PATTERN = /\.(js|mjs)$/i

/**
 * 枚举待执行的测试文件，按文件名排序以保证本地与 CI 输出顺序一致。
 */
async function collectTestFiles() {
  const entries = await readdir(testsDir, { withFileTypes: true })
  const files = []

  for (const entry of entries) {
    if (!entry.isFile() || !TEST_FILE_PATTERN.test(entry.name)) {
      continue
    }

    if (filters.length > 0 && !filters.some((filter) => entry.name.includes(filter))) {
      continue
    }

    files.push(path.join(testsDir, entry.name))
  }

  return files.sort((left, right) => left.localeCompare(right))
}

/**
 * 执行单个测试文件，返回 { file, ok, code, durationMs, reason }。
 * 使用 stdio: 'inherit' 让子进程输出直接透传，避免缓冲区截断用例自身的日志。
 */
function runTestFile(file) {
  return new Promise((resolve) => {
    const startedAt = Date.now()
    const child = spawn(process.execPath, [file], {
      cwd: repoRoot,
      stdio: 'inherit',
      windowsHide: true
    })

    let settled = false

    // 兜底：超时后强杀子进程，并记为失败
    const timer = setTimeout(() => {
      if (settled) {
        return
      }

      settled = true
      child.kill('SIGKILL')
      resolve({
        file,
        ok: false,
        code: null,
        durationMs: Date.now() - startedAt,
        reason: `超时（超过 ${timeoutMs}ms），已强制终止`
      })
    }, timeoutMs)

    child.on('error', (error) => {
      if (settled) {
        return
      }

      settled = true
      clearTimeout(timer)
      resolve({
        file,
        ok: false,
        code: null,
        durationMs: Date.now() - startedAt,
        reason: `进程启动失败：${error.message}`
      })
    })

    child.on('close', (code, signal) => {
      if (settled) {
        return
      }

      settled = true
      clearTimeout(timer)

      if (signal) {
        resolve({
          file,
          ok: false,
          code,
          durationMs: Date.now() - startedAt,
          reason: `被信号终止：${signal}`
        })
        return
      }

      resolve({
        file,
        ok: code === 0,
        code,
        durationMs: Date.now() - startedAt,
        reason: code === 0 ? '通过' : `退出码 ${code}`
      })
    })
  })
}

async function main() {
  const files = await collectTestFiles()

  if (files.length === 0) {
    console.error(
      filters.length > 0
        ? `[node-test] 未匹配到任何测试文件，过滤条件：${filters.join(', ')}`
        : `[node-test] 未在 ${testsDir} 找到任何 *.js / *.mjs 测试文件`
    )
    process.exitCode = 1
    return
  }

  console.log(`[node-test] 共发现 ${files.length} 个测试文件，开始串行执行`)

  const results = []
  for (const file of files) {
    console.log(`\n[node-test] >>> ${path.relative(repoRoot, file)}`)
    results.push(await runTestFile(file))
  }

  const passed = results.filter((item) => item.ok)
  const failed = results.filter((item) => !item.ok)

  console.log('\n[node-test] ===== 结果汇总 =====')
  for (const item of results) {
    const mark = item.ok ? 'PASS' : 'FAIL'
    const relative = path.relative(repoRoot, item.file)
    console.log(`[node-test] ${mark}  ${relative}  (${item.durationMs}ms)  ${item.reason}`)
  }

  console.log(
    `[node-test] 通过 ${passed.length} / 失败 ${failed.length} / 合计 ${results.length}`
  )

  if (failed.length > 0) {
    console.error(`[node-test] 失败文件：${failed.map((item) => path.basename(item.file)).join(', ')}`)
    process.exitCode = 1
  }
}

main().catch((error) => {
  console.error('[node-test] 执行器异常：', error)
  process.exitCode = 1
})
