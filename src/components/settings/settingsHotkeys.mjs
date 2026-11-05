// 设置页全局快捷键的纯模型：有哪些可录入的快捷键、各自的开关字段，以及撞键判定
// （不依赖 Vue，供 tests/test_settings_hotkeys.mjs 锁定；规则与 Rust 侧 apply_settings_update 的撞键校验一致）。

/**
 * 可在设置页录入的快捷键。field：表单 / settings 里旧版 `{modifiers, key, display}` 形状的字段；
 * enabledField：开关字段（null = 常开）；clearable：没有单独的开关，开关为否即「快捷键为空」，
 * 录入新键就启用（两种录音方式都是这样）。
 */
export const HOTKEY_TARGETS = Object.freeze([
  { field: 'hotkey', label: '按住录音', enabledField: 'hotkey_enabled', clearable: true },
  { field: 'record_toggle_hotkey', label: '点按录音', enabledField: 'record_toggle_enabled', clearable: true },
  { field: 'clipboard_panel_hotkey', label: '剪贴板悬浮框', enabledField: 'clipboard_panel_enabled' },
  { field: 'rewrite_hotkey', label: '优化选中文本', enabledField: 'rewrite_enabled' }
])

function sortedModifiers(modifiers = []) {
  return [...modifiers].map((modifier) => String(modifier)).sort().join()
}

/** 两个快捷键是否是同一组按键（只看修饰键集合与主键）。 */
export function sameHotkey(left, right) {
  return Boolean(left && right) &&
    (left.key || null) === (right.key || null) &&
    sortedModifiers(left.modifiers) === sortedModifiers(right.modifiers)
}

function isEnabled(form, target) {
  return !target.enabledField || form?.[target.enabledField] !== false
}

/**
 * 把 `next` 录成 `field` 的快捷键时，和哪个已启用的快捷键撞键；不撞返回 null。
 * 两边都启用才算撞（关掉的快捷键不注册，允许同键）。
 * @returns {{ field: string, label: string } | null}
 */
export function findHotkeyConflict(form, field, next) {
  const self = HOTKEY_TARGETS.find((target) => target.field === field)
  if (!self || !isEnabled(form, self)) {
    return null
  }
  const other = HOTKEY_TARGETS.find((target) =>
    target.field !== field && isEnabled(form, target) && sameHotkey(next, form?.[target.field])
  )
  return other ? { field: other.field, label: other.label } : null
}

const RECORD_FIELDS = new Set(['hotkey', 'record_toggle_hotkey'])

/** 快捷键是否已启用（可清空的快捷键：未启用即为空）。 */
export function isHotkeyEnabled(form, field) {
  const target = HOTKEY_TARGETS.find((item) => item.field === field)
  return Boolean(target) && isEnabled(form, target)
}

/**
 * 把 `next` 录成 `field` 的快捷键：可清空的快捷键录入即启用。
 * 撞上已启用的快捷键时返回 `{ error }`（不改表单），否则返回要合并进表单的 `{ patch }`。
 * 两种录音方式撞键时提示先清空另一个：同一个键只能用一种录音方式。
 */
export function captureHotkey(form, field, next) {
  const self = HOTKEY_TARGETS.find((target) => target.field === field)
  const enabling = self?.clearable ? { [self.enabledField]: true } : {}
  const conflict = findHotkeyConflict({ ...form, ...enabling }, field, next)
  if (conflict) {
    const display = next?.display || '这个组合'
    if (RECORD_FIELDS.has(field) && RECORD_FIELDS.has(conflict.field)) {
      return { error: `${display} 已用作${conflict.label}快捷键：同一个键只能用一种录音方式，请先清空「${conflict.label}」` }
    }
    return { error: `${display} 已用作${conflict.label}快捷键，请换一个` }
  }
  return { patch: { [field]: next, ...enabling } }
}

/** 清空可清空的快捷键（关掉开关，组合保留，便于之后恢复）；不可清空的返回 null。 */
export function clearHotkey(field) {
  const target = HOTKEY_TARGETS.find((item) => item.field === field)
  return target?.clearable ? { [target.enabledField]: false } : null
}

/**
 * 首页待命提示里的录音快捷键：`按住 ⌘⇧R · 点按 ⌘⇧T`，只启用一种时只写那一种，都为空返回空串。
 * @param {object} settings GET /api/settings 的视图（hotkey / record_toggle_hotkey 及其开关）
 */
export function recordHotkeySummary(settings) {
  const parts = []
  if (settings?.hotkey_enabled !== false && settings?.hotkey?.display) {
    parts.push(`按住 ${settings.hotkey.display}`)
  }
  if (settings?.record_toggle_enabled === true && settings?.record_toggle_hotkey?.display) {
    parts.push(`点按 ${settings.record_toggle_hotkey.display}`)
  }
  return parts.join(' · ')
}
