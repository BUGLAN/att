import test from 'node:test'
import assert from 'node:assert/strict'

import {
  clipboardPreviewKey,
  createPreviewRequestCache
} from '../src/components/clipboardPanel/previewCache.mjs'

test('缩略图缓存键包含条目 id 与版本', () => {
  assert.equal(clipboardPreviewKey({ id: 7, updated_at: 'v2' }), '7:v2')
  assert.notEqual(
    clipboardPreviewKey({ id: 7, updated_at: 'v1' }),
    clipboardPreviewKey({ id: 7, updated_at: 'v2' })
  )
})

test('同一缩略图在途请求只发起一次，不同版本分别请求', async () => {
  const cache = createPreviewRequestCache()
  let calls = 0
  let release
  const gate = new Promise((resolve) => { release = resolve })
  const item = { id: 'image-1', updated_at: 'v1' }
  const load = async () => {
    calls += 1
    await gate
    return 'blob-v1'
  }

  const first = cache.run(item, load)
  const second = cache.run({ ...item }, load)
  assert.equal(first, second)
  assert.equal(cache.size, 1)
  await Promise.resolve()
  assert.equal(calls, 1)

  const newer = cache.run({ ...item, updated_at: 'v2' }, async () => 'blob-v2')
  assert.equal(cache.size, 2)
  assert.equal(await newer, 'blob-v2')

  release()
  assert.equal(await first, 'blob-v1')
  assert.equal(cache.size, 0)
})
