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
    setDefaultPreset
  }
}

/**
 * LLM v2 设置（供应商档案 + 模型条目）。
 * 状态与动作都在 settingsStore；这里只补 toast 反馈这层交互语义。
 */
export function useLLMService() {
  const settingsStore = useSettingsStore()
  const appStore = useAppStore()

  const profiles = computed(() => settingsStore.llmProfiles)
  const models = computed(() => settingsStore.llmModels)
  const providers = computed(() => settingsStore.llmProviders)
  const defaultModel = computed(() => settingsStore.llmDefaultModel)
  const defaultModelId = computed(() => settingsStore.llmDefaultModelId)
  const skippedEntries = computed(() => settingsStore.llmSkippedEntries)

  async function loadLLMState() {
    return settingsStore.loadLLMState()
  }

  async function loadLLMProviders() {
    return settingsStore.loadLLMProviders()
  }

  async function activateModel(id, options = {}) {
    const { silent = false } = options
    try {
      await settingsStore.activateLLMModel(id)
      if (!silent) {
        appStore.showToast('已设为默认优化模型')
      }
    } catch (error) {
      appStore.showToast('设置默认模型失败')
      throw error
    }
  }

  async function verifyModel(id) {
    try {
      return await settingsStore.verifyLLMModel(id)
    } catch (error) {
      appStore.showToast('验证请求失败')
      throw error
    }
  }

  async function deleteModel(id, options = {}) {
    const { silent = false } = options
    try {
      await settingsStore.deleteLLMModel(id)
      if (!silent) {
        appStore.showToast('模型已删除')
      }
    } catch (error) {
      const detail = error?.message || ''
      appStore.showToast(detail ? `删除失败：${detail}` : '删除失败')
      throw error
    }
  }

  async function deleteProfile(id, options = {}) {
    const { silent = false } = options
    try {
      await settingsStore.deleteLLMProfile(id)
      if (!silent) {
        appStore.showToast('供应商档案已删除')
      }
    } catch (error) {
      const detail = error?.message || ''
      appStore.showToast(detail ? `删除失败：${detail}` : '删除失败')
      throw error
    }
  }

  return {
    profiles,
    models,
    providers,
    defaultModel,
    defaultModelId,
    skippedEntries,
    loadLLMState,
    loadLLMProviders,
    activateModel,
    verifyModel,
    deleteModel,
    deleteProfile,
    store: settingsStore
  }
}
