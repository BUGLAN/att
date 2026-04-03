const { BrowserWindow, screen } = require('electron')
const path = require('path')
const { isDevelopmentRuntime } = require('../utils/runtime')

const WINDOW_WIDTH = 420
const WINDOW_COMPACT_HEIGHT = 320
const WINDOW_EXPANDED_HEIGHT = 520
const WINDOW_SAFE_MARGIN = 24
const DEV_SERVER_URL = 'http://localhost:8090'

class SelectionToolbarManager {
  constructor(dependencies = {}) {
    this.BrowserWindow = dependencies.BrowserWindow || BrowserWindow
    this.screen = dependencies.screen || screen
    this.isDevelopmentRuntime = dependencies.isDevelopmentRuntime || isDevelopmentRuntime
    this.window = null
    this.isReady = false
    this.isVisible = false
    this.pendingActivationPayload = null
    this.lastShownText = ''
    this.userMovedWindow = false
    this.programmaticMoveLock = 0
    this.isPinned = false
    this.dismissWindows = []
    this.dismissShowTimer = null
  }

  createWindow() {
    if (this.window) {
      return this.window
    }

    const isDev = this.isDevelopmentRuntime()
    this.window = new this.BrowserWindow({
      width: WINDOW_WIDTH,
      height: WINDOW_COMPACT_HEIGHT,
      show: false,
      frame: false,
      transparent: true,
      backgroundColor: '#00000000',
      alwaysOnTop: true,
      resizable: false,
      maximizable: false,
      minimizable: false,
      fullscreenable: false,
      movable: true,
      skipTaskbar: true,
      focusable: true,
      hasShadow: true,
      webPreferences: {
        nodeIntegration: false,
        contextIsolation: true,
        backgroundThrottling: false,
        preload: path.join(__dirname, '../preload.js')
      }
    })

    this.window.setAlwaysOnTop(true, 'screen-saver')
    this.window.setVisibleOnAllWorkspaces(true, {
      visibleOnFullScreen: true
    })

    this.window.webContents.once('did-finish-load', () => {
      this.isReady = true
      this.sendActivation()
    })

    if (isDev) {
      this.window.loadURL(`${DEV_SERVER_URL}/selection-toolbar.html`)
    } else {
      this.window.loadFile(path.join(__dirname, '../../dist/selection-toolbar.html'))
    }

    this.window.on('blur', () => {
      if (this.isPinned) {
        return
      }
      this.hide()
    })

    this.window.on('move', () => {
      if (!this.window || !this.isVisible || this.programmaticMoveLock > 0) {
        return
      }
      this.userMovedWindow = true
      if (!this.isPinned) {
        this.updateDismissWindowsForCurrentBounds()
      }
    })

    this.window.on('closed', () => {
      this.destroyDismissWindows()
      this.window = null
      this.isReady = false
      this.isVisible = false
      this.pendingActivationPayload = null
      this.lastShownText = ''
      this.userMovedWindow = false
      this.programmaticMoveLock = 0
      this.isPinned = false
    })

    return this.window
  }

  calculateBounds(point = null) {
    const anchorPoint = point || this.screen.getCursorScreenPoint()
    const display = this.screen.getDisplayNearestPoint(anchorPoint)
    const { workArea } = display
    const nextHeight = Math.min(WINDOW_COMPACT_HEIGHT, Math.max(280, workArea.height - WINDOW_SAFE_MARGIN * 2))
    const nextX = workArea.x + workArea.width - WINDOW_WIDTH - WINDOW_SAFE_MARGIN
    const nextY = workArea.y + WINDOW_SAFE_MARGIN

    return {
      x: nextX,
      y: nextY,
      width: WINDOW_WIDTH,
      height: nextHeight
    }
  }

  buildDismissRegions(workArea, overlayBounds) {
    const overlayRight = overlayBounds.x + overlayBounds.width
    const overlayBottom = overlayBounds.y + overlayBounds.height
    const workAreaRight = workArea.x + workArea.width
    const workAreaBottom = workArea.y + workArea.height

    return [
      {
        slot: 'top',
        x: workArea.x,
        y: workArea.y,
        width: workArea.width,
        height: overlayBounds.y - workArea.y
      },
      {
        slot: 'bottom',
        x: workArea.x,
        y: overlayBottom,
        width: workArea.width,
        height: workAreaBottom - overlayBottom
      },
      {
        slot: 'left',
        x: workArea.x,
        y: overlayBounds.y,
        width: overlayBounds.x - workArea.x,
        height: overlayBounds.height
      },
      {
        slot: 'right',
        x: overlayRight,
        y: overlayBounds.y,
        width: workAreaRight - overlayRight,
        height: overlayBounds.height
      }
    ].filter((region) => region.width > 0 && region.height > 0)
  }

