const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const {
  DEFAULT_DESKTOP_SETTINGS,
  DesktopSettingsStore,
  normalizeDesktopSettings
} = require('../electron/desktop/settings')

test('normalizeDesktopSettings falls back to platform defaults for invalid hotkey', () => {
  const settings = normalizeDesktopSettings({
    hotkey: {
      modifiers: ['win'],
      key: null
    },
    pause_audio_on_recording: false
  }, 'win32')

  assert.deepEqual(settings.hotkey, DEFAULT_DESKTOP_SETTINGS.win32.hotkey)
  assert.equal(settings.pause_audio_on_recording, false)
})

test('DesktopSettingsStore persists desktop settings locally', () => {
  const tempRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'att-desktop-settings-'))
  const filePath = path.join(tempRoot, 'desktop-settings.json')

  try {
    const store = new DesktopSettingsStore({
      filePath,
      platform: 'win32'
    })

    const saved = store.save({
      hotkey: {
        modifiers: ['ctrl', 'alt'],
        key: 'r'
      },
      pause_audio_on_recording: false
    })

    assert.deepEqual(saved.hotkey, {
      modifiers: ['ctrl', 'alt'],
      key: 'r',
      display: 'Ctrl+Alt+R'
    })
    assert.equal(saved.pause_audio_on_recording, false)

    const reloaded = new DesktopSettingsStore({
      filePath,
      platform: 'win32'
    }).load()

    assert.deepEqual(reloaded, saved)
  } finally {
    fs.rmSync(tempRoot, { recursive: true, force: true })
  }
})
