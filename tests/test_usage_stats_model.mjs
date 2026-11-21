// 记录集「用量统计」展示模型（src/components/records/usageStatsModel.mjs）单元测试。
// 覆盖：数字与时长格式、一行小统计、逐日 / 按月柱状图、稀疏刻度。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  formatMetric,
  formatUsageDuration,
  formatUsageNumber,
  MONTH_BUCKET_THRESHOLD_DAYS,
  showTick,
  usageBars,
  usageSummary
} from '../src/components/records/usageStatsModel.mjs'

test('数字：千分位，一万以上用「万」', () => {
  assert.equal(formatUsageNumber(0), '0')
  assert.equal(formatUsageNumber(1204), '1,204')
  assert.equal(formatUsageNumber(9999), '9,999')
  assert.equal(formatUsageNumber(10000), '1 万')
  assert.equal(formatUsageNumber(123456), '12.3 万')
  assert.equal(formatUsageNumber(undefined), '0')
  assert.equal(formatUsageNumber(-5), '0')
})

test('时长：秒 / 分钟 / 小时', () => {
  assert.equal(formatUsageDuration(0), '0 秒')
  assert.equal(formatUsageDuration(44.6), '45 秒')
  assert.equal(formatUsageDuration(60), '1 分钟')
  assert.equal(formatUsageDuration(12 * 60 + 59), '12 分钟')
  assert.equal(formatUsageDuration(3600), '1 小时')
  assert.equal(formatUsageDuration(3600 + 5 * 60), '1 小时 5 分')
  assert.equal(formatUsageDuration(null), '0 秒')
})

test('一行小统计：四项用量，没有记录时「暂无记录」', () => {
  assert.equal(
    usageSummary({ count: 3, duration: 750, chars: 1204, tokens: 860 }),
    '3 条 · 12 分钟 · 1,204 字 · 860 token'
  )
  assert.equal(usageSummary({ count: 0, duration: 0, chars: 0, tokens: 0 }), '暂无记录')
  assert.equal(usageSummary(undefined), '暂无记录')
  // 旧记录没有 token：后端按 0 计
  assert.equal(usageSummary({ count: 1, duration: 5, chars: 2, tokens: 0 }), '1 条 · 5 秒 · 2 字 · 0 token')
  assert.equal(formatMetric('tokens', 42), '42 token')
})

test('逐日柱状图：按选中指标算相对高度，悬停提示带日期和四项用量', () => {
  const stats = {
    days: [
      { date: '2026-11-20', count: 1, duration: 10, chars: 5, tokens: 0 },
      { date: '2026-11-21', count: 4, duration: 30, chars: 20, tokens: 100 }
    ]
  }
  const bars = usageBars(stats, 'count')
  assert.deepEqual(bars.map((bar) => [bar.key, bar.label, bar.value, bar.ratio]), [
    ['2026-11-20', '11/20', 1, 0.25],
    ['2026-11-21', '11/21', 4, 1]
  ])
  assert.equal(bars[1].title, '2026 年 11 月 21 日：4 条 · 30 秒 · 20 字 · 100 token')
  assert.deepEqual(usageBars(stats, 'tokens').map((bar) => bar.ratio), [0, 1])
  assert.deepEqual(usageBars({ days: [{ date: '2026-11-21', count: 0 }] }, 'count').map((bar) => bar.ratio), [0], '全为 0 时不除以 0')
  assert.deepEqual(usageBars(null), [])
})

test('跨度很长时按月合并成一根柱', () => {
  const days = []
  const start = new Date(Date.UTC(2026, 6, 1))
  for (let i = 0; i <= MONTH_BUCKET_THRESHOLD_DAYS; i += 1) {
    const date = new Date(start.getTime() + i * 86400000).toISOString().slice(0, 10)
    days.push({ date, count: 1, duration: 60, chars: 10, tokens: 5 })
  }
  const bars = usageBars({ days }, 'count')
  assert.deepEqual(bars.map((bar) => bar.label), ['7 月', '8 月', '9 月', '10 月'])
  assert.equal(bars[0].value, 31)
  assert.equal(bars[0].title, '2026 年 7 月：31 条 · 31 分钟 · 310 字 · 155 token')
})

test('稀疏刻度：柱少全标，柱多隔几根标一次，首尾必标', () => {
  assert.ok([0, 1, 2, 3, 4, 5, 6].every((index) => showTick(index, 7)))
  const marked = Array.from({ length: 30 }, (_, index) => index).filter((index) => showTick(index, 30))
  assert.deepEqual(marked, [0, 5, 10, 15, 20, 25, 29])
})
