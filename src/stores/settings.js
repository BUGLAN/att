import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { useElectron } from '@/composables/useElectron'

const STREAMING_SERVICE_POLL_INTERVAL_MS = 1000
const STREAMING_SERVICE_POLL_TIMEOUT_MS = 5 * 60 * 1000

function getPlatform() {
  if (typeof window !== 'undefined' && window.electronAPI?.platform) {
    return window.electronAPI.platform
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
  const { api } = useElectron()
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
    pause_audio_on_recording: true,
    auto_optimize: false,
    auto_copy_to_clipboard: true,
    audio_input_device: 'default',
    hotkey: getDefaultHotkey(),
    clipboard_overlay_trigger: 'mouse_middle',
    clipboard_overlay_retrigger_action: 'toggle',
    clipboard_overlay_enter_behavior: 'copy_and_paste',
    clipboard_overlay_auto_focus_search: true,
    clipboard_overlay_close_on_click_copy: true,
    selection_toolbar_enabled: true,
    selection_toolbar_auto_add_to_wordbook: true,
    selection_toolbar_show_examples: true,
    selection_toolbar_trigger_delay_ms: 220,
    selection_toolbar_fallback_to_clipboard: true,
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

  const llmConfigs = ref([])
  const llmProviders = ref([])
  const currentLLMConfig = ref(null)
  const defaultLLMConfigId = ref(null)

  const defaultPreset = computed(() => {
    return presets.value.find((preset) => preset.id === defaultPresetId.value) || presets.value[0] || null
  })

  const defaultLLMConfig = computed(() => {
    return llmConfigs.value.find((config) => config.id === defaultLLMConfigId.value) ||
      llmConfigs.value.find((config) => config.is_default) ||
      null
  })

  async function loadSettings() {
    isLoading.value = true
    error.value = null

    try {
      const result = await api.getSettings()
      settings.value = {
        ...settings.value,
        ...result,
        hotkey: normalizeHotkey(result.hotkey)
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
      settings.value = {
        ...settings.value,
        ...result,
        hotkey: normalizeHotkey(isValidHotkey(result.hotkey) ? result.hotkey : payload.hotkey)
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

  async function savePreset(preset) {
    const result = await api.savePreset(preset)
    await loadPresets()
    return result
  }

  async function deletePreset(id) {
    await api.deletePreset(id)
    await loadPresets()
  }

  async function previewPreset(id, text) {
    return api.previewPreset(id, text)
  }

  async function setDefaultPreset(id) {
    await api.setDefaultPreset(id)
    defaultPresetId.value = id
    settings.value.default_preset_id = id
    settings.value.last_used_preset_id = id
  }

  async function loadLLMConfigs() {
    try {
      const result = await api.getLLMConfigs()
      llmConfigs.value = result.configs || []
      defaultLLMConfigId.value = result.default_config_id || null
      return llmConfigs.value
    } catch (e) {
      console.error('Failed to load LLM configs:', e)
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

  async function saveLLMConfig(config) {
    const result = await api.saveLLMConfig(config)
    await loadLLMConfigs()
    return result
  }

  async function deleteLLMConfig(id) {
    await api.deleteLLMConfig(id)
    await loadLLMConfigs()
  }

  async function testLLMConfig(id) {
    return api.testLLMConfig(id)
  }

  async function setDefaultLLMConfig(id) {
    await api.setDefaultLLMConfig(id)
    defaultLLMConfigId.value = id
    llmConfigs.value = llmConfigs.value.map((config) => ({
      ...config,
      is_default: config.id === id
    }))
  }

  return {
    settings,
    isLoading,
    error,
    runtimeStatus,
    presets,
    currentPreset,
    defaultPresetId,
    llmConfigs,
    llmProviders,
    currentLLMConfig,
    defaultLLMConfigId,
    defaultPreset,
    defaultLLMConfig,
    loadSettings,
    loadRuntimeStatus,
    startStreamingService,
    stopStreamingService,
    saveSettings,
    loadPresets,
    savePreset,
    deletePreset,
    previewPreset,
    setDefaultPreset,
    loadLLMConfigs,
    loadLLMProviders,
    saveLLMConfig,
    deleteLLMConfig,
    testLLMConfig,
    setDefaultLLMConfig
  }
})
