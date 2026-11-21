// 剪贴板历史容量（条数 / 天数上限）的选项与确认文案（不依赖 Vue，供 tests/test_clipboard_limits.mjs 锁定）。
//
// 后端：settings.clipboard_max_items / clipboard_retention_days，0 = 不限；改了就立即按新上限清理一次，
// 之后每次入库与每小时检查；置顶和收藏的永不自动删除。
// 用预设选项而不是自由输入：设置页自动保存，边打字边保存会先按「5」清掉一大片再变成「50」。

export const MAX_ITEMS_CHOICES = Object.freeze([0, 50, 100, 200, 500, 1000, 2000, 5000])
export const RETENTION_DAY_CHOICES = Object.freeze([0, 7, 30, 90, 180, 365])

/** 设置值归一为非负整数（缺省、非法 = 0 = 不限） */
export function normalizeLimit(value) {
  const number = Math.round(Number(value))
  return Number.isFinite(number) && number > 0 ? number : 0
}

function withCurrent(choices, current) {
  const value = normalizeLimit(current)
  return choices.includes(value) ? [...choices] : [...choices, value].sort((a, b) => a - b)
}

/** 条数下拉：0 = 不限；当前值不在预设里（手改过 settings.json）时也列出来 */
export function maxItemsOptions(current = 0) {
  return withCurrent(MAX_ITEMS_CHOICES, current).map((value) => ({
    value,
    label: value === 0 ? '不限' : `最近 ${value} 条`
  }))
}

/** 天数下拉：0 = 永久保留 */
export function retentionOptions(current = 0) {
  return withCurrent(RETENTION_DAY_CHOICES, current).map((value) => ({
    value,
    label: value === 0 ? '永久保留' : `${value} 天内`
  }))
}

/** 改紧了（从不限改成有上限，或上限变小）：会立即删掉超出的记录，要先确认 */
export function isTighter(previous, next) {
  const before = normalizeLimit(previous)
  const after = normalizeLimit(next)
  return after > 0 && (before === 0 || after < before)
}

/** 改紧上限前的确认文案 */
export function tightenConfirm(kind, value) {
  const limit = normalizeLimit(value)
  const what = kind === 'days'
    ? `超过 ${limit} 天没再复制过的记录`
    : `最近 ${limit} 条以外的旧记录`
  return {
    title: kind === 'days' ? `只保留 ${limit} 天内的剪贴板记录？` : `只保留最近 ${limit} 条剪贴板记录？`,
    description: `${what}会立即删除，图片一并删除，无法恢复；置顶和收藏的记录不受影响。之后新复制的内容也按这个上限自动清理。`
  }
}
