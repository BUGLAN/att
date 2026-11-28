// 设置页快捷键纯逻辑（src/components/settings/settingsHotkeys.mjs）单元测试。
// 覆盖：六个快捷键的撞键判定（与后端 apply_settings_update 同规则：两边都启用才算撞），
// 以及录音 / 显示主窗口 / 语音改写快捷键的录入即启用、清空、恢复默认与首页提示。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  HOTKEY_TARGETS,
  captureHotkey,
  clearHotkey,
  findHotkeyConflict,
  isHotkeyEnabled,
  recordHotkeySummary,
  resetHotkey,
  sameHotkey,
  HOTKEY_CAPTURE_ATTR,
  hotkeyCaptureFieldOf,
  hotkeyCaptureKeyAction
} from '../src/components/settings/settingsHotkeys.mjs'

const RECORD = { modifiers: ['cmd', 'shift'], key: 'r' }
const PANEL = { modifiers: ['ctrl', 'alt'], key: 'v' }
const REWRITE = { modifiers: ['alt'], key: 'a' }
const TOGGLE = { modifiers: ['cmd', 'shift'], key: 't', display: '⌘⇧T' }
const SHOW_WINDOW = { modifiers: ['ctrl', 'shift'], key: 'm', display: '⌃⇧M' }
const VOICE = { modifiers: ['ctrl', 'shift'], key: 'y', display: '⌃⇧Y' }
const CYCLE = { modifiers: ['ctrl', 'alt'], key: 'p', display: '⌃⌥P' }

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
    show_main_window_hotkey: SHOW_WINDOW,
    show_main_window_enabled: false,
    voice_rewrite_hotkey: VOICE,
    voice_rewrite_enabled: false,
    cycle_preset_hotkey: CYCLE,
    cycle_preset_enabled: false,
    ...overrides
  }
}

