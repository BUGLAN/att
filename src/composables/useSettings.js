import { computed } from 'vue'
import { useSettingsStore } from '@/stores/settings'
import { useAppStore } from '@/stores/app'

export function useSettings() {
  const settingsStore = useSettingsStore()
  const appStore = useAppStore()

  const settings = computed(() => settingsStore.settings)
  const isLoading = computed(() => settingsStore.isLoading)

  async function loadSettings() {
    await settingsStore.loadSettings()
  }

  async function saveSettings(newSettings, options = {}) {
    const { silent = false } = options
    try {
      const settingsToSave = newSettings || settingsStore.settings
      await settingsStore.saveSettings(settingsToSave)
      if (!silent) {
        appStore.showToast('设置已保存')
      }
    } catch (error) {
      appStore.showToast('保存设置失败')
      throw error
    }
  }

  async function updateSetting(key, value) {
    await saveSettings({ [key]: value })
  }

  return {
    settings,
    isLoading,
    loadSettings,
    saveSettings,
    updateSetting
  }
}

export function usePresets() {
  const settingsStore = useSettingsStore()
  const appStore = useAppStore()

  const presets = computed(() => settingsStore.presets)
  const defaultPreset = computed(() => settingsStore.defaultPreset)
  const defaultPresetId = computed(() => settingsStore.defaultPresetId)

  async function loadPresets() {
    await settingsStore.loadPresets()
  }

  async function savePreset(preset) {
    try {
      const result = await settingsStore.savePreset(preset)
      appStore.showToast('预设已保存')
      return result
    } catch (error) {
      appStore.showToast('保存预设失败')
      throw error
    }
  }

  async function deletePreset(id) {
    try {
      await settingsStore.deletePreset(id)
      appStore.showToast('预设已删除')
    } catch (error) {
      appStore.showToast('删除预设失败')
      throw error
    }
  }

  async function previewPreset(id, text) {
    return settingsStore.previewPreset(id, text)
  }

  async function setDefaultPreset(id, options = {}) {
    const { silent = false } = options
    try {
      await settingsStore.setDefaultPreset(id)
      if (!silent) {
        appStore.showToast('已设为默认预设')
      }
    } catch (error) {
      appStore.showToast('设置默认预设失败')
      throw error
    }
  }

  return {
    presets,
    defaultPreset,
    defaultPresetId,
    loadPresets,
    savePreset,
    deletePreset,
    previewPreset,
    setDefaultPreset
  }
}

export function useLLMConfig() {
  const settingsStore = useSettingsStore()
  const appStore = useAppStore()

  const llmConfigs = computed(() => settingsStore.llmConfigs)
  const llmProviders = computed(() => settingsStore.llmProviders)
  const defaultConfig = computed(() => settingsStore.defaultLLMConfig)
  const defaultConfigId = computed(() => settingsStore.defaultLLMConfigId)

  async function loadLLMConfigs() {
    await settingsStore.loadLLMConfigs()
  }

  async function loadLLMProviders() {
    await settingsStore.loadLLMProviders()
  }

  async function saveLLMConfig(config) {
    try {
      const result = await settingsStore.saveLLMConfig(config)
      appStore.showToast('配置已保存')
      return result
    } catch (error) {
      appStore.showToast('保存配置失败')
      throw error
    }
  }

  async function deleteLLMConfig(id) {
    try {
      await settingsStore.deleteLLMConfig(id)
      appStore.showToast('配置已删除')
    } catch (error) {
      appStore.showToast('删除配置失败')
      throw error
    }
  }

  async function testLLMConfig(id) {
    try {
      const result = await settingsStore.testLLMConfig(id)
      appStore.showToast(result.success ? '连接测试成功' : `连接测试失败: ${result.error || '未知错误'}`)
      return result
    } catch (error) {
      appStore.showToast('连接测试失败')
      throw error
    }
  }

  async function setDefaultLLMConfig(id, options = {}) {
    const { silent = false } = options
    try {
      await settingsStore.setDefaultLLMConfig(id)
      if (!silent) {
        appStore.showToast('已设为默认 LLM 配置')
      }
    } catch (error) {
      appStore.showToast('设置默认配置失败')
      throw error
    }
  }

  return {
    llmConfigs,
    llmProviders,
    defaultConfig,
    defaultConfigId,
    loadLLMConfigs,
    loadLLMProviders,
    saveLLMConfig,
    deleteLLMConfig,
    testLLMConfig,
    setDefaultLLMConfig
  }
}
