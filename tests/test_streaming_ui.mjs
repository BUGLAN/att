// NDJSON 解析 + 自动滚动判定单元测试。
import test from 'node:test'
import assert from 'node:assert/strict'

import { createNdjsonParser } from '../src/utils/ndjson.mjs'
import { shouldAutoScroll } from '../src/composables/recorder/streamScroll.mjs'

test('NDJSON：一次 push 多个完整行按顺序回调', () => {
  const rows = []
  const parser = createNdjsonParser((row) => rows.push(row))
  parser.push('{"type":"start","record_id":"r1"}\n{"type":"delta","text":"你"}\n')
  parser.flush()
  assert.deepEqual(rows, [
    { type: 'start', record_id: 'r1' },
    { type: 'delta', text: '你' }
  ])
})

test('NDJSON：跨 chunk 的半行会缓存到下一次 push', () => {
  const rows = []
  const parser = createNdjsonParser((row) => rows.push(row))
  parser.push('{"type":"delta","text":"你')
  parser.push('好"}\n')
  parser.flush()
  assert.deepEqual(rows, [{ type: 'delta', text: '你好' }])
})

test('NDJSON：兼容 CRLF、空行与末尾无换行的 flush', () => {
  const rows = []
  const parser = createNdjsonParser((row) => rows.push(row))
  parser.push('\r\n{"type":"done"}\r\n\n{"type":"error"}')
  parser.flush()
  assert.deepEqual(rows, [{ type: 'done' }, { type: 'error' }])
})

test('NDJSON：非法 JSON 抛 SyntaxError 并带上原始行', () => {
  const parser = createNdjsonParser(() => {})
  assert.throws(() => parser.push('{bad json}\n'), (error) => {
    assert.equal(error.name, 'SyntaxError')
    assert.match(error.message, /NDJSON/)
    assert.equal(error.line, '{bad json}')
    return true
  })
})

test('shouldAutoScroll：接近底部时允许自动滚动', () => {
  assert.equal(shouldAutoScroll({ scrollTop: 900, scrollHeight: 1000, clientHeight: 100 }), true)
  assert.equal(shouldAutoScroll({ scrollTop: 960, scrollHeight: 1000, clientHeight: 100 }), true)
})

test('shouldAutoScroll：用户向上滚动后不抢滚动位置', () => {
  assert.equal(shouldAutoScroll({ scrollTop: 400, scrollHeight: 1000, clientHeight: 100 }), false)
})

test('shouldAutoScroll：非法尺寸按允许处理，避免首次渲染不滚到底', () => {
  assert.equal(shouldAutoScroll({ scrollTop: undefined, scrollHeight: 1000, clientHeight: 100 }), true)
  assert.equal(shouldAutoScroll({ scrollTop: 0, scrollHeight: 0, clientHeight: 0 }), true)
})
