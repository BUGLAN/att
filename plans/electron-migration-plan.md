# Electron 应用迁移详细计划

## Context

将当前 PySide6 托盘程序迁移到 Electron，整合网页前端、FastAPI 后端和托盘应用。

**决策**：
- **打包方式**：嵌入 Python 环境
- **通信方式**：完全使用 IPC
- **前端框架**：Vue 3 + Vite
- **UI 库**：不使用组件库，**保持现有 CSS 样式完全不变**

**核心约束**：主页样式必须保持原样，CSS 文件直接迁移，不做修改。

---

## 目录结构

```
E:\repo\att\
├── electron/                     # Electron 主进程
│   ├── package.json
│   ├── main.js                   # 主进程入口
│   ├── preload.js                # 预加载脚本
│   ├── tray/
│   │   ├── index.js              # 托盘管理
│   │   ├── hotkey.js             # 全局热键
│   │   ├── menu.js               # 右键菜单
│   │   └── icons/                # 托盘图标
│   │       ├── idle.png
│   │       ├── recording.png
│   │       ├── transcribing.png
│   │       └── disconnected.png
│   ├── python/
│   │   └── manager.js            # Python 进程管理
│   └── ipc/
│       ├── api.js                # API 代理
│       └── events.js             # 实时事件
│
├── src/                          # Vue 3 前端
│   ├── index.html                # 入口 HTML
│   ├── main.js                   # Vue 入口
│   ├── App.vue                   # 根组件
│   ├── components/
│   │   ├── layout/
│   │   │   ├── Sidebar.vue       # 侧边栏
│   │   │   └── ConnectionStatus.vue
│   │   ├── home/
│   │   │   ├── RecorderSection.vue    # 录音区
│   │   │   ├── RecordButton.vue       # 录音按钮
│   │   │   ├── RecordingStatus.vue    # 状态显示
│   │   │   ├── RecordsSection.vue     # 历史列表
│   │   │   └── RecordCard.vue         # 记录卡片
│   │   ├── settings/
│   │   │   ├── SettingsPage.vue       # 设置页
│   │   │   ├── SettingItem.vue        # 设置项
│   │   │   ├── ToggleSwitch.vue       # 开关
│   │   │   ├── HotkeyInput.vue        # 热键输入
│   │   │   ├── LLMConfigManager.vue   # LLM 配置
│   │   │   └── PresetManager.vue      # 预设管理
│   │   └── common/
│   │       ├── Modal.vue              # 模态框
│   │       └── Toast.vue              # 提示
│   ├── composables/
│   │   ├── useRecorder.js        # 录音逻辑
│   │   ├── useRecords.js         # 记录管理
│   │   ├── useSettings.js        # 设置管理
│   │   ├── useLLMConfig.js       # LLM 配置
│   │   ├── usePresets.js         # 预设管理
│   │   └── useElectron.js        # Electron API
│   ├── stores/
│   │   ├── app.js                # 应用状态
│   │   ├── records.js            # 记录状态
│   │   └── settings.js           # 设置状态
│   ├── styles/
│   │   └── main.css              # **直接复制 style.css，不修改**
│   └── utils/
│       ├── format.js             # 格式化工具
│       └── mediaRecorder.js      # MediaRecorder 封装
│
├── app/                          # Python 后端（保持不变）
├── data/                         # 数据目录
├── models/                       # Whisper 模型
├── vite.config.js
├── electron-builder.yml
├── requirements.txt
└── package.json
```

---

## 详细实现

### 1. package.json

```json
{
  "name": "audio-to-text",
  "version": "1.0.0",
  "description": "Audio to Text - Electron App",
  "main": "electron/main.js",
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "preview": "vite preview",
    "electron:dev": "concurrently -k \"vite\" \"wait-on http://localhost:5173 && cross-env NODE_ENV=development electron .\"",
    "electron:build": "vite build && electron-builder",
    "electron:build:win": "vite build && electron-builder --win",
    "electron:build:mac": "vite build && electron-builder --mac"
  },
  "dependencies": {
    "vue": "^3.4.21",
    "pinia": "^2.1.7"
  },
  "devDependencies": {
    "@vitejs/plugin-vue": "^5.0.4",
    "vite": "^5.2.0",
    "electron": "^29.1.0",
    "electron-builder": "^24.13.3",
    "concurrently": "^8.2.2",
    "wait-on": "^7.2.0",
    "cross-env": "^7.0.3"
  }
}
```

