// 录音记录列表展示模型（src/components/records/recordListModel.mjs）单元测试。
// 覆盖：按天分组与标题、时刻 / 时长格式、行内状态只在需要时出现。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  dayLabel,
  formatClock,
  formatRecordDuration,
  groupRecordsByDay,
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
