// 悬浮窗收尾判定（纯逻辑，不依赖 Vue / 浏览器全局对象）。
//
// 背景：真流式录音的 stream_finished 只代表 ASR 收尾，自动优化仍在后端后台执行；
// 若此时按 complete 收尾，悬浮窗会在很短的时间后自动隐藏，整个优化期间用户看不到
// 任何“仍在处理”的提示。后台优化完成事件也可能晚于下一次录音到达，必须用
// record_id 判断事件是否属于当前悬浮窗，避免上一段录音的事件提前收掉新录音的提示。

/**
 * stream_finished 到达后，悬浮窗是否应继续等待后台自动优化。
 *
 * @param {{ autoOptimize?: boolean, recordId?: string, recordStatus?: string, optimizationCompleted?: boolean }} input
 * @returns {boolean}
 */
export function shouldAwaitBackgroundOptimization({
  autoOptimize,
  recordId,
  recordStatus,
  optimizationCompleted
} = {}) {
  if (!autoOptimize || !recordId) {
    return false
  }

  if (optimizationCompleted) {
    return false
  }

  return recordStatus !== 'optimized'
}

/**
 * 事件里的 record_id 是否属于当前悬浮窗会话。
 * 当前会话尚未绑定 record_id（空串）时一律返回 false：此时宁可让事件只更新记录列表，
 * 也不能误收掉刚显示出来的新录音悬浮窗。
 *
 * @param {{ active?: boolean, currentRecordId?: string, eventRecordId?: string }} input
 * @returns {boolean}
 */
export function ownsFloatingRecord({ active, currentRecordId, eventRecordId } = {}) {
  return Boolean(active && currentRecordId && eventRecordId && currentRecordId === eventRecordId)
}
