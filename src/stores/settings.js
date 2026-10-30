import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { useElectron } from '@/composables/useElectron'

const STREAMING_SERVICE_POLL_INTERVAL_MS = 1000
const STREAMING_SERVICE_POLL_TIMEOUT_MS = 5 * 60 * 1000

function getPlatform() {
  // Tauri 下 invoke 是异步的，而默认热键需要同步值：UA 推断足够（精确值走 useElectron.getPlatform()）
  if (typeof navigator === 'undefined') {
    return 'win32'
  }
  const ua = navigator.userAgent
  if (ua.includes('Win')) {
    return 'win32'
  }
  if (ua.includes('Mac')) {
    return 'darwin'
  }
  if (ua.includes('Linux')) {
    return 'linux'
  }
  return 'win32'
}

function getDefaultHotkey() {
  const platform = getPlatform()

  if (platform === 'darwin') {
    return {
      modifiers: ['cmd', 'shift'],
      key: 'r',
      display: '⌘⇧R'
    }
  }

  if (platform === 'win32') {
    return {
      modifiers: ['win', 'alt'],
      key: null,
      display: 'Win+Alt'
    }
  }

  return {
    modifiers: ['ctrl', 'shift'],
    key: 'r',
    display: 'Ctrl+Shift+R'
  }
}

function sortHotkeyModifiers(modifiers = []) {
  const priority = {
    cmd: 0,
    win: 0,
    ctrl: 1,
    alt: 2,
    shift: 3
  }

  return [...modifiers].sort((left, right) => {
    const leftPriority = priority[left] ?? Number.MAX_SAFE_INTEGER
    const rightPriority = priority[right] ?? Number.MAX_SAFE_INTEGER
    if (leftPriority !== rightPriority) {
      return leftPriority - rightPriority
    }
    return left.localeCompare(right)
  })
}

function buildHotkeyDisplay(modifiers, key) {
  const platform = getPlatform()
  const map = {
    alt: platform === 'darwin' ? '⌥' : 'Alt',
    ctrl: platform === 'darwin' ? '⌃' : 'Ctrl',
    cmd: platform === 'darwin' ? '⌘' : 'Cmd',
    shift: platform === 'darwin' ? '⇧' : 'Shift',
    win: 'Win'
  }

  const modifierText = sortHotkeyModifiers(modifiers).map((modifier) => map[modifier] || modifier)
  return [...modifierText, key ? String(key).toUpperCase() : null].filter(Boolean).join(platform === 'darwin' ? '' : '+')
}

function isValidHotkey(hotkey) {
  return Array.isArray(hotkey?.modifiers) &&
    hotkey.modifiers.length > 0 &&
    (
      (hotkey.key == null && hotkey.modifiers.length >= 2) ||
      (typeof hotkey?.key === 'string' && hotkey.key.length === 1)
    )
}

function normalizeHotkey(hotkey) {
  if (!isValidHotkey(hotkey)) {
    return getDefaultHotkey()
  }

  const modifiers = sortHotkeyModifiers(
    hotkey.modifiers
      .map((modifier) => String(modifier || '').toLowerCase())
      .filter((modifier, index, array) => modifier && array.indexOf(modifier) === index)
  )
  const key = typeof hotkey.key === 'string' && hotkey.key.length === 1
    ? hotkey.key.toLowerCase()
    : null

  return {
    modifiers,
    key,
    display: buildHotkeyDisplay(modifiers, key)
  }
}

