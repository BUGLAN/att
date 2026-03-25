/**
 * Electron Preload 脚本
 * 通过 contextBridge 暴露安全的 API 给渲染进程
 */

const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('electronAPI', {
  // 平台信息
  platform: process.platform,

  // === 窗口控制 ===
  minimizeWindow: () => ipcRenderer.send('window:minimize'),
  maximizeWindow: () => ipcRenderer.send('window:maximize'),
  closeWindow: () => ipcRenderer.send('window:close'),

  // === 托盘控制 ===
  setTrayIcon: (iconType) => ipcRenderer.send('tray:set-icon', iconType),

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
  getRecordAudio: (id) => ipcRenderer.invoke('api:records:audio', id),

  // 设置
  getSettings: () => ipcRenderer.invoke('api:settings:get'),
  saveSettings: (settings) => ipcRenderer.invoke('api:settings:save', settings),

  // LLM 配置
  getLLMConfigs: () => ipcRenderer.invoke('api:llm:list'),
  getLLMConfig: (id) => ipcRenderer.invoke('api:llm:get', id),
  saveLLMConfig: (config) => ipcRenderer.invoke('api:llm:save', config),
  deleteLLMConfig: (id) => ipcRenderer.invoke('api:llm:delete', id),
  testLLMConfig: (id) => ipcRenderer.invoke('api:llm:test', id),
  setDefaultLLMConfig: (id) => ipcRenderer.invoke('api:llm:set-default', id),
  getLLMProviders: () => ipcRenderer.invoke('api:llm:providers'),
  getLLMProviderModels: (provider) => ipcRenderer.invoke('api:llm:provider-models', provider),

  // 预设
  getPresets: () => ipcRenderer.invoke('api:prompts:list'),
  getPreset: (id) => ipcRenderer.invoke('api:prompts:get', id),
  savePreset: (preset) => ipcRenderer.invoke('api:prompts:save', preset),
  deletePreset: (id) => ipcRenderer.invoke('api:prompts:delete', id),
  previewPreset: (id, text) => ipcRenderer.invoke('api:prompts:preview', { id, text }),
  setDefaultPreset: (id) => ipcRenderer.invoke('api:prompts:set-default', id),

  // === 事件监听 ===

  onRecordStatus: (callback) => {
    console.log('[Preload] Registering onRecordStatus callback')
    ipcRenderer.on('record:status', (event, data) => {
      console.log('[Preload] Received record:status:', data.record_id, data.status)
      callback(data)
    })
  },
  onRecordCompleted: (callback) => {
    console.log('[Preload] Registering onRecordCompleted callback')
    ipcRenderer.on('record:completed', (event, data) => {
      console.log('[Preload] Received record:completed:', data.record_id, data.status)
      callback(data)
    })
  },
  onHotkeyStart: (callback) => {
    ipcRenderer.on('hotkey:start', () => callback())
  },
  onHotkeyStop: (callback) => {
    ipcRenderer.on('hotkey:stop', () => callback())
  },
  onPasteText: (callback) => {
    ipcRenderer.on('paste-text', () => callback())
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
