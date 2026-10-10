// 多段实时录音（multi 模式）的分段纯策略：时长阈值、是否切分、是否重启采集、是否可收尾。
//
// 本模块不依赖 Vue / 浏览器全局对象，也不持有任何录音状态：所有判定所需的
// 标志位、时长均由调用方（useRecorder.js）显式传入，便于 Node 直接单测。
//
// 对照原 useRecorder.js 中的内联实现：
//   MIN_REALTIME_SEGMENT_DURATION / MAX_REALTIME_SEGMENT_DURATION、
//   startRealtimeSegment 的守卫与定时器、
//   stopRealtimeSegment 的"停顿过短"判定、
//   uploadRealtimeSegment 的上传门槛、
//   finalizeRealtimeSession 的收尾条件与无语音提示文案。

// 一次分段录音的最短时长（秒）：非强制切分时低于该值就继续合并当前语句
export const MIN_REALTIME_SEGMENT_DURATION = 2.2

// 单段录音的最长时长（秒）：到点强制切分，避免一句话拖成超大分片
export const MAX_REALTIME_SEGMENT_DURATION = 10

// 强制切分（停止 / 重启）时允许放行的极短分片门槛（秒）
export const FORCE_MIN_REALTIME_SEGMENT_DURATION = 0.8

/**
 * MediaRecorder 是否处于不可用 / 已停止状态。
 * 等价于原实现的 `!mediaRecorder.value || mediaRecorder.value.state === 'inactive'`。
 */
export function isRecorderInactive(recorder) {
  return !recorder || recorder.state === 'inactive'
}

/**
 * 单段最长时长的定时器延时（毫秒）。非法入参回退到 MAX_REALTIME_SEGMENT_DURATION。
 */
export function resolveRealtimeMaxSegmentDelayMs(maxSeconds = MAX_REALTIME_SEGMENT_DURATION) {
  const seconds = Number.isFinite(maxSeconds) ? maxSeconds : MAX_REALTIME_SEGMENT_DURATION
  return seconds * 1000
}

/**
 * 计算当前分段已录制时长（秒）。
 * 与原实现一致：起始时间戳为假值（0 / null / undefined / NaN）时直接返回 0。
 */
export function computeRealtimeSegmentDurationSeconds(startedAtMs, nowMs) {
  if (!startedAtMs) {
    return 0
  }

  return (nowMs - startedAtMs) / 1000
}

/**
 * 是否可以开启一个新的分段录音。
 * 对应原守卫：`if (!stream.value || mediaRecorder.value || realtimeFinalizing) return`。
 */
export function canStartRealtimeSegment({ hasStream, hasRecorder, finalizing }) {
  return Boolean(hasStream) && !hasRecorder && !finalizing
}

/**
 * 一段录完后是否需要继续采集下一段。
 * 对应原实现：`const shouldContinueCapture = !realtimeStopRequested && !realtimeFinalizing`。
 */
export function shouldContinueRealtimeCapture({ stopRequested, finalizing }) {
  return !stopRequested && !finalizing
}

/**
 * 是否应该真正停止当前分段（进入 ondataavailable -> onstop 流程）。
 * 返回 false 表示"停顿过短，继续合并当前语句"。
 * 对应原 stopRealtimeSegment 中的判定：非强制 + 本段有语音 + 未请求停止 + 时长不足最短时长。
 */
export function shouldStopRealtimeSegment({
  force = false,
  segmentHasSpeech,
  stopRequested,
  segmentDuration
}) {
  if (force) {
    return true
  }

  if (segmentHasSpeech && !stopRequested && segmentDuration < MIN_REALTIME_SEGMENT_DURATION) {
    return false
  }

  return true
}

/**
 * 是否应该把这一段上传到后端。
 * 对应原 uploadRealtimeSegment 的三层门槛，判定顺序与原实现一致：
 *   1) 必须有会话、有数据、本段检测到过语音；
 *   2) 非强制时：时长达到 MIN_REALTIME_SEGMENT_DURATION；
 *   3) 强制时：时长不足 0.8s 且整场尚未捕获过语音则丢弃。
 */
export function shouldUploadRealtimeSegment({
  hasSession,
  hasContent,
  hadSpeech,
  force = false,
  segmentDuration,
  hasCapturedSpeech
}) {
  if (!hasSession || !hasContent || !hadSpeech) {
    return false
  }

  if (!force && segmentDuration < MIN_REALTIME_SEGMENT_DURATION) {
    return false
  }

  if (force && segmentDuration < FORCE_MIN_REALTIME_SEGMENT_DURATION && !hasCapturedSpeech) {
    return false
  }

  return true
}

/**
 * 是否可以收尾整场多段会话。
 * 对应原 finalizeRealtimeSession 的两道守卫：会话存在且未在收尾中，
 * 且没有在录的分段、没有在途上传、没有未完成的分段。
 */
export function shouldFinalizeRealtimeSession({
  hasSessionId,
  finalizing,
  hasRecorder,
  pendingUploads,
  pendingSegmentCount
}) {
  if (!hasSessionId || finalizing) {
    return false
  }

  if (hasRecorder || pendingUploads > 0 || pendingSegmentCount > 0) {
    return false
  }

  return true
}

/**
 * 整场录音是否没有捕获到有效语音。
 * 对应原实现：`!realtimeHasCapturedSpeech || processedSegmentCount.value === 0`。
 */
export function isMissingRealtimeSpeech({ hasCapturedSpeech, processedSegmentCount }) {
  return !hasCapturedSpeech || processedSegmentCount === 0
}

/**
 * 无有效语音时的用户提示文案（时长 >= 1s 视为"未检测到有效语音"）。
 * 文案与原实现逐字一致。
 */
export function deriveMissingSpeechErrorMessage(durationSeconds) {
  return durationSeconds >= 1 ? '未检测到有效语音' : '录音时间过短'
}

/**
 * 按 sequence 排序分段结果（缺失 / 非数字 sequence 排到最后，且保持原有稳定性）。
 * 对应原 updateLiveTranscriptPreview 里的 sort 比较函数。
 */
export function sortSegmentsBySequence(segments) {
  const list = Array.isArray(segments) ? [...segments] : []

  return list.sort((left, right) => {
    const leftSequence = typeof left.sequence === 'number' ? left.sequence : Number.MAX_SAFE_INTEGER
    const rightSequence = typeof right.sequence === 'number' ? right.sequence : Number.MAX_SAFE_INTEGER
    return leftSequence - rightSequence
  })
}

/**
 * 把分段文本拼接成整场预览文本（空段会产生空行，与原 join('\n\n') 行为一致）。
 */
export function joinSegmentTexts(segments) {
  const list = Array.isArray(segments) ? segments : []
  return list.map((segment) => segment.text).join('\n\n')
}
