/**
 * Electron 主进程入口
 * 管理 BrowserWindow、Python 后端、托盘、热键
 */

const { app, BrowserWindow, ipcMain, globalShortcut, clipboard } = require('electron')
const path = require('path')
const { PythonManager } = require('./python/manager')
const { TrayManager } = require('./tray/index')
const { APIProxy } = require('./ipc/api')
const { EventEmitter } = require('./ipc/events')

// 禁用硬件加速（某些系统上更稳定）
app.disableHardwareAcceleration()

let mainWindow = null
let pythonManager = null
let trayManager = null
let apiProxy = null
let eventEmitter = null

// 开发模式检测
const isDev = process.env.NODE_ENV === 'development'

/**
 * 创建主窗口
 */
async function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1200,
    height: 800,
    minWidth: 800,
    minHeight: 600,
    frame: false,  // 无边框窗口
    titleBarStyle: 'hidden',
    backgroundColor: '#0a0a0b',
    webPreferences: {
      nodeIntegration: false,
      contextIsolation: true,
      preload: path.join(__dirname, 'preload.js')
    }
  })

  // 处理麦克风权限请求
  mainWindow.webContents.session.setPermissionRequestHandler((webContents, permission, callback) => {
    if (permission === 'media') {
      // 自动允许媒体访问（麦克风、摄像头）
      callback(true)
    } else {
      callback(false)
    }
  })

  // 加载页面
  if (isDev) {
    await mainWindow.loadURL('http://localhost:5173')
    mainWindow.webContents.openDevTools()
  } else {
    await mainWindow.loadFile(path.join(__dirname, '../dist/index.html'))
  }

  // 窗口关闭
  mainWindow.on('closed', () => {
    mainWindow = null
  })
}

/**
 * 启动 Python 后端
 */
async function startBackend() {
  pythonManager = new PythonManager()

  try {
    const port = await pythonManager.start()
    console.log(`Python backend started on port ${port}`)

    // 初始化 API 代理
    apiProxy = new APIProxy(pythonManager)
    apiProxy.setupIPC()

    // 初始化事件发射器（mainWindow 稍后在窗口创建后设置）
    eventEmitter = new EventEmitter(pythonManager, null)
    eventEmitter.setupIPC()

    return port
  } catch (error) {
    console.error('Failed to start Python backend:', error)
    throw error  // 重新抛出，让调用者处理
  }
}

/**
 * 设置托盘
 */
function setupTray() {
  trayManager = new TrayManager(mainWindow, apiProxy)
  trayManager.create()

  // 托盘事件
  trayManager.on('quit', () => {
    app.quit()
  })

  trayManager.on('copy-and-paste', async (text) => {
    clipboard.writeText(text)
    // 通过渲染进程模拟粘贴
    mainWindow?.webContents.send('paste-text')
  })
}

/**
 * 设置全局热键
 */
function setupHotkey() {
  // 从设置加载热键，默认 Ctrl+Alt
  // Electron 加速器格式要求修饰键在前，顺序正确
  const defaultAccelerator = process.platform === 'darwin'
    ? 'CommandOrControl+Alt'
    : 'Control+Alt'

  try {
    globalShortcut.register(defaultAccelerator, () => {
      // 热键按下 - 开始录音
      mainWindow?.webContents.send('hotkey:start')
      trayManager?.setIcon('recording')
    })
  } catch (error) {
    console.error('Failed to register hotkey:', error)
  }
}

/**
 * 设置 IPC 处理
 */
function setupIPC() {
  // 窗口控制
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
    mainWindow?.close()
  })

  // 托盘图标控制
  ipcMain.on('tray:set-icon', (event, iconType) => {
    trayManager?.setIcon(iconType)
  })

  // 热键更新
  ipcMain.on('hotkey:update', (event, hotkey) => {
    globalShortcut.unregisterAll()
    try {
      globalShortcut.register(hotkey.accelerator, () => {
        mainWindow?.webContents.send('hotkey:start')
        trayManager?.setIcon('recording')
      })
    } catch (error) {
      console.error('Failed to update hotkey:', error)
    }
  })
}

// 应用启动 - 先启动后端再创建窗口，确保 IPC handlers 已注册
app.whenReady().then(async () => {
  try {
    // 1. 先启动后端 (注册 IPC handlers)
    await startBackend()
    console.log('Backend started')

    // 2. 再创建窗口 (前端可以安全调用 API)
    await createWindow()
    console.log('Window created')

    // 3. 窗口创建后，设置 EventEmitter 的 mainWindow 引用
    eventEmitter.mainWindow = mainWindow

    setupTray()
    setupHotkey()
    setupIPC()
    console.log('App ready')
  } catch (error) {
    console.error('Failed to start app:', error)
    app.quit()
  }
})

// 应用退出
app.on('will-quit', () => {
  globalShortcut.unregisterAll()
  pythonManager?.stop()
})

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') {
    app.quit()
  }
})

app.on('activate', () => {
  if (mainWindow === null) {
    createWindow()
  }
})
