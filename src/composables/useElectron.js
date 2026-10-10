import { ref } from 'vue'
import { api as httpApi, getBackendPort } from '@/api/client'

const isElectron = typeof window !== 'undefined' && !!window.electronAPI

function toPlainHotkey(hotkey) {
  if (!hotkey) {
    return null
  }

  return {
    modifiers: Array.isArray(hotkey.modifiers) ? [...hotkey.modifiers] : [],
    key: hotkey.key ?? null,
    display: hotkey.display ?? ''
  }
}

function toPlainObject(value) {
  return value == null ? value : JSON.parse(JSON.stringify(value))
}

export function useElectron() {
  const isReady = ref(false)

  function debugLog(scope, message, details) {
    globalThis.window?.electronAPI?.debugLog?.(scope, message, toPlainObject(details))
  }

  const api = {
    uploadAudio: (blob, filename = 'recording.webm') => httpApi.uploadAudio(blob, filename),
    createSession: () => httpApi.createSession(),
    uploadSessionSegment: (sessionId, blob, filename = 'segment.webm') => httpApi.uploadSessionSegment(sessionId, blob, filename),
    mergeSession: (sessionId) => httpApi.mergeSession(sessionId),
    deleteSession: (sessionId) => httpApi.deleteSession(sessionId),
    getWebSocketUrl: () => httpApi.getWebSocketUrl(),
    getStreamingWebSocketUrl: () => httpApi.getStreamingWebSocketUrl(),
    getRecords: () => httpApi.getRecords(),
    searchRecords: (params) => httpApi.searchRecords(params),
    getRecord: (id) => httpApi.getRecord(id),
    getRecordStatus: (id) => httpApi.getRecordStatus(id),
    updateRecord: (id, data) => httpApi.updateRecord(id, data),
    deleteRecord: (id) => httpApi.deleteRecord(id),
    optimizeRecord: (id) => httpApi.optimizeRecord(id),
    exportRecord: (id, format) => httpApi.exportRecord(id, format),
    getRecordAudio: (id) => httpApi.getRecordAudio(id),
    getHealth: (probeRuntime = false) => httpApi.getHealth(probeRuntime),
    getStreamingServiceStatus: () => httpApi.getStreamingServiceStatus(),
    startStreamingService: (waitUntilReady = true) => httpApi.startStreamingService(waitUntilReady),
    stopStreamingService: () => httpApi.stopStreamingService(),
    getClipboardItems: (params) => httpApi.getClipboardItems(params),
    getClipboardStats: () => httpApi.getClipboardStats(),
    createClipboardItem: (payload) => httpApi.createClipboardItem(payload),
    getClipboardItem: (id) => httpApi.getClipboardItem(id),
    updateClipboardItem: (id, data) => httpApi.updateClipboardItem(id, data),
    deleteClipboardItem: (id) => httpApi.deleteClipboardItem(id),
    getSettings: () => httpApi.getSettings(),
    saveSettings: (settings) => httpApi.saveSettings(settings),
    getLLMConfigs: () => httpApi.getLLMConfigs(),
    getLLMConfig: (id) => httpApi.getLLMConfig(id),
    saveLLMConfig: (config) => httpApi.saveLLMConfig(config),
    deleteLLMConfig: (id) => httpApi.deleteLLMConfig(id),
    testLLMConfig: (id) => httpApi.testLLMConfig(id),
    setDefaultLLMConfig: (id) => httpApi.setDefaultLLMConfig(id),
    getLLMProviders: () => httpApi.getLLMProviders(),
    getLLMProviderModels: (provider) => httpApi.getLLMProviderModels(provider),
    getPresets: () => httpApi.getPresets(),
    getPreset: (id) => httpApi.getPreset(id),
    savePreset: (preset) => httpApi.savePreset(preset),
    deletePreset: (id) => httpApi.deletePreset(id),
    previewPreset: (id, text) => httpApi.previewPreset(id, text),
    setDefaultPreset: (id) => httpApi.setDefaultPreset(id)
  }

  const windowControl = {
    minimize: () => globalThis.window?.electronAPI?.minimizeWindow(),
    maximize: () => globalThis.window?.electronAPI?.maximizeWindow(),
    close: () => globalThis.window?.electronAPI?.closeWindow()
  }

  const tray = {
    setIcon: (iconType) => globalThis.window?.electronAPI?.setTrayIcon(iconType),
    updateMenu: (records = []) => globalThis.window?.electronAPI?.updateTrayMenu(records)
  }

  const floatingWindow = {
    show: () => globalThis.window?.electronAPI?.showFloatingWindow(),
    hide: () => globalThis.window?.electronAPI?.hideFloatingWindow(),
    toggle: () => globalThis.window?.electronAPI?.toggleFloatingWindow(),
    updateState: (payload) => globalThis.window?.electronAPI?.updateFloatingWindowState(toPlainObject(payload))
  }

  // 统一浮层已整体下线：两组浮层封装与对应的两个事件监听一并删除。

  const clipboard = {
    readText: async () => {
      if (globalThis.window?.electronAPI?.readClipboardText) {
        try {
          const result = await globalThis.window.electronAPI.readClipboardText()
          if (typeof result === 'string') {
            return result
          }
        } catch (error) {
          console.error('[Clipboard] Electron clipboard read failed:', error)
        }
      }

      if (globalThis.navigator?.clipboard?.readText) {
        return globalThis.navigator.clipboard.readText()
      }

      throw new Error('Clipboard read API unavailable')
    },
    writeText: async (text) => {
      if (globalThis.window?.electronAPI?.writeClipboard) {
        try {
          const result = await globalThis.window.electronAPI.writeClipboard(text)
          if (result !== false) {
            return true
          }
        } catch (error) {
          console.error('[Clipboard] Electron clipboard write failed:', error)
        }
      }

      if (globalThis.navigator?.clipboard?.writeText) {
        await globalThis.navigator.clipboard.writeText(text)
        return true
      }

      throw new Error('Clipboard API unavailable')
    },
    writeImage: async (payload) => {
      if (globalThis.window?.electronAPI?.writeClipboardImage) {
        const result = await globalThis.window.electronAPI.writeClipboardImage(toPlainObject(payload))
        if (result !== false) {
          return true
        }
      }

      throw new Error('Image clipboard API unavailable')
    },
    pasteActive: async () => {
      if (globalThis.window?.electronAPI?.pasteClipboard) {
        const result = await globalThis.window.electronAPI.pasteClipboard()
        if (!result || result.success === false) {
          throw new Error(result?.error || 'Paste helper failed')
        }
        return true
      }

      throw new Error('System paste is only available in Electron')
    }
  }

  function onRecordStatus() {}
  function onRecordCompleted() {}

  function onHotkeyPress(callback) {
    globalThis.window?.electronAPI?.onHotkeyPress?.(callback)
  }

  function onHotkeyRelease(callback) {
    globalThis.window?.electronAPI?.onHotkeyRelease?.(callback)
  }

  function onPasteText(callback) {
    globalThis.window?.electronAPI?.onPasteText(callback)
  }

  function onNavigateSettings(callback) {
    globalThis.window?.electronAPI?.onNavigateSettings(callback)
  }

  function onClipboardChanged(callback) {
    globalThis.window?.electronAPI?.onClipboardChanged(callback)
  }

  function onDesktopPermissionRequired(callback) {
    globalThis.window?.electronAPI?.onDesktopPermissionRequired?.(callback)
  }

  function onDesktopAgentError(callback) {
    globalThis.window?.electronAPI?.onDesktopAgentError?.(callback)
  }

  function openSettingsPage() {
    globalThis.window?.electronAPI?.openSettingsPage()
  }

  function removeAllListeners(channel) {
    globalThis.window?.electronAPI?.removeAllListeners(channel)
  }

  async function updateHotkey(hotkey) {
    const plainHotkey = toPlainHotkey(hotkey)
    if (!Array.isArray(plainHotkey?.modifiers) || plainHotkey.modifiers.length === 0) {
      return {
        success: false,
        error: '快捷键至少需要一个修饰键'
      }
    }

    return globalThis.window?.electronAPI?.updateHotkey(plainHotkey)
  }

  async function syncInputSettings(settings) {
    return globalThis.window?.electronAPI?.syncInputSettings(toPlainObject(settings))
  }

  async function syncDesktopSettings(settings) {
    if (!globalThis.window?.electronAPI?.syncDesktopSettings) {
      return { success: false, error: 'Desktop settings sync unavailable' }
    }

    return globalThis.window.electronAPI.syncDesktopSettings(toPlainObject(settings))
  }

  async function getDesktopSettings() {
    return globalThis.window?.electronAPI?.getDesktopSettings?.()
  }

  async function saveDesktopSettings(settings) {
    return globalThis.window?.electronAPI?.saveDesktopSettings?.(toPlainObject(settings))
  }

  async function syncRecordingState(payload) {
    if (!globalThis.window?.electronAPI?.syncRecordingState) {
      return { success: false, error: 'Recording state sync unavailable' }
    }

    return globalThis.window.electronAPI.syncRecordingState(toPlainObject(payload))
  }

  function startStatusPolling() {}
  function stopStatusPolling() {}

  return {
    isElectron,
    isReady,
    api,
    windowControl,
    tray,
    floatingWindow,
    clipboard,
    onRecordStatus,
    onRecordCompleted,
    onHotkeyPress,
    onHotkeyRelease,
    onPasteText,
    onNavigateSettings,
    onClipboardChanged,
    onDesktopPermissionRequired,
    onDesktopAgentError,
    openSettingsPage,
    debugLog,
    removeAllListeners,
    updateHotkey,
    syncInputSettings,
    syncDesktopSettings,
    getDesktopSettings,
    saveDesktopSettings,
    syncRecordingState,
    startStatusPolling,
    stopStatusPolling,
    platform: globalThis.window?.electronAPI?.platform || 'web',
    getBackendPort
  }
}
