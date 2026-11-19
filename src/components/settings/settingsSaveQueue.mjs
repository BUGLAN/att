// 设置自动保存的「最新一次补存」队列（纯逻辑，便于单测）。
//
// 背景：设置页由防抖自动保存驱动。保存请求进行中用户继续改表单时，
// 旧实现会在 handleSave 入口直接 return，最后一笔改动永远不会再提交。
// 这里把请求串起来：保存中只记一个 pending 标记，当前请求结束后再按最新
// 表单状态跑一次；连续多次改动也只补存最新状态，不做无意义的逐笔重放。

/**
 * 创建一个串行、只补存最新状态的异步任务队列。
 *
 * @param {() => Promise<void> | void} run 实际保存任务；调用时自行读取最新状态
 * @param {object} [options]
 * @param {(error: unknown) => void} [options.onError] 任务抛错时的兜底回调
 * @returns {{request: () => void, dispose: () => void, readonly isRunning: boolean, readonly isPending: boolean, readonly isActive: boolean}}
 */
export function createLatestSaveQueue(run, options = {}) {
  if (typeof run !== 'function') {
    throw new TypeError('createLatestSaveQueue 需要保存函数')
  }

  const onError = typeof options.onError === 'function'
    ? options.onError
    : (error) => console.error('[settings-save] 保存任务失败:', error)

  let running = false
  let pending = false
  let disposed = false

  async function drain() {
    if (running || disposed) {
      return
    }

    running = true
    try {
      while (!disposed) {
        pending = false

        try {
          await run()
        } catch (error) {
          onError(error)
        }

        if (!pending) {
          break
        }
      }
    } finally {
      running = false
    }
  }

  function request() {
    if (disposed) {
      return
    }

    pending = true
    if (!running) {
      void drain()
    }
  }

  function dispose() {
    disposed = true
    pending = false
  }

  return {
    request,
    dispose,
    get isRunning() {
      return running
    },
    get isPending() {
      return pending
    },
    get isActive() {
      return running || pending
    }
  }
}
