<template>
  <div class="settings-page" :class="{ 'sidebar-collapsed': sidebarCollapsed }">
    <div class="container">
      <div class="settings-section">
        <!-- 模型设置 -->
        <div class="settings-group">
          <h3 class="settings-group-title">模型设置</h3>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">Whisper 模型</span>
              <span class="setting-desc">语音识别模型大小，越大越准确但越慢</span>
            </div>
            <select class="setting-select" v-model="settings.whisper_model" @change="saveSettings">
              <option value="tiny">tiny (最快)</option>
              <option value="base">base</option>
              <option value="small">small</option>
              <option value="medium">medium</option>
              <option value="large-v3">large-v3 (推荐)</option>
            </select>
          </div>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">计算设备</span>
              <span class="setting-desc">GPU 加速需要 CUDA 或 CoreML</span>
            </div>
            <select class="setting-select" v-model="settings.whisper_device" @change="saveSettings">
              <option value="auto">自动检测</option>
              <option value="cuda">CUDA (NVIDIA GPU)</option>
              <option value="cpu">CPU</option>
            </select>
          </div>
        </div>

        <!-- 录音设置 -->
        <div class="settings-group">
          <h3 class="settings-group-title">录音设置</h3>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">麦克风设备</span>
              <span class="setting-desc">选择录音使用的麦克风</span>
            </div>
            <select class="setting-select" v-model="settings.audio_input_device" @change="saveSettings">
              <option value="default">系统默认</option>
              <option v-for="device in audioInputDevices" :key="device.deviceId" :value="device.deviceId">
                {{ device.label || `麦克风 ${device.deviceId.slice(0, 8)}` }}
              </option>
            </select>
          </div>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">录音时暂停媒体</span>
              <span class="setting-desc">录音时自动暂停系统音频播放</span>
            </div>
            <label class="toggle-switch">
              <input type="checkbox" v-model="settings.pause_audio_on_recording" @change="saveSettings">
              <span class="toggle-slider"></span>
            </label>
          </div>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">自动优化文本</span>
              <span class="setting-desc">转写完成后自动调用 AI 优化</span>
            </div>
            <label class="toggle-switch">
              <input type="checkbox" v-model="settings.auto_optimize" @change="saveSettings">
              <span class="toggle-slider"></span>
            </label>
          </div>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">自动复制到剪贴板</span>
              <span class="setting-desc">转写完成后自动复制文本</span>
            </div>
            <label class="toggle-switch">
              <input type="checkbox" v-model="settings.auto_copy_to_clipboard" @change="saveSettings">
              <span class="toggle-slider"></span>
            </label>
          </div>
        </div>

        <!-- 快捷键设置 -->
        <div class="settings-group">
          <h3 class="settings-group-title">快捷键</h3>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">录音快捷键</span>
              <span class="setting-desc">按下开始录音，释放停止录音</span>
            </div>
            <div class="hotkey-input-wrapper">
              <input
                type="text"
                class="hotkey-input"
                :value="settings.hotkey?.display || '点击设置'"
                readonly
                placeholder="点击设置"
              >
              <button class="hotkey-reset-btn" @click="resetHotkey">↺</button>
            </div>
          </div>
        </div>

        <!-- AI 配置 -->
        <div class="settings-group">
          <h3 class="settings-group-title">AI 配置</h3>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">LLM 服务配置</span>
              <span class="setting-desc">配置 AI 文本优化的服务提供商</span>
            </div>
            <button class="btn" @click="openLLMModal">
              管理 ({{ llmConfigCount }})
            </button>
          </div>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">优化预设</span>
              <span class="setting-desc">管理文本优化的提示词预设</span>
            </div>
            <button class="btn" @click="openPresetModal">
              管理 ({{ presetCount }})
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import { useSettings, usePresets, useLLMConfig } from '@/composables/useSettings'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { settings, saveSettings } = useSettings()
const { presets, loadPresets } = usePresets()
const { llmConfigs, loadLLMConfigs } = useLLMConfig()

const sidebarCollapsed = computed(() => appStore.sidebarCollapsed)
const presetCount = computed(() => presets.value.length)
const llmConfigCount = computed(() => llmConfigs.value.length)

// 麦克风设备列表
const audioInputDevices = ref([])

// 获取麦克风设备列表
async function loadAudioInputDevices() {
  try {
    // 需要先请求权限才能获取设备标签
    await navigator.mediaDevices.getUserMedia({ audio: true })
    const devices = await navigator.mediaDevices.enumerateDevices()
    audioInputDevices.value = devices.filter(device => device.kind === 'audioinput')
    console.log('[Settings] Audio input devices:', audioInputDevices.value)
  } catch (error) {
    console.error('[Settings] Failed to get audio devices:', error)
  }
}

function openLLMModal() {
  appStore.openModal('llm')
}

function openPresetModal() {
  appStore.openModal('preset')
}

function resetHotkey() {
  const platform = window.electronAPI?.platform || 'win32'
  const defaultHotkey = platform === 'darwin'
    ? { modifiers: ['alt', 'cmd'], key: null, display: '⌥⌘' }
    : { modifiers: ['alt', 'ctrl'], key: null, display: 'Alt+Ctrl' }
  settings.value.hotkey = defaultHotkey
  saveSettings()
}

onMounted(() => {
  loadPresets()
  loadLLMConfigs()
  loadAudioInputDevices()
})
</script>