### 2. vite.config.js

```javascript
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import { resolve } from 'path'

export default defineConfig({
  plugins: [vue()],
  base: './',
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src')
    }
  },
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
    rollupOptions: {
      output: {
        manualChunks: {
          vue: ['vue', 'pinia']
        }
      }
    }
  },
  server: {
    port: 5173
  }
})
```

### 3. electron/main.js

```javascript
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

async function startBackend() {
  pythonManager = new PythonManager()

  try {
    const port = await pythonManager.start()
    console.log(`Python backend started on port ${port}`)

    // 初始化 API 代理
    apiProxy = new APIProxy(pythonManager)
    apiProxy.setupIPC()

    // 初始化事件发射器
    eventEmitter = new EventEmitter(pythonManager, mainWindow)
    eventEmitter.setupIPC()

    return port
  } catch (error) {
    console.error('Failed to start Python backend:', error)
    app.quit()
  }
}

function setupTray() {
  trayManager = new TrayManager(mainWindow, apiProxy)
  trayManager.create()

  // 托盘事件
  trayManager.on('quit', () => {
    app.quit()
  })

  trayManager.on('copy-and-paste', (text) => {
    clipboard.writeText(text)
    // 模拟粘贴
    const { keyboard } = require('@nut-tree/nut-js')
    if (process.platform === 'darwin') {
      keyboard.pressKey(0, 'command')
      keyboard.pressKey(0, 'v')
      keyboard.releaseKey(0, 'v')
      keyboard.releaseKey(0, 'command')
    } else {
      keyboard.pressKey(0, 'control')
      keyboard.pressKey(0, 'v')
      keyboard.releaseKey(0, 'v')
      keyboard.releaseKey(0, 'control')
    }
  })
}

function setupHotkey() {
  // 从设置加载热键
  const defaultAccelerator = process.platform === 'darwin' ? 'Alt+CommandOrControl' : 'Alt+Control'

  globalShortcut.register(defaultAccelerator, () => {
    // 热键按下 - 开始录音
    mainWindow?.webContents.send('hotkey:start')
    trayManager?.setIcon('recording')
  })
}

// 应用启动
app.whenReady().then(async () => {
  await createWindow()
  await startBackend()
  setupTray()
  setupHotkey()
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
```

### 4. electron/preload.js

```javascript
const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('electronAPI', {
  // 平台信息
  platform: process.platform,

  // === API 调用 ===

  // 音频
  uploadAudio: async (formData) => {
    // formData 需要特殊处理
    const { buffer, filename } = formData
    return ipcRenderer.invoke('api:audio:upload', { buffer, filename })
  },

  // 记录
  getRecords: () => ipcRenderer.invoke('api:records:list'),
  getRecord: (id) => ipcRenderer.invoke('api:records:get', id),
  getRecordStatus: (id) => ipcRenderer.invoke('api:records:status', id),
  updateRecord: (id, data) => ipcRenderer.invoke('api:records:update', { id, data }),
  deleteRecord: (id) => ipcRenderer.invoke('api:records:delete', id),
  optimizeRecord: (id) => ipcRenderer.invoke('api:records:optimize', id),
  exportRecord: (id, format) => ipcRenderer.invoke('api:records:export', { id, format }),

  // 设置
  getSettings: () => ipcRenderer.invoke('api:settings:get'),
  saveSettings: (settings) => ipcRenderer.invoke('api:settings:save', settings),

  // LLM 配置
  getLLMConfigs: () => ipcRenderer.invoke('api:llm:list'),
  saveLLMConfig: (config) => ipcRenderer.invoke('api:llm:save', config),
  deleteLLMConfig: (id) => ipcRenderer.invoke('api:llm:delete', id),
  testLLMConfig: (id) => ipcRenderer.invoke('api:llm:test', id),

  // 预设
  getPresets: () => ipcRenderer.invoke('api:prompts:list'),
  savePreset: (preset) => ipcRenderer.invoke('api:prompts:save', preset),
  deletePreset: (id) => ipcRenderer.invoke('api:prompts:delete', id),

  // === 事件监听 ===

  onRecordStatus: (callback) => {
    ipcRenderer.on('record:status', (event, data) => callback(data))
  },
  onRecordCompleted: (callback) => {
    ipcRenderer.on('record:completed', (event, data) => callback(data))
  },
  onHotkeyStart: (callback) => {
    ipcRenderer.on('hotkey:start', () => callback())
  },
  onHotkeyStop: (callback) => {
    ipcRenderer.on('hotkey:stop', () => callback())
  },

  // 移除监听
  removeAllListeners: (channel) => {
    ipcRenderer.removeAllListeners(channel)
  },

  // === 状态轮询 ===
  startStatusPolling: (recordId) => {
    ipcRenderer.send('polling:start', recordId)
  },
  stopStatusPolling: () => {
    ipcRenderer.send('polling:stop')
  }
})
```

