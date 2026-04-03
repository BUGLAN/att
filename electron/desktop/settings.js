const fs = require('fs')
const path = require('path')

const MODIFIER_PRIORITY = {
  cmd: 0,
  win: 0,
  ctrl: 1,
  alt: 2,
  shift: 3
}

const MODIFIER_LABELS = {
  alt: 'Alt',
  ctrl: 'Ctrl',
  shift: 'Shift',
  win: 'Win',
  cmd: 'Cmd'
}

const DEFAULT_DESKTOP_SETTINGS = {
  darwin: {
    hotkey: {
      modifiers: ['cmd', 'shift'],
      key: 'r',
      display: 'Cmd+Shift+R'
    },
    pause_audio_on_recording: true
  },
  win32: {
    hotkey: {
      modifiers: ['win', 'alt'],
      key: null,
      display: 'Win+Alt'
    },
    pause_audio_on_recording: true
  },
  default: {
    hotkey: {
      modifiers: ['ctrl', 'shift'],
      key: 'r',
      display: 'Ctrl+Shift+R'
    },
    pause_audio_on_recording: true
  }
}

function getDefaultDesktopSettings(platform = process.platform) {
  const defaults = DEFAULT_DESKTOP_SETTINGS[platform] || DEFAULT_DESKTOP_SETTINGS.default
  return JSON.parse(JSON.stringify(defaults))
}

function sortHotkeyModifiers(modifiers = []) {
  return [...modifiers].sort((left, right) => {
    const leftPriority = MODIFIER_PRIORITY[left] ?? Number.MAX_SAFE_INTEGER
    const rightPriority = MODIFIER_PRIORITY[right] ?? Number.MAX_SAFE_INTEGER
    if (leftPriority !== rightPriority) {
      return leftPriority - rightPriority
    }
    return left.localeCompare(right)
  })
}

function buildHotkeyDisplay(modifiers, key) {
  const parts = sortHotkeyModifiers(modifiers).map((modifier) => MODIFIER_LABELS[modifier] || modifier)
  if (key) {
    parts.push(String(key).toUpperCase())
  }
  return parts.join('+')
}

function normalizeHotkey(hotkey, platform = process.platform) {
  const fallback = getDefaultDesktopSettings(platform).hotkey
  if (!Array.isArray(hotkey?.modifiers) || hotkey.modifiers.length === 0) {
    return fallback
  }

  const modifiers = sortHotkeyModifiers(
    hotkey.modifiers
      .map((modifier) => String(modifier || '').toLowerCase())
      .filter((modifier, index, array) => modifier && array.indexOf(modifier) === index)
  )
  const key = typeof hotkey?.key === 'string' && /^[a-z0-9]$/i.test(hotkey.key)
    ? hotkey.key.toLowerCase()
    : null

  if (!key && modifiers.length < 2) {
    return fallback
  }

  return {
    modifiers,
    key,
    display: buildHotkeyDisplay(modifiers, key)
  }
}

function normalizeDesktopSettings(settings = {}, platform = process.platform) {
  const defaults = getDefaultDesktopSettings(platform)

  return {
    hotkey: normalizeHotkey(settings.hotkey || defaults.hotkey, platform),
    pause_audio_on_recording: settings.pause_audio_on_recording ?? defaults.pause_audio_on_recording
  }
}

class DesktopSettingsStore {
  constructor(options = {}) {
    this.platform = options.platform || process.platform
    this.filePath = options.filePath || path.join(process.cwd(), 'desktop-settings.json')
  }

  load() {
    if (!fs.existsSync(this.filePath)) {
      return normalizeDesktopSettings({}, this.platform)
    }

    try {
      const parsed = JSON.parse(fs.readFileSync(this.filePath, 'utf8'))
      return normalizeDesktopSettings(parsed, this.platform)
    } catch (error) {
      return normalizeDesktopSettings({}, this.platform)
    }
  }

  save(settings = {}) {
    const normalized = normalizeDesktopSettings(settings, this.platform)
    fs.mkdirSync(path.dirname(this.filePath), { recursive: true })
    fs.writeFileSync(this.filePath, JSON.stringify(normalized, null, 2))
    return normalized
  }
}

module.exports = {
  DEFAULT_DESKTOP_SETTINGS,
  DesktopSettingsStore,
  buildHotkeyDisplay,
  getDefaultDesktopSettings,
  normalizeDesktopSettings,
  normalizeHotkey,
  sortHotkeyModifiers
}
