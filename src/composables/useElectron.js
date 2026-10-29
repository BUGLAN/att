import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { api as httpApi, getBackendPort } from '@/api/client'

/**
 * 桌面能力包装层（Tauri 2 内核版，阶段 3a）。
 *
 * 形状兼容约定（架构文档 §10.1）：
 * - 导出面与旧 Electron 版完全一致，组件/composable 零改动
 * - `isElectron` 名称保留（语义 = 「桌面环境」），新增 `isDesktop` 别名
 * - 事件通道名沿用旧名（hotkey:press / paste-text / clipboard:changed …），
 *   Rust 侧事件桥按同名发射
 * - 3g 待接入项：openMediaFile / filePathForFile / pasteActive 返回降级值
 */

const isElectron = typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__
const isDesktop = isElectron

function toPlainObject(value) {
  return value == null ? value : JSON.parse(JSON.stringify(value))
}

// 同步平台推断（旧版读 window.electronAPI.platform 是同步的；Tauri invoke 是异步，
// 这里用 UA 做同步推断供默认值场景，精确值走 getPlatform()）
function detectPlatformSync() {
  if (typeof navigator === 'undefined') return 'web'
  const ua = navigator.userAgent
  if (ua.includes('Win')) return 'win32'
  if (ua.includes('Mac')) return 'darwin'
  if (ua.includes('Linux')) return 'linux'
  return 'web'
}

// ---------------- Tauri 事件通道管理（对齐旧 removeAllListeners 语义）----------------

const channelUnlisteners = new Map()

async function addChannelListener(channel, callback) {
  if (!isDesktop) return
  const { listen } = await import('@tauri-apps/api/event')
  const unlisten = await listen(channel, (event) => callback(event.payload))
  if (!channelUnlisteners.has(channel)) {
    channelUnlisteners.set(channel, new Set())
  }
  channelUnlisteners.get(channel).add(unlisten)
}

async function removeAllChannelListeners(channel) {
  const set = channelUnlisteners.get(channel)
  if (!set) return
  for (const unlisten of set) {
    try {
      await unlisten()
    } catch (error) {
      console.warn('[Desktop] Failed to unlisten channel:', channel, error)
    }
  }
  channelUnlisteners.delete(channel)
}

