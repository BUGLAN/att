/**
 * 设置管理 Composable
 * 处理设置页面操作
 */

import { computed, onMounted } from 'vue'
import { useSettingsStore } from '@/stores/settings'
import { useAppStore } from '@/stores/app'
import { useElectron } from './useElectron'

export function useSettings() {
  const settingsStore = useSettingsStore()
  const appStore = useAppStore()
  const { api } = useElectron()

  const settings = computed(() => settingsStore.settings)
  const isLoading = computed(() => settingsStore.isLoading)

  /**
   * 加载设置
   */
  async function loadSettings() {
    await settingsStore.loadSettings()
  }

  /**
   * 保存设置
   */
  async function saveSettings(newSettings) {
    try {
      await settingsStore.saveSettings(newSettings)
      appStore.showToast('设置已保存')
    } catch (error) {
      appStore.showToast('保存失败')
    }
  }

  /**
   * 更新单个设置项
   */
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

/**
 * 预设管理 Composable
 */
export function usePresets() {
  const settingsStore = useSettingsStore()
  const appStore = useAppStore()

  const presets = computed(() => settingsStore.presets)
  const defaultPreset = computed(() => settingsStore.defaultPreset)

  /**
   * 加载预设列表
   */
  async function loadPresets() {
    await settingsStore.loadPresets()
  }

  /**
   * 保存预设
   */
  async function savePreset(preset) {
    try {
      await settingsStore.savePreset(preset)
      appStore.showToast('预设已保存')
    } catch (error) {
      appStore.showToast('保存失败')
    }
  }

  /**
   * 删除预设
   */
  async function deletePreset(id) {
    try {
      await settingsStore.deletePreset(id)
      appStore.showToast('预设已删除')
    } catch (error) {
      appStore.showToast('删除失败')
    }
  }

  /**
   * 设置默认预设
   */
  async function setDefaultPreset(id) {
    try {
      await settingsStore.setDefaultPreset(id)
      appStore.showToast('已设为默认')
    } catch (error) {
      appStore.showToast('设置失败')
    }
  }

  return {
    presets,
    defaultPreset,
    loadPresets,
    savePreset,
    deletePreset,
    setDefaultPreset
  }
}

/**
 * LLM 配置管理 Composable
 */
export function useLLMConfig() {
  const settingsStore = useSettingsStore()
  const appStore = useAppStore()

  const llmConfigs = computed(() => settingsStore.llmConfigs)
  const llmProviders = computed(() => settingsStore.llmProviders)
  const defaultConfig = computed(() => settingsStore.defaultLLMConfig)

  /**
   * 加载 LLM 配置
   */
  async function loadLLMConfigs() {
    await settingsStore.loadLLMConfigs()
  }

  /**
   * 加载提供商列表
   */
  async function loadLLMProviders() {
    await settingsStore.loadLLMProviders()
  }

  /**
   * 保存 LLM 配置
   */
  async function saveLLMConfig(config) {
    try {
      await settingsStore.saveLLMConfig(config)
      appStore.showToast('配置已保存')
    } catch (error) {
      appStore.showToast('保存失败')
    }
  }

  /**
   * 删除 LLM 配置
   */
  async function deleteLLMConfig(id) {
    try {
      await settingsStore.deleteLLMConfig(id)
      appStore.showToast('配置已删除')
    } catch (error) {
      appStore.showToast('删除失败')
    }
  }

  /**
   * 测试 LLM 配置
   */
  async function testLLMConfig(id) {
    try {
      const result = await settingsStore.testLLMConfig(id)
      if (result.success) {
        appStore.showToast('连接成功')
      } else {
        appStore.showToast('连接失败: ' + result.error)
      }
      return result
    } catch (error) {
      appStore.showToast('测试失败')
      return { success: false, error: error.message }
    }
  }

  /**
   * 设置默认配置
   */
  async function setDefaultLLMConfig(id) {
    try {
      await settingsStore.setDefaultLLMConfig(id)
      appStore.showToast('已设为默认')
    } catch (error) {
      appStore.showToast('设置失败')
    }
  }

  return {
    llmConfigs,
    llmProviders,
    defaultConfig,
    loadLLMConfigs,
    loadLLMProviders,
    saveLLMConfig,
    deleteLLMConfig,
    testLLMConfig,
    setDefaultLLMConfig
  }
}
