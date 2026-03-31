let ioHook = null
let ioHookLoadError = null
let ioHookLoadAttempted = false

const DEFAULT_HOTKEY = {
  modifiers: ['win', 'alt'],
  key: null,
  display: 'Win+Alt'
}

const DEFAULT_SETTINGS = {
  hotkey: DEFAULT_HOTKEY,
  clipboard_overlay_trigger: 'mouse_middle',
  clipboard_overlay_retrigger_action: 'toggle',
  clipboard_overlay_auto_focus_search: true,
  selection_toolbar_enabled: true,
  selection_toolbar_trigger_delay_ms: 220,
  selection_toolbar_fallback_to_clipboard: true
}

const MODIFIER_LABELS = {
  alt: 'Alt',
  ctrl: 'Ctrl',
  shift: 'Shift',
  win: 'Win',
  cmd: 'Cmd'
}

const MODIFIER_PRIORITY = {
  cmd: 0,
  win: 0,
  ctrl: 1,
  alt: 2,
  shift: 3
}

const MODIFIER_RAWCODES = {
  alt: [18, 164, 165],
  ctrl: [17, 162, 163],
  shift: [16, 160, 161],
  win: [91, 92],
  cmd: [91, 92]
}

const LEFT_MOUSE_BUTTON = 1
const RIGHT_MOUSE_BUTTON = 2
const MIDDLE_MOUSE_BUTTON = 3
const MIN_SELECTION_DRAG_DISTANCE_PX = 12
const MAX_SELECTION_DRAG_DISTANCE_PX = 960
const MIN_SELECTION_DRAG_DURATION_MS = 80

function ensureIoHookLoaded() {
  if (ioHookLoadAttempted) {
    return ioHook
  }

  ioHookLoadAttempted = true

  try {
    const ioHookModule = require('@tkomde/iohook')
    ioHook = ioHookModule.default || ioHookModule
  } catch (error) {
    ioHookLoadError = error
  }

  return ioHook
}

function sortModifiers(modifiers = []) {
  return [...modifiers].sort((left, right) => {
    const leftPriority = MODIFIER_PRIORITY[left] ?? Number.MAX_SAFE_INTEGER
    const rightPriority = MODIFIER_PRIORITY[right] ?? Number.MAX_SAFE_INTEGER
    if (leftPriority !== rightPriority) {
      return leftPriority - rightPriority
    }
    return left.localeCompare(right)
  })
}

function normalizeHotkey(hotkey = {}) {
  const modifiers = sortModifiers(Array.isArray(hotkey.modifiers)
    ? hotkey.modifiers
      .map((modifier) => String(modifier || '').toLowerCase())
      .filter((modifier, index, array) => modifier && array.indexOf(modifier) === index)
    : [])

  const rawKey = typeof hotkey.key === 'string' ? hotkey.key.trim().toLowerCase() : null
  const key = rawKey && /^[a-z0-9]$/.test(rawKey) ? rawKey : null

  if (modifiers.length === 0 || (!key && modifiers.length < 2)) {
    return { ...DEFAULT_HOTKEY }
  }

  return {
    modifiers,
    key,
    display: buildHotkeyDisplay(modifiers, key)
  }
}

function buildHotkeyDisplay(modifiers, key) {
  const parts = modifiers.map((modifier) => MODIFIER_LABELS[modifier] || modifier)
  if (key) {
    parts.push(String(key).toUpperCase())
  }
  return parts.join('+')
}

function normalizeSettings(settings = {}) {
  return {
    hotkey: normalizeHotkey(settings.hotkey || DEFAULT_SETTINGS.hotkey),
    clipboard_overlay_trigger: settings.clipboard_overlay_trigger || DEFAULT_SETTINGS.clipboard_overlay_trigger,
    clipboard_overlay_retrigger_action: settings.clipboard_overlay_retrigger_action || DEFAULT_SETTINGS.clipboard_overlay_retrigger_action,
    clipboard_overlay_auto_focus_search: settings.clipboard_overlay_auto_focus_search !== false,
    selection_toolbar_enabled: settings.selection_toolbar_enabled !== false,
    selection_toolbar_trigger_delay_ms: Number(settings.selection_toolbar_trigger_delay_ms) > 0
      ? Number(settings.selection_toolbar_trigger_delay_ms)
      : DEFAULT_SETTINGS.selection_toolbar_trigger_delay_ms,
    // 当前版本仅实现了复制回退捕获，暂时强制开启该路径。
    selection_toolbar_fallback_to_clipboard: true
  }
}

