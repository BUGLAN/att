const { app, BrowserWindow, dialog, ipcMain, clipboard, nativeImage } = require('electron')
const path = require('path')
const fs = require('fs')
const { execFile } = require('child_process')
const { PythonManager } = require('./python/manager')
const { PythonManager: PythonRuntimeManager } = require('./python/runtime-manager')
const { DesktopAudioController } = require('./audio/controller')
const { TrayManager } = require('./tray/index')
const { FloatingWindowManager } = require('./floatingWindow')
const { ClipboardMonitor } = require('./clipboardMonitor')
const { logElectron, electronLogFile } = require('./logger')
const { DesktopSettingsStore } = require('./desktop/settings')
const { DesktopAgentManager } = require('./desktopAgent/manager')
const { isDevelopmentRuntime } = require('./utils/runtime')

let mainWindow = null
let pythonManager = null
let pythonRuntimeManager = null
let trayManager = null
let audioController = null
let desktopAgentManager = null
let desktopSettingsStore = null
let floatingWindowManager = null
let clipboardMonitor = null
let isQuitting = false
let pendingDesktopPermissionRequired = null
let pendingDesktopAgentError = null
const pasteHelperPath = path.join(__dirname, 'python', 'paste_helper.py')

const isDev = isDevelopmentRuntime()
const DEV_SERVER_URL = 'http://127.0.0.1:8090'
const hasSingleInstanceLock = app.requestSingleInstanceLock()

if (!hasSingleInstanceLock) {
  app.quit()
}

function resolveClipboardImagePath(imagePath) {
  const normalizedPath = String(imagePath || '').trim()
  if (!normalizedPath) {
    return ''
  }

  if (path.isAbsolute(normalizedPath)) {
    return normalizedPath
  }

  return path.join(__dirname, '..', 'data', 'clipboard', normalizedPath)
}

function sendToMainWindow(channel, payload) {
  if (!mainWindow || mainWindow.isDestroyed()) {
    return false
  }

  mainWindow.webContents.send(channel, payload)
  return true
}

function cacheDesktopAgentEvent(channel, payload) {
  if (channel === 'desktop:permission-required') {
    pendingDesktopPermissionRequired = payload
    return
  }

  if (channel === 'desktop:agent-error') {
    pendingDesktopAgentError = payload
  }
}

function flushPendingDesktopAgentEvents() {
  if (!mainWindow || mainWindow.isDestroyed()) {
    return
  }

  if (pendingDesktopPermissionRequired) {
    sendToMainWindow('desktop:permission-required', pendingDesktopPermissionRequired)
    pendingDesktopPermissionRequired = null
  }

  if (pendingDesktopAgentError) {
    sendToMainWindow('desktop:agent-error', pendingDesktopAgentError)
    pendingDesktopAgentError = null
  }
}

function hasReadyDesktopAgent() {
  return desktopAgentManager?.isReady === true
}

