// 设置页快捷键纯逻辑（src/components/settings/settingsHotkeys.mjs）单元测试。
// 覆盖：四个快捷键的撞键判定（与后端 apply_settings_update 同规则：两边都启用才算撞），
// 以及按住 / 点按两种录音快捷键的录入即启用、清空、同键二选一与首页提示。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  HOTKEY_TARGETS,
  captureHotkey,
  clearHotkey,
  findHotkeyConflict,
  isHotkeyEnabled,
  recordHotkeySummary,
  sameHotkey
} from '../src/components/settings/settingsHotkeys.mjs'

const RECORD = { modifiers: ['cmd', 'shift'], key: 'r' }
const PANEL = { modifiers: ['ctrl', 'alt'], key: 'v' }
const REWRITE = { modifiers: ['alt'], key: 'a' }
const TOGGLE = { modifiers: ['cmd', 'shift'], key: 't', display: '⌘⇧T' }

function form(overrides = {}) {
  return {
    hotkey: RECORD,
    hotkey_enabled: true,
    record_toggle_hotkey: RECORD,
    record_toggle_enabled: false,
    clipboard_panel_hotkey: PANEL,
    clipboard_panel_enabled: true,
    rewrite_hotkey: REWRITE,
    rewrite_enabled: true,
    ...overrides
  }
}

test('快捷键清单：按住录音 / 点按录音 / 剪贴板悬浮框 / 优化选中文本', () => {
  assert.deepEqual(
    HOTKEY_TARGETS.map((target) => target.field),
    ['hotkey', 'record_toggle_hotkey', 'clipboard_panel_hotkey', 'rewrite_hotkey']
  )
})

test('同一组按键：修饰键不看顺序，主键大小写已由调用方归一', () => {
  assert.ok(sameHotkey({ modifiers: ['shift', 'cmd'], key: 'r' }, RECORD))
  assert.ok(!sameHotkey({ modifiers: ['cmd'], key: 'r' }, RECORD))
  assert.ok(!sameHotkey(RECORD, null))
  assert.ok(sameHotkey({ modifiers: ['meta', 'alt'], key: null }, { modifiers: ['alt', 'meta'] }), '无主键组合')
})

test('撞键：报出撞上的是哪个快捷键', () => {
  assert.deepEqual(findHotkeyConflict(form(), 'rewrite_hotkey', PANEL), { field: 'clipboard_panel_hotkey', label: '剪贴板悬浮框' })
  assert.deepEqual(findHotkeyConflict(form(), 'hotkey', REWRITE), { field: 'rewrite_hotkey', label: '优化选中文本' })
  assert.deepEqual(findHotkeyConflict(form(), 'clipboard_panel_hotkey', RECORD), { field: 'hotkey', label: '按住录音' })
  assert.equal(findHotkeyConflict(form(), 'rewrite_hotkey', REWRITE), null, '和自己相同不算撞')
  assert.equal(findHotkeyConflict(form(), 'rewrite_hotkey', { modifiers: ['ctrl', 'alt'], key: 'p' }), null)
})

test('撞键：任一边关掉就不算撞（关掉的快捷键不注册）', () => {
  assert.equal(findHotkeyConflict(form({ clipboard_panel_enabled: false }), 'rewrite_hotkey', PANEL), null)
  assert.equal(findHotkeyConflict(form({ rewrite_enabled: false }), 'rewrite_hotkey', PANEL), null)
  assert.equal(findHotkeyConflict(form({ rewrite_enabled: false }), 'hotkey', REWRITE), null)
  assert.ok(findHotkeyConflict(form({ rewrite_enabled: undefined }), 'hotkey', REWRITE), '缺字段按开启处理')
})

test('点按录音：默认为空，录入即启用；与按住录音分键时两种都可用', () => {
  assert.equal(isHotkeyEnabled(form(), 'record_toggle_hotkey'), false)
  const result = captureHotkey(form(), 'record_toggle_hotkey', TOGGLE)
  assert.deepEqual(result, { patch: { record_toggle_hotkey: TOGGLE, record_toggle_enabled: true } })
  const next = { ...form(), ...result.patch }
  assert.ok(isHotkeyEnabled(next, 'hotkey') && isHotkeyEnabled(next, 'record_toggle_hotkey'))
})

test('同一个键只能启用一种录音方式：先清空另一个才能录', () => {
  const same = { ...RECORD, display: '⌘⇧R' }
  const blocked = captureHotkey(form(), 'record_toggle_hotkey', same)
  assert.match(blocked.error, /同一个键只能用一种录音方式/)
  assert.match(blocked.error, /先清空「按住录音」/)
  assert.equal(blocked.patch, undefined, '撞键时不改表单')

  const cleared = { ...form(), ...clearHotkey('hotkey') }
  assert.equal(cleared.hotkey_enabled, false)
  assert.deepEqual(cleared.hotkey, RECORD, '清空只关开关，组合保留')
  assert.deepEqual(captureHotkey(cleared, 'record_toggle_hotkey', same).patch, {
    record_toggle_hotkey: same,
    record_toggle_enabled: true
  })

  // 反过来：点按录音占着这个键时，给按住录音恢复同一个键也被拦下
  const toggleOnly = { ...cleared, record_toggle_enabled: true }
  assert.match(captureHotkey(toggleOnly, 'hotkey', same).error, /先清空「点按录音」/)
})

test('清空：只有两种录音快捷键可清空，其余用各自的开关', () => {
  assert.deepEqual(clearHotkey('record_toggle_hotkey'), { record_toggle_enabled: false })
  assert.equal(clearHotkey('rewrite_hotkey'), null)
  assert.equal(clearHotkey('clipboard_panel_hotkey'), null)
})

test('录入非录音快捷键：撞上已启用的点按录音也拒绝，提示换一个', () => {
  const toggleOn = form({ record_toggle_hotkey: TOGGLE, record_toggle_enabled: true })
  assert.equal(captureHotkey(toggleOn, 'rewrite_hotkey', TOGGLE).error, '⌘⇧T 已用作点按录音快捷键，请换一个')
  assert.deepEqual(captureHotkey(toggleOn, 'rewrite_hotkey', REWRITE).patch, { rewrite_hotkey: REWRITE })
})

test('首页待命提示：列出启用的录音快捷键，为空的不列', () => {
  const hold = { display: '⌘⇧R' }
  assert.equal(recordHotkeySummary({ hotkey: hold, hotkey_enabled: true }), '按住 ⌘⇧R')
  assert.equal(recordHotkeySummary({ hotkey: hold }), '按住 ⌘⇧R', '旧视图没有开关字段 = 按住录音启用')
  assert.equal(
    recordHotkeySummary({ hotkey: hold, record_toggle_hotkey: { display: '⌘⇧T' }, record_toggle_enabled: true }),
    '按住 ⌘⇧R · 点按 ⌘⇧T'
  )
  assert.equal(
    recordHotkeySummary({ hotkey: hold, hotkey_enabled: false, record_toggle_hotkey: hold, record_toggle_enabled: true }),
    '点按 ⌘⇧R'
  )
  assert.equal(recordHotkeySummary({ hotkey: hold, hotkey_enabled: false }), '')
  assert.equal(recordHotkeySummary(undefined), '')
})
