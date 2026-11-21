// 剪贴板历史容量选项（src/components/settings/clipboardLimits.mjs）单元测试。
// 覆盖：默认不限、预设选项与自定义值、改紧才确认、确认文案写明置顶收藏不受影响。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  isTighter,
  maxItemsOptions,
  normalizeLimit,
  retentionOptions,
  tightenConfirm
} from '../src/components/settings/clipboardLimits.mjs'

test('设置值归一：缺省 / 非法 / 负数都按不限', () => {
  assert.equal(normalizeLimit(undefined), 0)
  assert.equal(normalizeLimit('abc'), 0)
  assert.equal(normalizeLimit(-5), 0)
  assert.equal(normalizeLimit('50'), 50)
  assert.equal(normalizeLimit(29.6), 30)
})

test('下拉选项：不限 / 永久在首项，手改过的值也列出来', () => {
  const items = maxItemsOptions(0)
  assert.deepEqual(items[0], { value: 0, label: '不限' })
  assert.ok(items.some((option) => option.value === 50 && option.label === '最近 50 条'))
  assert.ok(maxItemsOptions(75).some((option) => option.value === 75), '当前值不在预设里也要能显示')
  assert.deepEqual(retentionOptions(0)[0], { value: 0, label: '永久保留' })
  assert.ok(retentionOptions(0).some((option) => option.label === '30 天内'))
})

test('改紧上限才需要确认（会立即删除）', () => {
  assert.equal(isTighter(0, 50), true, '从不限改成有上限')
  assert.equal(isTighter(500, 100), true, '上限变小')
  assert.equal(isTighter(100, 500), false, '放宽不删')
  assert.equal(isTighter(100, 0), false, '改回不限不删')
  assert.equal(isTighter(100, 100), false)
})

test('确认文案：说明会立即删除、置顶收藏不受影响', () => {
  const count = tightenConfirm('items', 50)
  assert.equal(count.title, '只保留最近 50 条剪贴板记录？')
  assert.match(count.description, /立即删除/)
  assert.match(count.description, /置顶和收藏的记录不受影响/)
  assert.equal(tightenConfirm('days', 30).title, '只保留 30 天内的剪贴板记录？')
})
