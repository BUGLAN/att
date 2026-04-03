const { BrowserWindow, screen } = require('electron')
const path = require('path')
const { isDevelopmentRuntime } = require('../utils/runtime')
const { OVERLAY_MODE, OVERLAY_CHANNELS } = require('./constants')

const WINDOW_SAFE_MARGIN = 24
const MODE_CONFIG = {
  [OVERLAY_MODE.selection]: {
    width: 420,
    height: 320,
    focusOnShow: false,
    closeOnBlur: false
  },
  [OVERLAY_MODE.clipboard]: {
    width: 380,
    height: 560,
    focusOnShow: true,
    closeOnBlur: true
  }
}
const DEV_SERVER_URL = 'http://localhost:8090'

class UnifiedOverlayManager {
  constructor(dependencies = {}) {
    this.BrowserWindow = dependencies.BrowserWindow || BrowserWindow
    this.screen = dependencies.screen || screen
    this.isDevelopmentRuntime = dependencies.isDevelopmentRuntime || isDevelopmentRuntime
    this.window = null
    this.isWindowLoaded = false
    this.isVisible = false
    this.isPinned = false
    this.currentMode = null
    this.pendingActivation = null
    this.rendererReady = false
    this.showSequence = 0
    this.programmaticMoveLock = 0
    this.userMovedWindow = false
    this.lastPinnedBounds = null
    this.lastManualBounds = {
      [OVERLAY_MODE.selection]: null,
      [OVERLAY_MODE.clipboard]: null
    }
    this.ignoreBlurUntil = 0
  }

  createWindow() {
    if (this.window) {
      return this.window
    }

    const selectionConfig = MODE_CONFIG[OVERLAY_MODE.selection]
    const isDev = this.isDevelopmentRuntime()
    this.window = new this.BrowserWindow({
      width: selectionConfig.width,
      height: selectionConfig.height,
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
      this.isWindowLoaded = true
      this.dispatchActivation()
    })

    if (isDev) {
      this.window.loadURL(`${DEV_SERVER_URL}/unified-overlay.html`)
    } else {
      this.window.loadFile(path.join(__dirname, '../../dist/unified-overlay.html'))
    }

    this.window.on('blur', () => {
      if (!this.isVisible || this.isPinned) {
        return
      }

      if (Date.now() < this.ignoreBlurUntil) {
        return
      }

      if (this.getModeConfig().closeOnBlur) {
        this.hide('blur')
      }
    })

    this.window.on('move', () => {
      if (!this.window || !this.isVisible || this.programmaticMoveLock > 0) {
        return
      }

      this.userMovedWindow = true
      if (this.currentMode) {
        this.lastManualBounds[this.currentMode] = this.window.getBounds()
      }
      if (this.isPinned) {
        this.lastPinnedBounds = this.window.getBounds()
      }
    })

    this.window.on('closed', () => {
      this.window = null
      this.isWindowLoaded = false
      this.isVisible = false
      this.currentMode = null
      this.pendingActivation = null
      this.rendererReady = false
      this.programmaticMoveLock = 0
      this.userMovedWindow = false
      this.ignoreBlurUntil = 0
    })

    return this.window
  }

  getModeConfig(mode = this.currentMode) {
    return MODE_CONFIG[mode] || MODE_CONFIG[OVERLAY_MODE.selection]
  }

  calculateSelectionBounds(point = null) {
    void point
    const display = typeof this.screen.getPrimaryDisplay === 'function'
      ? this.screen.getPrimaryDisplay()
      : this.screen.getDisplayNearestPoint(this.screen.getCursorScreenPoint())
    const { workArea } = display
    const config = this.getModeConfig(OVERLAY_MODE.selection)

    return {
      x: workArea.x + workArea.width - config.width - WINDOW_SAFE_MARGIN,
      y: workArea.y + WINDOW_SAFE_MARGIN,
      width: config.width,
      height: config.height
    }
  }

  calculateClipboardBounds() {
    const cursorPoint = this.screen.getCursorScreenPoint()
    const display = this.screen.getDisplayNearestPoint(cursorPoint)
    const { workArea } = display
    const config = this.getModeConfig(OVERLAY_MODE.clipboard)
    const minX = workArea.x + WINDOW_SAFE_MARGIN
    const minY = workArea.y + WINDOW_SAFE_MARGIN
    const maxX = workArea.x + workArea.width - config.width - WINDOW_SAFE_MARGIN
    const maxY = workArea.y + workArea.height - config.height - WINDOW_SAFE_MARGIN

    return {
      x: Math.max(minX, Math.min(Math.round(cursorPoint.x - (config.width / 2)), maxX)),
      y: Math.max(minY, Math.min(Math.round(cursorPoint.y - (config.height / 2)), maxY)),
      width: config.width,
      height: config.height
    }
  }

  getTargetBounds(mode, payload = {}) {
    const manualBounds = this.lastManualBounds[mode]
    if (manualBounds) {
      return {
        ...manualBounds,
        width: this.getModeConfig(mode).width,
        height: this.getModeConfig(mode).height
      }
    }

    if (this.isPinned && this.lastPinnedBounds) {
      return {
        ...this.lastPinnedBounds,
        width: this.getModeConfig(mode).width,
        height: this.getModeConfig(mode).height
      }
    }

    if (mode === OVERLAY_MODE.selection) {
      return this.calculateSelectionBounds(payload.point || null)
    }

    return this.calculateClipboardBounds()
  }

