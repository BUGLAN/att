# ATT 悬浮窗功能实现计划

## Context

ATT 是一个 Electron + FastAPI (Python) 的音频转文字应用。用户希望通过快捷键触发一个悬浮窗，显示在桌面中间偏下位置，用于实时显示录音状态和转写结果。

**现有架构**：
- Electron 主进程管理窗口和托盘 (`electron/main.js`)
- Vue 3 前端 (`src/` 目录)
- Python 后端通过 WebSocket 通信
- 已有快捷键监听机制 (`tray/hotkey.py`) 通过 WebSocket 广播录音命令

**用户确认的需求**：
- 窗口尺寸：360px × 80px
- 自动隐藏：转写完成后 3 秒
- 失去焦点：不隐藏，保持显示

## 实现方案

### 技术选型：Electron BrowserWindow 独立窗口

- 与现有架构一致，无需引入新依赖
- 支持跨应用置顶显示
- 可复用现有 WebSocket 通信机制

### 文件结构

```
electron/
├── main.js                      # 修改：集成悬浮窗管理
├── preload.js                   # 修改：添加悬浮窗 API
└── floatingWindow/              # 新增
    ├── index.js                 # 悬浮窗管理器
    └── preload.js               # 悬浮窗专用 preload

src/
├── floating.js                  # 新增：悬浮窗入口
├── floating.html                # 新增：悬浮窗 HTML
├── components/floating/         # 新增
│   ├── FloatingWindow.vue       # 主组件
│   ├── RecordingPill.vue        # 录音状态 pill
│   └── Waveform.vue             # 波形动画
└── styles/floating.css          # 新增：悬浮窗样式

vite.config.js                   # 修改：多入口构建
```

## 实现步骤

### Step 1: 创建悬浮窗管理器

**文件**: `electron/floatingWindow/index.js` (新建)

```javascript
const { BrowserWindow, screen } = require('electron')
const path = require('path')

class FloatingWindowManager {
  constructor() {
    this.floatingWindow = null
    this.isVisible = false
  }

  createWindow() {
    if (this.floatingWindow) {
      return this.floatingWindow
    }

    const isDev = process.env.NODE_ENV === 'development'

    this.floatingWindow = new BrowserWindow({
      width: 360,
      height: 80,
      frame: false,
      transparent: true,
      alwaysOnTop: true,
      resizable: false,
      skipTaskbar: true,
      hasShadow: false,
      focusable: true,
      webPreferences: {
        nodeIntegration: false,
        contextIsolation: true,
        preload: path.join(__dirname, 'preload.js')
      }
    })

    // 加载悬浮窗页面
    if (isDev) {
      this.floatingWindow.loadURL('http://localhost:5173/floating.html')
    } else {
      this.floatingWindow.loadFile(
        path.join(__dirname, '../../dist/floating.html')
      )
    }

    this.floatingWindow.on('closed', () => {
      this.floatingWindow = null
      this.isVisible = false
    })

    return this.floatingWindow
  }

  calculatePosition() {
    const primaryDisplay = screen.getPrimaryDisplay()
    const { width: screenWidth, height: screenHeight } = primaryDisplay.workAreaSize
    const windowWidth = 360
    const windowHeight = 80

    return {
      x: Math.floor((screenWidth - windowWidth) / 2),
      y: Math.floor(screenHeight - windowHeight - 120)
    }
  }

  show() {
    if (!this.floatingWindow) {
      this.createWindow()
    }

    const position = this.calculatePosition()
    this.floatingWindow.setPosition(position.x, position.y)
    this.floatingWindow.show()
    this.floatingWindow.focus()
    this.isVisible = true
  }

  hide() {
    if (this.floatingWindow) {
      this.floatingWindow.hide()
      this.isVisible = false
    }
  }

  toggle() {
    if (this.isVisible) {
      this.hide()
    } else {
      this.show()
    }
  }

  send(channel, data) {
    if (this.floatingWindow) {
      this.floatingWindow.webContents.send(channel, data)
    }
  }

  destroy() {
    if (this.floatingWindow) {
      this.floatingWindow.destroy()
      this.floatingWindow = null
      this.isVisible = false
    }
  }
}

module.exports = { FloatingWindowManager }
```

