// 转写状态轮询的纯策略：失败计数、失败上限判定与轮询节拍守卫。
//
// 背景（第 1 轮修复）：后端异常时 1Hz 轮询会永久静默失败，UI 永久卡在"转写中"。
// 原实现在 useRecorder.js 内联了 MAX_POLL_FAILURES = 30 的上限判定，这里把它抽成
// 可单测的纯函数。
//
// 说明：当前实现是**固定 1s 节拍**，没有任何指数退避；本次拆分只做等价搬迁，
// 不新增退避算法（新增退避会改变真实轮询时序）。

// 连续失败上限：达到即停止轮询、复位 UI 并提示用户
export const MAX_POLL_FAILURES = 30

// 轮询节拍（毫秒），与原 setInterval 的 1000 一致
export const POLL_INTERVAL_MS = 1000

/**
 * 累加一次轮询失败次数。非法（非有限数 / 非正数）入参按 0 起算，保证计数不会卡死。
 */
export function nextPollFailureCount(currentCount) {
  const base = Number.isFinite(currentCount) && currentCount > 0 ? Math.floor(currentCount) : 0
  return base + 1
}

/**
 * 失败次数是否已达到上限。limit 非法时回退到 MAX_POLL_FAILURES。
 */
export function hasReachedPollFailureLimit(failureCount, limit = MAX_POLL_FAILURES) {
  const safeLimit = Number.isFinite(limit) && limit > 0 ? Math.floor(limit) : MAX_POLL_FAILURES
  const count = Number.isFinite(failureCount) ? failureCount : 0
  return count >= safeLimit
}

/**
 * 本次 tick 是否应直接跳过（上一次轮询尚未返回，避免请求叠加）。
 * 对应原实现里的 `if (pollingInFlight) return`。
 */
export function shouldSkipPollTick(inFlight) {
  return Boolean(inFlight)
}

/**
 * 本次 tick 结束后是否应停止轮询。
 * 对应原实现里的 `if (completed) stopPolling()`，completed 由状态判定结果给出。
 */
export function shouldStopPollingAfterTick(completed) {
  return Boolean(completed)
}