test('快捷键清单：按住录音 / 点按录音 / 剪贴板悬浮框 / 优化选中文本 / 显示主窗口 / 语音改写 / 切换优化预设', () => {
  assert.deepEqual(
    HOTKEY_TARGETS.map((target) => target.field),
    [
      'hotkey',
      'record_toggle_hotkey',
      'clipboard_panel_hotkey',
      'rewrite_hotkey',
      'show_main_window_hotkey',
      'voice_rewrite_hotkey',
      'cycle_preset_hotkey'
    ]
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
  assert.deepEqual(findHotkeyConflict(form({ voice_rewrite_enabled: true }), 'hotkey', VOICE), {
    field: 'voice_rewrite_hotkey',
    label: '语音改写选中文本'
  })
})

test('撞键：任一边关掉就不算撞（关掉的快捷键不注册）', () => {
  assert.equal(findHotkeyConflict(form({ clipboard_panel_enabled: false }), 'rewrite_hotkey', PANEL), null)
  assert.equal(findHotkeyConflict(form({ rewrite_enabled: false }), 'rewrite_hotkey', PANEL), null)
  assert.equal(findHotkeyConflict(form({ rewrite_enabled: false }), 'hotkey', REWRITE), null)
  assert.ok(findHotkeyConflict(form({ rewrite_enabled: undefined }), 'hotkey', REWRITE), '缺字段按开启处理')
  assert.equal(findHotkeyConflict(form(), 'hotkey', VOICE), null, '语音改写默认关闭，不算撞键')
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

test('清空：录音、显示主窗口与语音改写快捷键可清空，其余用各自的开关', () => {
  assert.deepEqual(clearHotkey('record_toggle_hotkey'), { record_toggle_enabled: false })
  assert.deepEqual(clearHotkey('show_main_window_hotkey'), { show_main_window_enabled: false })
  assert.deepEqual(clearHotkey('voice_rewrite_hotkey'), { voice_rewrite_enabled: false })
  assert.equal(clearHotkey('rewrite_hotkey'), null)
  assert.equal(clearHotkey('clipboard_panel_hotkey'), null)
})

test('显示主窗口：默认为空，录入即启用，恢复默认清空', () => {
  assert.equal(isHotkeyEnabled(form(), 'show_main_window_hotkey'), false)
  const captured = captureHotkey(form(), 'show_main_window_hotkey', SHOW_WINDOW)
  assert.deepEqual(captured, { patch: { show_main_window_hotkey: SHOW_WINDOW, show_main_window_enabled: true } })
  const enabled = { ...form(), ...captured.patch }
  assert.ok(isHotkeyEnabled(enabled, 'show_main_window_hotkey'))

  assert.deepEqual(resetHotkey(enabled, 'show_main_window_hotkey', RECORD), {
    patch: { show_main_window_enabled: false }
  })
  assert.equal(isHotkeyEnabled({ ...enabled, ...resetHotkey(enabled, 'show_main_window_hotkey', RECORD).patch }, 'show_main_window_hotkey'), false)

  // 其余快捷键恢复默认仍然回到平台默认组合（并保持启用）
  assert.deepEqual(resetHotkey(form(), 'hotkey', RECORD), { patch: { hotkey: RECORD, hotkey_enabled: true } })
  assert.deepEqual(resetHotkey(form(), 'missing', RECORD), { error: '未知的快捷键' })
})

test('显示主窗口：和已启用的快捷键撞键时拒绝', () => {
  const sameAsRewrite = form({ show_main_window_hotkey: REWRITE, show_main_window_enabled: true })
  assert.equal(
    captureHotkey(sameAsRewrite, 'show_main_window_hotkey', { ...REWRITE, display: '⌥A' }).error,
    '⌥A 已用作优化选中文本快捷键，请换一个'
  )
  const otherRewrite = form({ rewrite_hotkey: { modifiers: ['alt'], key: 'x' }, show_main_window_hotkey: REWRITE, show_main_window_enabled: true })
  assert.deepEqual(findHotkeyConflict(otherRewrite, 'clipboard_panel_hotkey', REWRITE), {
    field: 'show_main_window_hotkey',
    label: '显示主窗口'
  })
  assert.equal(
    findHotkeyConflict(form(), 'rewrite_hotkey', SHOW_WINDOW),
    null,
    '显示主窗口关掉时不参与撞键判定'
  )
})

test('语音改写：默认为空，录入即启用，清空只关开关', () => {
  assert.equal(isHotkeyEnabled(form(), 'voice_rewrite_hotkey'), false)
  assert.deepEqual(captureHotkey(form(), 'voice_rewrite_hotkey', VOICE), {
    patch: { voice_rewrite_hotkey: VOICE, voice_rewrite_enabled: true }
  })
  assert.deepEqual(clearHotkey('voice_rewrite_hotkey'), { voice_rewrite_enabled: false })
  assert.deepEqual(resetHotkey({ ...form(), voice_rewrite_enabled: true }, 'voice_rewrite_hotkey', RECORD), {
    patch: { voice_rewrite_enabled: false }
  })
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

test('切换优化预设：默认为空，录入即启用，清空只关开关，恢复默认清空；参与撞键判定', () => {
  assert.equal(isHotkeyEnabled(form(), 'cycle_preset_hotkey'), false)
  assert.deepEqual(captureHotkey(form(), 'cycle_preset_hotkey', CYCLE), {
    patch: { cycle_preset_hotkey: CYCLE, cycle_preset_enabled: true }
  })
  assert.deepEqual(clearHotkey('cycle_preset_hotkey'), { cycle_preset_enabled: false })
  assert.deepEqual(resetHotkey(form({ cycle_preset_enabled: true }), 'cycle_preset_hotkey', RECORD), {
    patch: { cycle_preset_enabled: false }
  })
  assert.equal(findHotkeyConflict(form(), 'hotkey', CYCLE), null, '默认关闭，不算撞键')
  assert.deepEqual(findHotkeyConflict(form({ cycle_preset_enabled: true }), 'rewrite_hotkey', CYCLE), {
    field: 'cycle_preset_hotkey',
    label: '切换优化预设'
  })
})

test('录入生命周期：焦点离开录入框就退出录入并放行按键，Esc 退出，焦点在框上才录', () => {
  // 没在录入：一律不管
  assert.equal(hotkeyCaptureKeyAction({ target: null, focusedField: null, key: 'v' }), 'ignore')
  assert.equal(hotkeyCaptureKeyAction({ target: null, focusedField: 'hotkey', key: 'v' }), 'ignore')
  // 点了「按住录音」录入框后切到别的页面 / 别的输入框：⌘V 等按键放行，不能被录成快捷键
  assert.equal(hotkeyCaptureKeyAction({ target: 'hotkey', focusedField: null, key: 'v' }), 'release')
  assert.equal(hotkeyCaptureKeyAction({ target: 'hotkey', focusedField: 'rewrite_hotkey', key: 'v' }), 'release')
  // 焦点在正在录入的框上
  assert.equal(hotkeyCaptureKeyAction({ target: 'hotkey', focusedField: 'hotkey', key: 'Escape' }), 'cancel')
  assert.equal(hotkeyCaptureKeyAction({ target: 'hotkey', focusedField: 'hotkey', key: 'r' }), 'capture')
  assert.equal(hotkeyCaptureKeyAction({ target: 'hotkey', focusedField: 'hotkey', key: 'Meta' }), 'capture')
})

test('录入框标记：按属性认出是哪个快捷键的录入框', () => {
  const element = (attrs) => ({ getAttribute: (name) => attrs[name] ?? null })
  assert.equal(HOTKEY_CAPTURE_ATTR, 'data-hotkey-capture')
  assert.equal(hotkeyCaptureFieldOf(element({ 'data-hotkey-capture': 'rewrite_hotkey' })), 'rewrite_hotkey')
  assert.equal(hotkeyCaptureFieldOf(element({})), null)
  assert.equal(hotkeyCaptureFieldOf(null), null)
  assert.equal(hotkeyCaptureFieldOf({}), null, 'document.activeElement 可能是 body 之外的任意节点')
})
