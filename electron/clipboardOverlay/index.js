const { BrowserWindow, nativeImage, screen } = require('electron')
const fs = require('fs')
const path = require('path')
const { logElectronOverlay } = require('../logger')
const { isDevelopmentRuntime } = require('../utils/runtime')

const WINDOW_WIDTH = 380
const WINDOW_HEIGHT = 560

class ClipboardOverlayManager {
  constructor() {
    this.window = null
    this.dismissWindows = []
    this.dismissShowTimer = null
    this.isReady = false
    this.isVisible = false
    this.dragInProgress = false
    this.pinned = false
    this.pendingActivationPayload = { focusSearch: true, pinned: false }
    this.ignoreBlurUntil = 0
    this.showSequence = 0
    this.focusGuardPrimary = null
    this.focusGuardSecondary = null
    this.focusGuardFinal = null
  }

  createWindow() {
    if (this.window) {
      logElectronOverlay('overlay-manager', 'Reusing existing overlay window', {
        isVisible: this.isVisible,
        isReady: this.isReady
      })
      return this.window
    }

    const isDev = isDevelopmentRuntime()

    this.window = new BrowserWindow({
      width: WINDOW_WIDTH,
      height: WINDOW_HEIGHT,
      show: false,
      frame: false,
      transparent: true,
      backgroundColor: '#00000000',
      resizable: false,
      maximizable: false,
      minimizable: false,
      fullscreenable: false,
      skipTaskbar: true,
      alwaysOnTop: true,
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
    this.window.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: true })
    logElectronOverlay('overlay-manager', 'Created overlay window', {
      isDev,
      width: WINDOW_WIDTH,
      height: WINDOW_HEIGHT
    })

    this.window.once('ready-to-show', () => {
      logElectronOverlay('overlay-manager', 'Overlay ready-to-show')
    })

    this.window.webContents.once('did-finish-load', () => {
      this.isReady = true
      logElectronOverlay('overlay-manager', 'Overlay did-finish-load', this.pendingActivationPayload)
      this.sendActivation(this.pendingActivationPayload)
    })

    this.window.webContents.on('did-fail-load', (event, errorCode, errorDescription, validatedURL) => {
      logElectronOverlay('overlay-manager', 'Overlay did-fail-load', {
        errorCode,
        errorDescription,
        validatedURL
      })
    })

    if (isDev) {
      this.window.loadURL('http://localhost:5173/clipboard-overlay.html')
    } else {
      this.window.loadFile(path.join(__dirname, '../../dist/clipboard-overlay.html'))
    }

    this.window.on('blur', () => {
      const now = Date.now()
      logElectronOverlay('overlay-manager', 'Overlay blur received', {
        ignoreBlurUntil: this.ignoreBlurUntil,
        now
      })

      if (now < this.ignoreBlurUntil) {
        logElectronOverlay('overlay-manager', 'Blur received during focus grace period, attempting refocus')
        this.scheduleFocusGuard('blur-grace-primary', 18, this.showSequence)
        this.scheduleFocusGuard('blur-grace-secondary', 90, this.showSequence)
        return
      }

      if (this.pinned) {
        logElectronOverlay('overlay-manager', 'Ignoring blur because overlay is pinned')
        return
      }

      if (this.dragInProgress) {
        logElectronOverlay('overlay-manager', 'Ignoring blur while drag is in progress')
        return
      }

      this.hide()
    })

    this.window.on('show', () => {
      logElectronOverlay('overlay-manager', 'Overlay window show event')
    })

    this.window.on('focus', () => {
      logElectronOverlay('overlay-manager', 'Overlay window focus event')
    })

    this.window.on('hide', () => {
      logElectronOverlay('overlay-manager', 'Overlay window hide event')
    })

    this.window.on('closed', () => {
      logElectronOverlay('overlay-manager', 'Overlay window closed')
      this.destroyDismissWindows()
      this.window = null
      this.isReady = false
      this.isVisible = false
      this.dragInProgress = false
      this.pendingActivationPayload = { focusSearch: true, pinned: this.pinned }
      this.clearFocusGuards()
    })

    return this.window
  }