export const useSettingsStore = defineStore('settings', () => {
  const { api, getDesktopSettings, saveDesktopSettings } = useElectron()
  let streamingServicePollPromise = null

  const settings = ref({
    asr_provider: 'whisper',
    asr_language: 'zh',
    whisper_model: 'large-v3',
    whisper_device: 'auto',
    qwen_asr_model: 'Qwen/Qwen3-ASR-1.7B',
    qwen_asr_device: 'auto',
    qwen_asr_dtype: 'auto',
    qwen_asr_max_new_tokens: 256,
    qwen_asr_return_timestamps: false,
    qwen_asr_use_forced_aligner: false,
    qwen_asr_forced_aligner_model: 'Qwen/Qwen3-ForcedAligner-0.6B',
    qwen_asr_max_audio_seconds: 30,
    // 火山引擎豆包双向流式 ASR（在线识别）
    // 注意：明文 Access Token 不进前端默认值/本地状态，只以「是否已配置」的形式展示；
    // 用户在设置页输入后由后端加密落盘（doubao_asr_api_key_encrypted）。
    doubao_asr_api_key_set: false,
    doubao_asr_resource_id: 'volc.seedasr.sauc.duration',
    doubao_asr_endpoint: 'wss://openspeech.bytedance.com/api/v3/sauc/bigmodel',
    doubao_asr_end_window_size: 0,
    // ChatGPT 听写 ASR（在线批量）：令牌同样只以「是否已配置」展示，
    // 加密落盘为 chatgpt_asr_api_key_encrypted
    chatgpt_asr_api_key_set: false,
    chatgpt_asr_endpoint: 'https://chatgpt.com/backend-api/transcribe',
    chatgpt_asr_timeout_seconds: 120,
    pause_audio_on_recording: true,
    auto_optimize: false,
    auto_copy_to_clipboard: true,
    audio_input_device: 'default',
    hotkey: getDefaultHotkey(),
    // 统一浮层已下线：clipboard_overlay_* 键一并移除，
    // 后端把这些键登记进 RETIRED_SETTINGS_KEYS，旧 settings.json 里的残留值会被丢弃
    recording_mode: 'single',
    silence_threshold: 1.5,
    silence_level: -40,
    default_preset_id: null,
    last_used_preset_id: null
  })

  const isLoading = ref(false)
  const error = ref(null)
  const runtimeStatus = ref({
    status: 'unknown',
    runtime_environment: 'unknown',
    model_service_managed: true,
    model_service_running: false,
    model_service_loading: false,
    model_service_ready: false,
    model_service_error: null,
    model_service_supported: true,
    model_service_mode: 'provider',
    streaming_available: false,
    streaming_supported: false,
    streaming_backend: null,
    streaming_error: null,
    qwen_streaming_available: false,
    qwen_streaming_backend: null,
    qwen_streaming_error: null,
    qwen_streaming_service_managed: true,
    qwen_streaming_service_running: false,
    qwen_streaming_service_loading: false,
    qwen_streaming_service_ready: false,
    qwen_streaming_service_pid: null,
    qwen_streaming_service_url: null,
    qwen_streaming_service_error: null
  })

  const presets = ref([])
  const currentPreset = ref(null)
  const defaultPresetId = ref(null)

  // ===== LLM v2 状态（供应商档案 + 模型条目）=====
  const llmProfiles = ref([])
  const llmModels = ref([])
  const llmProviders = ref([])
  const llmDefaultModelId = ref(null)
  const llmFallbackModelId = ref(null)
  const llmSkippedEntries = ref([])
  // 向导/编辑弹窗的当前编辑目标（与旧 currentLLMConfig 同一套模式）
  const currentLLMProfile = ref(null)
  const currentLLMModel = ref(null)

  const defaultPreset = computed(() => {
    return presets.value.find((preset) => preset.id === defaultPresetId.value) || presets.value[0] || null
  })

  const llmDefaultModel = computed(() => {
    return llmModels.value.find((model) => model.id === llmDefaultModelId.value) || llmModels.value[0] || null
  })

  async function loadSettings() {
    isLoading.value = true
    error.value = null

    try {
      const result = await api.getSettings()
      let desktopSettings = null
      try {
        desktopSettings = await getDesktopSettings?.()
      } catch (desktopError) {
        console.warn('[Settings] Failed to load desktop settings:', desktopError)
      }
      settings.value = {
        ...settings.value,
        ...result,
        hotkey: normalizeHotkey(desktopSettings?.hotkey || result.hotkey),
        pause_audio_on_recording: desktopSettings?.pause_audio_on_recording ?? result.pause_audio_on_recording ?? settings.value.pause_audio_on_recording
      }
      defaultPresetId.value = settings.value.default_preset_id || defaultPresetId.value || null
      return settings.value
    } catch (e) {
      error.value = e.message
      console.error('Failed to load settings:', e)
      throw e
    } finally {
      isLoading.value = false
    }
  }

  async function saveSettings(newSettings) {
    try {
      const payload = {
        ...settings.value,
        ...newSettings
      }
      const result = await api.saveSettings(payload)
      let desktopSettings = null
      try {
        desktopSettings = await saveDesktopSettings?.({
          hotkey: payload.hotkey,
          pause_audio_on_recording: payload.pause_audio_on_recording
        })
      } catch (desktopError) {
        console.warn('[Settings] Failed to save desktop settings:', desktopError)
      }
      settings.value = {
        ...settings.value,
        ...result,
        hotkey: normalizeHotkey(desktopSettings?.hotkey || (isValidHotkey(result.hotkey) ? result.hotkey : payload.hotkey)),
        pause_audio_on_recording: desktopSettings?.pause_audio_on_recording ?? result.pause_audio_on_recording ?? payload.pause_audio_on_recording
      }
      defaultPresetId.value = settings.value.default_preset_id || defaultPresetId.value || null
      await loadRuntimeStatus(true)
      return result
    } catch (e) {
      error.value = e.message
      console.error('Failed to save settings:', e)
      throw e
    }
  }

  async function loadRuntimeStatus(probeRuntime = true) {
    try {
      const result = await api.getHealth(probeRuntime)
      runtimeStatus.value = {
        ...runtimeStatus.value,
        ...result
      }
      return runtimeStatus.value
    } catch (e) {
      error.value = e.message
      console.error('Failed to load runtime status:', e)
      throw e
    }
  }

  function applyStreamingServiceStatus(result) {
    const health = result?.health || {}
    runtimeStatus.value = {
      ...runtimeStatus.value,
      model_service_managed: result?.managed ?? runtimeStatus.value.model_service_managed,
      model_service_running: result?.running ?? runtimeStatus.value.model_service_running,
      model_service_loading: result?.loading ?? runtimeStatus.value.model_service_loading,
      model_service_ready: result?.ready ?? runtimeStatus.value.model_service_ready,
      model_service_error: result?.error ?? runtimeStatus.value.model_service_error,
      model_service_supported: result?.model_service_supported ?? runtimeStatus.value.model_service_supported,
      model_service_mode: result?.model_service_mode ?? runtimeStatus.value.model_service_mode,
      streaming_available: result?.streaming_available ?? health.streaming_available ?? runtimeStatus.value.streaming_available,
      streaming_supported: result?.streaming_supported ?? runtimeStatus.value.streaming_supported,
      streaming_backend: result?.streaming_backend ?? health.streaming_backend ?? runtimeStatus.value.streaming_backend,
      streaming_error: result?.streaming_error ?? health.error ?? runtimeStatus.value.streaming_error,
      qwen_streaming_service_managed: result?.managed ?? runtimeStatus.value.qwen_streaming_service_managed,
      qwen_streaming_service_running: result?.running ?? runtimeStatus.value.qwen_streaming_service_running,
      qwen_streaming_service_loading: result?.loading ?? runtimeStatus.value.qwen_streaming_service_loading,
      qwen_streaming_service_ready: result?.ready ?? runtimeStatus.value.qwen_streaming_service_ready,
      qwen_streaming_service_pid: result?.pid ?? runtimeStatus.value.qwen_streaming_service_pid,
      qwen_streaming_service_url: result?.base_url ?? runtimeStatus.value.qwen_streaming_service_url,
      qwen_streaming_service_error: result?.error ?? runtimeStatus.value.qwen_streaming_service_error,
      qwen_streaming_available: result?.streaming_available ?? health.loaded ?? runtimeStatus.value.qwen_streaming_available,
      qwen_streaming_backend: health.backend ?? runtimeStatus.value.qwen_streaming_backend,
      qwen_streaming_error: result?.streaming_error ?? health.error ?? runtimeStatus.value.qwen_streaming_error
    }
  }

  async function pollStreamingServiceUntilSettled() {
    if (streamingServicePollPromise) {
      return streamingServicePollPromise
    }

    streamingServicePollPromise = (async () => {
      const startedAt = Date.now()

      try {
        while (Date.now() - startedAt < STREAMING_SERVICE_POLL_TIMEOUT_MS) {
          const result = await api.getStreamingServiceStatus()
          applyStreamingServiceStatus(result)

          if (result.ready) {
            await loadRuntimeStatus(true)
            runtimeStatus.value = {
              ...runtimeStatus.value,
              model_service_loading: false,
              model_service_error: null,
              qwen_streaming_service_loading: false,
              qwen_streaming_service_error: null
            }
            return runtimeStatus.value
          }

          if (result.error && !result.loading) {
            runtimeStatus.value = {
              ...runtimeStatus.value,
              model_service_loading: false,
              model_service_error: result.error,
              qwen_streaming_service_loading: false,
              qwen_streaming_service_error: result.error
            }
            throw new Error(result.error)
          }

          await new Promise((resolve) => {
            window.setTimeout(resolve, STREAMING_SERVICE_POLL_INTERVAL_MS)
          })
        }

        runtimeStatus.value = {
          ...runtimeStatus.value,
          model_service_loading: false,
          model_service_error: '等待模型服务加载超时',
          qwen_streaming_service_loading: false,
          qwen_streaming_service_error: '等待模型服务加载超时'
        }
        throw new Error('等待模型服务加载超时')
      } finally {
        streamingServicePollPromise = null
      }
    })()

    return streamingServicePollPromise
  }

  async function startStreamingService(waitUntilReady = true) {
    try {
      runtimeStatus.value = {
        ...runtimeStatus.value,
        model_service_loading: true,
        model_service_error: null,
        qwen_streaming_service_loading: true,
        qwen_streaming_service_error: null
      }
      const result = await api.startStreamingService(waitUntilReady)
      applyStreamingServiceStatus(result)

      if (waitUntilReady) {
        await loadRuntimeStatus(true)
        runtimeStatus.value = {
          ...runtimeStatus.value,
          model_service_loading: false,
          model_service_error: result.error ?? null,
          qwen_streaming_service_loading: false,
          qwen_streaming_service_error: result.error ?? null
        }
      } else if (result.ready) {
        await loadRuntimeStatus(true)
        runtimeStatus.value = {
          ...runtimeStatus.value,
          model_service_loading: false,
          model_service_error: null,
          qwen_streaming_service_loading: false,
          qwen_streaming_service_error: null
        }
      } else {
        void pollStreamingServiceUntilSettled().catch((pollError) => {
          console.error('Failed to wait for streaming service readiness:', pollError)
        })
      }

      return runtimeStatus.value
    } catch (e) {
      runtimeStatus.value = {
        ...runtimeStatus.value,
        model_service_loading: false,
        model_service_error: e.message,
        qwen_streaming_service_loading: false,
        qwen_streaming_service_error: e.message
      }
      error.value = e.message
      console.error('Failed to start streaming service:', e)
      throw e
    }
  }

  async function stopStreamingService() {
    try {
      const result = await api.stopStreamingService()
      await loadRuntimeStatus(false)
      runtimeStatus.value = {
        ...runtimeStatus.value,
        model_service_running: result.running ?? false,
        model_service_loading: result.loading ?? false,
        model_service_ready: result.ready ?? false,
        model_service_error: result.error ?? null,
        model_service_supported: result.model_service_supported ?? runtimeStatus.value.model_service_supported,
        model_service_mode: result.model_service_mode ?? runtimeStatus.value.model_service_mode,
        streaming_available: result.streaming_available ?? false,
        streaming_supported: result.streaming_supported ?? runtimeStatus.value.streaming_supported,
        streaming_backend: result.streaming_backend ?? runtimeStatus.value.streaming_backend,
        streaming_error: result.streaming_error ?? null,
        qwen_streaming_service_running: result.running ?? false,
        qwen_streaming_service_loading: result.loading ?? false,
        qwen_streaming_service_ready: result.ready ?? false,
        qwen_streaming_service_pid: result.pid ?? null,
        qwen_streaming_service_error: result.error ?? null
      }
      return runtimeStatus.value
    } catch (e) {
      error.value = e.message
      console.error('Failed to stop streaming service:', e)
      throw e
    }
  }

  async function loadPresets() {
    try {
      const result = await api.getPresets()
      presets.value = result.presets || []
      const presetIds = new Set(presets.value.map((preset) => preset.id))
      const resolvedDefaultPresetId = presetIds.has(result.default_preset_id)
        ? result.default_preset_id
        : presets.value[0]?.id || null

      defaultPresetId.value = resolvedDefaultPresetId
      settings.value.default_preset_id = defaultPresetId.value
      settings.value.last_used_preset_id = defaultPresetId.value
      return presets.value
    } catch (e) {
      console.error('Failed to load presets:', e)
      throw e
    }
  }

  // 预设是只读的（唯一真相源是 prompts/*.txt）：这里只保留加载与设默认；
  // 历史上的 save/delete/preview 封装对应后端已删除的写接口，一并移除
  async function setDefaultPreset(id) {
    await api.setDefaultPreset(id)
    defaultPresetId.value = id
    settings.value.default_preset_id = id
    settings.value.last_used_preset_id = id
  }

  // 档案 + 模型 + 默认绑定一次拉全（设置页首屏）
  async function loadLLMState() {
    try {
      const result = await api.getLLMState()
      llmProfiles.value = result.profiles || []
      llmModels.value = result.models || []
      llmDefaultModelId.value = result.default_model_id || null
      llmFallbackModelId.value = result.fallback_model_id || null
      llmSkippedEntries.value = result.skipped_entries || []
      return result
    } catch (e) {
      console.error('Failed to load LLM state:', e)
      throw e
    }
  }

  async function loadLLMProviders() {
    try {
      const result = await api.getLLMProviders()
      llmProviders.value = result.providers || []
      return llmProviders.value
    } catch (e) {
      console.error('Failed to load LLM providers:', e)
      throw e
    }
  }

  // 以下动作完成后统一重拉 state：后端会在验证时回写状态字段，本地不自己拼
  async function refreshLLMState() {
    return loadLLMState()
  }

  async function createLLMProfile(payload) {
    const result = await api.createLLMProfile(payload)
    await loadLLMState()
    return result
  }

  async function updateLLMProfile(id, payload) {
    const result = await api.updateLLMProfile(id, payload)
    await loadLLMState()
    return result
  }

  async function deleteLLMProfile(id) {
    try {
      return await api.deleteLLMProfile(id)
    } finally {
      await loadLLMState()
    }
  }

  async function verifyLLMProfile(id) {
    return api.verifyLLMProfile(id)
  }

  async function verifyLLMDraft(payload) {
    return api.verifyLLMDraft(payload)
  }

  async function getLLMRemoteModels(id) {
    return api.getLLMRemoteModels(id)
  }

  async function createLLMModels(items) {
    const result = await api.createLLMModels(items)
    await loadLLMState()
    return result
  }

  async function updateLLMModel(id, payload) {
    const result = await api.updateLLMModel(id, payload)
    await loadLLMState()
    return result
  }

  async function deleteLLMModel(id) {
    try {
      return await api.deleteLLMModel(id)
    } finally {
      await loadLLMState()
    }
  }

  async function verifyLLMModel(id) {
    return api.verifyLLMModel(id)
  }

  async function activateLLMModel(id) {
    await api.activateLLMModel(id)
    llmDefaultModelId.value = id
  }

  async function updateLLMSettings(payload) {
    await api.updateLLMSettings(payload)
    llmDefaultModelId.value = payload.default_model_id ?? llmDefaultModelId.value
    llmFallbackModelId.value = payload.fallback_model_id ?? null
  }

  return {
    settings,
    isLoading,
    error,
    runtimeStatus,
    presets,
    currentPreset,
    defaultPresetId,
    llmProfiles,
    llmModels,
    llmProviders,
    llmDefaultModelId,
    llmFallbackModelId,
    llmSkippedEntries,
    llmDefaultModel,
    currentLLMProfile,
    currentLLMModel,
    defaultPreset,
    loadSettings,
    loadRuntimeStatus,
    startStreamingService,
    stopStreamingService,
    saveSettings,
    loadPresets,
    setDefaultPreset,
    loadLLMState,
    loadLLMProviders,
    refreshLLMState,
    createLLMProfile,
    updateLLMProfile,
    deleteLLMProfile,
    verifyLLMProfile,
    verifyLLMDraft,
    getLLMRemoteModels,
    createLLMModels,
    updateLLMModel,
    deleteLLMModel,
    verifyLLMModel,
    activateLLMModel,
    updateLLMSettings
  }
})
