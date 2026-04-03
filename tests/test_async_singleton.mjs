import test from 'node:test'
import assert from 'node:assert/strict'

import { createSingletonAsyncInvoker } from '../src/utils/asyncSingleton.mjs'

test('createSingletonAsyncInvoker reuses the same in-flight promise', async () => {
  let callCount = 0
  let release
  const blocker = new Promise((resolve) => {
    release = resolve
  })

  const invoke = createSingletonAsyncInvoker(async () => {
    callCount += 1
    await blocker
    return { callCount }
  })

  const pendingResults = [
    invoke(),
    invoke(),
    invoke()
  ]

  release()

  const results = await Promise.all(pendingResults)

  assert.equal(callCount, 1)
  assert.equal(results[0], results[1])
  assert.equal(results[1], results[2])
  assert.deepEqual(results[0], { callCount: 1 })
})

test('createSingletonAsyncInvoker allows a new run after the previous one settles', async () => {
  let callCount = 0

  const invoke = createSingletonAsyncInvoker(async () => {
    callCount += 1
    return callCount
  })

  const first = await invoke()
  const second = await invoke()

  assert.equal(first, 1)
  assert.equal(second, 2)
  assert.equal(callCount, 2)
})
