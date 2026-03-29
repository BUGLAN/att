const { app, BrowserWindow, ipcMain, clipboard } = require('electron')
const path = require('path')
const { execFile } = require('child_process')
const { PythonManager } = require('./python/manager')
const { DesktopAudioController } = require('./audio/controller')
const { TrayManager } = require('./tray/index')
const { FloatingWindowManager } = require('./floatingWindow')
const { ClipboardOverlayManager } = require('./clipboardOverlay')
const { ClipboardMonitor } = require('./clipboardMonitor')
const { logElectronOverlay, electronOverlayLogFile } = require('./logger')
const { DesktopInputManager, normalizeSettings } = require('./input/listener')
const { isDevelopmentRuntime } = require('./utils/runtime')

let mainWindow = null
let pythonManager = null
let trayManager = null
let audioController = null
let floatingWindowManager = null
let clipboardOverlayManager = null
let clipboardMonitor = null
let inputManager = null
let isQuitting = false
let lastClipboardOverlayCommand = {
  channel: null,
  timestamp: 0
}
const pasteHelperPath = path.join(__dirname, 'python', 'paste_helper.py')

const isDev = isDevelopmentRuntime()
const CLIPBOARD_OVERLAY_COMMAND_DEDUP_MS = 180

function shouldSuppressClipboardOverlayCommand(channel, payload = {}) {
  const now = Date.now()
  const sameChannel = lastClipboardOverlayCommand.channel === channel
  const withinWindow = (now - lastClipboardOverlayCommand.timestamp) <= CLIPBOARD_OVERLAY_COMMAND_DEDUP_MS

  if (sameChannel && withinWindow) {
    logElectronOverlay('main', 'Suppressed duplicate clipboard overlay command', {
      channel,
      payload,
      lastTimestamp: lastClipboardOverlayCommand.timestamp,
      now,
      dedupWindowMs: CLIPBOARD_OVERLAY_COMMAND_DEDUP_MS
    })
    return true
  }

  lastClipboardOverlayCommand = {
    channel,
    timestamp: now
  }
  return false
}

async function createWindow() {
  logElectronOverlay('main', 'Creating main window')
  mainWindow = new BrowserWindow({
    width: 1200,
    height: 800,
    minWidth: 800,
    minHeight: 600,
    icon: path.join(__dirname, 'icons', 'icon.png'),
    frame: false,
    titleBarStyle: 'hidden',
    backgroundColor: '#0a0a0b',
    backgroundThrottling: false,
    webPreferences: {
      nodeIntegration: false,
      contextIsolation: true,
      preload: path.join(__dirname, 'preload.js')
    }
  })

  mainWindow.webContents.session.setPermissionRequestHandler((webContents, permission, callback) => {
    callback(permission === 'media')
  })

  mainWindow.on('close', (event) => {
    if (process.platform === 'darwin' || isQuitting) {
      return
    }

    event.preventDefault()
    isQuitting = true
    app.quit()
  })

  if (isDev) {
    await mainWindow.loadURL('http://localhost:5173')
    mainWindow.webContents.openDevTools()
  } else {
    await mainWindow.loadFile(path.join(__dirname, '../dist/index.html'))
  }

  logElectronOverlay('main', 'Main window loaded', {
    isDev
  })

  mainWindow.on('closed', () => {
    mainWindow = null
  })

  if (trayManager) {
    trayManager.mainWindow = mainWindow
  }
}

async function showMainWindow({ navigateToSettings = false } = {}) {
  logElectronOverlay('main', 'Showing main window', {
    navigateToSettings
  })

  if (!mainWindow) {
    await createWindow()
  }

  if (mainWindow.isMinimized()) {
    mainWindow.restore()
  }

  mainWindow.show()

  if (navigateToSettings) {
    mainWindow.webContents.send('navigate:settings')
  }

  mainWindow.focus()
}

async function startBackend() {
  pythonManager = new PythonManager()
  const port = await pythonManager.start()
  audioController = new DesktopAudioController(() => pythonManager.getPythonPath())
  console.log(`Python backend started on port ${port}`)
  return port
}

function getDefaultDesktopInputSettings() {
  return normalizeSettings()
}

async function loadDesktopInputSettings(port) {
  const fallback = getDefaultDesktopInputSettings()

  try {
    const response = await fetch(`http://127.0.0.1:${port}/api/settings`)
    if (!response.ok) {
      throw new Error(`Backend settings request failed with status ${response.status}`)
    }

    const payload = await response.json()
    return normalizeSettings(payload)
  } catch (error) {
    logElectronOverlay('main', 'Failed to load desktop input settings from backend', {
      error: error.message,
      fallback
    })
    return fallback
  }
}

function handleClipboardOverlayTrigger(payload = {}) {
  if (shouldSuppressClipboardOverlayCommand('global-input:clipboard-overlay', payload)) {
    return
  }

  if (payload.retriggerAction === 'show_only') {
    clipboardOverlayManager?.show({
      focusSearch: payload.focusSearch !== false
    })
    return
  }

  clipboardOverlayManager?.toggle({
    focusSearch: payload.focusSearch !== false
  })
}

