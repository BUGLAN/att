const assert = require('node:assert/strict')

const {
  DesktopInputManager,
  normalizeHotkey,
  normalizeSettings
} = require('../electron/input/listener')

async function testNormalizeHotkeyOrder() {
  const hotkey = normalizeHotkey({
    modifiers: ['alt', 'win'],
    key: null,
    display: 'Alt+Win'
  })

  assert.deepEqual(hotkey.modifiers, ['win', 'alt'])
  assert.equal(hotkey.display, 'Win+Alt')
}

async function testToggleHotkeyTriggersOncePerPress() {
  const toggles = []

  const manager = new DesktopInputManager({
    onHotkeyToggle: (payload) => {
      toggles.push(payload)
    }
  })

  manager.updateSettings({
    hotkey: {
      modifiers: ['win', 'alt'],
      key: null
    }
  })

  manager.handleKeydown({ rawcode: 91, keycode: 91, metaKey: true })
  manager.handleKeydown({ rawcode: 18, keycode: 18, metaKey: true, altKey: true })

  assert.equal(toggles.length, 1)
  assert.equal(toggles[0].hotkey.display, 'Win+Alt')

  manager.handleKeydown({ rawcode: 18, keycode: 18, metaKey: true, altKey: true })
  manager.handleKeydown({ rawcode: 91, keycode: 91, metaKey: true, altKey: true })

  assert.equal(toggles.length, 1)
}

async function testToggleHotkeyResetsAfterRelease() {
  let toggles = 0

  const manager = new DesktopInputManager({
    onHotkeyToggle: () => {
      toggles += 1
    }
  })

  manager.updateSettings({
    hotkey: {
      modifiers: ['win', 'alt'],
      key: null
    }
  })

  manager.handleKeydown({ rawcode: 91, keycode: 91, metaKey: true })
  manager.handleKeydown({ rawcode: 18, keycode: 18, metaKey: true, altKey: true })
  manager.handleKeyup({ rawcode: 18, keycode: 18, metaKey: true, altKey: false })
  manager.handleKeyup({ rawcode: 91, keycode: 91, metaKey: false })

  manager.handleKeydown({ rawcode: 91, keycode: 91, metaKey: true })
  manager.handleKeydown({ rawcode: 18, keycode: 18, metaKey: true, altKey: true })

  assert.equal(toggles, 2)
}

async function testToggleHotkeyRequiresFullChordAgainAfterPartialRelease() {
  let toggles = 0

  const manager = new DesktopInputManager({
    onHotkeyToggle: () => {
      toggles += 1
    }
  })

  manager.updateSettings({
    hotkey: {
      modifiers: ['win', 'alt'],
      key: null
    }
  })

  manager.handleKeydown({ rawcode: 91, keycode: 91, metaKey: true })
  manager.handleKeydown({ rawcode: 18, keycode: 18, metaKey: true, altKey: true })
  manager.handleKeyup({ rawcode: 18, keycode: 18, metaKey: true, altKey: false })
  manager.handleKeydown({ rawcode: 18, keycode: 18, metaKey: true, altKey: true })

  assert.equal(toggles, 2)
}

async function testSelectionTriggerRequiresReasonableDrag() {
  const triggers = []
  const originalNow = Date.now
  let now = 1000
  Date.now = () => now

  try {
    const manager = new DesktopInputManager({
      onSelectionTrigger: (payload) => {
        triggers.push(payload)
      }
    })

    manager.updateSettings({
      selection_toolbar_enabled: true,
      selection_toolbar_trigger_delay_ms: 220
    })

    manager.handleMousedown({ button: 1, x: 100, y: 100 })
    now += 120
    manager.handleMouseup({ button: 1, x: 126, y: 110 })

    assert.equal(triggers.length, 1)
    assert.equal(triggers[0].trigger, 'mouse_selection')
  } finally {
    Date.now = originalNow
  }
}

async function testSelectionTriggerIgnoresShortAndOversizedDrag() {
  const triggers = []
  const originalNow = Date.now
  let now = 1000
  Date.now = () => now

  try {
    const manager = new DesktopInputManager({
      onSelectionTrigger: (payload) => {
        triggers.push(payload)
      }
    })

    manager.updateSettings({
      selection_toolbar_enabled: true,
      selection_toolbar_trigger_delay_ms: 220
    })

    manager.handleMousedown({ button: 1, x: 100, y: 100 })
    now += 20
    manager.handleMouseup({ button: 1, x: 140, y: 100 })

    manager.handleMousedown({ button: 1, x: 100, y: 100 })
    now += 160
    manager.handleMouseup({ button: 1, x: 1200, y: 120 })

    assert.equal(triggers.length, 0)
  } finally {
    Date.now = originalNow
  }
}

async function testNormalizeSettingsForcesSelectionClipboardFallbackForCurrentVersion() {
  const settings = normalizeSettings({
    selection_toolbar_enabled: true,
    selection_toolbar_fallback_to_clipboard: false
  })

  assert.equal(settings.selection_toolbar_enabled, true)
  assert.equal(settings.selection_toolbar_fallback_to_clipboard, true)
}

async function main() {
  await testNormalizeHotkeyOrder()
  await testToggleHotkeyTriggersOncePerPress()
  await testToggleHotkeyResetsAfterRelease()
  await testToggleHotkeyRequiresFullChordAgainAfterPartialRelease()
  await testSelectionTriggerRequiresReasonableDrag()
  await testSelectionTriggerIgnoresShortAndOversizedDrag()
  await testNormalizeSettingsForcesSelectionClipboardFallbackForCurrentVersion()
  process.exit(0)
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
