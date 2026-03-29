const assert = require('node:assert/strict')

const {
  DesktopInputManager,
  normalizeHotkey
} = require('../electron/input/listener')

function wait(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

async function testNormalizeHotkeyOrder() {
  const hotkey = normalizeHotkey({
    modifiers: ['alt', 'win'],
    key: null,
    display: 'Alt+Win'
  })

  assert.deepEqual(hotkey.modifiers, ['win', 'alt'])
  assert.equal(hotkey.display, 'Win+Alt')
}

async function testShortPressDoesNotTriggerRecording() {
  let started = 0
  let stopped = 0

  const manager = new DesktopInputManager({
    hotkeyHoldThresholdMs: 25,
    onHotkeyStart: () => {
      started += 1
    },
    onHotkeyStop: () => {
      stopped += 1
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
  await wait(10)
  manager.handleKeyup({ rawcode: 18, keycode: 18, metaKey: true, altKey: false })
  manager.handleKeyup({ rawcode: 91, keycode: 91, metaKey: false })
  await wait(30)

  assert.equal(started, 0)
  assert.equal(stopped, 0)
}

async function testLongPressStartsAndStopsRecording() {
  let started = 0
  let stopped = 0

  const manager = new DesktopInputManager({
    hotkeyHoldThresholdMs: 25,
    onHotkeyStart: () => {
      started += 1
    },
    onHotkeyStop: () => {
      stopped += 1
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
  await wait(35)

  assert.equal(started, 1)
  assert.equal(stopped, 0)

  manager.handleKeyup({ rawcode: 18, keycode: 18, metaKey: true, altKey: false })
  manager.handleKeyup({ rawcode: 91, keycode: 91, metaKey: false })
  await wait(5)

  assert.equal(stopped, 1)
}

async function main() {
  await testNormalizeHotkeyOrder()
  await testShortPressDoesNotTriggerRecording()
  await testLongPressStartsAndStopsRecording()
  process.exit(0)
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
