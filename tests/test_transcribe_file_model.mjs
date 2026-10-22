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
  normalizeTranscribeResult,
  buildProofreadDiff,
  splitSentences,
  TRANSCRIBE_ENGINES
} from '../src/composables/transcribe/fileTranscribeModel.mjs'

test('同步超时常量与契约一致（15 分钟）', () => {
  assert.equal(SYNC_REQUEST_TIMEOUT_MS, 15 * 60 * 1000)
  assert.equal(JOB_POLL_MAX_WAIT_MS, 15 * 60 * 1000)
  assert.equal(JOB_POLL_MAX_DELAY_MS, 5000)
})

test('buildTranscribeRequest：正常构造 snake_case 请求体（M2 含 engine/proofread）', () => {
  assert.deepEqual(
    buildTranscribeRequest({ path: '/tmp/dv/abc.mp4', language: 'zh', wantSegments: false }),
    { path: '/tmp/dv/abc.mp4', language: 'zh', want_segments: false, engine: 'whisper', proofread: false }
  )
  assert.deepEqual(
    buildTranscribeRequest({ path: ' /a/b.wav ', wantSegments: true, engine: 'qwen_asr' }),
    { path: '/a/b.wav', language: 'zh', want_segments: true, engine: 'qwen_asr', proofread: false }
  )
})

test('buildTranscribeRequest：校对开启附带模型与窗口参数；非法引擎回落 whisper', () => {
  const request = buildTranscribeRequest({
    path: '/a/b.wav',
    proofread: true,
    proofreadModelId: 'm-1',
    proofreadBatchSegments: 60,
    proofreadContextChars: 0
  })
  assert.equal(request.proofread, true)
  assert.equal(request.proofread_model_id, 'm-1')
  assert.deepEqual(request.proofread_options, { batch_segments: 60, context_chars: 0 })

  const bare = buildTranscribeRequest({ path: '/a/b.wav', proofread: true })
  assert.ok(!('proofread_model_id' in bare))
  assert.deepEqual(bare.proofread_options, { batch_segments: 40, context_chars: 200 })
  assert.equal(buildTranscribeRequest({ path: '/a/b.wav', engine: 'gpt' }).engine, 'whisper')
})

test('TRANSCRIBE_ENGINES：三引擎元数据齐备', () => {
  assert.deepEqual(
    TRANSCRIBE_ENGINES.map((item) => item.value),
    ['whisper', 'qwen_asr', 'doubao_asr']
  )
})

test('splitSentences：按句切分并过滤空串', () => {
  assert.deepEqual(splitSentences('一句。两句！三句？'), ['一句。', '两句！', '三句？'])
  assert.deepEqual(splitSentences('  '), [])
  assert.deepEqual(splitSentences(''), [])
})

test('buildProofreadDiff：逐句配对、句数不一致退化整段、无修改计 0', () => {
  const diff = buildProofreadDiff('大家好呃今天学习。第二句嗯。', '大家好，今天学习。第二句。')
  assert.equal(diff.pairs.length, 2)
  assert.equal(diff.changedCount, 2)
  assert.equal(diff.pairs[0].raw, '大家好呃今天学习。')

  const single = buildProofreadDiff('一句。两句。三句。', '合并后的一句话。')
  assert.equal(single.pairs.length, 1)
  assert.equal(single.changedCount, 1)

  assert.equal(buildProofreadDiff('一样。', '一样。').changedCount, 0)
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
  assert.equal(view.engineKey, 'whisper')
  assert.equal(view.cached, true)
  assert.equal(view.fileName, 'a.mp4')
  assert.deepEqual(view.segments, [{ start: 0, end: 1 }])
  assert.equal(view.rawText, '')            // 无校对字段时的缺省
  assert.equal(view.proofread, null)

  // 校对字段归一（M2）
  const proofed = normalizeTranscribeResult({
    text: '校对后', raw_text: '原文',
    proofread: { applied: 1, llm_model: 'deep', windows: '3', fallback_batches: [1] }
  }, { path: '/a.wav' })
  assert.equal(proofed.rawText, '原文')
  assert.equal(proofed.proofread.applied, true)
  assert.equal(proofed.proofread.windows, 3)
  assert.deepEqual(proofed.proofread.fallbackBatches, [1])

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
