const { BrowserWindow, screen } = require('electron')
const path = require('path')

const WINDOW_WIDTH = 512
const WINDOW_HEIGHT = 40
const WINDOW_BOTTOM_OFFSET = 32

function createDefaultState() {
  return {
    phase: 'idle',
    duration: 0,
    text: ''
  }
}

class FloatingWindowManager {
  constructor() {
    this.floatingWindow = null
    this.isVisible = false
    this.isReady = false
    this.lastState = createDefaultState()
  }

  createWindow() {
    if (this.floatingWindow) {
      return this.floatingWindow
    }

    const isDev = process.env.NODE_ENV === 'development'

    this.floatingWindow = new BrowserWindow({
      width: WINDOW_WIDTH,
      height: WINDOW_HEIGHT,
      show: false,
      frame: false,
      transparent: true,
      backgroundColor: '#00000000',
      alwaysOnTop: true,
      resizable: false,
      maximizable: false,
      minimizable: false,
      fullscreenable: false,
      movable: false,
      skipTaskbar: true,
      hasShadow: false,
      webPreferences: {
        nodeIntegration: false,
        contextIsolation: true,
        backgroundThrottling: false,
        preload: path.join(__dirname, 'preload.js')
      }
    })

    this.floatingWindow.setAlwaysOnTop(true, 'screen-saver')
    this.floatingWindow.setVisibleOnAllWorkspaces(true, {
      visibleOnFullScreen: true
    })

    this.floatingWindow.webContents.once('did-finish-load', () => {
      this.isReady = true
      this.sendState()
    })

    if (isDev) {
      this.floatingWindow.loadURL('http://localhost:5173/floating.html')
    } else {
      this.floatingWindow.loadFile(path.join(__dirname, '../../dist/floating.html'))
    }

    this.floatingWindow.on('closed', () => {
      this.floatingWindow = null
      this.isVisible = false
      this.isReady = false
    })

    return this.floatingWindow
  }

  calculatePosition() {
    const { workArea } = screen.getPrimaryDisplay()

    return {
      x: Math.floor(workArea.x + ((workArea.width - WINDOW_WIDTH) / 2)),
      y: Math.floor(workArea.y + workArea.height - WINDOW_HEIGHT - WINDOW_BOTTOM_OFFSET)
    }
  }

  show() {
    const window = this.createWindow()
    const position = this.calculatePosition()

    window.setPosition(position.x, position.y)
    if (typeof window.showInactive === 'function') {
      window.showInactive()
    } else {
      window.show()
    }

    this.isVisible = true
    this.sendState()
  }

  hide() {
    if (!this.floatingWindow) {
      return
    }

    this.floatingWindow.hide()
    this.isVisible = false
    this.lastState = createDefaultState()
    this.sendState()
  }

  toggle() {
    if (this.isVisible) {
      this.hide()
      return
    }

    this.show()
  }

  updateState(payload = {}) {
    this.lastState = {
      ...createDefaultState(),
      ...this.lastState,
      ...payload
    }

    if (!this.floatingWindow && this.lastState.phase !== 'idle') {
      this.createWindow()
    }

    this.sendState()
  }

  sendState() {
    if (!this.floatingWindow || this.floatingWindow.isDestroyed() || !this.isReady) {
      return
    }

    this.floatingWindow.webContents.send('floating:state', this.lastState)
  }

  destroy() {
    if (!this.floatingWindow) {
      return
    }

    this.floatingWindow.destroy()
    this.floatingWindow = null
    this.isVisible = false
    this.isReady = false
    this.lastState = createDefaultState()
  }
}

module.exports = { FloatingWindowManager }
