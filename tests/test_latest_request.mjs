import test from 'node:test'
import assert from 'node:assert/strict'

import { createLatestRequestGuard, runLatest } from '../src/utils/latestRequest.mjs'

function deferred() {
  let resolve
  let reject
  const promise = new Promise((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

test('旧请求晚到时不覆盖新结果，也不提前结束新请求的 loading', async () => {
  const guard = createLatestRequestGuard()
  const first = deferred()
  const second = deferred()
  const results = []
  let starts = 0
  let finishes = 0
  const handlers = {
    onStart: () => { starts += 1 },
    onResult: (value) => results.push(value),
    onFinally: () => { finishes += 1 }
  }

  const firstRun = runLatest(guard, () => first.promise, handlers)
  const secondRun = runLatest(guard, () => second.promise, handlers)

  second.resolve('第二次查询')
  assert.deepEqual(await secondRun, { applied: true, stale: false, result: '第二次查询' })
  first.resolve('第一次查询')
  assert.deepEqual(await firstRun, { applied: false, stale: true })

  assert.deepEqual(results, ['第二次查询'])
  assert.equal(starts, 2)
  assert.equal(finishes, 1, '只有当前请求能收尾 loading')
})

test('旧请求失败不报错；当前请求失败才上报', async () => {
  const guard = createLatestRequestGuard()
  const stale = deferred()
  const latest = deferred()
  const errors = []
  let finishes = 0
  const handlers = {
    onError: (error) => errors.push(error.message),
    onFinally: () => { finishes += 1 }
  }

  const staleRun = runLatest(guard, () => stale.promise, handlers)
  const latestRun = runLatest(guard, () => latest.promise, handlers)

  latest.reject(new Error('当前错误'))
  assert.deepEqual(await latestRun, { applied: false, stale: false, error: new Error('当前错误') })
  stale.reject(new Error('旧错误'))
  assert.deepEqual(await staleRun, { applied: false, stale: true, error: new Error('旧错误') })

  assert.deepEqual(errors, ['当前错误'])
  assert.equal(finishes, 1)
})
