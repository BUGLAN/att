// 设置页二级导航模型（src/components/settings/settingsNav.mjs）单元测试。
// 覆盖：分组顺序、未知页回落、搜索按标题/描述/关键词命中且去掉空分组。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  DEFAULT_SECTION_ID,
  SETTINGS_SECTIONS,
  buildNavGroups,
  findSection,
  resolveSectionId,
  sectionMatches
} from '../src/components/settings/settingsNav.mjs'

test('导航分组：四组按固定顺序，每页都归属某组且 id 唯一', () => {
  const groups = buildNavGroups()
  assert.deepEqual(groups.map((group) => group.label), ['通用', '语音识别', 'AI', '系统'])
  const ids = groups.flatMap((group) => group.items.map((item) => item.id))
  assert.equal(ids.length, SETTINGS_SECTIONS.length)
  assert.equal(new Set(ids).size, ids.length)
})

test('页面解析：未知 id 回落默认页', () => {
  assert.equal(resolveSectionId('hotkey'), 'hotkey')
  assert.equal(resolveSectionId('nope'), DEFAULT_SECTION_ID)
  assert.equal(resolveSectionId(undefined), DEFAULT_SECTION_ID)
  assert.equal(findSection('nope'), null)
})

test('搜索：关键词大小写不敏感，只保留命中页并去掉空分组', () => {
  const groups = buildNavGroups('  Token ')
  assert.deepEqual(groups.map((group) => group.label), ['语音识别'])
  assert.equal(groups[0].items[0].id, 'asr')

  const hotkey = buildNavGroups('热键')
  assert.equal(hotkey.length, 1)
  assert.equal(hotkey[0].items[0].id, 'hotkey')

  const clipboard = buildNavGroups('悬浮框')
  assert.deepEqual(clipboard.flatMap((group) => group.items.map((item) => item.id)), ['clipboard'])

  assert.deepEqual(buildNavGroups('完全不存在的设置'), [])
  assert.ok(sectionMatches(findSection('permissions'), '辅助功能'))
})
