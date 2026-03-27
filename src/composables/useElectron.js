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

function buildAccelerator(hotkey) {
  if (!hotkey?.key) {
    return ''
  }

  const modifiers = hotkey?.modifiers || []
  const platform = globalThis.window?.electronAPI?.platform || 'web'
  const sortWeight = platform === 'win32'
    ? { alt: 1, ctrl: 2, shift: 3, win: 4, cmd: 5 }
    : { cmd: 1, ctrl: 2, alt: 3, shift: 4, win: 5 }

  const parts = [...modifiers]
    .sort((left, right) => (sortWeight[left] || 99) - (sortWeight[right] || 99))
    .map((modifier) => {
    if (modifier === 'cmd') return 'CommandOrControl'
    if (modifier === 'ctrl') return 'Control'
    if (modifier === 'alt') return 'Alt'
    if (modifier === 'shift') return 'Shift'
    if (modifier === 'win') return 'Super'
    return modifier
  })

  if (hotkey?.key) {
    parts.push(String(hotkey.key).toUpperCase())
  }

  return parts.join('+')
}

export function useElectron() {
  const isReady = ref(false)

  const api = {
    uploadAudio: (blob, filename = 'recording.webm') => httpApi.uploadAudio(blob, filename),
    createSession: () => httpApi.createSession(),
    uploadSessionSegment: (sessionId, blob, filename = 'segment.webm') => httpApi.uploadSessionSegment(sessionId, blob, filename),
    mergeSession: (sessionId) => httpApi.mergeSession(sessionId),
    deleteSession: (sessionId) => httpApi.deleteSession(sessionId),
    getRecords: () => httpApi.getRecords(),
    searchRecords: (params) => httpApi.searchRecords(params),
    getRecord: (id) => httpApi.getRecord(id),
    getRecordStatus: (id) => httpApi.getRecordStatus(id),
    updateRecord: (id, data) => httpApi.updateRecord(id, data),
    deleteRecord: (id) => httpApi.deleteRecord(id),
    optimizeRecord: (id) => httpApi.optimizeRecord(id),
    exportRecord: (id, format) => httpApi.exportRecord(id, format),
    getRecordAudio: (id) => httpApi.getRecordAudio(id),
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

  function onHotkeyStart(callback) {
    globalThis.window?.electronAPI?.onHotkeyStart(callback)
  }

  function onHotkeyStop(callback) {
    globalThis.window?.electronAPI?.onHotkeyStop(callback)
  }

  function onPasteText(callback) {
    globalThis.window?.electronAPI?.onPasteText(callback)
  }

  function onNavigateSettings(callback) {
    globalThis.window?.electronAPI?.onNavigateSettings(callback)
  }

  function removeAllListeners(channel) {
    globalThis.window?.electronAPI?.removeAllListeners(channel)
  }

  async function updateHotkey(hotkey) {
    const plainHotkey = toPlainHotkey(hotkey)
    const accelerator = buildAccelerator(plainHotkey)
    if (!accelerator) {
      return {
        success: false,
        error: '快捷键必须包含一个字母或数字键'
      }
    }

    return globalThis.window?.electronAPI?.updateHotkey({
      ...plainHotkey,
      accelerator
    })
  }

  async function syncDesktopSettings(settings) {
    if (!globalThis.window?.electronAPI?.syncDesktopSettings) {
      return { success: false, error: 'Desktop settings sync unavailable' }
    }

    return globalThis.window.electronAPI.syncDesktopSettings(toPlainObject(settings))
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
    onHotkeyStart,
    onHotkeyStop,
    onPasteText,
    onNavigateSettings,
    removeAllListeners,
    updateHotkey,
    syncDesktopSettings,
    syncRecordingState,
    startStatusPolling,
    stopStatusPolling,
    platform: globalThis.window?.electronAPI?.platform || 'web',
    getBackendPort
  }
}
