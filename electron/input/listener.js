let ioHook = null
let ioHookLoadError = null
let ioHookLoadAttempted = false

const DEFAULT_HOTKEY = {
  modifiers: ['win', 'alt'],
  key: null,
  display: 'Win+Alt'
}

const DEFAULT_HOTKEY_HOLD_THRESHOLD_MS = 150

const DEFAULT_SETTINGS = {
  hotkey: DEFAULT_HOTKEY,
  clipboard_overlay_trigger: 'mouse_middle',
  clipboard_overlay_retrigger_action: 'toggle',
  clipboard_overlay_auto_focus_search: true
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
    clipboard_overlay_auto_focus_search: settings.clipboard_overlay_auto_focus_search !== false
  }
}

class DesktopInputManager {
  constructor(options = {}) {
    this.onHotkeyStart = options.onHotkeyStart
    this.onHotkeyStop = options.onHotkeyStop
    this.onClipboardTrigger = options.onClipboardTrigger
    this.onError = options.onError
    this.log = options.log || (() => {})

    this.settings = normalizeSettings()
    this.started = false
    this.hotkeyPressed = false
    this.pendingHotkeyActivation = false
    this.clipboardChordTriggered = false
    this.activeModifiers = new Set()
    this.activeKeys = new Set()
    this.activeMouseButtons = new Set()
    this.hotkeyActivationTimer = null
    this.hotkeyHoldThresholdMs = Number.isFinite(options.hotkeyHoldThresholdMs)
      ? Math.max(0, options.hotkeyHoldThresholdMs)
      : DEFAULT_HOTKEY_HOLD_THRESHOLD_MS

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
    this.hotkeyPressed = false
    this.pendingHotkeyActivation = false
    this.clipboardChordTriggered = false
    this.clearPendingHotkeyActivation()
    this.activeModifiers.clear()
    this.activeKeys.clear()
    this.activeMouseButtons.clear()
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
      if (!Number.isNaN(button)) {
        this.activeMouseButtons.add(button)
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
      }
    } catch (error) {
      this.onError?.(error)
    }
  }

  handleMouseup(event = {}) {
    const button = Number(event.button)
    if (!Number.isNaN(button)) {
      this.activeMouseButtons.delete(button)
    }

    if (
      this.settings.clipboard_overlay_trigger === 'mouse_left_right_chord' &&
      (!this.activeMouseButtons.has(LEFT_MOUSE_BUTTON) || !this.activeMouseButtons.has(RIGHT_MOUSE_BUTTON))
    ) {
      this.clipboardChordTriggered = false
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

    if (!hotkey.modifiers.every((modifier) => this.activeModifiers.has(modifier))) {
      return false
    }

    if (!hotkey.key) {
      return true
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
      if (!this.hotkeyPressed && !this.pendingHotkeyActivation) {
        this.scheduleHotkeyActivation()
      }
      return
    }

    this.clearPendingHotkeyActivation()

    if (this.hotkeyPressed) {
      this.hotkeyPressed = false
      this.onHotkeyStop?.({
        hotkey: this.settings.hotkey
      })
    }
  }

  scheduleHotkeyActivation() {
    if (this.hotkeyPressed || this.pendingHotkeyActivation) {
      return
    }

    this.pendingHotkeyActivation = true

    if (this.hotkeyHoldThresholdMs === 0) {
      this.activateHotkey()
      return
    }

    this.hotkeyActivationTimer = setTimeout(() => {
      this.hotkeyActivationTimer = null
      this.activateHotkey()
    }, this.hotkeyHoldThresholdMs)
  }

  clearPendingHotkeyActivation() {
    if (this.hotkeyActivationTimer) {
      clearTimeout(this.hotkeyActivationTimer)
      this.hotkeyActivationTimer = null
    }

    this.pendingHotkeyActivation = false
  }

  activateHotkey() {
    if (!this.pendingHotkeyActivation || this.hotkeyPressed || !this.matchesHotkey()) {
      this.clearPendingHotkeyActivation()
      return
    }

    this.pendingHotkeyActivation = false
    this.hotkeyPressed = true
    this.onHotkeyStart?.({
      hotkey: this.settings.hotkey,
      holdThresholdMs: this.hotkeyHoldThresholdMs
    })
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
}

module.exports = {
  DesktopInputManager,
  DEFAULT_HOTKEY_HOLD_THRESHOLD_MS,
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

