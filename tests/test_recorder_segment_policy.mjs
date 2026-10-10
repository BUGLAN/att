// 多段录音分段策略模块（src/composables/recorder/segmentPolicy.mjs）单元测试。
// 覆盖正常路径 + 边界：空 / 未定义状态、极短分段、强制切分、时长阈值临界点。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  FORCE_MIN_REALTIME_SEGMENT_DURATION,
  MAX_REALTIME_SEGMENT_DURATION,
  MIN_REALTIME_SEGMENT_DURATION,
  canStartRealtimeSegment,
  computeRealtimeSegmentDurationSeconds,
  deriveMissingSpeechErrorMessage,
  isMissingRealtimeSpeech,
  isRecorderInactive,
  joinSegmentTexts,
  resolveRealtimeMaxSegmentDelayMs,
  shouldContinueRealtimeCapture,
  shouldFinalizeRealtimeSession,
  shouldStopRealtimeSegment,
  shouldUploadRealtimeSegment,
  sortSegmentsBySequence
} from '../src/composables/recorder/segmentPolicy.mjs'

test('分段时长阈值常量与原实现一致', () => {
  assert.equal(MIN_REALTIME_SEGMENT_DURATION, 2.2)
  assert.equal(MAX_REALTIME_SEGMENT_DURATION, 10)
  assert.equal(FORCE_MIN_REALTIME_SEGMENT_DURATION, 0.8)
})

test('isRecorderInactive：空值与 inactive 都视为不可用', () => {
  assert.equal(isRecorderInactive(undefined), true)
  assert.equal(isRecorderInactive(null), true)
  assert.equal(isRecorderInactive({ state: 'inactive' }), true)
  assert.equal(isRecorderInactive({ state: 'recording' }), false)
  assert.equal(isRecorderInactive({ state: 'paused' }), false)
})

test('resolveRealtimeMaxSegmentDelayMs：默认 10s，非法入参回退默认值', () => {
  assert.equal(resolveRealtimeMaxSegmentDelayMs(), 10000)
  assert.equal(resolveRealtimeMaxSegmentDelayMs(5), 5000)
  assert.equal(resolveRealtimeMaxSegmentDelayMs(Number.NaN), 10000)
  assert.equal(resolveRealtimeMaxSegmentDelayMs(undefined), 10000)
})

test('computeRealtimeSegmentDurationSeconds：起始时间为假值返回 0', () => {
  assert.equal(computeRealtimeSegmentDurationSeconds(0, 5000), 0)
  assert.equal(computeRealtimeSegmentDurationSeconds(undefined, 5000), 0)
  assert.equal(computeRealtimeSegmentDurationSeconds(null, 5000), 0)
})

test('computeRealtimeSegmentDurationSeconds：正常与负值（时钟回拨）都按原公式计算', () => {
  assert.equal(computeRealtimeSegmentDurationSeconds(1000, 3200), 2.2)
  assert.equal(computeRealtimeSegmentDurationSeconds(2000, 1000), -1)
  assert.equal(computeRealtimeSegmentDurationSeconds(500, 500), 0)
})

test('canStartRealtimeSegment：需要流就绪、无在录分段、未在收尾', () => {
  assert.equal(canStartRealtimeSegment({ hasStream: {}, hasRecorder: null, finalizing: false }), true)
  assert.equal(canStartRealtimeSegment({ hasStream: null, hasRecorder: null, finalizing: false }), false)
  assert.equal(canStartRealtimeSegment({ hasStream: {}, hasRecorder: {}, finalizing: false }), false)
  assert.equal(canStartRealtimeSegment({ hasStream: {}, hasRecorder: null, finalizing: true }), false)
  assert.equal(canStartRealtimeSegment({}), false)
})

test('shouldContinueRealtimeCapture：仅当未请求停止且未在收尾时为真', () => {
  assert.equal(shouldContinueRealtimeCapture({ stopRequested: false, finalizing: false }), true)
  assert.equal(shouldContinueRealtimeCapture({ stopRequested: true, finalizing: false }), false)
  assert.equal(shouldContinueRealtimeCapture({ stopRequested: false, finalizing: true }), false)
})

test('shouldStopRealtimeSegment：强制切分永远为真', () => {
  assert.equal(
    shouldStopRealtimeSegment({ force: true, segmentHasSpeech: false, stopRequested: false, segmentDuration: 0 }),
    true
  )
  assert.equal(
    shouldStopRealtimeSegment({ force: true, segmentHasSpeech: true, stopRequested: true, segmentDuration: 0 }),
    true
  )
})

test('shouldStopRealtimeSegment：有语音但停顿过短时继续合并', () => {
  assert.equal(
    shouldStopRealtimeSegment({ segmentHasSpeech: true, stopRequested: false, segmentDuration: 1 }),
    false
  )
  // 临界点：恰好达到最短时长就允许切分
  assert.equal(
    shouldStopRealtimeSegment({
      segmentHasSpeech: true,
      stopRequested: false,
      segmentDuration: MIN_REALTIME_SEGMENT_DURATION
    }),
    true
  )
})

test('shouldStopRealtimeSegment：无语音 / 已请求停止时立即切分', () => {
  assert.equal(
    shouldStopRealtimeSegment({ segmentHasSpeech: false, stopRequested: false, segmentDuration: 0.1 }),
    true
  )
  assert.equal(
    shouldStopRealtimeSegment({ segmentHasSpeech: true, stopRequested: true, segmentDuration: 0.1 }),
    true
  )
})

