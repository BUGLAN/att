import test from 'node:test'
import assert from 'node:assert/strict'

import { createLatestSaveQueue } from '../src/components/settings/settingsSaveQueue.mjs'

function deferred() {
  let resolve
  let reject
  const promise = new Promise((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

async function waitUntil(predicate, timeoutMs = 1000) {
  const startedAt = Date.now()
  while (!predicate()) {
    if (Date.now() - startedAt > timeoutMs) {
      throw new Error('等待保存队列结束超时')
    }
    await new Promise((resolve) => setTimeout(resolve, 0))
  }
}

test('保存中继续改动只补存最新一次，不丢最后一笔', async () => {
  let version = 0
  let runs = 0
  const saved = []
  const firstRunStarted = deferred()
  const releaseFirstRun = deferred()

  const queue = createLatestSaveQueue(async () => {
    runs += 1
    const current = version
    if (runs === 1) {
      firstRunStarted.resolve()
      await releaseFirstRun.promise
    }
    saved.push(current)
  })

  version = 1
  queue.request()
  await firstRunStarted.promise

  version = 2
  queue.request()
  version = 3
  queue.request()
  releaseFirstRun.resolve()

  await waitUntil(() => !queue.isActive)
  assert.deepEqual(saved, [1, 3], '中间版本被合并，最后版本必须补存')
  assert.equal(runs, 2)
})

test('保存失败走 onError，且队列之后仍能继续保存', async () => {
  const errors = []
  let attempts = 0
  const queue = createLatestSaveQueue(async () => {
    attempts += 1
    if (attempts === 1) {
      throw new Error('网络断开')
    }
  }, {
    onError: (error) => errors.push(error.message)
  })

  queue.request()
  await waitUntil(() => !queue.isActive)
  assert.deepEqual(errors, ['网络断开'])

  queue.request()
  await waitUntil(() => !queue.isActive)
  assert.equal(attempts, 2)
  assert.deepEqual(errors, ['网络断开'])
})

test('dispose 后不再执行排队中的保存', async () => {
  let runs = 0
  const firstRun = deferred()
  const queue = createLatestSaveQueue(async () => {
    runs += 1
    if (runs === 1) {
      await firstRun.promise
    }
  })

  queue.request()
  await waitUntil(() => runs === 1)
  queue.request()
  queue.dispose()
  firstRun.resolve()
  await waitUntil(() => !queue.isActive)
  assert.equal(runs, 1, 'dispose 只允许当前正在跑的这一笔完成')
})