async function initializeDesktopInput(port) {
  const settings = await loadDesktopInputSettings(port)
  inputManager = new DesktopInputManager({
    onHotkeyToggle: (payload) => {
      logElectronOverlay('main', 'Desktop hotkey toggle', payload)
      mainWindow?.webContents.send('hotkey:toggle', payload)
    },
    onClipboardTrigger: (payload) => {
      logElectronOverlay('main', 'Desktop clipboard trigger', payload)
      handleClipboardOverlayTrigger(payload)
    },
    onError: (error) => {
      logElectronOverlay('main', 'Desktop input error', {
        error: error.message,
        stack: error.stack
      })
    },
    log: (message, details) => {
      logElectronOverlay('desktop-input', message, details)
    }
  })

  inputManager.updateSettings(settings)
  inputManager.start()
  logElectronOverlay('main', 'Desktop input initialized', settings)
}

function setupTray() {
  trayManager = new TrayManager(mainWindow, null)
  trayManager.create()

  trayManager.on('quit', () => {
    app.quit()
  })

  trayManager.on('copy-and-paste', async (text) => {
    clipboardMonitor?.markInternalWrite(text, 'tray_copy_and_paste')
    clipboard.writeText(text)
    mainWindow?.webContents.send('paste-text')
  })
}

function notifyClipboardChanged(payload = {}) {
  mainWindow?.webContents.send('clipboard:changed', payload)
  clipboardOverlayManager?.notifyDataChanged(payload)
}

function setupIPC() {
  logElectronOverlay('main', 'Setting up IPC handlers', {
    logFile: electronOverlayLogFile
  })

  ipcMain.handle('backend:port', () => {
    return pythonManager?.port || 8000
  })

  ipcMain.handle('clipboard:write', (event, text) => {
    clipboardMonitor?.markInternalWrite(text, 'renderer_write')
    clipboard.writeText(text)
    return true
  })

  ipcMain.handle('clipboard:read-text', () => {
    return clipboard.readText()
  })

  ipcMain.handle('clipboard:paste-active', async () => {
    return new Promise((resolve) => {
      const pythonPath = pythonManager?.getPythonPath?.()
      if (!pythonPath) {
        resolve({ success: false, error: 'Python runtime unavailable' })
        return
      }

      execFile(
        pythonPath,
        [pasteHelperPath],
        { cwd: path.join(__dirname, '..') },
        (error, stdout, stderr) => {
          const output = (stdout || '').trim()
          if (output) {
            try {
              const parsed = JSON.parse(output)
              if (error && parsed.success !== false) {
                resolve({
                  ...parsed,
                  success: false,
                  error: stderr?.trim() || error.message
                })
                return
              }
              resolve(parsed)
              return
            } catch (parseError) {
              // Fall through to generic error handling.
            }
          }

          resolve({
            success: false,
            error: (stderr || '').trim() || error?.message || 'Paste helper returned no output'
          })
        }
      )
    })
  })

  ipcMain.handle('desktop:sync-settings', async (event, settings = {}) => {
    if (!audioController) {
      return { success: false, error: 'Audio controller unavailable' }
    }

    const result = await audioController.updateSettings(settings)
    if (!result.success) {
      console.error('[Audio] Failed to sync settings:', result.error)
    }
    return result
  })

  ipcMain.handle('desktop:recording-state', async (event, payload = {}) => {
    if (!audioController) {
      return { success: false, error: 'Audio controller unavailable' }
    }

    const result = payload.recording
      ? await audioController.pauseForRecording()
      : await audioController.resumeAfterRecording()

    if (!result.success) {
      console.error('[Audio] Failed to sync recording state:', result.error)
    }
    return result
  })

  ipcMain.on('window:minimize', () => {
    mainWindow?.minimize()
  })

  ipcMain.on('window:maximize', () => {
    if (mainWindow?.isMaximized()) {
      mainWindow.unmaximize()
    } else {
      mainWindow?.maximize()
    }
  })

  ipcMain.on('window:close', () => {
    isQuitting = true
    app.quit()
  })

  ipcMain.on('tray:set-icon', (event, iconType) => {
    trayManager?.setIcon(iconType)
  })

  ipcMain.on('tray:update-menu', (event, records = []) => {
    trayManager?.updateMenu(records)
  })

  ipcMain.handle('hotkey:update', async (event, hotkey = null) => {
    if (!inputManager) {
      return {
        success: false,
        error: 'Desktop input manager unavailable'
      }
    }

    const settings = inputManager.updateSettings({ hotkey })
    return {
      success: true,
      managedBy: 'electron',
      hotkey: settings.hotkey
    }
  })

  ipcMain.handle('desktop:sync-input-settings', async (event, payload = {}) => {
    if (!inputManager) {
      return {
        success: false,
        error: 'Desktop input manager unavailable'
      }
    }

    const settings = inputManager.updateSettings(payload)
    return {
      success: true,
      managedBy: 'electron',
      settings
    }
  })

  ipcMain.on('floating:show', () => {
    floatingWindowManager?.show()
  })

  ipcMain.on('floating:hide', () => {
    floatingWindowManager?.hide()
  })

  ipcMain.on('floating:toggle', () => {
    floatingWindowManager?.toggle()
  })

  ipcMain.on('floating:update-state', (event, payload = {}) => {
    floatingWindowManager?.updateState(payload)
  })

  ipcMain.on('clipboard-overlay:show', (event, payload = {}) => {
    if (shouldSuppressClipboardOverlayCommand('clipboard-overlay:show', payload)) {
      return
    }

    logElectronOverlay('main', 'Received clipboard-overlay:show', payload)
    clipboardOverlayManager?.show(payload)
  })

  ipcMain.on('clipboard-overlay:hide', () => {
    logElectronOverlay('main', 'Received clipboard-overlay:hide')
    clipboardOverlayManager?.hide()
  })

  ipcMain.on('clipboard-overlay:toggle', (event, payload = {}) => {
    if (shouldSuppressClipboardOverlayCommand('clipboard-overlay:toggle', payload)) {
      return
    }

    logElectronOverlay('main', 'Received clipboard-overlay:toggle', payload)
    clipboardOverlayManager?.toggle(payload)
  })

  ipcMain.on('clipboard-overlay:toggle-pin', () => {
    logElectronOverlay('main', 'Received clipboard-overlay:toggle-pin')
    clipboardOverlayManager?.togglePinned()
  })

  ipcMain.on('clipboard-overlay:backdrop-pointer-down', (event, payload = {}) => {
    logElectronOverlay('main', 'Received clipboard-overlay:backdrop-pointer-down', payload)
    clipboardOverlayManager?.handleBackdropPointerDown(payload)
  })

  ipcMain.on('clipboard-overlay:drag-state', (event, payload = {}) => {
    logElectronOverlay('main', 'Received clipboard-overlay:drag-state', payload)
    clipboardOverlayManager?.setDragState(payload.active === true)
  })

  ipcMain.on('clipboard-overlay:start-native-drag', (event, payload = {}) => {
    logElectronOverlay('main', 'Received clipboard-overlay:start-native-drag', {
      itemId: payload.itemId || null,
      itemType: payload.itemType || null
    })
    clipboardOverlayManager?.startNativeDrag(event.sender, payload)
  })

  ipcMain.on('debug:log', (event, payload = {}) => {
    logElectronOverlay(
      payload.scope || 'renderer',
      payload.message || 'Renderer debug log',
      payload.details
    )
  })

  ipcMain.on('main:open-settings', async () => {
    await showMainWindow({ navigateToSettings: true })
  })
}

