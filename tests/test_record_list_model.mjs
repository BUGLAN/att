// 录音记录列表展示模型（src/components/records/recordListModel.mjs）单元测试。
// 覆盖：按天分组与标题、时刻 / 时长格式、行内状态只在需要时出现。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  DATE_RANGE_OPTIONS,
  STATUS_FILTER_OPTIONS,
  dateRangeFor,
  dayLabel,
  formatClock,
  formatRecordDuration,
  groupRecordsByDay,
  localDateString,
  recordStatusMeta
} from '../src/components/records/recordListModel.mjs'

const now = new Date(2026, 9, 31, 9, 40)

test('分组标题：今天 / 昨天 / 本年月日 / 跨年带年份', () => {
  assert.equal(dayLabel(new Date(2026, 9, 31, 0, 1), now), '今天')
  assert.equal(dayLabel(new Date(2026, 9, 30, 23, 59), now), '昨天')
  assert.equal(dayLabel(new Date(2026, 9, 29, 8), now), '10 月 29 日')
  assert.equal(dayLabel(new Date(2025, 11, 31), now), '2025 年 12 月 31 日')
  assert.equal(dayLabel('not a date', now), '更早')
})

test('按天分组：保持顺序，相邻同日合并', () => {
  const records = [
    { id: 1, created_at: new Date(2026, 9, 31, 9).toISOString() },
    { id: 2, created_at: new Date(2026, 9, 31, 8).toISOString() },
    { id: 3, created_at: new Date(2026, 9, 30, 20).toISOString() },
    { id: 4, created_at: new Date(2026, 9, 28, 20).toISOString() }
  ]
  const groups = groupRecordsByDay(records, now)
  assert.deepEqual(groups.map((group) => group.label), ['今天', '昨天', '10 月 28 日'])
  assert.deepEqual(groups[0].records.map((record) => record.id), [1, 2])
  assert.deepEqual(groupRecordsByDay([], now), [])
})

test('时刻与时长格式', () => {
  assert.equal(formatClock(new Date(2026, 9, 31, 7, 5)), '07:05')
  assert.equal(formatClock(''), '--:--')
  assert.equal(formatRecordDuration(71.6), '1:11')
  assert.equal(formatRecordDuration(8), '0:08')
  assert.equal(formatRecordDuration(undefined), '0:00')
})

test('行内状态：常态不标，进行中 / 失败 / 已编辑才标', () => {
  assert.equal(recordStatusMeta({ status: 'optimized' }), null)
  assert.equal(recordStatusMeta({ status: 'transcribed' }), null)
  assert.equal(recordStatusMeta({ status: 'transcribed' }, { optimizing: true }).text, '优化中')
  assert.equal(recordStatusMeta({ status: 'transcribing' }).text, '转写中')
  assert.equal(recordStatusMeta({ status: 'failed', is_modified: true }).text, '失败')
  assert.equal(recordStatusMeta({ status: 'optimized', is_modified: true }).text, '已编辑')
})

test('行内状态：没听到人声的记录带原因标记', () => {
  const note = '没有听到人声，识别结果未优化、未输入'
  const meta = recordStatusMeta({ status: 'transcribed', error: note })
  assert.equal(meta.text, '未听到人声')
  assert.equal(meta.dot, 'warn')
  assert.equal(meta.title, note)
  assert.equal(recordStatusMeta({ status: 'transcribed', error: '网络错误' }), null)
})

test('日期筛选：预设换算成本地日期范围，自定义可只填一端、颠倒自动对调', () => {
  const at = new Date(2026, 10, 21, 0, 30) // 2026-11-21 00:30，本地时间
  assert.equal(localDateString(at), '2026-11-21')
  assert.deepEqual(dateRangeFor('all', at), { date_from: '', date_to: '' })
  assert.deepEqual(dateRangeFor('today', at), { date_from: '2026-11-21', date_to: '2026-11-21' })
  assert.deepEqual(dateRangeFor('7d', at), { date_from: '2026-11-15', date_to: '2026-11-21' })
  assert.deepEqual(dateRangeFor('30d', at), { date_from: '2026-10-23', date_to: '2026-11-21' })
  assert.deepEqual(dateRangeFor('custom', at, { from: '2026-11-10' }), { date_from: '2026-11-10', date_to: '' })
  assert.deepEqual(dateRangeFor('custom', at, { from: '2026-11-20', to: '2026-11-01' }), {
    date_from: '2026-11-01',
    date_to: '2026-11-20'
  })
  assert.deepEqual(dateRangeFor('custom', at, { from: 'bad' }), { date_from: '', date_to: '' })
  assert.deepEqual(DATE_RANGE_OPTIONS.map((option) => option.value), ['all', 'today', '7d', '30d', 'custom'])
  assert.deepEqual(STATUS_FILTER_OPTIONS.map((option) => option.value), ['', 'transcribed', 'optimized', 'failed'])
})

