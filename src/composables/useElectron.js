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
 * - 录音走 `recording.*`（Rust 录音 Actor），全局快捷键在 Rust 侧直达 Actor
 * - 待接入项：openMediaFile / filePathForFile 返回降级值
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
// 每个通道的「清空代次」：listen 是异步的，注册尚未完成时被 removeAllListeners 清空，
// 完成后若代次已变就立即注销，否则这个监听会漏网（卸载后仍在回调、重挂载后重复回调）
const channelGenerations = new Map()

async function addChannelListener(channel, callback) {
  if (!isDesktop) return
  const generation = channelGenerations.get(channel) || 0
  const { listen } = await import('@tauri-apps/api/event')
  const unlisten = await listen(channel, (event) => callback(event.payload))
  if ((channelGenerations.get(channel) || 0) !== generation) {
    unlisten()
    return
  }
  if (!channelUnlisteners.has(channel)) {
    channelUnlisteners.set(channel, new Set())
  }
  channelUnlisteners.get(channel).add(unlisten)
}

async function removeAllChannelListeners(channel) {
  channelGenerations.set(channel, (channelGenerations.get(channel) || 0) + 1)
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
    setDefaultPreset: (id) => httpApi.setDefaultPreset(id),
    createPreset: (payload) => httpApi.createPreset(payload),
    updatePreset: (id, payload) => httpApi.updatePreset(id, payload),
    deletePreset: (id) => httpApi.deletePreset(id)
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

  // 剪贴板悬浮框（clipboard-panel 窗口用）：条目由页面写进剪贴板，收起 / 还焦点 / 模拟粘贴在 Rust 侧
  const clipboardPanel = {
    getContext: () => (isDesktop ? invoke('get_clipboard_panel_context') : Promise.resolve({ enter_action: 'paste', hotkey_display: '' })),
    hide: () => (isDesktop ? invoke('hide_clipboard_panel') : Promise.resolve()),
    finish: (paste) => (isDesktop ? invoke('clipboard_panel_finish', { paste: Boolean(paste) }) : Promise.resolve()),
    // 拖动调整面板尺寸（逻辑像素）；save 时记进设置，返回夹取后的实际尺寸
    resize: (width, height, save = false) => (isDesktop ? invoke('clipboard_panel_resize', { width, height, save }) : Promise.resolve(null)),
    resetSize: () => (isDesktop ? invoke('clipboard_panel_reset_size') : Promise.resolve()),
    // 上下拖动：离屏幕底边的距离（逻辑像素）；save 时记进设置，返回夹取后的实际距离
    move: (bottomOffset, save = false) => (isDesktop ? invoke('clipboard_panel_move', { bottomOffset, save }) : Promise.resolve(null)),
    resetPosition: () => (isDesktop ? invoke('clipboard_panel_reset_position') : Promise.resolve()),
    onShow: (callback) => void addChannelListener('clipboard-panel:show', callback)
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

  // 原生录音（Tauri）：采集/识别/落库/粘贴都在 Rust 录音 Actor，前端只发指令与订阅状态
  const recording = {
    start: () => (isDesktop ? invoke('recording_start') : Promise.resolve()),
    stop: () => (isDesktop ? invoke('recording_stop') : Promise.resolve()),
    cancel: () => (isDesktop ? invoke('recording_cancel') : Promise.resolve()),
    getState: () => (isDesktop ? invoke('recording_get_state') : Promise.resolve({ phase: 'idle' })),
    onState: (callback) => void addChannelListener('recording:state', callback),
    onPartial: (callback) => void addChannelListener('recording:partial', callback),
    onLevel: (callback) => void addChannelListener('recording:level', callback),
    // 自动优化增量 { record_id, text }
    onOptimization: (callback) => void addChannelListener('recording:optimization', callback),
    // 已转后台的上一段优化结束 { record, optimized, cancelled, error, message }
    onBackgroundOptimization: (callback) => void addChannelListener('recording:background-optimization', callback)
  }

  // 麦克风列表（Rust cpal 枚举，返回设备名数组）
  async function listAudioDevices() {
    if (!isDesktop) return []
    return invoke('list_audio_devices')
  }

  // 全局快捷键状态：{ enabled, error }
  async function getHotkeyStatus() {
    if (!isDesktop) return { enabled: false, error: null }
    return invoke('get_hotkey_status')
  }

  // 设置页「权限」：{ platform, items: [{ id, label, desc, state }] }（只查询，不触发授权弹窗）
  async function getPermissionStatus() {
    if (!isDesktop) return { platform: 'web', items: [] }
    return invoke('get_permission_status')
  }

  async function openPermissionSettings(id) {
    if (!isDesktop) return
    return invoke('open_permission_settings', { id })
  }

  // 设置页「诊断与关于」：{ version, data_dir, log_file, prompts_dir, backend_port }
  async function getAppInfo() {
    if (!isDesktop) return null
    return invoke('get_app_info')
  }

  // 在文件管理器中显示：kind = 'data' | 'log'
  async function revealAppPath(kind) {
    if (!isDesktop) return
    return invoke('reveal_app_path', { kind })
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

  // 「转文字」页：系统文件选择对话框（Rust 侧 dialog 插件）；取消返回 null
  async function openMediaFile() {
    if (!isDesktop) return null
    return invoke('open_media_file')
  }

  // WebView 拿不到拖入 File 的本地路径：桌面端改由 onFileDrop（原生拖放事件）直接给路径
  function filePathForFile() {
    return ''
  }

  // 原生拖放：callback({ type: 'enter' | 'over' | 'leave' | 'drop', paths })；返回注销函数
  async function onFileDrop(callback) {
    if (!isDesktop) return () => {}
    const { getCurrentWebview } = await import('@tauri-apps/api/webview')
    return getCurrentWebview().onDragDropEvent((event) => {
      callback({ type: event.payload.type, paths: event.payload.paths || [] })
    })
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
    clipboardPanel,
    recording,
    listAudioDevices,
    getHotkeyStatus,
    getPermissionStatus,
    openPermissionSettings,
    getAppInfo,
    revealAppPath,
    onHotkeyPress,
    onHotkeyRelease,
    onPasteText,
    onNavigateSettings,
    onClipboardChanged,
    onFileDrop,
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
