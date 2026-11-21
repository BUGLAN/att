// 记录集顶部「用量统计」的纯展示模型（不依赖 Vue，供 tests/test_usage_stats_model.mjs 锁定）。
//
// 数据来自 GET /api/stats?range=7d|30d|all：{ range, from, to, today, total, days: [{ date, count, duration, chars, tokens }] }。
// 收起时一行小统计（今天 / 近 7 天）；展开后按天画柱状图，可切范围与指标。

/** 统计范围（值与后端 range 参数一致） */
export const USAGE_RANGES = Object.freeze([
  { value: '7d', label: '近 7 天' },
  { value: '30d', label: '近 30 天' },
  { value: 'all', label: '全部' }
])

/** 柱状图可切换的指标 */
export const USAGE_METRICS = Object.freeze([
  { key: 'count', label: '条数' },
  { key: 'duration', label: '时长' },
  { key: 'chars', label: '字数' },
  { key: 'tokens', label: 'Token' }
])

/** 「全部」跨度超过这么多天时按月合并成一根柱，避免几百根细柱挤在一起 */
export const MONTH_BUCKET_THRESHOLD_DAYS = 92

function toNumber(value) {
  const number = Number(value)
  return Number.isFinite(number) && number > 0 ? number : 0
}

/** 整数加千分位；一万以上用「1.2 万」 */
export function formatUsageNumber(value) {
  const number = Math.round(toNumber(value))
  if (number >= 10000) {
    return `${(number / 10000).toFixed(1).replace(/\.0$/, '')} 万`
  }
  return String(number).replace(/\B(?=(\d{3})+(?!\d))/g, ',')
}

/** 录音时长：45 秒 / 12 分钟 / 1 小时 5 分 */
export function formatUsageDuration(seconds) {
  const total = Math.round(toNumber(seconds))
  if (total < 60) return `${total} 秒`
  const minutes = Math.floor(total / 60)
  if (minutes < 60) return `${minutes} 分钟`
  const hours = Math.floor(minutes / 60)
  const rest = minutes % 60
  return rest ? `${hours} 小时 ${rest} 分` : `${hours} 小时`
}

/** 某个指标的取值格式化（柱的提示、图例共用） */
export function formatMetric(metric, value) {
  switch (metric) {
    case 'duration':
      return formatUsageDuration(value)
    case 'count':
      return `${formatUsageNumber(value)} 条`
    case 'chars':
      return `${formatUsageNumber(value)} 字`
    case 'tokens':
      return `${formatUsageNumber(value)} token`
    default:
      return formatUsageNumber(value)
  }
}

/** 一段时间用量的一行文字：「3 条 · 12 分钟 · 1,204 字 · 860 token」；没有记录时「暂无记录」 */
export function usageSummary(totals) {
  if (!toNumber(totals?.count)) return '暂无记录'
  return [
    formatMetric('count', totals.count),
    formatUsageDuration(totals.duration),
    formatMetric('chars', totals.chars),
    formatMetric('tokens', totals.tokens)
  ].join(' · ')
}

function parseDay(value) {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(String(value || ''))
  return match ? { year: Number(match[1]), month: Number(match[2]), day: Number(match[3]) } : null
}

/** 按月合并逐日数据（保持升序） */
function bucketByMonth(days) {
  const buckets = []
  for (const day of days) {
    const parsed = parseDay(day.date)
    if (!parsed) continue
    const key = `${parsed.year}-${String(parsed.month).padStart(2, '0')}`
    let last = buckets[buckets.length - 1]
    if (!last || last.key !== key) {
      last = { key, year: parsed.year, month: parsed.month, count: 0, duration: 0, chars: 0, tokens: 0 }
      buckets.push(last)
    }
    for (const metric of ['count', 'duration', 'chars', 'tokens']) {
      last[metric] += toNumber(day[metric])
    }
  }
  return buckets
}

/**
 * 柱状图数据：每根柱 { key, label, title, value, ratio }，ratio ∈ [0, 1] 是相对最高一根的高度。
 * label 是横轴刻度（逐日「11/21」，按月「2026 年 11 月」→「11 月」），title 是悬停提示（日期 + 四项用量）。
 */
export function usageBars(stats, metric = 'count') {
  const days = Array.isArray(stats?.days) ? stats.days : []
  const byMonth = days.length > MONTH_BUCKET_THRESHOLD_DAYS
  const items = byMonth
    ? bucketByMonth(days).map((bucket) => ({
      ...bucket,
      label: `${bucket.month} 月`,
      heading: `${bucket.year} 年 ${bucket.month} 月`
    }))
    : days.map((day) => {
      const parsed = parseDay(day.date)
      return {
        ...day,
        key: day.date,
        label: parsed ? `${parsed.month}/${parsed.day}` : String(day.date || ''),
        heading: parsed ? `${parsed.year} 年 ${parsed.month} 月 ${parsed.day} 日` : String(day.date || '')
      }
    })
  const max = items.reduce((highest, item) => Math.max(highest, toNumber(item[metric])), 0)
  return items.map((item) => {
    const value = toNumber(item[metric])
    return {
      key: item.key,
      label: item.label,
      value,
      ratio: max > 0 ? value / max : 0,
      title: `${item.heading}：${usageSummary(item)}`
    }
  })
}

/** 柱状图底下稀疏刻度：柱多时只每隔几根标一次，首尾必标 */
export function tickEvery(barCount) {
  if (barCount <= 10) return 1
  if (barCount <= 31) return 5
  return Math.ceil(barCount / 8)
}

export function showTick(index, barCount) {
  const every = tickEvery(barCount)
  return index === 0 || index === barCount - 1 || (index % every === 0 && barCount - 1 - index >= every / 2)
}