### Step 2: 创建悬浮窗 Preload

**文件**: `electron/floatingWindow/preload.js` (新建)

```javascript
const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('floatingAPI', {
  platform: process.platform,

  // 窗口控制
  close: () => ipcRenderer.send('floating:hide'),
  openSettings: () => ipcRenderer.send('main:open-settings'),

  // 录音控制
  startRecording: () => ipcRenderer.send('recording:start'),
  stopRecording: () => ipcRenderer.send('recording:stop'),

  // 事件监听
  onRecordingStart: (callback) => {
    ipcRenderer.on('recording:start', () => callback())
  },
  onRecordingStop: (callback) => {
    ipcRenderer.on('recording:stop', () => callback())
  },
  onTranscriptionUpdate: (callback) => {
    ipcRenderer.on('transcription:update', (event, data) => callback(data))
  },
  onTranscriptionComplete: (callback) => {
    ipcRenderer.on('transcription:complete', (event, data) => callback(data))
  },

  removeAllListeners: (channel) => {
    ipcRenderer.removeAllListeners(channel)
  }
})
```

### Step 3: 修改 Electron 主进程

**文件**: `electron/main.js` (修改)

在文件顶部添加导入：
```javascript
const { FloatingWindowManager } = require('./floatingWindow/index')
```

在全局变量区域添加：
```javascript
let floatingWindowManager = null
```

在 `setupIPC()` 函数中添加：
```javascript
// 悬浮窗控制
ipcMain.on('floating:show', () => {
  floatingWindowManager?.show()
})

ipcMain.on('floating:hide', () => {
  floatingWindowManager?.hide()
})

ipcMain.on('floating:toggle', () => {
  floatingWindowManager?.toggle()
})

ipcMain.on('main:open-settings', () => {
  mainWindow?.show()
  mainWindow?.focus()
})

// 转发录音事件到悬浮窗
ipcMain.on('recording:start', () => {
  floatingWindowManager?.send('recording:start')
})

ipcMain.on('recording:stop', () => {
  floatingWindowManager?.send('recording:stop')
})
```

在 `app.whenReady()` 中添加初始化：
```javascript
floatingWindowManager = new FloatingWindowManager()
```

### Step 4: 创建悬浮窗 HTML 入口

**文件**: `src/floating.html` (新建)

```html
<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>ATT Floating</title>
</head>
<body>
  <div id="app"></div>
  <script type="module" src="./floating.js"></script>
</body>
</html>
```

### Step 5: 创建悬浮窗 Vue 入口

**文件**: `src/floating.js` (新建)

```javascript
import { createApp } from 'vue'
import { createPinia } from 'pinia'
import FloatingWindow from './components/floating/FloatingWindow.vue'
import './styles/floating.css'

const app = createApp(FloatingWindow)
const pinia = createPinia()

app.use(pinia)
app.mount('#app')
```

### Step 6: 创建悬浮窗样式

**文件**: `src/styles/floating.css` (新建)

