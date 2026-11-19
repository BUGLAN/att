// 只接受「最后一次发起」的异步响应。
//
// 搜索 / 筛选 / 翻页会在短时间内发出多次请求，旧请求晚到时不能覆盖新结果，
// 也不能把新请求的 loading 提前复位；错误同样只由当前请求上报。

export function createLatestRequestGuard() {
  let sequence = 0
  return {
    begin() {
      sequence += 1
      return sequence
    },
    isCurrent(token) {
      return token === sequence
    }
  }
}

/**
 * 执行一次请求，并把结果 / 错误 / 收尾回调限制在当前请求上。
 *
 * @param {{ begin: () => number, isCurrent: (token: number) => boolean }} guard
 * @param {() => Promise<unknown>} task
 * @param {{ onStart?: () => void, onResult?: (value: unknown) => void, onError?: (error: unknown) => void, onFinally?: () => void }} [handlers]
 * @returns {Promise<{ applied: boolean, stale: boolean, result?: unknown, error?: unknown }>}
 */
export async function runLatest(guard, task, handlers = {}) {
  const token = guard.begin()
  handlers.onStart?.()

  try {
    const result = await task()
    if (!guard.isCurrent(token)) {
      return { applied: false, stale: true }
    }
    handlers.onResult?.(result)
    return { applied: true, stale: false, result }
  } catch (error) {
    if (!guard.isCurrent(token)) {
      return { applied: false, stale: true, error }
    }
    handlers.onError?.(error)
    return { applied: false, stale: false, error }
  } finally {
    if (guard.isCurrent(token)) {
      handlers.onFinally?.()
    }
  }
}