### 5. electron/ipc/api.js

```javascript
const { ipcMain } = require('electron')
const FormData = require('form-data')
const fs = require('fs')
const path = require('path')

class APIProxy {
  constructor(pythonManager) {
    this.pythonManager = pythonManager
  }

  get baseUrl() {
    return `http://127.0.0.1:${this.pythonManager.port}/api`
  }

  async request(method, endpoint, data = null) {
    const url = `${this.baseUrl}${endpoint}`
    const options = {
      method,
      headers: { 'Content-Type': 'application/json' }
    }

    if (data && method !== 'GET') {
      options.body = JSON.stringify(data)
    }

    const response = await fetch(url, options)
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}: ${response.statusText}`)
    }
    return response.json()
  }

  setupIPC() {
    // === 音频上传 ===
    ipcMain.handle('api:audio:upload', async (event, { buffer, filename }) => {
      const url = `${this.baseUrl}/audio/upload`
      const form = new FormData()

      // 将 Buffer 写入临时文件
      const tempPath = path.join(require('os').tmpdir(), filename)
      fs.writeFileSync(tempPath, Buffer.from(buffer))

      form.append('audio', fs.createReadStream(tempPath), filename)

      const response = await fetch(url, {
        method: 'POST',
        headers: form.getHeaders(),
        body: form
      })

      // 清理临时文件
      fs.unlinkSync(tempPath)

      return response.json()
    })

    // === 记录管理 ===
    ipcMain.handle('api:records:list', async () => {
      return this.request('GET', '/records')
    })

    ipcMain.handle('api:records:get', async (event, id) => {
      return this.request('GET', `/records/${id}`)
    })

    ipcMain.handle('api:records:status', async (event, id) => {
      return this.request('GET', `/records/${id}/status`)
    })

    ipcMain.handle('api:records:update', async (event, { id, data }) => {
      return this.request('PUT', `/records/${id}`, data)
    })

    ipcMain.handle('api:records:delete', async (event, id) => {
      return this.request('DELETE', `/records/${id}`)
    })

    ipcMain.handle('api:records:optimize', async (event, id) => {
      return this.request('POST', `/records/${id}/optimize`)
    })

    ipcMain.handle('api:records:export', async (event, { id, format }) => {
      return this.request('GET', `/records/${id}/export?format=${format}`)
    })

    // === 设置 ===
    ipcMain.handle('api:settings:get', async () => {
      return this.request('GET', '/settings')
    })

    ipcMain.handle('api:settings:save', async (event, settings) => {
      return this.request('PUT', '/settings', settings)
    })

    // === LLM 配置 ===
    ipcMain.handle('api:llm:list', async () => {
      return this.request('GET', '/llm/configs')
    })

    ipcMain.handle('api:llm:save', async (event, config) => {
      if (config.id) {
        return this.request('PUT', `/llm/configs/${config.id}`, config)
      }
      return this.request('POST', '/llm/configs', config)
    })

    ipcMain.handle('api:llm:delete', async (event, id) => {
      return this.request('DELETE', `/llm/configs/${id}`)
    })

    ipcMain.handle('api:llm:test', async (event, id) => {
      return this.request('POST', `/llm/configs/${id}/test`)
    })

    // === 预设 ===
    ipcMain.handle('api:prompts:list', async () => {
      return this.request('GET', '/prompts')
    })

    ipcMain.handle('api:prompts:save', async (event, preset) => {
      if (preset.id) {
        return this.request('PUT', `/prompts/${preset.id}`, preset)
      }
      return this.request('POST', '/prompts', preset)
    })

    ipcMain.handle('api:prompts:delete', async (event, id) => {
      return this.request('DELETE', `/prompts/${id}`)
    })
  }
}

module.exports = { APIProxy }
```

### 6. electron/ipc/events.js

