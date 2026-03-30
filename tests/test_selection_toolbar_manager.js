const assert = require('node:assert/strict')

const { SelectionToolbarManager } = require('../electron/selectionToolbar')

class FakeBrowserWindow {
  constructor(options = {}) {
    this.options = options
    this.bounds = null
    this.visible = false
    this.handlers = {}
    this.webContents = {
      sent: [],
      once: (event, handler) => {
        if (event === 'did-finish-load') {
          this.handlers[event] = handler
        }
      },
      send: (channel, payload) => {
        this.webContents.sent.push({ channel, payload })
      }
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
    return this.bounds || { x: 0, y: 0, width: 420, height: 640 }
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

async function testShowAtSelectionSendsActivationPayload() {
  const manager = new SelectionToolbarManager({
    BrowserWindow: FakeBrowserWindow,
    screen: {
      getCursorScreenPoint: () => ({ x: 320, y: 240 }),
      getDisplayNearestPoint: () => ({
        workArea: { x: 0, y: 0, width: 1440, height: 900 }
      })
    },
    isDevelopmentRuntime: () => false
  })

  manager.showAtSelection({
    text: 'accurate',
    point: { x: 300, y: 220 },
    trigger: 'mouse_selection'
  })

  manager.window.handlers['did-finish-load']()

  assert.equal(manager.window.webContents.sent[0].channel, 'selection-toolbar:activate')
  assert.equal(manager.window.webContents.sent[0].payload.text, 'accurate')
  assert.equal(manager.window.bounds.width, 420)
  assert.equal(manager.window.visible, true)
  assert.equal(manager.window.options.movable, true)
}

async function testCalculateBoundsPinsToolbarToTopRight() {
  const manager = new SelectionToolbarManager({
    BrowserWindow: FakeBrowserWindow,
    screen: {
      getCursorScreenPoint: () => ({ x: 1200, y: 860 }),
      getDisplayNearestPoint: () => ({
        workArea: { x: 0, y: 0, width: 1440, height: 900 }
      })
    },
    isDevelopmentRuntime: () => false
  })

  const bounds = manager.calculateBounds({ x: 1200, y: 860 })

  assert.equal(bounds.y, 24)
  assert.equal(bounds.x, 996)
  assert.equal(bounds.height, 320)
}

async function testShowAtSelectionKeepsDraggedPositionForSameText() {
  const manager = new SelectionToolbarManager({
    BrowserWindow: FakeBrowserWindow,
    screen: {
      getCursorScreenPoint: () => ({ x: 320, y: 240 }),
      getDisplayNearestPoint: () => ({
        workArea: { x: 0, y: 0, width: 1440, height: 900 }
      })
    },
    isDevelopmentRuntime: () => false
  })

  manager.showAtSelection({
    text: 'accurate',
    point: { x: 300, y: 220 },
    trigger: 'mouse_selection'
  })
  manager.window.handlers['did-finish-load']()

  manager.window.bounds = { x: 640, y: 400, width: 420, height: 640 }
  manager.window.handlers.move()

  manager.showAtSelection({
    text: 'accurate',
    point: { x: 300, y: 220 },
    trigger: 'mouse_selection'
  })

  assert.equal(manager.window.bounds.x, 640)
  assert.equal(manager.window.bounds.y, 400)
}

async function testShowAtSelectionKeepsOriginalPositionForSameVisibleText() {
  const manager = new SelectionToolbarManager({
    BrowserWindow: FakeBrowserWindow,
    screen: {
      getCursorScreenPoint: () => ({ x: 320, y: 240 }),
      getDisplayNearestPoint: () => ({
        workArea: { x: 0, y: 0, width: 1440, height: 900 }
      })
    },
    isDevelopmentRuntime: () => false
  })

  manager.showAtSelection({
    text: 'accurate',
    point: { x: 300, y: 220 },
    trigger: 'mouse_selection'
  })
  manager.window.handlers['did-finish-load']()

  const firstBounds = { ...manager.window.bounds }

  manager.showAtSelection({
    text: 'accurate',
    point: { x: 520, y: 480 },
    trigger: 'mouse_selection'
  })

  assert.deepEqual(manager.window.bounds, firstBounds)
}

async function testShowAtSelectionDoesNotResendActivationForSameVisibleText() {
  const manager = new SelectionToolbarManager({
    BrowserWindow: FakeBrowserWindow,
    screen: {
      getCursorScreenPoint: () => ({ x: 320, y: 240 }),
      getDisplayNearestPoint: () => ({
        workArea: { x: 0, y: 0, width: 1440, height: 900 }
      })
    },
    isDevelopmentRuntime: () => false
  })

  manager.showAtSelection({
    text: 'accurate',
    point: { x: 300, y: 220 },
    trigger: 'mouse_selection'
  })
  manager.window.handlers['did-finish-load']()

  assert.equal(manager.window.webContents.sent.length, 1)

  manager.showAtSelection({
    text: 'accurate',
    point: { x: 520, y: 480 },
    trigger: 'mouse_selection'
  })

  assert.equal(manager.window.webContents.sent.length, 1)
}

async function testPinnedToolbarIgnoresBlur() {
  const manager = new SelectionToolbarManager({
    BrowserWindow: FakeBrowserWindow,
    screen: {
      getCursorScreenPoint: () => ({ x: 320, y: 240 }),
      getDisplayNearestPoint: () => ({
        workArea: { x: 0, y: 0, width: 1440, height: 900 }
      })
    },
    isDevelopmentRuntime: () => false
  })

  manager.showAtSelection({
    text: 'accurate',
    point: { x: 300, y: 220 },
    trigger: 'mouse_selection'
  })
  manager.window.handlers['did-finish-load']()

  manager.togglePinned(true)
  manager.window.handlers.blur()

  assert.equal(manager.window.visible, true)
  assert.equal(manager.isVisible, true)
}

async function testPinnedToolbarKeepsManualPositionForNewSelection() {
  const manager = new SelectionToolbarManager({
    BrowserWindow: FakeBrowserWindow,
    screen: {
      getCursorScreenPoint: () => ({ x: 320, y: 240 }),
      getDisplayNearestPoint: () => ({
        workArea: { x: 0, y: 0, width: 1440, height: 900 }
      })
    },
    isDevelopmentRuntime: () => false
  })

  manager.showAtSelection({
    text: 'accurate',
    point: { x: 300, y: 220 },
    trigger: 'mouse_selection'
  })
  manager.window.handlers['did-finish-load']()

  manager.togglePinned(true)
  manager.window.bounds = { x: 700, y: 180, width: 420, height: 640 }
  manager.window.handlers.move()

  manager.showAtSelection({
    text: 'updated',
    point: { x: 520, y: 480 },
    trigger: 'mouse_selection'
  })

  assert.equal(manager.window.bounds.x, 700)
  assert.equal(manager.window.bounds.y, 180)
}

async function testBackdropPointerDownHidesWhenNotPinned() {
  const manager = new SelectionToolbarManager({
    BrowserWindow: FakeBrowserWindow,
    screen: {
      getCursorScreenPoint: () => ({ x: 320, y: 240 }),
      getDisplayNearestPoint: () => ({
        workArea: { x: 0, y: 0, width: 1440, height: 900 }
      })
    },
    isDevelopmentRuntime: () => false
  })

  manager.showAtSelection({
    text: 'accurate',
    point: { x: 300, y: 220 },
    trigger: 'mouse_selection'
  })
  manager.window.handlers['did-finish-load']()

  manager.handleBackdropPointerDown({ button: 0 })

  assert.equal(manager.isVisible, false)
  assert.equal(manager.window.visible, false)
}

async function testResizeToContentExpandsHeightWithoutMovingTopAnchor() {
  const manager = new SelectionToolbarManager({
    BrowserWindow: FakeBrowserWindow,
    screen: {
      getCursorScreenPoint: () => ({ x: 320, y: 240 }),
      getDisplayNearestPoint: () => ({
        workArea: { x: 0, y: 0, width: 1440, height: 900 }
      })
    },
    isDevelopmentRuntime: () => false
  })

  manager.showAtSelection({
    text: 'accurate',
    point: { x: 300, y: 220 },
    trigger: 'mouse_selection'
  })
  manager.window.handlers['did-finish-load']()

  const beforeResize = { ...manager.window.bounds }
  manager.resizeToContent(520)

  assert.equal(manager.window.bounds.y, beforeResize.y)
  assert.equal(manager.window.bounds.height, 520)
}

async function main() {
  await testShowAtSelectionSendsActivationPayload()
  await testCalculateBoundsPinsToolbarToTopRight()
  await testShowAtSelectionKeepsDraggedPositionForSameText()
  await testShowAtSelectionKeepsOriginalPositionForSameVisibleText()
  await testShowAtSelectionDoesNotResendActivationForSameVisibleText()
  await testPinnedToolbarIgnoresBlur()
  await testPinnedToolbarKeepsManualPositionForNewSelection()
  await testBackdropPointerDownHidesWhenNotPinned()
  await testResizeToContentExpandsHeightWithoutMovingTopAnchor()
  process.exit(0)
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