```css
/* 悬浮窗样式 - 精确复刻用户设计 */
@import url('https://fonts.googleapis.com/css2?family=Manrope:wght@400;700;800&family=Inter:wght@400;500;600&display=swap');
@import url('https://fonts.googleapis.com/css2?family=Material+Symbols+Outlined:wght,FILL@100..700,0..1&display=swap');

:root {
  /* Material You 配色 - 来自用户提供的设计 */
  --primary: #ffc557;
  --primary-glow: rgba(255, 197, 87, 0.2);
  --on-primary: #422d00;
  --surface: #131313;
  --on-surface: #e5e2e1;
  --on-surface-variant: #d0c5af;
  --outline-variant: #4d4635;
  --error: #ffb4ab;

  /* 字体 */
  --font-headline: 'Manrope', system-ui, sans-serif;
  --font-body: 'Inter', system-ui, sans-serif;
}

* {
  margin: 0;
  padding: 0;
  box-sizing: border-box;
}

html, body {
  background: transparent !important;
  overflow: hidden;
  -webkit-user-select: none;
  user-select: none;
  font-family: var(--font-body);
}

#app {
  width: 100vw;
  height: 100vh;
  display: flex;
  align-items: flex-end;
  justify-content: center;
  padding-bottom: 0;
}

/* 玻璃态 Pill */
.glass-pill {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 12px 16px;
  background: rgba(20, 19, 19, 0.9);
  border: 1px solid rgba(255, 255, 255, 0.05);
  border-radius: 40px;
  backdrop-filter: blur(16px);
  -webkit-backdrop-filter: blur(16px);
  box-shadow:
    0 4px 24px rgba(0, 0, 0, 0.4),
    inset 0 1px 0 rgba(255, 255, 255, 0.05);
  min-width: 320px;
  max-width: 360px;
  animation: slide-up 0.5s ease-out;
}

@keyframes slide-up {
  from {
    opacity: 0;
    transform: translateY(20px);
  }
  to {
    opacity: 1;
    transform: translateY(0);
  }
}

/* 状态指示器 */
.status-indicator {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
}

.status-icon-wrapper {
  width: 40px;
  height: 40px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  position: relative;
  transition: all 0.3s ease;
}

.status-icon-wrapper.idle {
  background: rgba(255, 255, 255, 0.05);
}

.status-icon-wrapper.recording {
  background: var(--primary-glow);
}

/* 脉冲效果 */
.status-icon-wrapper.recording::before {
  content: '';
  position: absolute;
  inset: 0;
  border-radius: 50%;
  background: var(--primary);
  animation: pulse-ring 2s cubic-bezier(0.455, 0.03, 0.515, 0.955) infinite;
}

@keyframes pulse-ring {
  0% { transform: scale(0.8); opacity: 0.5; }
  100% { transform: scale(1.3); opacity: 0; }
}

.status-icon {
  font-family: 'Material Symbols Outlined';
  font-size: 20px;
  color: var(--primary);
  position: relative;
  z-index: 1;
}

/* 分隔线 */
.divider {
  width: 1px;
  height: 24px;
  background: rgba(255, 255, 255, 0.1);
}

/* 波形动画 */
.waveform {
  display: flex;
  align-items: center;
  gap: 2px;
  height: 24px;
}

.wave-bar {
  width: 2px;
  background: linear-gradient(to top, var(--primary), #e8a70f);
  border-radius: 1px;
  transition: height 0.15s ease;
}

.wave-bar:nth-child(1) { height: 8px; opacity: 0.4; }
.wave-bar:nth-child(2) { height: 16px; opacity: 0.6; }
.wave-bar:nth-child(3) { height: 24px; opacity: 1; }
.wave-bar:nth-child(4) { height: 12px; opacity: 0.8; }
.wave-bar:nth-child(5) { height: 20px; opacity: 0.9; }

.waveform.active .wave-bar {
  animation: wave 0.6s ease-in-out infinite;
}

.wave-bar:nth-child(1) { animation-delay: 0s; }
.wave-bar:nth-child(2) { animation-delay: 0.1s; }
.wave-bar:nth-child(3) { animation-delay: 0.2s; }
.wave-bar:nth-child(4) { animation-delay: 0.3s; }
.wave-bar:nth-child(5) { animation-delay: 0.4s; }

@keyframes wave {
  0%, 100% { height: 8px; }
  50% { height: 24px; }
}

/* 内容区域 */
.content-area {
  flex: 1;
  min-width: 0;
}

.content-label {
  font-size: 14px;
  font-weight: 500;
  color: var(--on-surface);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.content-label .highlight {
  color: var(--primary);
  opacity: 0.9;
}

.status-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 2px;
}

.status-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--primary);
  animation: blink 1.5s ease-in-out infinite;
}

@keyframes blink {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.4; }
}

.status-label {
  font-size: 10px;
  color: var(--on-surface-variant);
  text-transform: uppercase;
  font-weight: 700;
  letter-spacing: 0.05em;
  opacity: 0.7;
}

/* 时长显示 */
.duration {
  font-family: 'Manrope', monospace;
  font-size: 14px;
  font-weight: 700;
  color: var(--on-surface);
}

/* 操作按钮 */
.action-buttons {
  display: flex;
  align-items: center;
  gap: 4px;
}

.action-btn {
  width: 32px;
  height: 32px;
  border-radius: 8px;
  border: none;
  background: transparent;
  color: var(--on-surface-variant);
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: all 0.15s ease;
}

.action-btn:hover {
  background: rgba(255, 255, 255, 0.05);
  color: var(--primary);
}

.action-btn.close:hover {
  color: var(--error);
}

.action-btn .material-symbols-outlined {
  font-size: 18px;
}

/* 转写中状态 */
.spinner {
  width: 16px;
  height: 16px;
  border: 2px solid var(--outline-variant);
  border-top-color: var(--primary);
  border-radius: 50%;
  animation: spin 1s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

/* 转写完成预览 */
.preview-text {
  font-size: 13px;
  color: var(--on-surface-variant);
  line-height: 1.4;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 200px;
}

/* Material Symbols 配置 */
.material-symbols-outlined {
  font-variation-settings: 'FILL' 0, 'wght' 400, 'GRAD' 0, 'opsz' 20;
}
```