async function createWindow() {
  logElectron('main', 'Creating main window')
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

  // macOS 下 titleBarStyle: 'hidden' 会保留原生红黄绿按钮，与自绘标题栏
  // （src/components/layout/TitleBar.vue）的品牌区重叠；这里显式隐藏，
  // 让 macOS 与 Windows（frame: false 时无原生按钮）保持同一套自绘标题栏。
  if (process.platform === 'darwin') {
    mainWindow.setWindowButtonVisibility(false)
  }

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

  mainWindow.webContents.on('did-finish-load', () => {
    flushPendingDesktopAgentEvents()
  })

  if (isDev) {
    try {
      await mainWindow.loadURL(DEV_SERVER_URL)
      mainWindow.webContents.openDevTools()
    } catch (error) {
      // Vite 开发服务器没在跑（例如只重启了 Electron、没走 electron:dev 脚本）：
      // loadURL 会以 ERR_CONNECTION_REFUSED 拒绝，窗口全黑。回退加载本地构建产物，
      // 保证仓库 checkout 下直接启动也总能出界面。
      const distIndex = path.join(__dirname, '..', 'dist', 'index.html')
      if (fs.existsSync(distIndex)) {
        logElectron('main', 'Dev server unreachable, fallback to dist build', {
          error: String(error && error.message ? error.message : error)
        })
        await mainWindow.loadFile(distIndex)
      } else {
        throw error
      }
    }
  } else {
    await mainWindow.loadFile(path.join(__dirname, '../dist/index.html'))
  }

  logElectron('main', 'Main window loaded', {
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
  logElectron('main', 'Showing main window', {
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

function getDesktopSettingsStore() {
  if (!desktopSettingsStore) {
    desktopSettingsStore = new DesktopSettingsStore({
      filePath: path.join(app.getPath('userData'), 'desktop-settings.json'),
      platform: process.platform
    })
  }

  return desktopSettingsStore
}

function loadLocalDesktopSettings() {
  return getDesktopSettingsStore().load()
}

function saveLocalDesktopSettings(partialSettings = {}) {
  const store = getDesktopSettingsStore()
  const currentSettings = store.load()
  return store.save({
    ...currentSettings,
    ...partialSettings
  })
}

async function startDesktopAgent() {
  const desktopSettings = loadLocalDesktopSettings()
  pythonRuntimeManager = new PythonRuntimeManager()

  desktopAgentManager = new DesktopAgentManager({
    pythonPath: pythonRuntimeManager.getPythonPath(),
    scriptPath: path.join(__dirname, 'python', 'desktop_agent.py'),
    runtimeRoot: pythonRuntimeManager.getRuntimeRoot(),
    spawnProcess: (pythonPath, args, options) => (
      pythonRuntimeManager.spawnBackendProcess(pythonPath, args, options)
    ),
    log: (message, details) => {
      logElectron('desktop-agent', message, details)
    }
  })

  desktopAgentManager.on('ready', (payload) => {
    logElectron('main', 'Desktop agent ready', payload)
    mainWindow?.webContents.send('desktop:agent-ready', payload)
  })
  desktopAgentManager.on('hotkey:press', (payload) => {
    logElectron('main', 'Desktop agent hotkey press', payload)
    mainWindow?.webContents.send('hotkey:press', payload)
  })
  desktopAgentManager.on('hotkey:release', (payload) => {
    logElectron('main', 'Desktop agent hotkey release', payload)
    mainWindow?.webContents.send('hotkey:release', payload)
  })
  desktopAgentManager.on('permission:required', (payload) => {
    logElectron('main', 'Desktop agent permission required', payload)
    cacheDesktopAgentEvent('desktop:permission-required', payload)
    sendToMainWindow('desktop:permission-required', payload)
  })
  desktopAgentManager.on('agent:error', (payload) => {
    logElectron('main', 'Desktop agent error', payload)
    cacheDesktopAgentEvent('desktop:agent-error', payload)
    sendToMainWindow('desktop:agent-error', payload)
  })
  desktopAgentManager.on('exit', (payload) => {
    logElectron('main', 'Desktop agent exited', payload)
    desktopAgentManager = null

    if (payload.expected || isQuitting) {
      return
    }

    const errorPayload = {
      error: 'Desktop agent exited unexpectedly',
      ...payload
    }
    cacheDesktopAgentEvent('desktop:agent-error', errorPayload)
    sendToMainWindow('desktop:agent-error', errorPayload)
  })

  await desktopAgentManager.start(desktopSettings)
  return desktopSettings
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
}

function setupIPC() {
  logElectron('main', 'Setting up IPC handlers', {
    logFile: electronLogFile
  })

  ipcMain.handle('backend:port', () => {
    return pythonManager?.port || 8000
  })

  // 「转文字」页选择本地音视频文件：返回绝对路径，用户取消返回 null
  ipcMain.handle('dialog:open-media-file', async () => {
    const result = await dialog.showOpenDialog(mainWindow, {
      title: '选择要转写的音视频文件',
      properties: ['openFile'],
      filters: [
        {
          name: '音视频文件',
          extensions: ['mp4', 'flv', 'webm', 'mkv', 'mov', 'avi', 'm4v', 'ts',
            'wav', 'mp3', 'm4a', 'flac', 'ogg', 'opus', 'aac', 'wma', 'amr']
        },
        { name: '所有文件', extensions: ['*'] }
      ]
    })
    if (result.canceled || !Array.isArray(result.filePaths) || result.filePaths.length === 0) {
      return null
    }
    return result.filePaths[0]
  })

  ipcMain.handle('clipboard:write', (event, text) => {
    clipboardMonitor?.markInternalWrite(text, 'renderer_write')
    clipboard.writeText(text)
    return true
  })

  ipcMain.handle('clipboard:read-text', () => {
    return clipboard.readText()
  })

  ipcMain.handle('clipboard:write-image', (event, payload = {}) => {
    const imagePath = resolveClipboardImagePath(payload.imagePath)
    const imageToken = String(payload.imageToken || '').trim()

    if (!imagePath) {
      return false
    }

    const image = nativeImage.createFromPath(imagePath)
    if (image.isEmpty()) {
      return false
    }

    if (imageToken) {
      clipboardMonitor?.markInternalWrite(imageToken, 'renderer_write_image')
    }

    clipboard.writeImage(image)
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
    const desktopSettings = saveLocalDesktopSettings(settings)

    if (hasReadyDesktopAgent()) {
      const result = await desktopAgentManager.updateMutePolicy({
        pause_audio_on_recording: desktopSettings.pause_audio_on_recording
      })
      return {
        ...(result || { success: true }),
        settings: desktopSettings
      }
    }

    if (!audioController) {
      return { success: false, error: 'Audio controller unavailable', settings: desktopSettings }
    }

    const result = await audioController.updateSettings(desktopSettings)
    if (!result.success) {
      console.error('[Audio] Failed to sync settings:', result.error)
    }
    return {
      ...result,
      settings: desktopSettings
    }
  })

  ipcMain.handle('desktop:recording-state', async (event, payload = {}) => {
    if (hasReadyDesktopAgent()) {
      return desktopAgentManager.setRecordingActive(payload.recording === true)
    }

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

  // 快捷键更新统一走 desktop:sync-input-settings（hotkey:update 是旧通道，已删除）

  ipcMain.handle('desktop:sync-input-settings', async (event, payload = {}) => {
    let desktopSettings = null
    if (payload.hotkey) {
      desktopSettings = saveLocalDesktopSettings({ hotkey: payload.hotkey })
      if (hasReadyDesktopAgent()) {
        await desktopAgentManager.updateHotkey(desktopSettings.hotkey)
      }
    }

    // 统一获取桌面设置，避免重复读取文件
    const resultSettings = desktopSettings || loadLocalDesktopSettings()
    return {
      success: true,
      managedBy: hasReadyDesktopAgent() ? 'desktop-agent' : 'desktop-settings-only',
      settings: {
        hotkey: payload.hotkey || resultSettings.hotkey
      },
      desktopSettings: resultSettings
    }
  })

  ipcMain.handle('desktop:settings:get', async () => {
    return loadLocalDesktopSettings()
  })

  ipcMain.handle('desktop:settings:save', async (event, settings = {}) => {
    const desktopSettings = saveLocalDesktopSettings(settings)

    if (hasReadyDesktopAgent()) {
      if (settings.hotkey) {
        await desktopAgentManager.updateHotkey(desktopSettings.hotkey)
      }

      if (Object.prototype.hasOwnProperty.call(settings, 'pause_audio_on_recording')) {
        await desktopAgentManager.updateMutePolicy({
          pause_audio_on_recording: desktopSettings.pause_audio_on_recording
        })
      }
    }

    return desktopSettings
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

  ipcMain.on('debug:log', (event, payload = {}) => {
    logElectron(
      payload.scope || 'renderer',
      payload.message || 'Renderer debug log',
      payload.details
    )
  })

  ipcMain.on('main:open-settings', async () => {
    await showMainWindow({ navigateToSettings: true })
  })
}

if (hasSingleInstanceLock) {
  app.whenReady().then(async () => {
    try {
      logElectron('main', 'App whenReady start', {
        isDev
      })
      let desktopSettings = null
      try {
        desktopSettings = await startDesktopAgent()
        logElectron('main', 'Desktop agent started', {
          desktopSettings
        })
      } catch (desktopAgentError) {
        logElectron('main', 'Desktop agent failed to start', {
          error: desktopAgentError.message,
          stack: desktopAgentError.stack
        })
        desktopAgentManager = null
        cacheDesktopAgentEvent('desktop:agent-error', {
          error: desktopAgentError.message || 'Desktop agent failed to start'
        })
      }
      const port = await startBackend()
      console.log('Backend started on port', port)
      logElectron('main', 'Backend started', {
        port
      })

      floatingWindowManager = new FloatingWindowManager()
      floatingWindowManager.preload()
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
      logElectron('main', 'Window managers initialized')
      setupIPC()
      await createWindow()
      setupTray()
      console.log('App ready')
      logElectron('main', 'App ready complete')
    } catch (error) {
      console.error('Failed to start app:', error)
      logElectron('main', 'Failed to start app', {
        error: error.message,
        stack: error.stack
      })
      app.quit()
    }
  })
}

app.on('before-quit', () => {
  logElectron('main', 'before-quit received')
  isQuitting = true
  trayManager?.destroy()
  floatingWindowManager?.destroy()
  clipboardMonitor?.stop()
})

app.on('will-quit', () => {
  logElectron('main', 'will-quit received')
  desktopAgentManager?.stop()
  floatingWindowManager?.destroy()
  clipboardMonitor?.stop()
  pythonManager?.stop()
})

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') {
    app.quit()
  }
})

app.on('activate', () => {
  logElectron('main', 'activate received', {
    hasMainWindow: mainWindow !== null
  })
  if (mainWindow === null) {
    showMainWindow().catch((error) => {
      console.error('Failed to reactivate main window:', error)
      logElectron('main', 'Failed to reactivate main window', {
        error: error.message,
        stack: error.stack
      })
    })
    return
  }

  showMainWindow().catch((error) => {
    console.error('Failed to show main window:', error)
    logElectron('main', 'Failed to show main window', {
      error: error.message,
      stack: error.stack
    })
  })
})

app.on('second-instance', () => {
  showMainWindow().catch((error) => {
    console.error('Failed to show main window for second instance:', error)
    logElectron('main', 'Failed to show main window for second instance', {
      error: error.message,
      stack: error.stack
    })
  })
})