  calculatePosition() {
    const cursorPoint = screen.getCursorScreenPoint()
    const display = screen.getDisplayNearestPoint(cursorPoint)
    const { workArea } = display
    const minX = workArea.x
    const minY = workArea.y
    const maxX = workArea.x + workArea.width - WINDOW_WIDTH
    const maxY = workArea.y + workArea.height - WINDOW_HEIGHT
    const centeredX = Math.round(cursorPoint.x - (WINDOW_WIDTH / 2))
    const centeredY = Math.round(cursorPoint.y - (WINDOW_HEIGHT / 2))

    return {
      x: Math.min(Math.max(centeredX, minX), maxX),
      y: Math.min(Math.max(centeredY, minY), maxY),
      cursorPoint,
      displayId: display.id,
      workArea
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
      const window = new BrowserWindow({
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

      window.on('show', () => {
        logElectronOverlay('overlay-manager', 'Dismiss window show event', {
          slot: region.slot,
          bounds: region
        })
      })

      window.on('hide', () => {
        logElectronOverlay('overlay-manager', 'Dismiss window hide event', {
          slot: region.slot
        })
      })

      window.on('closed', () => {
        logElectronOverlay('overlay-manager', 'Dismiss window closed', {
          slot: region.slot
        })
      })

      return {
        slot: region.slot,
        window,
        bounds: region
      }
    })

    logElectronOverlay('overlay-manager', 'Created dismiss windows', {
      count: this.dismissWindows.length,
      slots: this.dismissWindows.map((item) => item.slot)
    })
  }

  show(payload = {}) {
    const position = this.calculatePosition()
    const overlayBounds = {
      x: position.x,
      y: position.y,
      width: WINDOW_WIDTH,
      height: WINDOW_HEIGHT
    }
    const window = this.createWindow()
    const showSequence = ++this.showSequence

    this.pendingActivationPayload = {
      focusSearch: payload.focusSearch !== false,
      pinned: this.pinned
    }
    this.ignoreBlurUntil = Date.now() + 180
    this.clearFocusGuards()

    logElectronOverlay('overlay-manager', 'Showing overlay window', {
      payload,
      pendingActivationPayload: this.pendingActivationPayload,
      position,
      showSequence
    })

    window.setPosition(position.x, position.y)
    window.setFocusable(true)
    window.show()
    this.isVisible = true
    this.focusWindow('show')

    if (!this.pinned) {
      this.createDismissWindows(position.workArea, overlayBounds)
      this.scheduleDismissWindows(showSequence)
    }

    this.scheduleFocusGuard('post-show-primary', 28, showSequence)
    this.scheduleFocusGuard('post-show-secondary', 96, showSequence)
    this.scheduleFinalFocusGuard(showSequence, 180)

    logElectronOverlay('overlay-manager', 'Overlay window shown', {
      bounds: window.getBounds(),
      isVisible: window.isVisible(),
      isFocused: window.isFocused()
    })

    this.sendActivation(this.pendingActivationPayload)
  }

  hide() {
    if (!this.window) {
      logElectronOverlay('overlay-manager', 'Hide requested without overlay window')
      return
    }

    this.clearFocusGuards()
    this.destroyDismissWindows()
    this.window.hide()
    this.isVisible = false
    logElectronOverlay('overlay-manager', 'Overlay hidden', {
      isVisible: this.window.isVisible()
    })
  }

  toggle(payload = {}) {
    logElectronOverlay('overlay-manager', 'Toggle requested', {
      isVisible: this.isVisible,
      payload
    })

    if (this.isVisible) {
      this.hide()
      return
    }

    this.show(payload)
  }

  sendActivation(payload = {}) {
    if (!this.window || this.window.isDestroyed() || !this.isReady) {
      logElectronOverlay('overlay-manager', 'Activation deferred', {
        hasWindow: !!this.window,
        isDestroyed: this.window?.isDestroyed?.() || false,
        isReady: this.isReady,
        payload
      })
      return
    }

    logElectronOverlay('overlay-manager', 'Sending activation to overlay renderer', payload)
    this.window.webContents.send('clipboard-overlay:activate', payload)
    this.sendPinnedState()
  }

  destroy() {
    if (!this.window && this.dismissWindows.length === 0) {
      logElectronOverlay('overlay-manager', 'Destroy requested without overlay window')
      return
    }

    this.clearFocusGuards()
    this.destroyDismissWindows()
    if (!this.window) {
      return
    }

    logElectronOverlay('overlay-manager', 'Destroying overlay window')
    this.window.destroy()
    this.window = null
    this.isReady = false
    this.isVisible = false
    this.dragInProgress = false
  }

  togglePinned() {
    this.setPinned(!this.pinned)
  }

  setPinned(active) {
    this.pinned = active === true
    this.ignoreBlurUntil = Date.now() + 120
    logElectronOverlay('overlay-manager', 'Updated pinned state', {
      pinned: this.pinned,
      isVisible: this.isVisible
    })

    if (this.isVisible) {
      if (this.pinned) {
        this.destroyDismissWindows()
      } else {
        const bounds = this.window?.getBounds?.()
        if (bounds) {
          const display = screen.getDisplayMatching(bounds)
          this.createDismissWindows(display.workArea, bounds)
          this.scheduleDismissWindows(this.showSequence)
        }
      }
    }

    this.sendPinnedState()
  }

  setDragState(active) {
    this.dragInProgress = active === true

    if (this.dragInProgress) {
      this.ignoreBlurUntil = Date.now() + 5000
      logElectronOverlay('overlay-manager', 'Overlay drag started')
      return
    }

    logElectronOverlay('overlay-manager', 'Overlay drag ended', {
      isFocused: this.window?.isFocused?.() || false,
      isVisible: this.window?.isVisible?.() || false
    })

    if (this.window && this.isVisible && !this.window.isFocused()) {
      this.hide()
    }
  }

  notifyDataChanged(payload = {}) {
    if (!this.window || this.window.isDestroyed() || !this.isReady) {
      return
    }

    this.window.webContents.send('clipboard:changed', payload)
  }

  sendPinnedState() {
    if (!this.window || this.window.isDestroyed() || !this.isReady) {
      return
    }

    this.window.webContents.send('clipboard-overlay:pinned-changed', {
      pinned: this.pinned
    })
  }

  handleBackdropPointerDown(payload = {}) {
    logElectronOverlay('overlay-manager', 'Dismiss window pointer down received', {
      button: payload.button ?? null,
      pinned: this.pinned,
      isVisible: this.isVisible
    })

    if (!this.isVisible || this.pinned || this.dragInProgress) {
      return
    }

    this.hide()
  }

  startNativeDrag(sender, payload = {}) {
    const content = String(payload.content || '').trim()
    if (!content || !sender || typeof sender.startDrag !== 'function') {
      return
    }

    const tempDir = path.join(require('electron').app.getPath('temp'), 'att-clipboard-drag')
    fs.mkdirSync(tempDir, { recursive: true })

    const baseName = String(payload.title || payload.itemType || 'clipboard-item')
      .replace(/[<>:"/\\|?*\x00-\x1F]/g, '_')
      .replace(/\s+/g, ' ')
      .trim()
      .slice(0, 48) || 'clipboard-item'

    const filePath = path.join(tempDir, `${Date.now()}-${baseName}.txt`)
    fs.writeFileSync(filePath, `${content}\n`, 'utf8')

    sender.startDrag({
      file: filePath,
      icon: nativeImage.createFromPath(path.join(__dirname, '../icons/icon.png'))
    })

    logElectronOverlay('overlay-manager', 'Started native drag for overlay item', {
      filePath,
      itemType: payload.itemType || null,
      itemId: payload.itemId || null
    })
  }

  clearFocusGuards() {
    for (const timer of [this.focusGuardPrimary, this.focusGuardSecondary, this.focusGuardFinal]) {
      if (timer) {
        clearTimeout(timer)
      }
    }

    this.focusGuardPrimary = null
    this.focusGuardSecondary = null
    this.focusGuardFinal = null
  }

  clearDismissShowTimer() {
    if (this.dismissShowTimer) {
      clearTimeout(this.dismissShowTimer)
      this.dismissShowTimer = null
    }
  }

  focusWindow(reason) {
    if (!this.window || this.window.isDestroyed() || !this.isVisible) {
      return
    }

    const focusedWindow = BrowserWindow.getFocusedWindow()
    if (focusedWindow && focusedWindow !== this.window && !focusedWindow.isDestroyed()) {
      focusedWindow.blur()
    }

    this.window.setFocusable(true)
    this.window.setAlwaysOnTop(true, 'screen-saver')
    this.window.moveTop()
    this.window.focus()
    this.window.webContents.focus()

    logElectronOverlay('overlay-manager', 'Applied overlay focus request', {
      reason,
      blurredOtherWindow: !!focusedWindow && focusedWindow !== this.window,
      isFocused: this.window.isFocused(),
      isVisible: this.window.isVisible()
    })
  }

  scheduleFocusGuard(reason, delay, showSequence) {
    const timer = setTimeout(() => {
      if (
        !this.window ||
        this.window.isDestroyed() ||
        !this.isVisible ||
        this.showSequence !== showSequence ||
        this.pinned
      ) {
        return
      }

      if (!this.window.isFocused()) {
        this.focusWindow(reason)
        return
      }

      logElectronOverlay('overlay-manager', 'Skipped focus guard because overlay is already focused', {
        reason,
        showSequence
      })
    }, delay)

    if (reason.includes('secondary')) {
      this.focusGuardSecondary = timer
      return
    }

    this.focusGuardPrimary = timer
  }

  scheduleFinalFocusGuard(showSequence, delay) {
    this.focusGuardFinal = setTimeout(() => {
      if (
        !this.window ||
        this.window.isDestroyed() ||
        !this.isVisible ||
        this.showSequence !== showSequence ||
        this.pinned ||
        this.dragInProgress
      ) {
        return
      }

      if (this.window.isFocused()) {
        logElectronOverlay('overlay-manager', 'Overlay remained focused after grace period', {
          showSequence
        })
        return
      }

      logElectronOverlay('overlay-manager', 'Overlay failed to keep focus after grace period, hiding window', {
        showSequence
      })
      this.hide()
    }, delay)
  }

  showDismissWindows() {
    if (this.pinned || this.dismissWindows.length === 0) {
      return
    }

    for (const item of this.dismissWindows) {
      item.window.showInactive()
    }

    this.window?.moveTop()
    logElectronOverlay('overlay-manager', 'Showing dismiss windows', {
      count: this.dismissWindows.length,
      slots: this.dismissWindows.map((item) => item.slot)
    })
  }

  scheduleDismissWindows(showSequence) {
    this.clearDismissShowTimer()
    this.dismissShowTimer = setTimeout(() => {
      if (
        !this.window ||
        this.window.isDestroyed() ||
        !this.isVisible ||
        this.showSequence !== showSequence ||
        this.pinned
      ) {
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
    logElectronOverlay('overlay-manager', 'Destroyed dismiss windows')
  }
}

module.exports = { ClipboardOverlayManager }
