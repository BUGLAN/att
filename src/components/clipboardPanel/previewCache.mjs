// 剪贴板图片预览的请求去重：缓存键包含 id 与版本，同一版图片只发起一次请求。
// Vue 组件负责把完成的 blob 转成对象地址，并只让当前条目版本写入界面。

export function clipboardPreviewKey(item) {
  const version = item?.updated_at || item?.created_at || ''
  return `${String(item?.id ?? '')}:${String(version)}`
}

export function createPreviewRequestCache() {
  const pending = new Map()

  return {
    run(item, load) {
      const key = clipboardPreviewKey(item)
      const current = pending.get(key)
      if (current) return current

      const request = Promise.resolve()
        .then(load)
        .finally(() => {
          if (pending.get(key) === request) {
            pending.delete(key)
          }
        })
      pending.set(key, request)
      return request
    },
    has(item) {
      return pending.has(clipboardPreviewKey(item))
    },
    get size() {
      return pending.size
    }
  }
}