class DesktopInputManager {
  constructor(options = {}) {
    this.onHotkeyToggle = options.onHotkeyToggle
    this.onClipboardTrigger = options.onClipboardTrigger
    this.onSelectionTrigger = options.onSelectionTrigger
    this.onPointerDown = options.onPointerDown
    this.onError = options.onError
    this.log = options.log || (() => {})

    this.settings = normalizeSettings()
    this.started = false
    this.hotkeyLatched = false
    this.clipboardChordTriggered = false
    this.activeModifiers = new Set()
    this.activeKeys = new Set()
    this.activeMouseButtons = new Set()
    this.selectionTracking = {
      startPoint: null,
      dragging: false,
      startedAt: 0
    }

    this.boundKeydown = this.handleKeydown.bind(this)
    this.boundKeyup = this.handleKeyup.bind(this)
    this.boundMousedown = this.handleMousedown.bind(this)
    this.boundMouseup = this.handleMouseup.bind(this)
  }

  updateSettings(partialSettings = {}) {
    this.settings = normalizeSettings({
      ...this.settings,
      ...partialSettings
    })
    this.log('Updated desktop input settings', this.settings)
    return this.settings
  }

  start() {
    if (this.started) {
      return
    }

    ensureIoHookLoaded()

    if (!ioHook) {
      const error = new Error(
        `无法加载 iohook 原生模块: ${ioHookLoadError?.message || '未知错误'}`
      )
      error.cause = ioHookLoadError
      this.onError?.(error)
      this.log('Desktop input manager unavailable because iohook failed to load', {
        error: ioHookLoadError?.message || 'unknown'
      })
      return
    }

    ioHook.on('keydown', this.boundKeydown)
    ioHook.on('keyup', this.boundKeyup)
    ioHook.on('mousedown', this.boundMousedown)
    ioHook.on('mouseup', this.boundMouseup)
    ioHook.start()

    this.started = true
    this.log('Desktop input manager started')
  }

  stop() {
    if (!this.started) {
      return
    }

    ioHook.off?.('keydown', this.boundKeydown)
    ioHook.off?.('keyup', this.boundKeyup)
    ioHook.off?.('mousedown', this.boundMousedown)
    ioHook.off?.('mouseup', this.boundMouseup)

    if (!ioHook.off) {
      ioHook.removeAllListeners('keydown')
      ioHook.removeAllListeners('keyup')
      ioHook.removeAllListeners('mousedown')
      ioHook.removeAllListeners('mouseup')
    }

    ioHook.stop()
    this.started = false
    this.hotkeyLatched = false
    this.clipboardChordTriggered = false
    this.activeModifiers.clear()
    this.activeKeys.clear()
    this.activeMouseButtons.clear()
    this.selectionTracking = {
      startPoint: null,
      dragging: false,
      startedAt: 0
    }
    this.log('Desktop input manager stopped')
  }

  handleKeydown(event = {}) {
    try {
      this.syncModifierState(event, true)

      const key = this.resolveKey(event)
      if (key) {
        this.activeKeys.add(key)
      }

      this.refreshHotkeyState()
    } catch (error) {
      this.onError?.(error)
    }
  }

  handleKeyup(event = {}) {
    try {
      this.syncModifierState(event, false)

      const key = this.resolveKey(event)
      if (key) {
        this.activeKeys.delete(key)
      }

      this.refreshHotkeyState()
    } catch (error) {
      this.onError?.(error)
    }
  }

  handleMousedown(event = {}) {
    try {
      const button = Number(event.button)
      const point = this.resolvePoint(event)
      if (!Number.isNaN(button)) {
        this.activeMouseButtons.add(button)
      }

      if (button === LEFT_MOUSE_BUTTON) {
        this.selectionTracking = {
          startPoint: point,
          dragging: false,
          startedAt: Date.now()
        }
      }

      if (this.settings.clipboard_overlay_trigger === 'mouse_middle' && button === MIDDLE_MOUSE_BUTTON) {
        this.triggerClipboardOverlay()
        return
      }

      if (
        this.settings.clipboard_overlay_trigger === 'mouse_left_right_chord' &&
        this.activeMouseButtons.has(LEFT_MOUSE_BUTTON) &&
        this.activeMouseButtons.has(RIGHT_MOUSE_BUTTON) &&
        !this.clipboardChordTriggered
      ) {
        this.clipboardChordTriggered = true
        this.triggerClipboardOverlay()
        return
      }

      this.onPointerDown?.({
        button,
        point
      })
    } catch (error) {
      this.onError?.(error)
    }
  }