app.whenReady().then(async () => {
  try {
    logElectronOverlay('main', 'App whenReady start', {
      isDev
    })
    const port = await startBackend()
    console.log('Backend started on port', port)
    logElectronOverlay('main', 'Backend started', {
      port
    })

    floatingWindowManager = new FloatingWindowManager()
    floatingWindowManager.preload()
    clipboardOverlayManager = new ClipboardOverlayManager()
    clipboardMonitor = new ClipboardMonitor({
      getBackendPort: () => pythonManager?.port,
      onCaptured: (payload) => {
        notifyClipboardChanged({
          reason: 'captured',
          itemId: payload?.id || null
        })
      }
    })
    clipboardMonitor.start()
    logElectronOverlay('main', 'Window managers initialized')
    setupIPC()
    await createWindow()
    setupTray()
    await initializeDesktopInput(port)
    console.log('App ready')
    logElectronOverlay('main', 'App ready complete')
  } catch (error) {
    console.error('Failed to start app:', error)
    logElectronOverlay('main', 'Failed to start app', {
      error: error.message,
      stack: error.stack
    })
    app.quit()
  }
})

app.on('before-quit', () => {
  logElectronOverlay('main', 'before-quit received')
  isQuitting = true
  trayManager?.destroy()
  floatingWindowManager?.destroy()
  clipboardOverlayManager?.destroy()
  clipboardMonitor?.stop()
})

app.on('will-quit', () => {
  logElectronOverlay('main', 'will-quit received')
  inputManager?.stop()
  floatingWindowManager?.destroy()
  clipboardOverlayManager?.destroy()
  clipboardMonitor?.stop()
  pythonManager?.stop()
})

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') {
    app.quit()
  }
})

app.on('activate', () => {
  logElectronOverlay('main', 'activate received', {
    hasMainWindow: mainWindow !== null
  })
  if (mainWindow === null) {
    showMainWindow().catch((error) => {
      console.error('Failed to reactivate main window:', error)
      logElectronOverlay('main', 'Failed to reactivate main window', {
        error: error.message,
        stack: error.stack
      })
    })
    return
  }

  showMainWindow().catch((error) => {
    console.error('Failed to show main window:', error)
    logElectronOverlay('main', 'Failed to show main window', {
      error: error.message,
      stack: error.stack
    })
  })
})
