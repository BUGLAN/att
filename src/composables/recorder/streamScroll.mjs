// 流式文本自动滚动判定（纯逻辑）：用户手动向上滚动后不要抢回滚动位置。
export const AUTO_SCROLL_THRESHOLD_PX = 48

export function shouldAutoScroll({
  scrollTop,
  scrollHeight,
  clientHeight,
  threshold = AUTO_SCROLL_THRESHOLD_PX
} = {}) {
  const top = Number(scrollTop)
  const height = Number(scrollHeight)
  const client = Number(clientHeight)
  const limit = Number(threshold)

  if (!Number.isFinite(top) || !Number.isFinite(height) || !Number.isFinite(client)) {
    return true
  }

  const safeLimit = Number.isFinite(limit) && limit >= 0 ? limit : AUTO_SCROLL_THRESHOLD_PX
  return height - (top + client) <= safeLimit
}