  applyBounds(bounds) {
    if (!this.window || !bounds) {
      return
    }

    this.programmaticMoveLock += 1
    this.window.setBounds(bounds)

    setImmediate(() => {
      this.programmaticMoveLock = Math.max(0, this.programmaticMoveLock - 1)
    })
  }

  normalizePointForBounds(point) {
    if (!point || !Number.isFinite(point.x) || !Number.isFinite(point.y)) {
      return null
    }

    if (typeof this.screen.screenToDipPoint === 'function') {
      return this.screen.screenToDipPoint(point)
    }

    return point
  }

  buildActivation(mode, payload = {}) {
    this.showSequence += 1
    return {
      mode,
      pinned: this.isPinned,
      sequence: this.showSequence,
      focusSearch: payload.focusSearch !== false,
      point: payload.point || null,
      trigger: payload.trigger || null,
      text: typeof payload.text === 'string' ? payload.text.trim() : ''
    }
  }

  openSelection(payload = {}) {
    const text = String(payload.text || '').trim()
    if (!text) {
      return false
    }

    if (this.isVisible && this.currentMode === OVERLAY_MODE.clipboard) {
      return false
    }

    const window = this.createWindow()
    const activation = this.buildActivation(OVERLAY_MODE.selection, {
      ...payload,
      text
    })

    this.currentMode = OVERLAY_MODE.selection
    this.pendingActivation = activation
    this.rendererReady = false

    if (!this.isVisible || !this.userMovedWindow || this.isPinned) {
      this.applyBounds(this.getTargetBounds(OVERLAY_MODE.selection, payload))
    }

    if (this.isWindowLoaded) {
      this.dispatchActivation()
    }

    return window
  }

  openClipboard(payload = {}) {
    const window = this.createWindow()
    const activation = this.buildActivation(OVERLAY_MODE.clipboard, payload)

    this.currentMode = OVERLAY_MODE.clipboard
    this.pendingActivation = activation
    this.rendererReady = false
    this.ignoreBlurUntil = Date.now() + 240
    this.applyBounds(this.getTargetBounds(OVERLAY_MODE.clipboard, payload))

    if (this.isWindowLoaded) {
      this.dispatchActivation()
    }

    return window
  }

  dispatchActivation() {
    if (!this.window || !this.isWindowLoaded || !this.pendingActivation) {
      return
    }

    this.window.webContents.send(OVERLAY_CHANNELS.activate, this.pendingActivation)
    this.sendPinnedState()
  }

  markRendererReady() {
    if (!this.window || !this.pendingActivation) {
      return
    }

    this.rendererReady = true
    if (!this.isVisible) {
      this.showPreparedWindow()
      return
    }

    if (this.getModeConfig().focusOnShow) {
      this.focusWindow()
    }
  }

  showPreparedWindow() {
    if (!this.window || !this.pendingActivation || !this.rendererReady) {
      return
    }

    const config = this.getModeConfig(this.pendingActivation.mode)
    if (!this.isVisible) {
      if (config.focusOnShow) {
        this.ignoreBlurUntil = Date.now() + 240
      }

      if (!config.focusOnShow && typeof this.window.showInactive === 'function') {
        this.window.showInactive()
      } else {
        this.window.show()
      }
    }

    this.isVisible = true
    if (config.focusOnShow) {
      this.focusWindow()
    }
  }

  focusWindow() {
    if (!this.window || this.window.isDestroyed()) {
      return
    }

    this.window.setFocusable(true)
    this.window.moveTop?.()
    this.window.focus?.()
    this.window.webContents.focus?.()
  }

  hide(reason = 'manual') {
    void reason
    if (!this.window) {
      return
    }

    // 销毁窗口而不是隐藏，避免下次打开时闪烁
    this.window.destroy()
    this.window = null
    this.isWindowLoaded = false
    this.isVisible = false
    this.userMovedWindow = false
    this.ignoreBlurUntil = 0
    this.rendererReady = false
    this.pendingActivation = null
  }

  setPinned(active) {
    this.isPinned = active === true
    if (this.isPinned && this.window) {
      this.lastPinnedBounds = this.window.getBounds()
    }
    this.sendPinnedState()
  }

  sendPinnedState() {
    if (!this.window || !this.isWindowLoaded) {
      return
    }

    this.window.webContents.send(OVERLAY_CHANNELS.pinnedChanged, {
      pinned: this.isPinned
    })
  }

  handleActionComplete(payload = {}) {
    void payload
  }

  notifyDataChanged(payload = {}) {
    if (!this.window || this.window.isDestroyed() || !this.isWindowLoaded) {
      return
    }

    this.window.webContents.send('clipboard:changed', payload)
  }

  destroy() {
    if (!this.window) {
      return
    }

    this.window.destroy()
    this.window = null
    this.isWindowLoaded = false
    this.isVisible = false
    this.pendingActivation = null
    this.rendererReady = false
  }

  handleGlobalPointerDown(payload = {}) {
    if (!this.window || this.window.isDestroyed() || !this.isVisible || this.isPinned) {
      return
    }

    const point = this.normalizePointForBounds(payload.point || null)
    if (!point) {
      return
    }

    const bounds = this.window.getBounds?.()
    if (!bounds) {
      return
    }

    const insideBounds = (
      point.x >= bounds.x &&
      point.x <= bounds.x + bounds.width &&
      point.y >= bounds.y &&
      point.y <= bounds.y + bounds.height
    )

    if (!insideBounds) {
      this.hide('outside-pointer')
    }
  }
}

module.exports = {
  UnifiedOverlayManager,
  MODE_CONFIG
}