```javascript
const { ipcMain } = require('electron')

class EventEmitter {
  constructor(pythonManager, mainWindow) {
    this.pythonManager = pythonManager
    this.mainWindow = mainWindow
    this.pollingIntervals = new Map()
  }

  get baseUrl() {
    return `http://127.0.0.1:${this.pythonManager.port}/api`
  }

  setupIPC() {
    // 开始轮询
    ipcMain.on('polling:start', (event, recordId) => {
      this.startPolling(recordId)
    })

    // 停止轮询
    ipcMain.on('polling:stop', () => {
      this.stopAllPolling()
    })
  }

  startPolling(recordId) {
    // 如果已在轮询，先停止
    if (this.pollingIntervals.has(recordId)) {
      clearInterval(this.pollingIntervals.get(recordId))
    }

    const interval = setInterval(async () => {
      try {
        const response = await fetch(`${this.baseUrl}/records/${recordId}/status`)
        const status = await response.json()

        // 发送状态到渲染进程
        this.mainWindow?.webContents.send('record:status', status)

        // 检查是否完成
        if (status.status === 'transcribed' || status.status === 'optimized') {
          this.stopPolling(recordId)

          // 发送完成事件
          this.mainWindow?.webContents.send('record:completed', status)
        }

        // 检查是否失败
        if (status.status === 'failed') {
          this.stopPolling(recordId)
        }
      } catch (error) {
        console.error('Polling error:', error)
      }
    }, 1000)

    this.pollingIntervals.set(recordId, interval)
  }

  stopPolling(recordId) {
    if (this.pollingIntervals.has(recordId)) {
      clearInterval(this.pollingIntervals.get(recordId))
      this.pollingIntervals.delete(recordId)
    }
  }

  stopAllPolling() {
    for (const interval of this.pollingIntervals.values()) {
      clearInterval(interval)
    }
    this.pollingIntervals.clear()
  }
}

module.exports = { EventEmitter }
```

### 7. src/index.html

```html
<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Audio to Text</title>
  <!-- 样式直接引入，保持不变 -->
  <link rel="stylesheet" href="/src/styles/main.css">
</head>
<body>
  <div id="app"></div>
  <script type="module" src="/src/main.js"></script>
</body>
</html>
```

### 8. src/main.js

```javascript
import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import './styles/main.css'  // 直接使用原样式文件

const app = createApp(App)
const pinia = createPinia()

app.use(pinia)
app.mount('#app')
```

### 9. src/App.vue

```vue
<template>
  <div class="app-container">
    <!-- 侧边栏 -->
    <Sidebar
      :collapsed="sidebarCollapsed"
      @toggle="toggleSidebar"
    />

    <!-- 主内容区 -->
    <main class="main-content" :class="{ 'sidebar-collapsed': sidebarCollapsed }">
      <!-- 首页 -->
      <div v-if="currentPage === 'home'" class="container">
        <Header />
        <RecorderSection />
        <RecordsSection />
      </div>

      <!-- 设置页 -->
      <SettingsPage v-else-if="currentPage === 'settings'" />
    </main>

    <!-- 模态框 -->
    <RecordEditModal v-if="modals.recordEdit" />
    <PresetModal v-if="modals.preset" />
    <PresetEditModal v-if="modals.presetEdit" />
    <LLMModal v-if="modals.llm" />
    <LLMEditModal v-if="modals.llmEdit" />

    <!-- Toast -->
    <Toast />
  </div>
</template>

<script setup>
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'

import Sidebar from '@/components/layout/Sidebar.vue'
import Header from '@/components/layout/Header.vue'
import RecorderSection from '@/components/home/RecorderSection.vue'
import RecordsSection from '@/components/home/RecordsSection.vue'
import SettingsPage from '@/components/settings/SettingsPage.vue'

import RecordEditModal from '@/components/modals/RecordEditModal.vue'
import PresetModal from '@/components/modals/PresetModal.vue'
import PresetEditModal from '@/components/modals/PresetEditModal.vue'
import LLMModal from '@/components/modals/LLMModal.vue'
import LLMEditModal from '@/components/modals/LLMEditModal.vue'
import Toast from '@/components/common/Toast.vue'

import { useElectron } from '@/composables/useElectron'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { onHotkeyStart, onHotkeyStop, removeAllListeners } = useElectron()

const currentPage = computed(() => appStore.currentPage)
const sidebarCollapsed = computed(() => appStore.sidebarCollapsed)
const modals = computed(() => appStore.modals)

