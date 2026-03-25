/**
 * Electron API 封装
 * 提供统一的 Electron API 访问接口
 */

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
      return globalThis.window?.electronAPI?.uploadAudio({
        buffer: Array.from(new Uint8Array(buffer)),
        filename
      })
    },

    // 记录
    getRecords: () => globalThis.window?.electronAPI?.getRecords(),
    getRecord: (id) => globalThis.window?.electronAPI?.getRecord(id),
    getRecordStatus: (id) => globalThis.window?.electronAPI?.getRecordStatus(id),
    updateRecord: (id, data) => globalThis.window?.electronAPI?.updateRecord(id, data),
    deleteRecord: (id) => globalThis.window?.electronAPI?.deleteRecord(id),
    optimizeRecord: (id) => globalThis.window?.electronAPI?.optimizeRecord(id),
    exportRecord: (id, format) => globalThis.window?.electronAPI?.exportRecord(id, format),
    getRecordAudio: (id) => globalThis.window?.electronAPI?.getRecordAudio(id),

    // 设置
    getSettings: () => globalThis.window?.electronAPI?.getSettings(),
    saveSettings: (settings) => globalThis.window?.electronAPI?.saveSettings(settings),

    // LLM 配置
    getLLMConfigs: () => globalThis.window?.electronAPI?.getLLMConfigs(),
    getLLMConfig: (id) => globalThis.window?.electronAPI?.getLLMConfig(id),
    saveLLMConfig: (config) => globalThis.window?.electronAPI?.saveLLMConfig(config),
    deleteLLMConfig: (id) => globalThis.window?.electronAPI?.deleteLLMConfig(id),
    testLLMConfig: (id) => globalThis.window?.electronAPI?.testLLMConfig(id),
    setDefaultLLMConfig: (id) => globalThis.window?.electronAPI?.setDefaultLLMConfig(id),
    getLLMProviders: () => globalThis.window?.electronAPI?.getLLMProviders(),
    getLLMProviderModels: (provider) => globalThis.window?.electronAPI?.getLLMProviderModels(provider),

    // 预设
    getPresets: () => globalThis.window?.electronAPI?.getPresets(),
    getPreset: (id) => globalThis.window?.electronAPI?.getPreset(id),
    savePreset: (preset) => globalThis.window?.electronAPI?.savePreset(preset),
    deletePreset: (id) => globalThis.window?.electronAPI?.deletePreset(id),
    previewPreset: (id, text) => globalThis.window?.electronAPI?.previewPreset(id, text),
    setDefaultPreset: (id) => globalThis.window?.electronAPI?.setDefaultPreset(id)
  }

  // === 窗口控制 ===
  // 注意：不要使用 const window，会覆盖全局 window 对象！
  const windowControl = {
    minimize: () => globalThis.window?.electronAPI?.minimizeWindow(),
    maximize: () => globalThis.window?.electronAPI?.maximizeWindow(),
    close: () => globalThis.window?.electronAPI?.closeWindow()
  }

  // === 托盘控制 ===
  const tray = {
    setIcon: (iconType) => globalThis.window?.electronAPI?.setTrayIcon(iconType)
  }

  // === 事件监听 ===
  function onRecordStatus(callback) {
    console.log('[useElectron] Registering onRecordStatus callback')
    globalThis.window?.electronAPI?.onRecordStatus(callback)
  }

  function onRecordCompleted(callback) {
    console.log('[useElectron] Registering onRecordCompleted callback')
    globalThis.window?.electronAPI?.onRecordCompleted(callback)
  }

  function onHotkeyStart(callback) {
    globalThis.window?.electronAPI?.onHotkeyStart(callback)
  }

  function onHotkeyStop(callback) {
    globalThis.window?.electronAPI?.onHotkeyStop(callback)
  }

  function onPasteText(callback) {
    globalThis.window?.electronAPI?.onPasteText(callback)
  }

  function removeAllListeners(channel) {
    globalThis.window?.electronAPI?.removeAllListeners(channel)
  }

  // === 状态轮询 ===
  function startStatusPolling(recordId) {
    globalThis.window?.electronAPI?.startStatusPolling(recordId)
  }

  function stopStatusPolling() {
    globalThis.window?.electronAPI?.stopStatusPolling()
  }

  return {
    isElectron,
    isReady,
    api,
    windowControl,
    tray,
    onRecordStatus,
    onRecordCompleted,
    onHotkeyStart,
    onHotkeyStop,
    onPasteText,
    removeAllListeners,
    startStatusPolling,
    stopStatusPolling,
    platform: globalThis.window?.electronAPI?.platform || 'web'
  }
}