export function useElectron() {
  const isReady = ref(isDesktop)

  function debugLog(scope, message, details) {
    if (isDesktop) {
      void invoke('debug_log', { scope, message, details: toPlainObject(details) }).catch(() => {})
    }
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
    optimizeRecordStream: (id, options) => httpApi.optimizeRecordStream(id, options),
    cancelOptimizeRecord: (id) => httpApi.cancelOptimizeRecord(id),
    exportRecord: (id, format) => httpApi.exportRecord(id, format),
    getRecordAudio: (id) => httpApi.getRecordAudio(id),
    getHealth: (probeRuntime = false) => httpApi.getHealth(probeRuntime),
    getStreamingServiceStatus: () => httpApi.getStreamingServiceStatus(),
    startStreamingService: (waitUntilReady = true) => httpApi.startStreamingService(waitUntilReady),
    stopStreamingService: () => httpApi.stopStreamingService(),
    getClipboardItems: (params) => httpApi.getClipboardItems(params),
    getClipboardStats: () => httpApi.getClipboardStats(),
    createClipboardItem: (payload) => httpApi.createClipboardItem(payload),
    updateClipboardItem: (id, data) => httpApi.updateClipboardItem(id, data),
    deleteClipboardItem: (id) => httpApi.deleteClipboardItem(id),
    getSettings: () => httpApi.getSettings(),
    saveSettings: (settings) => httpApi.saveSettings(settings),
    // LLM v2（供应商档案 + 模型条目）
    getLLMState: () => httpApi.getLLMState(),
    getLLMProviders: () => httpApi.getLLMProviders(),
    createLLMProfile: (payload) => httpApi.createLLMProfile(payload),
    updateLLMProfile: (id, payload) => httpApi.updateLLMProfile(id, payload),
    deleteLLMProfile: (id) => httpApi.deleteLLMProfile(id),
    verifyLLMProfile: (id) => httpApi.verifyLLMProfile(id),
    getLLMRemoteModels: (id) => httpApi.getLLMRemoteModels(id),
    verifyLLMDraft: (payload) => httpApi.verifyLLMDraft(payload),
    createLLMModels: (items) => httpApi.createLLMModels(items),
    updateLLMModel: (id, payload) => httpApi.updateLLMModel(id, payload),
    deleteLLMModel: (id) => httpApi.deleteLLMModel(id),
    verifyLLMModel: (id) => httpApi.verifyLLMModel(id),
    activateLLMModel: (id) => httpApi.activateLLMModel(id),
    updateLLMSettings: (payload) => httpApi.updateLLMSettings(payload),
    getPresets: () => httpApi.getPresets(),
    setDefaultPreset: (id) => httpApi.setDefaultPreset(id)
  }

  const windowControl = {
    minimize: () => isDesktop && void invoke('window_minimize'),
    maximize: () => isDesktop && void invoke('window_toggle_maximize'),
    close: () => isDesktop && void invoke('window_close')
  }

  const tray = {
    setIcon: (iconType) => isDesktop && void invoke('set_tray_icon', { iconType }),
    updateMenu: (records = []) => isDesktop && void invoke('update_tray_menu', { records })
  }

  const floatingWindow = {
    show: () => isDesktop && void invoke('show_floating_window'),
    hide: () => isDesktop && void invoke('hide_floating_window'),
    toggle: () => isDesktop && void invoke('toggle_floating_window'),
    updateState: (payload) => isDesktop && void invoke('update_floating_window_state', { payload: toPlainObject(payload) })
  }

  const clipboard = {
    readText: async () => {
      if (isDesktop) {
        try {
          return await invoke('read_clipboard_text')
        } catch (error) {
          console.error('[Clipboard] Tauri clipboard read failed:', error)
        }
      }

      if (globalThis.navigator?.clipboard?.readText) {
        return globalThis.navigator.clipboard.readText()
      }

      throw new Error('Clipboard read API unavailable')
    },
    writeText: async (text) => {
      if (isDesktop) {
        try {
          const result = await invoke('write_clipboard_text', { text })
          if (result !== false) {
            return true
          }
        } catch (error) {
          console.error('[Clipboard] Tauri clipboard write failed:', error)
        }
      }

      if (globalThis.navigator?.clipboard?.writeText) {
        await globalThis.navigator.clipboard.writeText(text)
        return true
      }

      throw new Error('Clipboard API unavailable')
    },
    writeImage: async (payload) => {
      if (isDesktop) {
        const result = await invoke('write_clipboard_image', { payload: toPlainObject(payload) })
        if (result !== false) {
          return true
        }
      }

      throw new Error('Image clipboard API unavailable')
    },
    pasteActive: async () => {
      if (isDesktop) {
        return invoke('paste_active')
      }

      throw new Error('System paste is only available in desktop mode')
    }
  }

  function onHotkeyPress(callback) {
    void addChannelListener('hotkey:press', callback)
  }

  function onHotkeyRelease(callback) {
    void addChannelListener('hotkey:release', callback)
  }

  function onPasteText(callback) {
    void addChannelListener('paste-text', callback)
  }

  function onNavigateSettings(callback) {
    void addChannelListener('navigate:settings', callback)
  }

  function onClipboardChanged(callback) {
    void addChannelListener('clipboard:changed', callback)
  }

  function onDesktopPermissionRequired(callback) {
    void addChannelListener('desktop:permission-required', callback)
  }

  function onDesktopAgentError(callback) {
    void addChannelListener('desktop:agent-error', callback)
  }

  function removeAllListeners(channel) {
    void removeAllChannelListeners(channel)
  }

  async function syncInputSettings(settings) {
    if (!isDesktop) {
      return { success: false, error: 'Desktop settings sync unavailable' }
    }
    return invoke('sync_input_settings', { payload: toPlainObject(settings) })
  }

  async function syncDesktopSettings(settings) {
    if (!isDesktop) {
      return { success: false, error: 'Desktop settings sync unavailable' }
    }
    return invoke('sync_desktop_settings', { payload: toPlainObject(settings) })
  }

  async function getDesktopSettings() {
    if (!isDesktop) {
      return null
    }
    return invoke('get_desktop_settings')
  }

  async function saveDesktopSettings(settings) {
    if (!isDesktop) {
      return { success: false, error: 'Desktop settings sync unavailable' }
    }
    return invoke('save_desktop_settings', { payload: toPlainObject(settings) })
  }

  async function syncRecordingState(payload) {
    if (!isDesktop) {
      return { success: false, error: 'Recording state sync unavailable' }
    }
    return invoke('sync_recording_state', { payload: toPlainObject(payload) })
  }

  // 「转文字」页：系统文件选择对话框（3g 接入 dialog 插件；当前返回 null 走路径输入）
  async function openMediaFile() {
    return null
  }

  // 「转文字」页：拖拽落下的 File 对象换本地绝对路径（3g 接入 Tauri onDragDrop）
  function filePathForFile() {
    return ''
  }

  // 精确平台值（异步）：settings 默认值用同步 detectPlatformSync 即可
  async function getPlatform() {
    if (!isDesktop) {
      return detectPlatformSync()
    }
    return invoke('get_platform')
  }

  return {
    isElectron,
    isDesktop,
    isReady,
    api,
    windowControl,
    tray,
    floatingWindow,
    clipboard,
    onHotkeyPress,
    onHotkeyRelease,
    onPasteText,
    onNavigateSettings,
    onClipboardChanged,
    onDesktopPermissionRequired,
    onDesktopAgentError,
    debugLog,
    removeAllListeners,
    syncInputSettings,
    syncDesktopSettings,
    getDesktopSettings,
    saveDesktopSettings,
    syncRecordingState,
    openMediaFile,
    filePathForFile,
    getPlatform,
    platform: detectPlatformSync(),
    getBackendPort
  }
}