const toggleSidebar = () => {
  appStore.toggleSidebar()
}

// 热键监听
onMounted(async () => {
  await settingsStore.loadSettings()

  onHotkeyStart(() => {
    appStore.startRecording()
  })

  onHotkeyStop(() => {
    appStore.stopRecording()
  })
})

onUnmounted(() => {
  removeAllListeners('hotkey:start')
  removeAllListeners('hotkey:stop')
})
</script>

<style scoped>
.app-container {
  display: flex;
  min-height: 100vh;
}

.main-content {
  flex: 1;
  margin-left: 260px;
  transition: margin-left var(--transition-base);
}

.main-content.sidebar-collapsed {
  margin-left: 60px;
}
</style>
```

### 10. src/composables/useElectron.js

```javascript
import { ref, onUnmounted } from 'vue'

// 检测是否在 Electron 环境中
const isElectron = typeof window !== 'undefined' && window.electronAPI

export function useElectron() {
  const isReady = ref(false)

  // === API 方法 ===
  const api = {
    // 音频
    async uploadAudio(blob, filename = 'recording.webm') {
      const buffer = await blob.arrayBuffer()
      return window.electronAPI?.uploadAudio({
        buffer: Array.from(new Uint8Array(buffer)),
        filename
      })
    },

    // 记录
    getRecords: () => window.electronAPI?.getRecords(),
    getRecord: (id) => window.electronAPI?.getRecord(id),
    getRecordStatus: (id) => window.electronAPI?.getRecordStatus(id),
    updateRecord: (id, data) => window.electronAPI?.updateRecord(id, data),
    deleteRecord: (id) => window.electronAPI?.deleteRecord(id),
    optimizeRecord: (id) => window.electronAPI?.optimizeRecord(id),
    exportRecord: (id, format) => window.electronAPI?.exportRecord(id, format),

    // 设置
    getSettings: () => window.electronAPI?.getSettings(),
    saveSettings: (settings) => window.electronAPI?.saveSettings(settings),

    // LLM 配置
    getLLMConfigs: () => window.electronAPI?.getLLMConfigs(),
    saveLLMConfig: (config) => window.electronAPI?.saveLLMConfig(config),
    deleteLLMConfig: (id) => window.electronAPI?.deleteLLMConfig(id),
    testLLMConfig: (id) => window.electronAPI?.testLLMConfig(id),

    // 预设
    getPresets: () => window.electronAPI?.getPresets(),
    savePreset: (preset) => window.electronAPI?.savePreset(preset),
    deletePreset: (id) => window.electronAPI?.deletePreset(id)
  }

  // === 事件监听 ===
  function onRecordStatus(callback) {
    window.electronAPI?.onRecordStatus(callback)
  }

  function onRecordCompleted(callback) {
    window.electronAPI?.onRecordCompleted(callback)
  }

  function onHotkeyStart(callback) {
    window.electronAPI?.onHotkeyStart(callback)
  }

  function onHotkeyStop(callback) {
    window.electronAPI?.onHotkeyStop(callback)
  }

  function removeAllListeners(channel) {
    window.electronAPI?.removeAllListeners(channel)
  }

  // === 状态轮询 ===
  function startStatusPolling(recordId) {
    window.electronAPI?.startStatusPolling(recordId)
  }

  function stopStatusPolling() {
    window.electronAPI?.stopStatusPolling()
  }

  return {
    isElectron,
    isReady,
    api,
    onRecordStatus,
    onRecordCompleted,
    onHotkeyStart,
    onHotkeyStop,
    removeAllListeners,
    startStatusPolling,
    stopStatusPolling,
    platform: window.electronAPI?.platform || 'web'
  }
}
```

### 11. src/composables/useRecorder.js

```javascript
import { ref, computed } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import { useElectron } from './useElectron'

