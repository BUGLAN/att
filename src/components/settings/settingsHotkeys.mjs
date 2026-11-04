// 设置页全局快捷键的纯模型：有哪些可录入的快捷键、各自的开关字段，以及撞键判定
// （不依赖 Vue，供 tests/test_settings_hotkeys.mjs 锁定；规则与 Rust 侧 apply_settings_update 的撞键校验一致）。

/**
 * 可在设置页录入的快捷键。field：表单 / settings 里旧版 `{modifiers, key, display}` 形状的字段；
 * enabledField：开关字段（null = 常开）。
 */
export const HOTKEY_TARGETS = Object.freeze([
  { field: 'hotkey', label: '录音', enabledField: null },
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
