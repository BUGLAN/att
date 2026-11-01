// 剪贴板悬浮框的纯逻辑：分段加载 / 键盘意图 / 回车行为（不依赖 Vue，供 tests/test_clipboard_panel_model.mjs 锁定）。
//
// 剪贴板记录可能很多：每次唤起只取第一段，翻到快到头时再取下一段；
// 搜索与筛选交给后端（同样分段），本地不再持有全量列表。

/** 每段条数：一屏通常放 5–10 张卡片，一段够翻好几屏 */
export const PANEL_PAGE_SIZE = 30
/** 选中项离已加载末尾不到这么多张、或卡片带滚到离末尾不足半屏时，预取下一段 */
export const PANEL_PREFETCH_AHEAD = 8

export const PANEL_FILTERS = Object.freeze([
  { value: 'all', label: '全部' },
  { value: 'favorite', label: '收藏' },
  { value: 'text', label: '文本' },
  { value: 'image', label: '图片' },
  { value: 'file', label: '文件' },
  { value: 'link', label: '链接' }
])

/** 筛选 → 后端查询参数；「文本」包含长文本（code） */
export function buildPanelQuery({ filter = 'all', query = '', offset = 0, limit = PANEL_PAGE_SIZE } = {}) {
  const params = { offset, limit, with_total: true }
  const needle = String(query || '').trim()
  if (needle) params.query = needle
  if (filter === 'favorite') params.favorite_only = true
  else if (filter === 'text') params.item_type = 'text,code'
  else if (filter && filter !== 'all') params.item_type = filter
  return params
}

/** 后端分页响应 → { items, total }；兼容旧的纯数组响应（当作只有这一段） */
export function normalizePage(result) {
  if (Array.isArray(result)) return { items: result, total: result.length }
  const items = Array.isArray(result?.items) ? result.items : []
  return { items, total: Number.isFinite(result?.total) ? result.total : items.length }
}

/**
 * 追加下一段：按 id 去重。分段期间有新内容复制进来时，后面的条目整体后移一位，
 * 下一段的开头会与已加载的末尾重复。
 */
export function appendPage(existing, page) {
  const seen = new Set(existing.map((item) => item.id))
  return existing.concat(page.filter((item) => !seen.has(item.id)))
}

/** 是否该取下一段：没在加载、还有没取的；且选中项接近末尾，或卡片带滚到离末尾不足半屏 */
export function shouldLoadMore({ loaded, total, loading = false, index = 0, scrollLeft = 0, clientWidth = 0, scrollWidth = 0 }) {
  if (loading || loaded >= total) return false
  if (index >= loaded - PANEL_PREFETCH_AHEAD) return true
  return scrollWidth > 0 && scrollLeft + clientWidth >= scrollWidth - clientWidth / 2
}

export function cycleFilter(current, delta) {
  const index = Math.max(0, PANEL_FILTERS.findIndex((f) => f.value === current))
  const length = PANEL_FILTERS.length
  return PANEL_FILTERS[(index + delta + length) % length].value
}

export function moveIndex(index, delta, length) {
  if (!length) return 0
  return (index + delta + length) % length
}

/**
 * 键盘事件 → 意图。输入法组字中（isComposing）一律不处理，免得回车确认候选词时把条目贴出去。
 * - ←→ 在卡片带里选择；Tab / ⇧Tab 切筛选；↵ 提交，⌘↵ / Ctrl↵ 走另一种回车行为；⌘1–9 直接提交第几条；Esc 先清搜索再收起
 */
export function panelKeyIntent(event, { hasQuery = false } = {}) {
  if (!event || event.isComposing) return null
  const mod = Boolean(event.metaKey || event.ctrlKey)
  switch (event.key) {
    case 'ArrowLeft': return { type: 'move', delta: -1 }
    case 'ArrowRight': return { type: 'move', delta: 1 }
    case 'Tab': return { type: 'filter', delta: event.shiftKey ? -1 : 1 }
    case 'Enter': return { type: 'commit', alternate: mod }
    case 'Escape': return hasQuery ? { type: 'clear-query' } : { type: 'close' }
    default:
      if (mod && /^[1-9]$/.test(event.key)) return { type: 'quick', index: Number(event.key) - 1 }
      return null
  }
}

/** 回车行为：设置里的默认（paste / copy），alternate 时取另一种 */
export function resolveCommitAction(enterAction, alternate = false) {
  const primary = enterAction === 'copy' ? 'copy' : 'paste'
  if (!alternate) return primary
  return primary === 'paste' ? 'copy' : 'paste'
}

export function commitActionLabel(action) {
  return action === 'copy' ? '仅复制' : '粘贴'
}

/** 列表右侧的相对时间：刚刚 / 5 分钟前 / 3 小时前 / 昨天 / 10-29 */
export function relativeTime(value, now = Date.now()) {
  const time = new Date(value || 0).getTime()
  if (!Number.isFinite(time) || time <= 0) return ''
  const minutes = Math.floor((now - time) / 60000)
  if (minutes < 1) return '刚刚'
  if (minutes < 60) return `${minutes} 分钟前`
  if (minutes < 24 * 60) return `${Math.floor(minutes / 60)} 小时前`
  const date = new Date(time)
  const yesterday = new Date(now)
  yesterday.setDate(yesterday.getDate() - 1)
  if (date.toDateString() === yesterday.toDateString()) return '昨天'
  return `${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
}

/** 文件名（路径最后一段）；图片优先用展示名 */
export function panelFileName(item, displayTitle = '') {
  if (item?.item_type === 'image' && displayTitle) return displayTitle
  return String(item?.content || '').trim().split(/[\\/]/).pop() || displayTitle
}

/** 链接卡片：域名 + 其余部分；解析不了就整条当域名 */
export function splitLink(value) {
  const text = String(value || '').trim()
  try {
    const url = new URL(text)
    return { host: url.host, rest: `${url.pathname}${url.search}${url.hash}`.replace(/^\/$/, '') }
  } catch {
    return { host: text, rest: '' }
  }
}