### Step 7: 创建主悬浮窗组件

**文件**: `src/components/floating/FloatingWindow.vue` (新建)

```vue
<template>
  <div class="glass-pill" :class="windowState">
    <!-- 状态指示器 -->
    <div class="status-indicator">
      <div class="status-icon-wrapper" :class="statusClass">
        <span class="material-symbols-outlined status-icon">mic</span>
      </div>
    </div>

    <!-- 分隔线 -->
    <div class="divider"></div>

    <!-- 录音中状态 -->
    <template v-if="isRecording">
      <div class="waveform active">
        <div class="wave-bar" v-for="i in 5" :key="i"></div>
      </div>
      <div class="duration">{{ formattedDuration }}</div>
    </template>

    <!-- 转写中状态 -->
    <template v-else-if="isTranscribing">
      <div class="spinner"></div>
      <div class="content-area">
        <span class="content-label">转写中...</span>
      </div>
    </template>

    <!-- 完成状态 -->
    <template v-else-if="transcriptionText">
      <div class="content-area">
        <span class="preview-text">{{ truncatedText }}</span>
      </div>
    </template>

    <!-- 默认状态 -->
    <template v-else>
      <div class="content-area">
        <p class="content-label">
          正在转写：<span class="highlight">请继续说话，实时内容正在生成中...</span>
        </p>
        <div class="status-row">
          <span class="status-dot"></span>
          <span class="status-label">正在监听</span>
        </div>
      </div>
    </template>

    <!-- 操作按钮 -->
    <div class="action-buttons">
      <button class="action-btn" @click="openSettings" title="设置">
        <span class="material-symbols-outlined">settings</span>
      </button>
      <button class="action-btn close" @click="closeWindow" title="关闭">
        <span class="material-symbols-outlined">close</span>
      </button>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, onMounted, onUnmounted } from 'vue'

// 状态
const isRecording = ref(false)
const isTranscribing = ref(false)
const duration = ref(0)
const transcriptionText = ref('')
let durationTimer = null
let hideTimer = null

// 计算属性
const windowState = computed(() => {
  if (isRecording.value) return 'recording'
  if (isTranscribing.value) return 'transcribing'
  if (transcriptionText.value) return 'complete'
  return 'idle'
})

const statusClass = computed(() => {
  if (isRecording.value) return 'recording'
  return 'idle'
})

const formattedDuration = computed(() => {
  const mins = Math.floor(duration.value / 60)
  const secs = duration.value % 60
  return `${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`
})

const truncatedText = computed(() => {
  if (transcriptionText.value.length > 40) {
    return transcriptionText.value.substring(0, 40) + '...'
  }
  return transcriptionText.value
})

// 方法
function startRecording() {
  isRecording.value = true
  isTranscribing.value = false
  transcriptionText.value = ''
  duration.value = 0

  durationTimer = setInterval(() => {
    duration.value++
  }, 1000)
}

function stopRecording() {
  isRecording.value = false
  if (durationTimer) {
    clearInterval(durationTimer)
    durationTimer = null
  }
  isTranscribing.value = true
}

function onTranscriptionComplete(data) {
  isTranscribing.value = false
  transcriptionText.value = data.text || ''

  // 3 秒后自动隐藏
  if (hideTimer) clearTimeout(hideTimer)
  hideTimer = setTimeout(() => {
    window.floatingAPI?.close()
  }, 3000)
}

function closeWindow() {
  window.floatingAPI?.close()
}

function openSettings() {
  window.floatingAPI?.openSettings()
}

// 生命周期
onMounted(() => {
  // 监听 Electron 事件
  window.floatingAPI?.onRecordingStart(() => startRecording())
  window.floatingAPI?.onRecordingStop(() => stopRecording())
  window.floatingAPI?.onTranscriptionComplete((data) => onTranscriptionComplete(data))
})

onUnmounted(() => {
  if (durationTimer) clearInterval(durationTimer)
  if (hideTimer) clearTimeout(hideTimer)
  window.floatingAPI?.removeAllListeners('recording:start')
  window.floatingAPI?.removeAllListeners('recording:stop')
  window.floatingAPI?.removeAllListeners('transcription:complete')
})
</script>
```

### Step 8: 修改 Vite 配置

**文件**: `vite.config.js` (修改)

将现有配置修改为多入口：

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
  root: 'src',
  publicDir: '../public',
  build: {
    outDir: '../dist',
    emptyOutDir: true,
    assetsDir: 'assets',
    rollupOptions: {
      input: {
        main: resolve(__dirname, 'src/index.html'),
        floating: resolve(__dirname, 'src/floating.html')
      },
      output: {
        manualChunks: {
          vue: ['vue', 'pinia']
        }
      }
    }
  },
  server: {
    port: 5173,
    strictPort: true
  }
})
```

### Step 9: 修改主 Preload

**文件**: `electron/preload.js` (修改)

在现有的 `contextBridge.exposeInMainWorld` 中添加悬浮窗控制 API：

```javascript
// 在 electronAPI 对象中添加
showFloatingWindow: () => ipcRenderer.send('floating:show'),
hideFloatingWindow: () => ipcRenderer.send('floating:hide'),
toggleFloatingWindow: () => ipcRenderer.send('floating:toggle'),
```

### Step 10: 修改前端录音逻辑

**文件**: `src/composables/useRecorder.js` (修改)

在处理 `recording_command` 消息时，通知 Electron 显示/隐藏悬浮窗：

```javascript
// 在 handleWebSocketMessage 函数中
if (message.type === 'recording_command') {
  if (message.action === 'start') {
    window.electronAPI?.showFloatingWindow()
    await startRecording()
  } else if (message.action === 'stop') {
    stopRecording()
  }
  return
}
```

## 关键文件清单

| 文件 | 操作 | 说明 |
|------|------|------|
| `electron/floatingWindow/index.js` | 新建 | 悬浮窗管理器 |
| `electron/floatingWindow/preload.js` | 新建 | 悬浮窗 API |
| `electron/main.js` | 修改 | 集成悬浮窗管理 |
| `src/floating.html` | 新建 | HTML 入口 |
| `src/floating.js` | 新建 | Vue 入口 |
| `src/components/floating/FloatingWindow.vue` | 新建 | 主组件 |
| `src/styles/floating.css` | 新建 | 悬浮窗样式 |
| `vite.config.js` | 修改 | 多入口构建 |
| `electron/preload.js` | 修改 | 添加悬浮窗 API |
| `src/composables/useRecorder.js` | 修改 | 集成悬浮窗显示 |

## 验证方式

1. 运行 `npm run electron:dev`
2. 按下快捷键触发录音
3. 验证悬浮窗显示在屏幕底部中央
4. 验证波形动画和录音时长
5. 验证转写完成后显示文本预览
6. 验证 3 秒后自动隐藏
