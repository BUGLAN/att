// 设置页快捷键纯逻辑（src/components/settings/settingsHotkeys.mjs）单元测试。
// 覆盖：三个快捷键的撞键判定（与后端 apply_settings_update 同规则：两边都启用才算撞）。
import test from 'node:test'
import assert from 'node:assert/strict'

import { HOTKEY_TARGETS, findHotkeyConflict, sameHotkey } from '../src/components/settings/settingsHotkeys.mjs'

const RECORD = { modifiers: ['cmd', 'shift'], key: 'r' }
const PANEL = { modifiers: ['ctrl', 'alt'], key: 'v' }
const REWRITE = { modifiers: ['alt'], key: 'a' }

function form(overrides = {}) {
  return {
    hotkey: RECORD,
    clipboard_panel_hotkey: PANEL,
    clipboard_panel_enabled: true,
    rewrite_hotkey: REWRITE,
    rewrite_enabled: true,
    ...overrides
  }
}

test('快捷键清单：录音 / 剪贴板悬浮框 / 优化选中文本', () => {
  assert.deepEqual(HOTKEY_TARGETS.map((target) => target.field), ['hotkey', 'clipboard_panel_hotkey', 'rewrite_hotkey'])
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
  assert.deepEqual(findHotkeyConflict(form(), 'clipboard_panel_hotkey', RECORD), { field: 'hotkey', label: '录音' })
  assert.equal(findHotkeyConflict(form(), 'rewrite_hotkey', REWRITE), null, '和自己相同不算撞')
  assert.equal(findHotkeyConflict(form(), 'rewrite_hotkey', { modifiers: ['ctrl', 'alt'], key: 'p' }), null)
})

test('撞键：任一边关掉就不算撞（关掉的快捷键不注册）', () => {
  assert.equal(findHotkeyConflict(form({ clipboard_panel_enabled: false }), 'rewrite_hotkey', PANEL), null)
  assert.equal(findHotkeyConflict(form({ rewrite_enabled: false }), 'rewrite_hotkey', PANEL), null)
  assert.equal(findHotkeyConflict(form({ rewrite_enabled: false }), 'hotkey', REWRITE), null)
  assert.ok(findHotkeyConflict(form({ rewrite_enabled: undefined }), 'hotkey', REWRITE), '缺字段按开启处理')
})
