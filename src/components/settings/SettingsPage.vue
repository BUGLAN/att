<template>
  <div class="settings-page">
    <header class="header settings-header">
      <h1>设置</h1>
      <p class="subtitle">配置你的录音、提示词和 LLM 服务</p>
    </header>

    <section class="settings-section">
        <div class="settings-group">
          <h3 class="settings-group-title">语音识别</h3>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">Whisper 模型</span>
              <span class="setting-desc">模型越大识别越准，但速度也会更慢。</span>
            </div>
            <select v-model="form.whisper_model" class="setting-select">
              <option value="tiny">tiny</option>
              <option value="base">base</option>
              <option value="small">small</option>
              <option value="medium">medium</option>
              <option value="large-v3">large-v3</option>
            </select>
          </div>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">计算设备</span>
              <span class="setting-desc">选择用于语音识别的推理设备。</span>
            </div>
            <select v-model="form.whisper_device" class="setting-select">
              <option value="auto">自动</option>
              <option value="cuda">CUDA</option>
              <option value="cpu">CPU</option>
            </select>
          </div>
        </div>

        <div class="settings-group">
          <h3 class="settings-group-title">录音设置</h3>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">录音模式</span>
              <span class="setting-desc">保留旧网页的单段 / 多段配置入口。</span>
            </div>
            <select v-model="form.recording_mode" class="setting-select">
              <option value="single">单段录音</option>
              <option value="multi">多段录音</option>
            </select>
          </div>

          <div v-if="form.recording_mode === 'multi'" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">静音分段阈值</span>
              <span class="setting-desc">静音持续超过该秒数时自动切段。</span>
            </div>
            <input v-model.number="form.silence_threshold" class="setting-input" type="number" min="0.5" max="5" step="0.5">
          </div>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">麦克风设备</span>
              <span class="setting-desc">选择录音使用的输入设备。</span>
            </div>
            <select v-model="form.audio_input_device" class="setting-select">
              <option value="default">系统默认</option>
              <option v-for="device in audioInputDevices" :key="device.deviceId" :value="device.deviceId">
                {{ device.label || `麦克风 ${device.deviceId.slice(0, 8)}` }}
              </option>
            </select>
          </div>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">录音时暂停其他音频</span>
              <span class="setting-desc">通过 Electron 系统层通知桌面端处理。</span>
            </div>
            <label class="toggle-switch">
              <input v-model="form.pause_audio_on_recording" type="checkbox">
              <span class="toggle-slider"></span>
            </label>
          </div>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">自动 AI 优化</span>
              <span class="setting-desc">转写完成后自动调用默认 LLM 进行优化。</span>
            </div>
            <label class="toggle-switch">
              <input v-model="form.auto_optimize" type="checkbox">
              <span class="toggle-slider"></span>
            </label>
          </div>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">自动复制到剪贴板</span>
              <span class="setting-desc">转写或优化完成后自动复制文本，并尝试粘贴到当前焦点输入框。</span>
            </div>
            <label class="toggle-switch">
              <input v-model="form.auto_copy_to_clipboard" type="checkbox">
              <span class="toggle-slider"></span>
            </label>
          </div>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">默认优化预设</span>
              <span class="setting-desc">直接读取 `prompts` 目录中的提示词文件，默认优先使用“通用优化”。</span>
            </div>
            <div class="setting-actions-inline">
              <select v-model="selectedPresetId" class="setting-select" :disabled="!builtinPresets.length">
                <option v-if="!builtinPresets.length" value="">未发现 prompts/*.txt</option>
                <option v-for="preset in builtinPresets" :key="preset.id" :value="preset.id">
                  {{ preset.name }}
                </option>
              </select>
              <button class="btn" type="button" :disabled="!builtinPresets.length" @click="openPresetModal">查看提示词</button>
            </div>
          </div>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">录音快捷键</span>
              <span class="setting-desc">点击输入框后按下组合键。现在由后端监听热键，按住开始录音，松开结束；支持 `Win+Alt` 这类无主键组合。</span>
            </div>
            <div class="hotkey-input-wrapper">
              <input
                :value="hotkeyDisplay"
                class="hotkey-input"
                :class="{ listening: isCapturingHotkey }"
                type="text"
                readonly
                placeholder="点击录入快捷键"
                @click="startHotkeyCapture"
              >
              <button class="hotkey-reset-btn" type="button" @click="resetHotkey">↺</button>
            </div>
          </div>
        </div>

      <div class="settings-group">
        <h3 class="settings-group-title">LLM 配置</h3>

        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">默认 LLM 配置</span>
            <span class="setting-desc">选择用于文本优化的默认模型服务。</span>
          </div>
          <div class="setting-actions-inline">
            <select v-model="selectedLLMConfigId" class="setting-select">
              <option value="">未设置</option>
              <option v-for="config in llmConfigs" :key="config.id" :value="config.id">
                {{ config.name }}
              </option>
            </select>
            <button class="btn" type="button" @click="openLLMModal">管理配置</button>
          </div>
        </div>
      </div>
    </section>
  </div>
</template>

<script setup>
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import { useSettings, usePresets, useLLMConfig } from '@/composables/useSettings'
import { useElectron } from '@/composables/useElectron'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { settings, saveSettings } = useSettings()
const { presets, defaultPresetId, loadPresets, setDefaultPreset } = usePresets()
const { llmConfigs, loadLLMConfigs, loadLLMProviders, setDefaultLLMConfig, defaultConfigId } = useLLMConfig()
const { syncDesktopSettings, platform } = useElectron()

const form = ref({})
const audioInputDevices = ref([])
const isCapturingHotkey = ref(false)
const selectedLLMConfigId = ref('')
const selectedPresetId = ref('')
const isInitialized = ref(false)
const syncDepth = ref(0)
const isSavingSettings = ref(false)
const isSavingLLM = ref(false)
const isSavingPreset = ref(false)
const lastSavedSettingsSnapshot = ref('')
const lastSavedLLMConfigId = ref('')
const lastSavedPresetId = ref('')

const builtinPresets = computed(() => presets.value.filter((preset) => preset.is_builtin))
const isSyncing = computed(() => syncDepth.value > 0)

const hotkeyDisplay = computed(() => {
  return form.value.hotkey?.display || '点击录入快捷键'
})

function getDefaultHotkey() {
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

function buildHotkeyDisplay(modifiers, key) {
  const map = {
    alt: platform === 'darwin' ? '⌥' : 'Alt',
    ctrl: platform === 'darwin' ? '⌃' : 'Ctrl',
    cmd: platform === 'darwin' ? '⌘' : 'Cmd',
    shift: platform === 'darwin' ? '⇧' : 'Shift',
    win: 'Win'
  }

  const modifierText = modifiers.map((modifier) => map[modifier] || modifier)
  return [...modifierText, key ? String(key).toUpperCase() : null].filter(Boolean).join(platform === 'darwin' ? '' : '+')
}

function normalizeSettings(value = {}) {
  const normalizedHotkey = Array.isArray(value.hotkey?.modifiers) && value.hotkey.modifiers.length > 0
    ? value.hotkey
    : getDefaultHotkey()
  form.value = {
    whisper_model: value.whisper_model || 'large-v3',
    whisper_device: value.whisper_device || 'auto',
    pause_audio_on_recording: value.pause_audio_on_recording ?? true,
    auto_optimize: value.auto_optimize ?? false,
    auto_copy_to_clipboard: value.auto_copy_to_clipboard ?? true,
    audio_input_device: value.audio_input_device || 'default',
    hotkey: normalizedHotkey,
    recording_mode: value.recording_mode || 'single',
    silence_threshold: value.silence_threshold ?? 1.5,
    silence_level: value.silence_level ?? -40
  }
}

function buildSettingsPayload() {
  return {
    whisper_model: form.value.whisper_model,
    whisper_device: form.value.whisper_device,
    pause_audio_on_recording: form.value.pause_audio_on_recording,
    auto_optimize: form.value.auto_optimize,
    auto_copy_to_clipboard: form.value.auto_copy_to_clipboard,
    audio_input_device: form.value.audio_input_device,
    hotkey: form.value.hotkey || getDefaultHotkey(),
    recording_mode: form.value.recording_mode,
    silence_threshold: form.value.silence_threshold,
    silence_level: form.value.silence_level
  }
}

function serializeSettingsPayload(payload) {
  return JSON.stringify(payload)
}

function getPreferredBuiltinPresetId() {
  return builtinPresets.value.find((preset) => preset.name === '通用优化')?.id ||
    builtinPresets.value[0]?.id ||
    ''
}

function resolveSelectedPresetId() {
  const presetIds = new Set(builtinPresets.value.map((preset) => preset.id))
  if (defaultPresetId.value && presetIds.has(defaultPresetId.value)) {
    return defaultPresetId.value
  }
  return getPreferredBuiltinPresetId()
}

function runSyncUpdate(callback) {
  syncDepth.value += 1

  try {
    callback()
  } finally {
    Promise.resolve().then(() => {
      syncDepth.value = Math.max(0, syncDepth.value - 1)
    })
  }
}

async function loadAudioInputDevices() {
  try {
    await navigator.mediaDevices.getUserMedia({ audio: true })
    const devices = await navigator.mediaDevices.enumerateDevices()
    audioInputDevices.value = devices.filter((device) => device.kind === 'audioinput')
  } catch (error) {
    console.error('Failed to enumerate audio input devices:', error)
  }
}

function openPresetModal() {
  const currentPreset = builtinPresets.value.find((preset) => preset.id === selectedPresetId.value) ||
    builtinPresets.value[0] ||
    null

  if (!currentPreset) {
    return
  }

  settingsStore.currentPreset = currentPreset
  appStore.openModal('presetEdit')
}

function openLLMModal() {
  appStore.openModal('llm')
}

function startHotkeyCapture() {
  isCapturingHotkey.value = true
}

function stopHotkeyCapture() {
  isCapturingHotkey.value = false
}

function resetHotkey() {
  form.value.hotkey = getDefaultHotkey()
}

function handleHotkeyKeydown(event) {
  if (!isCapturingHotkey.value) {
    return
  }

  event.preventDefault()
  event.stopPropagation()

  const modifiers = []
  if (event.altKey) modifiers.push('alt')
  if (event.ctrlKey) modifiers.push('ctrl')
  if (event.metaKey) modifiers.push(platform === 'darwin' ? 'cmd' : 'win')
  if (event.shiftKey) modifiers.push('shift')

  const key = ['Alt', 'Control', 'Meta', 'Shift'].includes(event.key)
    ? null
    : event.key.toLowerCase()

  if (modifiers.length === 0) {
    return
  }

  if (!key && modifiers.length < 2) {
    return
  }

  form.value.hotkey = {
    modifiers,
    key: key && key.length === 1 ? key : null,
    display: buildHotkeyDisplay(modifiers, key && key.length === 1 ? key : null)
  }
  stopHotkeyCapture()
}

async function handleSave() {
  if (!isInitialized.value || isSyncing.value || isSavingSettings.value) {
    return
  }

  const payload = buildSettingsPayload()
  const snapshot = serializeSettingsPayload(payload)

  if (snapshot === lastSavedSettingsSnapshot.value) {
    return
  }

  isSavingSettings.value = true

  try {
    await saveSettings(payload, { silent: true })
    lastSavedSettingsSnapshot.value = snapshot

    await syncDesktopSettings({
      pause_audio_on_recording: payload.pause_audio_on_recording
    })
  } finally {
    isSavingSettings.value = false
  }
}

async function handleDefaultLLMSave() {
  if (!isInitialized.value || isSyncing.value || isSavingLLM.value) {
    return
  }

  if (!selectedLLMConfigId.value || selectedLLMConfigId.value === lastSavedLLMConfigId.value) {
    return
  }

  isSavingLLM.value = true

  try {
    await setDefaultLLMConfig(selectedLLMConfigId.value, { silent: true })
    lastSavedLLMConfigId.value = selectedLLMConfigId.value
  } finally {
    isSavingLLM.value = false
  }
}

async function handleDefaultPresetSave() {
  if (!isInitialized.value || isSyncing.value || isSavingPreset.value) {
    return
  }

  const presetIds = new Set(builtinPresets.value.map((preset) => preset.id))
  if (!selectedPresetId.value || !presetIds.has(selectedPresetId.value)) {
    return
  }

  if (selectedPresetId.value === lastSavedPresetId.value) {
    return
  }

  isSavingPreset.value = true

  try {
    await setDefaultPreset(selectedPresetId.value, { silent: true })
    lastSavedPresetId.value = selectedPresetId.value
  } finally {
    isSavingPreset.value = false
  }
}

function debounce(fn, delay) {
  let timer = null
  return function(...args) {
    if (timer) clearTimeout(timer)
    timer = setTimeout(() => fn.apply(this, args), delay)
  }
}

const debouncedSaveSettings = debounce(handleSave, 500)
const debouncedSaveLLM = debounce(handleDefaultLLMSave, 500)
const debouncedSavePreset = debounce(handleDefaultPresetSave, 500)

watch(settings, (value) => {
  runSyncUpdate(() => {
    normalizeSettings(value)
    lastSavedSettingsSnapshot.value = serializeSettingsPayload(buildSettingsPayload())
  })
}, { immediate: true, deep: true })

watch(defaultConfigId, (value) => {
  runSyncUpdate(() => {
    selectedLLMConfigId.value = value || ''
    lastSavedLLMConfigId.value = selectedLLMConfigId.value
  })
}, { immediate: true })

watch([defaultPresetId, builtinPresets], () => {
  runSyncUpdate(() => {
    selectedPresetId.value = resolveSelectedPresetId()
    lastSavedPresetId.value = selectedPresetId.value
  })
}, { immediate: true, deep: true })

watch(form, () => {
  if (!isInitialized.value || isSyncing.value) {
    return
  }
  debouncedSaveSettings()
}, { deep: true })

watch(selectedLLMConfigId, () => {
  if (!isInitialized.value || isSyncing.value) {
    return
  }
  debouncedSaveLLM()
})

watch(selectedPresetId, () => {
  if (!isInitialized.value || isSyncing.value) {
    return
  }
  debouncedSavePreset()
})

onMounted(async () => {
  await Promise.all([
    settingsStore.loadSettings(),
    loadPresets(),
    loadLLMConfigs(),
    loadLLMProviders(),
    loadAudioInputDevices()
  ])

  runSyncUpdate(() => {
    normalizeSettings(settings.value)
    selectedLLMConfigId.value = defaultConfigId.value || ''
    selectedPresetId.value = resolveSelectedPresetId()
    lastSavedSettingsSnapshot.value = serializeSettingsPayload(buildSettingsPayload())
    lastSavedLLMConfigId.value = selectedLLMConfigId.value
    lastSavedPresetId.value = selectedPresetId.value
  })

  isInitialized.value = true
  window.addEventListener('keydown', handleHotkeyKeydown, true)
})

onUnmounted(() => {
  window.removeEventListener('keydown', handleHotkeyKeydown, true)
})
</script>