export function useRecorder() {
  const appStore = useAppStore()
  const settingsStore = useSettingsStore()
  const { api, startStatusPolling, stopStatusPolling, onRecordStatus, onRecordCompleted } = useElectron()

  const isRecording = computed(() => appStore.isRecording)
  const duration = ref(0)
  const mediaRecorder = ref(null)
  const chunks = ref([])
  const durationTimer = ref(null)

  // 开始录音
  async function startRecording() {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true })
      mediaRecorder.value = new MediaRecorder(stream, {
        mimeType: 'audio/webm;codecs=opus'
      })

      chunks.value = []

      mediaRecorder.value.ondataavailable = (e) => {
        if (e.data.size > 0) {
          chunks.value.push(e.data)
        }
      }

      mediaRecorder.value.onstop = async () => {
        const blob = new Blob(chunks.value, { type: 'audio/webm' })
        await uploadRecording(blob)
        stream.getTracks().forEach(track => track.stop())
      }

      mediaRecorder.value.start(1000) // 每秒收集一次
      appStore.setRecording(true)

      // 开始计时
      duration.value = 0
      durationTimer.value = setInterval(() => {
        duration.value++
      }, 1000)

    } catch (error) {
      console.error('Failed to start recording:', error)
      appStore.showToast('无法访问麦克风')
    }
  }

  // 停止录音
  function stopRecording() {
    if (mediaRecorder.value && mediaRecorder.value.state !== 'inactive') {
      mediaRecorder.value.stop()
      appStore.setRecording(false)

      if (durationTimer.value) {
        clearInterval(durationTimer.value)
        durationTimer.value = null
      }
    }
  }

  // 上传录音
  async function uploadRecording(blob) {
    try {
      appStore.setTranscribing(true)
      const result = await api.uploadAudio(blob, `recording_${Date.now()}.webm`)

      if (result.record_id) {
        appStore.setCurrentRecordId(result.record_id)
        // 开始轮询状态
        startStatusPolling(result.record_id)
      }
    } catch (error) {
      console.error('Upload failed:', error)
      appStore.showToast('上传失败')
      appStore.setTranscribing(false)
    }
  }

  // 监听状态更新
  onRecordStatus((status) => {
    appStore.setRecordStatus(status)
  })

  // 监听完成
  onRecordCompleted((data) => {
    appStore.setTranscribing(false)
    stopStatusPolling()

    // 自动复制粘贴
    if (settingsStore.settings.auto_copy_to_clipboard) {
      const text = data.raw_text || data.optimized_text
      if (text) {
        // 通过 Electron 复制粘贴
        window.electronAPI?.copyAndPaste(text)
      }
    }

    appStore.showToast('转写完成')
  })

  return {
    isRecording,
    duration,
    startRecording,
    stopRecording
  }
}
```

### 12. electron-builder.yml

```yaml
appId: com.att.audio-to-text
productName: Audio to Text
directories:
  buildResources: build
  output: release

files:
  - dist/**/*
  - electron/**/*
  - app/**/*.py
  - package.json

extraResources:
  - from: python-dist
    to: python
    filter:
      - "**/*"
  - from: app
    to: app
    filter:
      - "**/*.py"
  - from: data
    to: data
  - from: models
    to: models

mac:
  category: public.app-category.productivity
  icon: build/icon.icns
  target:
    - dmg
    - zip
  hardenedRuntime: true
  gatekeeperAssess: false

win:
  icon: build/icon.ico
  target:
    - nsis
    - portable

nsis:
  oneClick: false
  allowToChangeInstallationDirectory: true
```

---

## 实现步骤

### Phase 1: 项目初始化 (Day 1)
1. 创建 `package.json`，安装依赖
2. 创建 `vite.config.js`
3. 复制 `static/css/style.css` 到 `src/styles/main.css`（**不修改**）
4. 创建基础 Vue 结构

### Phase 2: Electron 主进程 (Day 2-3)
1. 实现 `electron/main.js`
2. 实现 `electron/python/manager.js`
3. 实现 `electron/tray/` 模块
4. 实现 `electron/ipc/` 通信层

### Phase 3: Vue 组件迁移 (Day 4-5)
1. 将 HTML 结构转换为 Vue 组件
2. 保持 CSS class 名称不变
3. 实现 composables（useRecorder, useRecords, useSettings）
4. 实现 Pinia stores

### Phase 4: 功能完善 (Day 6-7)
1. 录音功能测试
2. 转写流程测试
3. 设置页面测试
4. 托盘功能测试

### Phase 5: 打包发布 (Day 8)
1. Python 环境嵌入
2. 各平台打包测试
3. 安装程序制作

---

## 验证方法

1. `npm run electron:dev` 启动开发模式
2. 确认样式与原版完全一致
3. 确认录音功能正常
4. 确认转写、优化功能正常
5. 确认托盘图标和菜单正常
6. 确认全局热键正常
7. `npm run electron:build` 打包测试