  handleMouseup(event = {}) {
    const button = Number(event.button)
    const point = this.resolvePoint(event)
    const wasSelectionTrigger = (
      button === LEFT_MOUSE_BUTTON &&
      this.settings.selection_toolbar_enabled &&
      this.didCompleteSelection(point)
    )

    if (!Number.isNaN(button)) {
      this.activeMouseButtons.delete(button)
    }

    if (
      this.settings.clipboard_overlay_trigger === 'mouse_left_right_chord' &&
      (!this.activeMouseButtons.has(LEFT_MOUSE_BUTTON) || !this.activeMouseButtons.has(RIGHT_MOUSE_BUTTON))
    ) {
      this.clipboardChordTriggered = false
    }

    if (wasSelectionTrigger) {
      this.onSelectionTrigger?.({
        trigger: 'mouse_selection',
        point,
        delayMs: this.settings.selection_toolbar_trigger_delay_ms,
        allowClipboardFallback: this.settings.selection_toolbar_fallback_to_clipboard !== false
      })
    }

    if (button === LEFT_MOUSE_BUTTON) {
      this.selectionTracking = {
        startPoint: null,
        dragging: false,
        startedAt: 0
      }
    }
  }

  triggerClipboardOverlay() {
    this.onClipboardTrigger?.({
      focusSearch: this.settings.clipboard_overlay_auto_focus_search,
      retriggerAction: this.settings.clipboard_overlay_retrigger_action,
      trigger: this.settings.clipboard_overlay_trigger
    })
  }

  matchesHotkey() {
    const { hotkey } = this.settings

    if (this.activeModifiers.size !== hotkey.modifiers.length) {
      return false
    }

    if (!hotkey.modifiers.every((modifier) => this.activeModifiers.has(modifier))) {
      return false
    }

    if (!hotkey.key) {
      return this.activeKeys.size === 0
    }

    if (this.activeKeys.size !== 1) {
      return false
    }

    return this.activeKeys.has(hotkey.key)
  }

  syncModifierState(event = {}, isKeydown) {
    const nextModifiers = new Set()

    if (event.ctrlKey) {
      nextModifiers.add('ctrl')
    }
    if (event.altKey) {
      nextModifiers.add('alt')
    }
    if (event.shiftKey) {
      nextModifiers.add('shift')
    }
    if (event.metaKey) {
      nextModifiers.add('win')
    }

    const modifier = this.resolveModifier(event)
    if (modifier) {
      if (isKeydown) {
        nextModifiers.add(modifier)
      } else {
        nextModifiers.delete(modifier)
      }
    }

    this.activeModifiers = nextModifiers
  }

  refreshHotkeyState() {
    if (this.matchesHotkey()) {
      if (!this.hotkeyLatched) {
        this.hotkeyLatched = true
        this.onHotkeyToggle?.({
          hotkey: this.settings.hotkey
        })
      }
      return
    }

    this.hotkeyLatched = false
  }

  resolveModifier(event = {}) {
    const rawcode = Number(event.rawcode)
    const keycode = Number(event.keycode)

    for (const [modifier, codes] of Object.entries(MODIFIER_RAWCODES)) {
      if (codes.includes(rawcode) || codes.includes(keycode)) {
        return modifier
      }
    }

    return null
  }

  resolveKey(event = {}) {
    if (typeof event.keychar === 'number' && event.keychar > 0) {
      const keychar = String.fromCharCode(event.keychar).toLowerCase()
      if (/^[a-z0-9]$/.test(keychar)) {
        return keychar
      }
    }

    const rawcode = Number(event.rawcode)
    if (rawcode >= 65 && rawcode <= 90) {
      return String.fromCharCode(rawcode).toLowerCase()
    }

    if (rawcode >= 48 && rawcode <= 57) {
      return String.fromCharCode(rawcode)
    }

    return null
  }

  resolvePoint(event = {}) {
    const x = Number(event.x)
    const y = Number(event.y)
    if (Number.isFinite(x) && Number.isFinite(y)) {
      return { x, y }
    }
    return null
  }

  didCompleteSelection(currentPoint) {
    const startPoint = this.selectionTracking.startPoint
    if (!startPoint || !currentPoint) {
      return false
    }

    const distanceX = Math.abs(currentPoint.x - startPoint.x)
    const distanceY = Math.abs(currentPoint.y - startPoint.y)
    const dragDistance = Math.max(distanceX, distanceY)
    const dragDurationMs = this.selectionTracking.startedAt > 0
      ? Math.max(0, Date.now() - this.selectionTracking.startedAt)
      : 0

    return (
      dragDistance >= MIN_SELECTION_DRAG_DISTANCE_PX &&
      dragDistance <= MAX_SELECTION_DRAG_DISTANCE_PX &&
      dragDurationMs >= MIN_SELECTION_DRAG_DURATION_MS
    )
  }
}

module.exports = {
  DesktopInputManager,
  buildHotkeyDisplay,
  normalizeHotkey,
  normalizeSettings,
  sortModifiers,
  isDesktopInputAvailable: () => Boolean(ensureIoHookLoaded()),
  getDesktopInputLoadError: () => {
    ensureIoHookLoaded()
    return ioHookLoadError
  }
}

