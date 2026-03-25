/**
 * 设置状态管理
 * 管理用户设置、预设、LLM 配置
 */

import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { useElectron } from '@/composables/useElectron'

export const useSettingsStore = defineStore('settings', () => {
  const { api } = useElectron()

  // 获取平台信息（从 Electron preload 暴露的 API）
  const getPlatform = () => {
    if (typeof window !== 'undefined' && window.electronAPI?.platform) {
      return window.electronAPI.platform
    }
    return 'win32'  // 默认 Windows
  }

  // === 设置状态 ===
  const settings = ref({
    whisper_model: 'large-v3',
    whisper_device: 'auto',
    pause_audio_on_recording: true,
    auto_optimize: false,
    auto_copy_to_clipboard: true,
    audio_input_device: 'default',  // 麦克风设备 ID
    hotkey: {
      modifiers: getPlatform() === 'darwin' ? ['alt', 'cmd'] : ['alt', 'ctrl'],
      key: null,
      display: getPlatform() === 'darwin' ? '⌥⌘' : 'Alt+Ctrl'
    },
    recording_mode: 'single',
    silence_threshold: 1.5,
    silence_level: -40,
    default_preset_id: 'builtin-general',
    last_used_preset_id: null
  })

  const isLoading = ref(false)
  const error = ref(null)

  // === 预设状态 ===
  const presets = ref([])
  const currentPreset = ref(null)

  // === LLM 配置状态 ===
  const llmConfigs = ref([])
  const llmProviders = ref([])
  const currentLLMConfig = ref(null)

  // === 计算属性 ===
  const defaultPreset = computed(() => {
    return presets.value.find(p => p.id === settings.value.default_preset_id)
  })

  const defaultLLMConfig = computed(() => {
    return llmConfigs.value.find(c => c.is_default)
  })

  // === 设置操作 ===
  async function loadSettings() {
    isLoading.value = true
    error.value = null

    try {
      const result = await api.getSettings()
      settings.value = { ...settings.value, ...result }
    } catch (e) {
      error.value = e.message
      console.error('Failed to load settings:', e)
    } finally {
      isLoading.value = false
    }
  }

  async function saveSettings(newSettings) {
    try {
      const result = await api.saveSettings(newSettings)
      settings.value = { ...settings.value, ...result }
      return result
    } catch (e) {
      error.value = e.message
      console.error('Failed to save settings:', e)
      throw e
    }
  }

  // === 预设操作 ===
  async function loadPresets() {
    try {
      const result = await api.getPresets()
      presets.value = Object.values(result)
    } catch (e) {
      console.error('Failed to load presets:', e)
    }
  }

  async function savePreset(preset) {
    try {
      const result = await api.savePreset(preset)
      const index = presets.value.findIndex(p => p.id === result.id)
      if (index !== -1) {
        presets.value[index] = result
      } else {
        presets.value.push(result)
      }
      return result
    } catch (e) {
      console.error('Failed to save preset:', e)
      throw e
    }
  }

  async function deletePreset(id) {
    try {
      await api.deletePreset(id)
      presets.value = presets.value.filter(p => p.id !== id)
    } catch (e) {
      console.error('Failed to delete preset:', e)
      throw e
    }
  }

  async function setDefaultPreset(id) {
    try {
      await api.setDefaultPreset(id)
      settings.value.default_preset_id = id
    } catch (e) {
      console.error('Failed to set default preset:', e)
      throw e
    }
  }

  // === LLM 配置操作 ===
  async function loadLLMConfigs() {
    try {
      const result = await api.getLLMConfigs()
      llmConfigs.value = Object.values(result)
    } catch (e) {
      console.error('Failed to load LLM configs:', e)
    }
  }

  async function loadLLMProviders() {
    try {
      const result = await api.getLLMProviders()
      llmProviders.value = result
    } catch (e) {
      console.error('Failed to load LLM providers:', e)
    }
  }

  async function saveLLMConfig(config) {
    try {
      const result = await api.saveLLMConfig(config)
      const index = llmConfigs.value.findIndex(c => c.id === result.id)
      if (index !== -1) {
        llmConfigs.value[index] = result
      } else {
        llmConfigs.value.push(result)
      }
      return result
    } catch (e) {
      console.error('Failed to save LLM config:', e)
      throw e
    }
  }

  async function deleteLLMConfig(id) {
    try {
      await api.deleteLLMConfig(id)
      llmConfigs.value = llmConfigs.value.filter(c => c.id !== id)
    } catch (e) {
      console.error('Failed to delete LLM config:', e)
      throw e
    }
  }

  async function testLLMConfig(id) {
    try {
      const result = await api.testLLMConfig(id)
      return result
    } catch (e) {
      console.error('Failed to test LLM config:', e)
      throw e
    }
  }

  async function setDefaultLLMConfig(id) {
    try {
      await api.setDefaultLLMConfig(id)
      // 更新本地状态
      llmConfigs.value = llmConfigs.value.map(c => ({
        ...c,
        is_default: c.id === id
      }))
    } catch (e) {
      console.error('Failed to set default LLM config:', e)
      throw e
    }
  }

  return {
    // 状态
    settings,
    isLoading,
    error,
    presets,
    currentPreset,
    llmConfigs,
    llmProviders,
    currentLLMConfig,

    // 计算属性
    defaultPreset,
    defaultLLMConfig,

    // 操作
    loadSettings,
    saveSettings,
    loadPresets,
    savePreset,
    deletePreset,
    setDefaultPreset,
    loadLLMConfigs,
    loadLLMProviders,
    saveLLMConfig,
    deleteLLMConfig,
    testLLMConfig,
    setDefaultLLMConfig
  }
})