  createDismissWindows(workArea, overlayBounds) {
    this.destroyDismissWindows()

    const regions = this.buildDismissRegions(workArea, overlayBounds)
    this.dismissWindows = regions.map((region) => {
      const window = new this.BrowserWindow({
        x: region.x,
        y: region.y,
        width: region.width,
        height: region.height,
        show: false,
        frame: false,
        transparent: true,
        backgroundColor: '#00000000',
        resizable: false,
        movable: false,
        minimizable: false,
        maximizable: false,
        fullscreenable: false,
        skipTaskbar: true,
        focusable: true,
        hasShadow: false,
        webPreferences: {
          nodeIntegration: false,
          contextIsolation: true,
          backgroundThrottling: false,
          preload: path.join(__dirname, 'backdropPreload.js')
        }
      })

      window.setAlwaysOnTop(true, 'pop-up-menu')
      window.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true })
      window.loadURL('data:text/html;charset=utf-8,<html><body style="margin:0;background:rgba(0,0,0,0.01);"></body></html>')

      return {
        slot: region.slot,
        window,
        bounds: region
      }
    })
  }

  clearDismissShowTimer() {
    if (!this.dismissShowTimer) {
      return
    }

    clearTimeout(this.dismissShowTimer)
    this.dismissShowTimer = null
  }

  showDismissWindows() {
    if (this.isPinned || this.dismissWindows.length === 0) {
      return
    }

    for (const item of this.dismissWindows) {
      item.window.showInactive()
    }

    this.window?.moveTop()
  }

  scheduleDismissWindows() {
    this.clearDismissShowTimer()
    this.dismissShowTimer = setTimeout(() => {
      if (!this.window || this.window.isDestroyed() || !this.isVisible || this.isPinned) {
        return
      }

      this.showDismissWindows()
      this.dismissShowTimer = null
    }, 72)
  }

  destroyDismissWindows() {
    this.clearDismissShowTimer()
    if (this.dismissWindows.length === 0) {
      return
    }

    for (const item of this.dismissWindows) {
      if (item.window && !item.window.isDestroyed()) {
        item.window.destroy()
      }
    }

    this.dismissWindows = []
  }

  getDisplayForBounds(bounds) {
    return this.screen.getDisplayNearestPoint({
      x: bounds.x + Math.round(bounds.width / 2),
      y: bounds.y + Math.round(bounds.height / 2)
    })
  }

  updateDismissWindowsForCurrentBounds() {
    if (!this.window || this.isPinned || !this.isVisible) {
      return
    }

    const bounds = this.window.getBounds()
    const display = this.getDisplayForBounds(bounds)
    this.createDismissWindows(display.workArea, bounds)
    this.scheduleDismissWindows()
  }

  applyBounds(bounds = {}) {
    if (!this.window) {
      return
    }

    this.programmaticMoveLock += 1
    this.window.setBounds(bounds)

    setImmediate(() => {
      this.programmaticMoveLock = Math.max(0, this.programmaticMoveLock - 1)
    })
  }

  showAtSelection(payload = {}) {
    const text = String(payload.text || '').trim()
    if (!text) {
      return
    }

    const window = this.createWindow()
    const isSameText = text === this.lastShownText
    this.pendingActivationPayload = {
      text,
      point: payload.point || null,
      trigger: payload.trigger || 'selection'
    }

    const shouldReuseVisibleState = this.isVisible && isSameText
    const shouldKeepManualBounds = this.isVisible && (this.isPinned || this.userMovedWindow)

    if (!shouldReuseVisibleState && !shouldKeepManualBounds) {
      this.applyBounds(this.calculateBounds(payload.point))
      this.userMovedWindow = false
    }
    if (!this.isVisible && typeof window.showInactive === 'function') {
      window.showInactive()
    } else if (!this.isVisible) {
      window.show()
    }

    this.isVisible = true
    this.lastShownText = text
    if (!this.isPinned) {
      this.updateDismissWindowsForCurrentBounds()
    } else {
      this.destroyDismissWindows()
    }
    if (!shouldReuseVisibleState) {
      this.sendActivation()
    }
  }

  resizeToContent(contentHeight) {
    if (!this.window) {
      return
    }

    const nextHeight = Math.max(
      WINDOW_COMPACT_HEIGHT,
      Math.min(WINDOW_EXPANDED_HEIGHT, Math.round(Number(contentHeight) || WINDOW_COMPACT_HEIGHT))
    )
    const currentBounds = this.window.getBounds()

    if (currentBounds.height === nextHeight) {
      return
    }

    this.applyBounds({
      ...currentBounds,
      height: nextHeight
    })

    if (!this.isPinned && this.isVisible) {
      this.updateDismissWindowsForCurrentBounds()
    }
  }

  sendActivation() {
    if (!this.window || !this.isReady || !this.pendingActivationPayload) {
      return
    }

    this.window.webContents.send('selection-toolbar:activate', this.pendingActivationPayload)
  }

  sendPinnedState() {
    if (!this.window || !this.isReady) {
      return
    }

    this.window.webContents.send('selection-toolbar:pinned-changed', {
      pinned: this.isPinned
    })
  }

  togglePinned(forceValue = null) {
    this.isPinned = typeof forceValue === 'boolean' ? forceValue : !this.isPinned
    if (this.isVisible) {
      if (this.isPinned) {
        this.destroyDismissWindows()
      } else {
        this.updateDismissWindowsForCurrentBounds()
      }
    }
    this.sendPinnedState()
    return this.isPinned
  }

  handleBackdropPointerDown(payload = {}) {
    void payload
    if (!this.isVisible || this.isPinned) {
      return
    }

    this.hide()
  }

  hide() {
    if (!this.window) {
      return
    }

    this.destroyDismissWindows()
    this.window.hide()
    this.isVisible = false
  }

  destroy() {
    if (!this.window) {
      return
    }

    this.window.destroy()
    this.destroyDismissWindows()
    this.window = null
    this.isReady = false
    this.isVisible = false
    this.pendingActivationPayload = null
    this.lastShownText = ''
    this.userMovedWindow = false
    this.programmaticMoveLock = 0
    this.isPinned = false
  }
}

module.exports = {
  SelectionToolbarManager
}
