export function createSingletonAsyncInvoker(asyncOperation) {
  let pendingPromise = null

  return (...args) => {
    if (pendingPromise) {
      return pendingPromise
    }

    pendingPromise = Promise.resolve()
      .then(() => asyncOperation(...args))
      .finally(() => {
        pendingPromise = null
      })

    return pendingPromise
  }
}
