import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { useElectron } from '@/composables/useElectron'

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

function isValidHotkey(hotkey) {
  return Array.isArray(hotkey?.modifiers) &&
    hotkey.modifiers.length > 0 &&
    (
      (hotkey.key == null && hotkey.modifiers.length >= 2) ||
      (typeof hotkey?.key === 'string' && hotkey.key.length === 1)
    )
}

export const useSettingsStore = defineStore('settings', () => {
  const { api } = useElectron()

  const settings = ref({
    whisper_model: 'large-v3',
    whisper_device: 'auto',
    pause_audio_on_recording: true,
    auto_optimize: false,
    auto_copy_to_clipboard: true,
    audio_input_device: 'default',
    hotkey: getDefaultHotkey(),
    clipboard_overlay_trigger: 'mouse_middle',
    clipboard_overlay_retrigger_action: 'toggle',
    clipboard_overlay_enter_behavior: 'copy_and_paste',
    clipboard_overlay_auto_focus_search: true,
    clipboard_overlay_close_on_click_copy: false,
    recording_mode: 'single',
    silence_threshold: 1.5,
    silence_level: -40,
    default_preset_id: null,
    last_used_preset_id: null
  })

  const isLoading = ref(false)
  const error = ref(null)

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
        hotkey: isValidHotkey(result.hotkey) ? result.hotkey : getDefaultHotkey()
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
        hotkey: isValidHotkey(result.hotkey) ? result.hotkey : (isValidHotkey(payload.hotkey) ? payload.hotkey : getDefaultHotkey())
      }
      defaultPresetId.value = settings.value.default_preset_id || defaultPresetId.value || null
      return result
    } catch (e) {
      error.value = e.message
      console.error('Failed to save settings:', e)
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
