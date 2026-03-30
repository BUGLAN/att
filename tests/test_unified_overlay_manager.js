const assert = require('node:assert/strict')

const { UnifiedOverlayManager } = require('../electron/overlay')

class FakeBrowserWindow {
  constructor(options = {}) {
    this.options = options
    this.bounds = {
      x: 0,
      y: 0,
      width: options.width,
      height: options.height
    }
    this.visible = false
    this.handlers = {}
    this.webContents = {
      sent: [],
      once: (event, handler) => {
        this.handlers[event] = handler
      },
      send: (channel, payload) => {
        this.webContents.sent.push({ channel, payload })
      },
      focus: () => {}
    }
  }

  setAlwaysOnTop() {}
  setVisibleOnAllWorkspaces() {}
  loadURL() {}
  loadFile() {}
  on(event, handler) {
    this.handlers[event] = handler
  }
  setBounds(bounds) {
    this.bounds = bounds
  }
  getBounds() {
    return this.bounds
  }
  show() {
    this.visible = true
  }
  showInactive() {
    this.visible = true
  }
  moveTop() {}
  setFocusable() {}
  focus() {}
  hide() {
    this.visible = false
  }
  isDestroyed() {
    return false
  }
  destroy() {
    this.visible = false
  }
}

function createManager() {
  return new UnifiedOverlayManager({
    BrowserWindow: FakeBrowserWindow,
    screen: {
      getCursorScreenPoint: () => ({ x: 320, y: 240 }),
      getDisplayNearestPoint: () => ({
        workArea: { x: 0, y: 0, width: 1440, height: 900 }
      }),
      screenToDipPoint: (point) => point
    },
    isDevelopmentRuntime: () => false
  })
}

async function testSelectionWaitsForReadyBeforeShow() {
  const manager = createManager()
  manager.openSelection({
    text: 'hello',
    point: { x: 200, y: 180 },
    trigger: 'mouse_selection'
  })

  manager.window.handlers['did-finish-load']()

  assert.equal(manager.window.visible, false)
  assert.equal(manager.window.webContents.sent[0].channel, 'overlay:activate')
  assert.equal(manager.window.webContents.sent[0].payload.mode, 'selection')

  manager.markRendererReady()
  assert.equal(manager.window.visible, true)

  const bounds = manager.window.getBounds()
  assert.equal(bounds.x, 996)
  assert.equal(bounds.y, 24)
}

async function testClipboardModeShowsAfterReady() {
  const manager = createManager()
  manager.openClipboard({ focusSearch: true })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  assert.equal(manager.currentMode, 'clipboard')
  assert.equal(manager.window.visible, true)
  assert.equal(manager.pendingActivation.mode, 'clipboard')
}

async function testClipboardIgnoresSelectionWhileVisible() {
  const manager = createManager()
  manager.openClipboard({ focusSearch: true })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  const result = manager.openSelection({
    text: 'ignored',
    point: { x: 120, y: 120 }
  })

  assert.equal(result, false)
  assert.equal(manager.currentMode, 'clipboard')
}

async function testClipboardBlurDuringGraceDoesNotHideWindow() {
  const manager = createManager()
  manager.openClipboard({ focusSearch: true })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  manager.window.handlers.blur()

  assert.equal(manager.window.visible, true)
  assert.equal(manager.isVisible, true)
}

async function testClipboardOutsidePointerHidesWindow() {
  const manager = createManager()
  manager.openClipboard({ focusSearch: true })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  manager.handleGlobalPointerDown({
    point: { x: 40, y: 40 }
  })

  assert.equal(manager.window.visible, false)
  assert.equal(manager.isVisible, false)
}

async function testClipboardInsidePointerKeepsWindowVisible() {
  const manager = createManager()
  manager.openClipboard({ focusSearch: true })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  const bounds = manager.window.getBounds()
  manager.handleGlobalPointerDown({
    point: {
      x: bounds.x + 20,
      y: bounds.y + 20
    }
  })

  assert.equal(manager.window.visible, true)
  assert.equal(manager.isVisible, true)
}

async function testClipboardPointerUsesDipCoordinates() {
  const manager = new UnifiedOverlayManager({
    BrowserWindow: FakeBrowserWindow,
    screen: {
      getCursorScreenPoint: () => ({ x: 320, y: 240 }),
      getDisplayNearestPoint: () => ({
        workArea: { x: 0, y: 0, width: 1440, height: 900 }
      }),
      screenToDipPoint: (point) => ({
        x: Math.round(point.x / 1.5),
        y: Math.round(point.y / 1.5)
      })
    },
    isDevelopmentRuntime: () => false
  })

  manager.openClipboard({ focusSearch: true })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  const bounds = manager.window.getBounds()
  manager.handleGlobalPointerDown({
    point: {
      x: Math.round((bounds.x + 24) * 1.5),
      y: Math.round((bounds.y + 24) * 1.5)
    }
  })

  assert.equal(manager.window.visible, true)
  assert.equal(manager.isVisible, true)
}

async function testSelectionOutsidePointerHidesWindow() {
  const manager = createManager()
  manager.openSelection({
    text: 'hello',
    point: { x: 200, y: 180 }
  })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  manager.handleGlobalPointerDown({
    point: { x: 40, y: 40 }
  })

  assert.equal(manager.window.visible, false)
  assert.equal(manager.isVisible, false)
}

async function testSelectionInsidePointerKeepsWindowVisible() {
  const manager = createManager()
  manager.openSelection({
    text: 'hello',
    point: { x: 200, y: 180 }
  })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  const bounds = manager.window.getBounds()
  manager.handleGlobalPointerDown({
    point: {
      x: bounds.x + 20,
      y: bounds.y + 20
    }
  })

  assert.equal(manager.window.visible, true)
  assert.equal(manager.isVisible, true)
}

async function testSelectionReusesLastMovedPosition() {
  const manager = createManager()
  manager.openSelection({
    text: 'hello',
    point: { x: 200, y: 180 }
  })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  manager.window.setBounds({
    x: 720,
    y: 180,
    width: 420,
    height: 320
  })
  manager.programmaticMoveLock = 0
  manager.window.handlers.move()
  manager.hide('manual')

  manager.openSelection({
    text: 'world',
    point: { x: 120, y: 120 }
  })

  const bounds = manager.window.getBounds()
  assert.equal(bounds.x, 720)
  assert.equal(bounds.y, 180)
}

async function testSelectionActionCompleteKeepsWindowVisible() {
  const manager = createManager()
  manager.openSelection({
    text: 'hello',
    point: { x: 200, y: 180 }
  })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  manager.handleActionComplete({
    mode: 'selection',
    actionType: 'translate'
  })

  assert.equal(manager.window.visible, true)
  assert.equal(manager.isVisible, true)
}

async function main() {
  await testSelectionWaitsForReadyBeforeShow()
  await testClipboardModeShowsAfterReady()
  await testClipboardIgnoresSelectionWhileVisible()
  await testClipboardBlurDuringGraceDoesNotHideWindow()
  await testClipboardOutsidePointerHidesWindow()
  await testClipboardInsidePointerKeepsWindowVisible()
  await testClipboardPointerUsesDipCoordinates()
  await testSelectionOutsidePointerHidesWindow()
  await testSelectionInsidePointerKeepsWindowVisible()
  await testSelectionReusesLastMovedPosition()
  await testSelectionActionCompleteKeepsWindowVisible()
  process.exit(0)
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
