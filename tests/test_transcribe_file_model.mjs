import test from 'node:test'
import assert from 'node:assert/strict'

import {
  JOB_POLL_MAX_DELAY_MS,
  JOB_POLL_MAX_WAIT_MS,
  SYNC_REQUEST_TIMEOUT_MS,
  buildTranscribeRequest,
  fileNameOf,
  formatDuration,
  isAsyncJobAccepted,
  isJobPollExpired,
  isSupportedMediaPath,
  nextJobPollDelay,
  normalizeTranscribeResult
} from '../src/composables/transcribe/fileTranscribeModel.mjs'

test('同步超时常量与契约一致（15 分钟）', () => {
  assert.equal(SYNC_REQUEST_TIMEOUT_MS, 15 * 60 * 1000)
  assert.equal(JOB_POLL_MAX_WAIT_MS, 15 * 60 * 1000)
  assert.equal(JOB_POLL_MAX_DELAY_MS, 5000)
})

test('buildTranscribeRequest：正常构造 snake_case 请求体', () => {
  assert.deepEqual(
    buildTranscribeRequest({ path: 'E:\\tmp\\dv\\abc.mp4', language: 'zh', wantSegments: false }),
    { path: 'E:\\tmp\\dv\\abc.mp4', language: 'zh', want_segments: false }
  )
  assert.deepEqual(
    buildTranscribeRequest({ path: ' /a/b.wav ', wantSegments: true }),
    { path: '/a/b.wav', language: 'zh', want_segments: true }
  )
})

test('buildTranscribeRequest：空路径返回 null（不发请求）', () => {
  assert.equal(buildTranscribeRequest({ path: '' }), null)
  assert.equal(buildTranscribeRequest({ path: '   ' }), null)
  assert.equal(buildTranscribeRequest({}), null)
})

test('isAsyncJobAccepted：只认非空字符串 job_id', () => {
  assert.equal(isAsyncJobAccepted({ job_id: 'abc' }), true)
  assert.equal(isAsyncJobAccepted({ job_id: '' }), false)
  assert.equal(isAsyncJobAccepted({ text: '你好' }), false)
  assert.equal(isAsyncJobAccepted(null), false)
  assert.equal(isAsyncJobAccepted('str'), false)
})

test('nextJobPollDelay：1s 起步逐次 +500ms，5s 封顶', () => {
  assert.equal(nextJobPollDelay(1), 1000)
  assert.equal(nextJobPollDelay(2), 1500)
  assert.equal(nextJobPollDelay(3), 2000)
  assert.equal(nextJobPollDelay(9), 5000)
  assert.equal(nextJobPollDelay(50), 5000)
})

test('nextJobPollDelay：非法入参按第 1 次处理，不产生负延迟', () => {
  assert.equal(nextJobPollDelay(0), 1000)
  assert.equal(nextJobPollDelay(-3), 1000)
  assert.equal(nextJobPollDelay(Number.NaN), 1000)
  assert.equal(nextJobPollDelay(undefined), 1000)
})

test('isJobPollExpired：未到期 false，到期 true，非法按 0 处理', () => {
  assert.equal(isJobPollExpired(0), false)
  assert.equal(isJobPollExpired(1000), false)
  assert.equal(isJobPollExpired(15 * 60 * 1000), true)
  assert.equal(isJobPollExpired(Number.NaN), false)
  assert.equal(isJobPollExpired(-1), false)
})

test('轮询仿真：9 次内累计等待达到 15 秒封顶节奏', () => {
  let elapsed = 0
  for (let attempt = 1; attempt <= 9; attempt += 1) {
    elapsed += nextJobPollDelay(attempt)
  }
  assert.equal(elapsed, 1000 + 1500 + 2000 + 2500 + 3000 + 3500 + 4000 + 4500 + 5000)
  assert.equal(isJobPollExpired(elapsed), false)
})

test('normalizeTranscribeResult：归一字段并兜底缺省', () => {
  const view = normalizeTranscribeResult(
    { text: '你好', duration_seconds: '61.5', engine: 'whisper:medium', cached: 1, segments: [{ start: 0, end: 1 }] },
    { path: 'E:\\dv\\a.mp4' }
  )
  assert.equal(view.text, '你好')
  assert.equal(view.durationSeconds, 61.5)
  assert.equal(view.engine, 'whisper:medium')
  assert.equal(view.cached, true)
  assert.equal(view.fileName, 'a.mp4')
  assert.deepEqual(view.segments, [{ start: 0, end: 1 }])

  const empty = normalizeTranscribeResult(null)
  assert.equal(empty, null)

  const minimal = normalizeTranscribeResult({}, { path: '' })
  assert.equal(minimal.text, '')
  assert.equal(minimal.durationSeconds, 0)
  assert.equal(minimal.cached, false)
  assert.deepEqual(minimal.segments, [])
})

test('fileNameOf：兼容反斜杠 / 正斜杠 / 尾部分隔符', () => {
  assert.equal(fileNameOf('E:\\tmp\\dv\\abc.mp4'), 'abc.mp4')
  assert.equal(fileNameOf('/tmp/dv/abc.mp4'), 'abc.mp4')
  assert.equal(fileNameOf('abc.mp4'), 'abc.mp4')
  assert.equal(fileNameOf(''), '')
})

test('formatDuration：61 → 1 分 01 秒，0 / 非法 → 0 秒', () => {
  assert.equal(formatDuration(0), '0 秒')
  assert.equal(formatDuration(45), '45 秒')
  assert.equal(formatDuration(61), '1 分 01 秒')
  assert.equal(formatDuration(3600), '60 分 00 秒')
  assert.equal(formatDuration(Number.NaN), '0 秒')
  assert.equal(formatDuration(-5), '0 秒')
})

test('isSupportedMediaPath：大小写不敏感，txt 拒绝', () => {
  assert.equal(isSupportedMediaPath('E:\\a\\b.MP4'), true)
  assert.equal(isSupportedMediaPath('/x/y.WAV'), true)
  assert.equal(isSupportedMediaPath('/x/y.txt'), false)
  assert.equal(isSupportedMediaPath(''), false)
})