test('shouldUploadRealtimeSegment：常规路径达到最短时长才上传', () => {
  assert.equal(
    shouldUploadRealtimeSegment({
      hasSession: 's1',
      hasContent: 1024,
      hadSpeech: true,
      segmentDuration: 3
    }),
    true
  )
  assert.equal(
    shouldUploadRealtimeSegment({
      hasSession: 's1',
      hasContent: 1024,
      hadSpeech: true,
      segmentDuration: 2.19
    }),
    false
  )
  assert.equal(
    shouldUploadRealtimeSegment({
      hasSession: 's1',
      hasContent: 1024,
      hadSpeech: true,
      segmentDuration: MIN_REALTIME_SEGMENT_DURATION
    }),
    true
  )
})

test('shouldUploadRealtimeSegment：缺少会话 / 数据 / 语音一律不上传', () => {
  const base = { hasSession: 's1', hasContent: 1024, hadSpeech: true, segmentDuration: 5 }
  assert.equal(shouldUploadRealtimeSegment({ ...base, hasSession: null }), false)
  assert.equal(shouldUploadRealtimeSegment({ ...base, hasContent: 0 }), false)
  assert.equal(shouldUploadRealtimeSegment({ ...base, hadSpeech: false }), false)
  assert.equal(shouldUploadRealtimeSegment({}), false)
})

test('shouldUploadRealtimeSegment：强制上传时低于 0.8s 的极短片段看是否有历史语音', () => {
  assert.equal(
    shouldUploadRealtimeSegment({
      hasSession: 's1',
      hasContent: 512,
      hadSpeech: true,
      force: true,
      segmentDuration: 0.5,
      hasCapturedSpeech: false
    }),
    false
  )
  assert.equal(
    shouldUploadRealtimeSegment({
      hasSession: 's1',
      hasContent: 512,
      hadSpeech: true,
      force: true,
      segmentDuration: 0.5,
      hasCapturedSpeech: true
    }),
    true
  )
  // 临界点：达到 0.8s 即可上传，不再看历史语音
  assert.equal(
    shouldUploadRealtimeSegment({
      hasSession: 's1',
      hasContent: 512,
      hadSpeech: true,
      force: true,
      segmentDuration: FORCE_MIN_REALTIME_SEGMENT_DURATION,
      hasCapturedSpeech: false
    }),
    true
  )
})

test('shouldFinalizeRealtimeSession：无在途任务时才允许收尾', () => {
  assert.equal(
    shouldFinalizeRealtimeSession({
      hasSessionId: 's1',
      finalizing: false,
      hasRecorder: null,
      pendingUploads: 0,
      pendingSegmentCount: 0
    }),
    true
  )

  const base = {
    hasSessionId: 's1',
    finalizing: false,
    hasRecorder: null,
    pendingUploads: 0,
    pendingSegmentCount: 0
  }
  assert.equal(shouldFinalizeRealtimeSession({ ...base, hasSessionId: null }), false)
  assert.equal(shouldFinalizeRealtimeSession({ ...base, finalizing: true }), false)
  assert.equal(shouldFinalizeRealtimeSession({ ...base, hasRecorder: {} }), false)
  assert.equal(shouldFinalizeRealtimeSession({ ...base, pendingUploads: 1 }), false)
  assert.equal(shouldFinalizeRealtimeSession({ ...base, pendingSegmentCount: 1 }), false)
  assert.equal(shouldFinalizeRealtimeSession({}), false)
})

test('isMissingRealtimeSpeech：无捕获或零分段都算无有效语音', () => {
  assert.equal(isMissingRealtimeSpeech({ hasCapturedSpeech: true, processedSegmentCount: 3 }), false)
  assert.equal(isMissingRealtimeSpeech({ hasCapturedSpeech: true, processedSegmentCount: 0 }), true)
  assert.equal(isMissingRealtimeSpeech({ hasCapturedSpeech: false, processedSegmentCount: 5 }), true)
  assert.equal(isMissingRealtimeSpeech({}), true)
})

test('deriveMissingSpeechErrorMessage：按时长区分两种提示文案', () => {
  assert.equal(deriveMissingSpeechErrorMessage(0), '录音时间过短')
  assert.equal(deriveMissingSpeechErrorMessage(0.9), '录音时间过短')
  assert.equal(deriveMissingSpeechErrorMessage(1), '未检测到有效语音')
  assert.equal(deriveMissingSpeechErrorMessage(120), '未检测到有效语音')
})

test('sortSegmentsBySequence：按 sequence 升序且不修改入参', () => {
  const segments = [
    { id: 'b', sequence: 2, text: '第二句' },
    { id: 'a', sequence: 1, text: '第一句' }
  ]
  const ordered = sortSegmentsBySequence(segments)
  assert.deepEqual(ordered.map((segment) => segment.id), ['a', 'b'])
  assert.deepEqual(segments.map((segment) => segment.id), ['b', 'a'])
})

test('sortSegmentsBySequence：缺失 / 非数字 sequence 排到最后', () => {
  const ordered = sortSegmentsBySequence([
    { id: 'x', text: '无序号' },
    { id: 'y', sequence: '3', text: '字符串序号' },
    { id: 'z', sequence: 1, text: '第一句' }
  ])
  assert.deepEqual(ordered.map((segment) => segment.id), ['z', 'x', 'y'])
})

test('sortSegmentsBySequence：空输入与非法入参返回空数组', () => {
  assert.deepEqual(sortSegmentsBySequence([]), [])
  assert.deepEqual(sortSegmentsBySequence(undefined), [])
})

test('joinSegmentTexts：用空行拼接分段文本', () => {
  assert.equal(joinSegmentTexts([{ text: '第一句' }, { text: '第二句' }]), '第一句\n\n第二句')
  assert.equal(joinSegmentTexts([]), '')
  assert.equal(joinSegmentTexts(undefined), '')
})
