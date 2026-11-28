// 设置页全局快捷键的纯模型：有哪些可录入的快捷键、各自的开关字段、撞键判定与恢复默认
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
  { field: 'rewrite_hotkey', label: '优化选中文本', enabledField: 'rewrite_enabled' },
  {
    field: 'show_main_window_hotkey',
    label: '显示主窗口',
    enabledField: 'show_main_window_enabled',
    clearable: true,
    defaultEmpty: true
  },
  // 语音改写默认为空：录入即启用，清空只关开关；按住快捷键说出要求
  {
    field: 'voice_rewrite_hotkey',
    label: '语音改写选中文本',
    enabledField: 'voice_rewrite_enabled',
    clearable: true,
    defaultEmpty: true
  },
  // 切换优化预设默认为空：录入即启用；点按一下切到下一个预设
  {
    field: 'cycle_preset_hotkey',
    label: '切换优化预设',
    enabledField: 'cycle_preset_enabled',
    clearable: true,
    defaultEmpty: true
  }
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
 * 恢复默认：默认为空的快捷键（显示主窗口、语音改写、切换优化预设）关闭开关，其余回到传入的平台默认组合。
 * @returns {{ patch: object } | { error: string }}
 */
export function resetHotkey(form, field, defaultHotkey) {
  const target = HOTKEY_TARGETS.find((item) => item.field === field)
  if (!target) {
    return { error: '未知的快捷键' }
  }
  if (target.defaultEmpty) {
    return { patch: clearHotkey(field) }
  }
  return captureHotkey(form, field, defaultHotkey)
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

/**
 * 快捷键录入框的标记属性：值是正在录入的字段名（HOTKEY_TARGETS 的 field）。
 * 设置页 keydown 监听挂在 window 捕获阶段、设置页 v-show 常驻，必须靠它确认焦点还在录入框上。
 */
export const HOTKEY_CAPTURE_ATTR = 'data-hotkey-capture'

/** 元素是哪个快捷键的录入框（不是录入框返回 null）。 */
export function hotkeyCaptureFieldOf(element) {
  const field = element?.getAttribute?.(HOTKEY_CAPTURE_ATTR)
  return field || null
}

/**
 * 录入状态下一次 keydown 该怎么处理。
 *
 * 背景（真实踩坑）：录入状态原来只在录到组合、清空、恢复默认、切设置分区时复位；
 * 点了录入框再切到「转文字」等页面，所有按键都被拦下，⌘V / Ctrl+V 还会被录成全局快捷键。
 *
 * @param {object} params
 * @param {string | null} params.target 正在录入的字段（null = 没在录入）
 * @param {string | null} params.focusedField 当前焦点所在录入框的字段（见 {@link hotkeyCaptureFieldOf}）
 * @param {string} [params.key] KeyboardEvent.key
 * @returns {'ignore' | 'release' | 'cancel' | 'capture'}
 *   ignore：没在录入，不管；release：焦点已离开录入框，退出录入并放行这次按键；
 *   cancel：按了 Esc，退出录入（拦下这次按键）；capture：按录入处理
 */
export function hotkeyCaptureKeyAction({ target, focusedField, key }) {
  if (!target) {
    return 'ignore'
  }
  if (focusedField !== target) {
    return 'release'
  }
  if (key === 'Escape') {
    return 'cancel'
  }
  return 'capture'
}
