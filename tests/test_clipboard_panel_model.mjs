// 剪贴板悬浮框纯逻辑（src/components/clipboardPanel/clipboardPanelModel.mjs）单元测试。
// 覆盖：分段查询参数、分页响应兼容、追加去重、何时取下一段、键盘意图（含输入法组字）、回车行为、文件名 / 链接拆分、相对时间。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  PANEL_FILTERS,
  PANEL_PAGE_SIZE,
  appendPage,
  buildPanelQuery,
  cycleFilter,
  moveIndex,
  normalizePage,
  panelFileName,
  panelKeyIntent,
  relativeTime,
  resolveCommitAction,
  shouldLoadMore,
  splitLink
} from '../src/components/clipboardPanel/clipboardPanelModel.mjs'

test('分段查询：文本含长文本、收藏走 favorite_only、搜索去空白，总带 with_total', () => {
  assert.deepEqual(buildPanelQuery(), { offset: 0, limit: PANEL_PAGE_SIZE, with_total: true })
  assert.equal(buildPanelQuery({ filter: 'text' }).item_type, 'text,code')
  assert.equal(buildPanelQuery({ filter: 'favorite' }).favorite_only, true)
  assert.equal(buildPanelQuery({ filter: 'image', offset: 30 }).item_type, 'image')
  assert.equal(buildPanelQuery({ filter: 'image', offset: 30 }).offset, 30)
  assert.equal(buildPanelQuery({ query: '  docs ' }).query, 'docs')
  assert.equal('query' in buildPanelQuery({ query: '   ' }), false)
})

test('分页响应：{items,total} 原样取，旧的数组响应当作只有一段', () => {
  assert.deepEqual(normalizePage({ items: [{ id: 1 }], total: 40 }), { items: [{ id: 1 }], total: 40 })
  assert.deepEqual(normalizePage([{ id: 1 }, { id: 2 }]), { items: [{ id: 1 }, { id: 2 }], total: 2 })
  assert.deepEqual(normalizePage(null), { items: [], total: 0 })
})

test('追加下一段：新内容让条目后移时，重复的 id 不再出现', () => {
  const merged = appendPage([{ id: 'a' }, { id: 'b' }], [{ id: 'b' }, { id: 'c' }])
  assert.deepEqual(merged.map((i) => i.id), ['a', 'b', 'c'])
})

test('何时取下一段：加载中 / 已取完不取；选中接近末尾或滚到离末尾半屏内才取', () => {
  const base = { loaded: 30, total: 100, scrollLeft: 0, clientWidth: 1000, scrollWidth: 6000 }
  assert.equal(shouldLoadMore({ ...base, index: 5 }), false)
  assert.equal(shouldLoadMore({ ...base, index: 22 }), true)
  assert.equal(shouldLoadMore({ ...base, index: 5, scrollLeft: 4600 }), true)
  assert.equal(shouldLoadMore({ ...base, index: 29, loading: true }), false)
  assert.equal(shouldLoadMore({ ...base, index: 29, loaded: 100 }), false)
})

test('选择与筛选循环', () => {
  assert.equal(moveIndex(0, -1, 4), 3)
  assert.equal(moveIndex(3, 1, 4), 0)
  assert.equal(moveIndex(2, 1, 0), 0)
  assert.equal(cycleFilter('all', -1), PANEL_FILTERS.at(-1).value)
  assert.equal(cycleFilter('link', 1), 'all')
})

test('键盘意图：←→ 选卡片；输入法组字时不处理；Esc 先清搜索再收起', () => {
  assert.deepEqual(panelKeyIntent({ key: 'ArrowLeft' }), { type: 'move', delta: -1 })
  assert.deepEqual(panelKeyIntent({ key: 'ArrowRight' }), { type: 'move', delta: 1 })
  assert.equal(panelKeyIntent({ key: 'Enter', isComposing: true }), null)
  assert.deepEqual(panelKeyIntent({ key: 'Enter' }), { type: 'commit', alternate: false })
  assert.deepEqual(panelKeyIntent({ key: 'Enter', metaKey: true }), { type: 'commit', alternate: true })
  assert.deepEqual(panelKeyIntent({ key: '3', ctrlKey: true }), { type: 'quick', index: 2 })
  assert.equal(panelKeyIntent({ key: '3' }), null, '不带修饰键的数字是在搜索框里打字')
  assert.deepEqual(panelKeyIntent({ key: 'Tab', shiftKey: true }), { type: 'filter', delta: -1 })
  assert.deepEqual(panelKeyIntent({ key: 'Escape' }, { hasQuery: true }), { type: 'clear-query' })
  assert.deepEqual(panelKeyIntent({ key: 'Escape' }), { type: 'close' })
})

test('回车行为：默认粘贴，⌘↵ 换成另一种；设置为仅复制时反过来', () => {
  assert.equal(resolveCommitAction('paste'), 'paste')
  assert.equal(resolveCommitAction('paste', true), 'copy')
  assert.equal(resolveCommitAction('copy'), 'copy')
  assert.equal(resolveCommitAction('copy', true), 'paste')
  assert.equal(resolveCommitAction(undefined), 'paste')
})

test('文件名、链接拆分与相对时间', () => {
  assert.equal(panelFileName({ item_type: 'file', content: '/a/b/报告.pdf' }), '报告.pdf')
  assert.equal(panelFileName({ item_type: 'file', content: 'C:\\docs\\a.txt' }), 'a.txt')
  assert.equal(panelFileName({ item_type: 'image', content: 'x.png' }, '截屏'), '截屏')
  assert.deepEqual(splitLink('https://example.com/Docs?a=1'), { host: 'example.com', rest: '/Docs?a=1' })
  assert.deepEqual(splitLink('https://example.com/'), { host: 'example.com', rest: '' })
  assert.deepEqual(splitLink('不是链接'), { host: '不是链接', rest: '' })
  const now = new Date('2026-11-01T12:00:00').getTime()
  assert.equal(relativeTime('2026-11-01T11:59:40', now), '刚刚')
  assert.equal(relativeTime('2026-11-01T11:30:00', now), '30 分钟前')
  assert.equal(relativeTime('2026-10-31T09:00:00', now), '昨天')
  assert.equal(relativeTime('2026-10-20T09:00:00', now), '10-20')
  assert.equal(relativeTime(''), '')
})
