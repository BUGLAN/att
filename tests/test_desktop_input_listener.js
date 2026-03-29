const assert = require('node:assert/strict')

const {
  DesktopInputManager,
  normalizeHotkey
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

async function main() {
  await testNormalizeHotkeyOrder()
  await testToggleHotkeyTriggersOncePerPress()
  await testToggleHotkeyResetsAfterRelease()
  await testToggleHotkeyRequiresFullChordAgainAfterPartialRelease()
  process.exit(0)
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
