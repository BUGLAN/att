const { app, BrowserWindow, ipcMain, globalShortcut, clipboard } = require('electron')
const path = require('path')
const { execFile } = require('child_process')
const { PythonManager } = require('./python/manager')
const { DesktopAudioController } = require('./audio/controller')
const { TrayManager } = require('./tray/index')
const { FloatingWindowManager } = require('./floatingWindow')

let mainWindow = null
let pythonManager = null
let trayManager = null
let audioController = null
let floatingWindowManager = null
let isQuitting = false
const pasteHelperPath = path.join(__dirname, 'python', 'paste_helper.py')

const isDev = process.env.NODE_ENV === 'development'

async function createWindow() {
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

  mainWindow.on('closed', () => {
    mainWindow = null
  })

  if (trayManager) {
    trayManager.mainWindow = mainWindow
  }
}

async function showMainWindow({ navigateToSettings = false } = {}) {
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

function setupTray() {
  trayManager = new TrayManager(mainWindow, null)
  trayManager.create()

  trayManager.on('quit', () => {
    app.quit()
  })

  trayManager.on('copy-and-paste', async (text) => {
    clipboard.writeText(text)
    mainWindow?.webContents.send('paste-text')
  })
}

function setupIPC() {
  ipcMain.handle('backend:port', () => {
    return pythonManager?.port || 8000
  })

  ipcMain.handle('clipboard:write', (event, text) => {
    clipboard.writeText(text)
    return true
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

  ipcMain.handle('hotkey:update', async () => {
    return {
      success: true,
      managedBy: 'backend'
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

  ipcMain.on('main:open-settings', async () => {
    await showMainWindow({ navigateToSettings: true })
  })
}

app.whenReady().then(async () => {
  try {
    const port = await startBackend()
    console.log('Backend started on port', port)

    floatingWindowManager = new FloatingWindowManager()
    setupIPC()
    await createWindow()
    setupTray()
    console.log('App ready')
  } catch (error) {
    console.error('Failed to start app:', error)
    app.quit()
  }
})

app.on('before-quit', () => {
  isQuitting = true
  trayManager?.destroy()
  floatingWindowManager?.destroy()
})

app.on('will-quit', () => {
  globalShortcut.unregisterAll()
  floatingWindowManager?.destroy()
  pythonManager?.stop()
})

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') {
    app.quit()
  }
})

app.on('activate', () => {
  if (mainWindow === null) {
    showMainWindow().catch((error) => {
      console.error('Failed to reactivate main window:', error)
    })
    return
  }

  showMainWindow().catch((error) => {
    console.error('Failed to show main window:', error)
  })
})
