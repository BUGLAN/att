<template>
  <div class="settings-page">
    <!-- 二级导航：放在 fieldset 之外，初始化期间也能切页 / 搜索 -->
    <aside class="settings-nav" aria-label="设置分组">
      <div class="settings-nav__head">
        <h1 class="settings-nav__title">设置</h1>
        <label class="settings-nav__search">
          <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
            <circle cx="11" cy="11" r="7" />
            <path d="M20 20l-3.5-3.5" />
          </svg>
          <input
            v-model="navQuery"
            type="search"
            placeholder="搜索设置项"
            aria-label="搜索设置"
            @keydown.enter.prevent="goFirstMatch"
          >
        </label>
      </div>

      <nav class="settings-nav__groups">
        <div v-for="group in navGroups" :key="group.label" class="settings-nav__group">
          <div class="settings-nav__group-label">{{ group.label }}</div>
          <button
            v-for="item in group.items"
            :key="item.id"
            type="button"
            class="settings-nav__item"
            :aria-current="item.id === activeSection ? 'page' : undefined"
            @click="goSection(item.id)"
          >
            <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
              <path :d="SECTION_ICONS[item.id]" />
            </svg>
            <span>{{ item.label }}</span>
          </button>
        </div>
        <p v-if="!navGroups.length" class="settings-nav__empty">没有匹配「{{ navQuery.trim() }}」的设置项</p>
      </nav>

      <div class="settings-nav__foot" :class="{ saving: isSavingSettings || isSavingPreset }">
        <span class="settings-nav__dot" aria-hidden="true"></span>
        {{ isSavingSettings || isSavingPreset ? '正在保存…' : '改动自动保存' }}
      </div>
    </aside>

    <div class="settings-main">
      <header class="settings-main__head">
        <div class="settings-main__crumb">设置 / {{ currentSection.group }}</div>
        <h2 class="settings-main__title">{{ currentSection.label }}</h2>
        <p class="settings-main__desc">{{ currentSection.desc }}</p>
      </header>

      <!-- 初始化完成前禁用全部表单控件：此阶段 watch(form) 的守卫会丢弃改动，
           用户若在这时勾选开关，改动既不会保存也不会提示（真实踩坑） -->
      <p v-if="!isInitialized" class="settings-pending">正在读取已保存的设置，稍后即可修改…</p>
      <fieldset class="settings-fieldset" :disabled="!isInitialized">
        <component :is="SECTION_COMPONENTS[activeSection]" :key="activeSection" />
      </fieldset>
    </div>
  </div>
</template>

<script setup>
import { computed, onMounted, onUnmounted, provide, ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import { useSettings, usePresets } from '@/composables/useSettings'
import { resolveDefaultPresetId, resolveRewritePresetId, suggestNewName } from './presetLogic.mjs'
import { useElectron } from '@/composables/useElectron'
import { hasPendingSettingsEdits } from './settingsFormSync.mjs'
import { hotwordsText, parseHotwords } from './asrHotwords.mjs'
import { findHotkeyConflict } from './settingsHotkeys.mjs'
import { SETTINGS_CTX } from './settingsContext.js'
import { DEFAULT_SECTION_ID, buildNavGroups, findSection, resolveSectionId } from './settingsNav.mjs'
import SettingsRecordingPage from './pages/SettingsRecordingPage.vue'
import SettingsHotkeyPage from './pages/SettingsHotkeyPage.vue'
import SettingsClipboardPage from './pages/SettingsClipboardPage.vue'
import SettingsOutputPage from './pages/SettingsOutputPage.vue'
import SettingsAsrPage from './pages/SettingsAsrPage.vue'
import SettingsLlmPage from './pages/SettingsLlmPage.vue'
import SettingsPresetsPage from './pages/SettingsPresetsPage.vue'
import SettingsPermissionsPage from './pages/SettingsPermissionsPage.vue'
import SettingsDiagnosticsPage from './pages/SettingsDiagnosticsPage.vue'

const SECTION_COMPONENTS = {
  recording: SettingsRecordingPage,
  hotkey: SettingsHotkeyPage,
  clipboard: SettingsClipboardPage,
  output: SettingsOutputPage,
  asr: SettingsAsrPage,
  llm: SettingsLlmPage,
  presets: SettingsPresetsPage,
  permissions: SettingsPermissionsPage,
  diagnostics: SettingsDiagnosticsPage
}

const SECTION_ICONS = {
  recording: 'M12 3a3 3 0 0 0-3 3v6a3 3 0 0 0 6 0V6a3 3 0 0 0-3-3zM5 11a7 7 0 0 0 14 0M12 18v3',
  hotkey: 'M3 7h18v10H3zM7 11h.01M11 11h.01M15 11h.01M8 14h8',
  clipboard: 'M9 3h6v4H9zM7 5H5v16h14V5h-2M9 12h6M9 16h4',
  output: 'M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8zM19 17l.8 2.2L22 20l-2.2.8L19 23l-.8-2.2L16 20l2.2-.8z',
  asr: 'M3 12h2M7 8v8M11 5v14M15 9v6M19 7v10',
  llm: 'M7 7h10v10H7zM10 3v4M14 3v4M10 17v4M14 17v4M3 10h4M3 14h4M17 10h4M17 14h4',
  presets: 'M4 6h16M4 12h10M4 18h7',
  permissions: 'M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z',
  diagnostics: 'M3 12h4l3-7 4 14 3-7h4'
}

// 上次停留的分页：仅作本机便利，读写失败（隐私模式 / 存储被禁）时回落默认页
const SECTION_STORAGE_KEY = 'att.settings.section'

function readStoredSection() {
  try {
    return window.localStorage.getItem(SECTION_STORAGE_KEY) || ''
  } catch {
    return ''
  }
}

function writeStoredSection(id) {
  try {
    window.localStorage.setItem(SECTION_STORAGE_KEY, id)
  } catch {
    // 忽略：不影响切页本身
  }
}

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { settings, saveSettings } = useSettings()
const { presets, defaultPresetId, loadPresets, setDefaultPreset } = usePresets()
const { syncDesktopSettings, syncInputSettings, platform, isDesktop, listAudioDevices } = useElectron()

const form = ref({})
const audioInputDevices = ref([])
// 正在录入哪个快捷键：HOTKEY_TARGETS 里的字段名（hotkey / clipboard_panel_hotkey / rewrite_hotkey）或 null
const captureTarget = ref(null)
const isCapturingHotkey = computed(() => captureTarget.value === 'hotkey')
const isCapturingPanelHotkey = computed(() => captureTarget.value === 'clipboard_panel_hotkey')
const isCapturingRewriteHotkey = computed(() => captureTarget.value === 'rewrite_hotkey')
const hotkeyError = ref('')

const selectedPresetId = ref('')
const isInitialized = ref(false)
const syncDepth = ref(0)
const isSavingSettings = ref(false)
const isSavingPreset = ref(false)
const lastSavedSettingsSnapshot = ref('')
const lastSavedPresetId = ref('')

// 全部预设：内置（只读）在前，自定义在后；下拉框与预设页都用它
const allPresets = computed(() => presets.value)
const isSyncing = computed(() => syncDepth.value > 0)
const streamingRuntimeAvailable = computed(() => settingsStore.runtimeStatus.qwen_streaming_available === true)
const streamingRuntimeSummary = computed(() => {
  const runtimeEnvironment = settingsStore.runtimeStatus.runtime_environment || 'unknown'
  const backend = settingsStore.runtimeStatus.qwen_streaming_backend

  if (streamingRuntimeAvailable.value) {
    return backend
      ? `当前后端环境：${runtimeEnvironment}，真流式可用（${backend}）`
      : `当前后端环境：${runtimeEnvironment}，真流式可用`
  }

  const reason = settingsStore.runtimeStatus.qwen_streaming_error
  if (reason) {
    return `当前后端环境：${runtimeEnvironment}，真流式不可用：${reason}`
  }

  return `当前后端环境：${runtimeEnvironment}，若在 PowerShell 启动后端将自动降级为单段录音`
})
const activeSection = ref(resolveSectionId(appStore.settingsSection || readStoredSection() || DEFAULT_SECTION_ID))
const currentSection = computed(() => findSection(activeSection.value))
const navQuery = ref('')
const navGroups = computed(() => buildNavGroups(navQuery.value))

function goSection(id) {
  const next = resolveSectionId(id)
  if (next !== activeSection.value) {
    // 离开快捷键页时结束录入，否则下一次按键会被悄悄录成快捷键
    stopHotkeyCapture()
    activeSection.value = next
  }
  writeStoredSection(next)
}

function goFirstMatch() {
  const first = navGroups.value[0]?.items[0]
  if (first) {
    goSection(first.id)
  }
}

const hotkeyDisplay = computed(() => {
  return form.value.hotkey?.display || '点击录入快捷键'
})
const panelHotkeyDisplay = computed(() => {
  return form.value.clipboard_panel_hotkey?.display || '点击录入快捷键'
})
const rewriteHotkeyDisplay = computed(() => {
  return form.value.rewrite_hotkey?.display || '点击录入快捷键'
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

// 剪贴板悬浮框默认 ⌃⌥V / Ctrl+Alt+V（与 Rust 侧 HotkeyBinding::default_for 一致）
function getDefaultPanelHotkey() {
  return normalizeHotkeyConfig({ modifiers: ['ctrl', 'alt'], key: 'v' })
}

// 优化选中文本默认 ⌥A / Alt+A（与 Rust 侧 HotkeyBinding::default_for 一致）
function getDefaultRewriteHotkey() {
  return normalizeHotkeyConfig({ modifiers: ['alt'], key: 'a' })
}

function getDefaultHotkeyFor(field) {
  if (field === 'clipboard_panel_hotkey') return getDefaultPanelHotkey()
  if (field === 'rewrite_hotkey') return getDefaultRewriteHotkey()
  return getDefaultHotkey()
}

function buildHotkeyDisplay(modifiers, key) {
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

function normalizeHotkeyConfig(hotkey, fallback = getDefaultHotkey()) {
  if (!Array.isArray(hotkey?.modifiers) || hotkey.modifiers.length === 0) {
    return fallback
  }

  const modifiers = sortHotkeyModifiers(
    hotkey.modifiers
      .map((modifier) => String(modifier || '').toLowerCase())
      .filter((modifier, index, array) => modifier && array.indexOf(modifier) === index)
  )
  const key = typeof hotkey?.key === 'string' && hotkey.key.length === 1
    ? hotkey.key.toLowerCase()
    : null

  if (!key && modifiers.length < 2) {
    return fallback
  }

  return {
    modifiers,
    key,
    display: buildHotkeyDisplay(modifiers, key)
  }
}

function normalizeSettings(value = {}) {
  const normalizedHotkey = normalizeHotkeyConfig(value.hotkey)
  form.value = {
    asr_provider: value.asr_provider || 'whisper',
    asr_language: value.asr_language || 'zh',
    // 文本框原文：解析结果与服务端一致时保留用户正在敲的内容（见 hotwordsText）
    asr_hotwords_text: hotwordsText(value.asr_hotwords, form.value?.asr_hotwords_text),
    whisper_model: value.whisper_model || 'large-v3',
    whisper_device: value.whisper_device || 'auto',
    qwen_asr_model: value.qwen_asr_model || 'Qwen/Qwen3-ASR-1.7B',
    qwen_asr_device: value.qwen_asr_device || 'auto',
    qwen_asr_dtype: value.qwen_asr_dtype || 'auto',
    qwen_asr_max_new_tokens: value.qwen_asr_max_new_tokens ?? 256,
    qwen_asr_return_timestamps: value.qwen_asr_return_timestamps ?? false,
    qwen_asr_use_forced_aligner: value.qwen_asr_use_forced_aligner ?? false,
    qwen_asr_forced_aligner_model: value.qwen_asr_forced_aligner_model || 'Qwen/Qwen3-ForcedAligner-0.6B',
    qwen_asr_max_audio_seconds: value.qwen_asr_max_audio_seconds ?? 30,
    // 豆包语音：明文 Token 只存在于表单内存里，初始值恒为空
    doubao_asr_api_key: '',
    doubao_asr_api_key_clear: false,
    doubao_asr_api_key_set: value.doubao_asr_api_key_set ?? false,
    doubao_asr_resource_id: value.doubao_asr_resource_id || 'volc.seedasr.sauc.duration',
    doubao_asr_endpoint: value.doubao_asr_endpoint || 'wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async',
    doubao_asr_end_window_size: value.doubao_asr_end_window_size ?? 800,
    // ChatGPT 听写：明文令牌只存在于表单内存里，初始值恒为空
    chatgpt_asr_api_key: '',
    chatgpt_asr_api_key_clear: false,
    chatgpt_asr_api_key_set: value.chatgpt_asr_api_key_set ?? false,
    chatgpt_asr_endpoint: value.chatgpt_asr_endpoint || 'https://chatgpt.com/backend-api/transcribe',
    chatgpt_asr_timeout_seconds: value.chatgpt_asr_timeout_seconds ?? 120,
    pause_audio_on_recording: value.pause_audio_on_recording ?? true,
    auto_optimize: value.auto_optimize ?? false,
    auto_copy_to_clipboard: value.auto_copy_to_clipboard ?? true,
    audio_input_device: value.audio_input_device || 'default',
    hotkey: normalizedHotkey,
    clipboard_panel_enabled: value.clipboard_panel_enabled ?? true,
    clipboard_panel_hotkey: normalizeHotkeyConfig(value.clipboard_panel_hotkey, getDefaultPanelHotkey()),
    clipboard_panel_enter_action: value.clipboard_panel_enter_action === 'copy' ? 'copy' : 'paste',
    rewrite_enabled: value.rewrite_enabled ?? true,
    rewrite_hotkey: normalizeHotkeyConfig(value.rewrite_hotkey, getDefaultRewriteHotkey()),
    // 空串 = 跟随默认优化预设
    rewrite_preset_id: value.rewrite_preset_id || '',
    recording_mode: value.recording_mode || 'single',
    silence_threshold: value.silence_threshold ?? 1.5,
    silence_level: value.silence_level ?? -40
  }
}

function buildSettingsPayload() {
  const enableQwenTimestamps = !!form.value.qwen_asr_return_timestamps
  const payload = {
    asr_provider: form.value.asr_provider,
    asr_language: form.value.asr_language,
    asr_hotwords: parseHotwords(form.value.asr_hotwords_text),
    whisper_model: form.value.whisper_model,
    whisper_device: form.value.whisper_device,
    qwen_asr_model: form.value.qwen_asr_model,
    qwen_asr_device: form.value.qwen_asr_device,
    qwen_asr_dtype: form.value.qwen_asr_dtype,
    qwen_asr_max_new_tokens: form.value.qwen_asr_max_new_tokens,
    qwen_asr_return_timestamps: enableQwenTimestamps,
    qwen_asr_use_forced_aligner: enableQwenTimestamps && form.value.qwen_asr_use_forced_aligner,
    qwen_asr_forced_aligner_model: form.value.qwen_asr_forced_aligner_model,
    qwen_asr_max_audio_seconds: form.value.qwen_asr_max_audio_seconds,
    doubao_asr_resource_id: form.value.doubao_asr_resource_id,
    doubao_asr_endpoint: form.value.doubao_asr_endpoint,
    doubao_asr_end_window_size: form.value.doubao_asr_end_window_size,
    chatgpt_asr_endpoint: form.value.chatgpt_asr_endpoint,
    chatgpt_asr_timeout_seconds: form.value.chatgpt_asr_timeout_seconds,
    pause_audio_on_recording: form.value.pause_audio_on_recording,
    auto_optimize: form.value.auto_optimize,
    auto_copy_to_clipboard: form.value.auto_copy_to_clipboard,
    audio_input_device: form.value.audio_input_device,
    hotkey: normalizeHotkeyConfig(form.value.hotkey),
    clipboard_panel_enabled: Boolean(form.value.clipboard_panel_enabled),
    clipboard_panel_hotkey: normalizeHotkeyConfig(form.value.clipboard_panel_hotkey, getDefaultPanelHotkey()),
    clipboard_panel_enter_action: form.value.clipboard_panel_enter_action,
    rewrite_enabled: Boolean(form.value.rewrite_enabled),
    rewrite_hotkey: normalizeHotkeyConfig(form.value.rewrite_hotkey, getDefaultRewriteHotkey()),
    rewrite_preset_id: form.value.rewrite_preset_id || '',
    recording_mode: form.value.recording_mode,
    silence_threshold: form.value.silence_threshold,
    silence_level: form.value.silence_level
  }

  // 明文 Token 只在「用户确实输入了」时才提交：
  // 空串代表「本次不修改」，后端会保留已保存的密钥（否则整份快照会把密钥清掉）。
  if (form.value.doubao_asr_api_key) {
    payload.doubao_asr_api_key = form.value.doubao_asr_api_key
  }

  if (form.value.doubao_asr_api_key_clear) {
    payload.doubao_asr_api_key_clear = true
  }

  if (form.value.chatgpt_asr_api_key) {
    payload.chatgpt_asr_api_key = form.value.chatgpt_asr_api_key
  }

  if (form.value.chatgpt_asr_api_key_clear) {
    payload.chatgpt_asr_api_key_clear = true
  }

  return payload
}

function serializeSettingsPayload(payload) {
  return JSON.stringify(payload)
}

// 默认预设已不存在（被删 / 文件改名）时回退「通用优化」，再否则第一个
function resolveSelectedPresetId() {
  return resolveDefaultPresetId(defaultPresetId.value, allPresets.value)
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
  // Tauri 模式下不触碰 getUserMedia：页面是 v-show 常驻挂载，挂载即请求麦克风
  // 会触发系统权限弹窗。采集在 Rust 侧（cpal），设备按名称选择，枚举也走 Rust 命令。
  if (isDesktop) {
    try {
      const names = await listAudioDevices()
      audioInputDevices.value = (names || []).map((name) => ({ deviceId: name, label: name }))
    } catch (error) {
      console.error('Failed to list audio input devices:', error)
      audioInputDevices.value = []
    }
    return
  }

  try {
    await navigator.mediaDevices.getUserMedia({ audio: true })
    const devices = await navigator.mediaDevices.enumerateDevices()
    audioInputDevices.value = devices.filter((device) => device.kind === 'audioinput')
  } catch (error) {
    console.error('Failed to enumerate audio input devices:', error)
  }
}

// 打开预设弹窗：传入 preset 时打开该预设，否则打开当前默认预设。
// 内置预设只读 → 查看；自定义预设 → 直接编辑
function openPresetModal(preset) {
  const target = (preset?.id ? preset : null) ||
    allPresets.value.find((item) => item.id === selectedPresetId.value) ||
    allPresets.value[0] ||
    null

  if (!target) {
    return
  }

  settingsStore.presetEditor = {
    mode: target.is_builtin === false ? 'edit' : 'view',
    preset: target,
    name: target.name,
    prompt_template: target.prompt_template || ''
  }
  appStore.openModal('presetEdit')
}

// 新增空白自定义预设
function openPresetCreator() {
  settingsStore.presetEditor = {
    mode: 'create',
    preset: null,
    name: suggestNewName(allPresets.value),
    prompt_template: ''
  }
  appStore.openModal('presetEdit')
}

// 删除自定义预设：后端负责把正被选用处回退（默认 → 通用优化，优化选中文本 → 跟随默认），
// store 随后重拉设置与列表，这里的 watch 会把表单同步到回退后的值
async function deletePreset(preset) {
  try {
    await settingsStore.deletePreset(preset.id)
    appStore.showToast(`已删除预设「${preset.name}」`, { tone: 'success' })
  } catch (error) {
    appStore.showToast(String(error?.message || error), { tone: 'error' })
    throw error
  }
}

function startHotkeyCapture(target = 'hotkey') {
  hotkeyError.value = ''
  captureTarget.value = target
}

function stopHotkeyCapture() {
  captureTarget.value = null
}

function resetHotkey(target = 'hotkey') {
  hotkeyError.value = ''
  form.value[target] = getDefaultHotkeyFor(target)
}

/** KeyA–KeyZ / Digit0–9 → 'a'–'z' / '0'–'9'；修饰键与其他键返回 null（旧版只支持字母数字主键） */
function hotkeyKeyFromCode(code) {
  const match = /^(?:Key([A-Z])|Digit([0-9]))$/.exec(String(code || ''))
  return match ? (match[1] || match[2]).toLowerCase() : null
}

function handleHotkeyKeydown(event) {
  const target = captureTarget.value
  if (!target) {
    return
  }

  event.preventDefault()
  event.stopPropagation()

  const modifiers = []
  if (event.altKey) modifiers.push('alt')
  if (event.ctrlKey) modifiers.push('ctrl')
  if (event.metaKey) modifiers.push(platform === 'darwin' ? 'cmd' : 'win')
  if (event.shiftKey) modifiers.push('shift')

  // 主键按物理键位取（event.code）：Mac 上 ⌥V 的 event.key 是「√」、⇧1 是「!」，
  // 按字符录会存成后端不认的组合，被悄悄回落成默认快捷键
  const key = hotkeyKeyFromCode(event.code)

  if (modifiers.length === 0) {
    return
  }

  if (!key && modifiers.length < 2) {
    return
  }

  const next = normalizeHotkeyConfig({
    modifiers,
    key
  })
  // 两个全局快捷键撞键时只会触发其中一个（后端同样会拒绝保存）
  const conflict = findHotkeyConflict(form.value, target, next)
  if (conflict) {
    hotkeyError.value = `${next.display} 已用作${conflict.label}快捷键，请换一个`
    stopHotkeyCapture()
    return
  }
  hotkeyError.value = ''
  form.value[target] = next
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

    // 提交后立刻清掉表单里的明文 Token，并重算快照：
    // 否则下一次保存会把「带 Token 的快照」与「已清空的表单」判成变更，反复提交。
    form.value.doubao_asr_api_key = ''
    form.value.doubao_asr_api_key_clear = false
    form.value.chatgpt_asr_api_key = ''
    form.value.chatgpt_asr_api_key_clear = false
    lastSavedSettingsSnapshot.value = serializeSettingsPayload(buildSettingsPayload())

    const inputSyncResult = await syncInputSettings({
      hotkey: payload.hotkey
    })
    if (inputSyncResult?.success === false) {
      throw new Error(inputSyncResult.error || '桌面输入同步失败')
    }

    await syncDesktopSettings({
      pause_audio_on_recording: payload.pause_audio_on_recording
    })
  } finally {
    isSavingSettings.value = false
  }
}

async function handleDefaultPresetSave() {
  if (!isInitialized.value || isSyncing.value || isSavingPreset.value) {
    return
  }

  const presetIds = new Set(allPresets.value.map((preset) => preset.id))
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
const debouncedSavePreset = debounce(handleDefaultPresetSave, 500)

watch(settings, (value) => {
  runSyncUpdate(() => {
    // 用户有未提交的改动时不要用服务端值重建表单：
    // 否则刚改的项会被静默回滚，待保存内容也会被判成「无变化」而丢掉（真实踩坑）
    if (hasPendingSettingsEdits({
      initialized: isInitialized.value,
      draftSnapshot: serializeSettingsPayload(buildSettingsPayload()),
      savedSnapshot: lastSavedSettingsSnapshot.value
    })) {
      return
    }

    normalizeSettings(value)
    lastSavedSettingsSnapshot.value = serializeSettingsPayload(buildSettingsPayload())
  })
}, { immediate: true, deep: true })

watch([defaultPresetId, allPresets], () => {
  runSyncUpdate(() => {
    selectedPresetId.value = resolveSelectedPresetId()
    lastSavedPresetId.value = selectedPresetId.value
  })
}, { immediate: true, deep: true })

// 「优化选中文本」选的预设被删掉（或别处改了文件）时回退「跟随默认」，避免下拉框空白、把失效 id 写回去。
// 列表为空（还没加载 / 加载失败）时不动，防止误清
watch(allPresets, (list) => {
  if (!isInitialized.value || !list.length || !form.value.rewrite_preset_id) {
    return
  }
  const resolved = resolveRewritePresetId(form.value.rewrite_preset_id, list)
  if (resolved !== form.value.rewrite_preset_id) {
    form.value.rewrite_preset_id = resolved
  }
})

watch(form, () => {
  if (!isInitialized.value || isSyncing.value) {
    return
  }
  debouncedSaveSettings()
}, { deep: true })

watch(selectedPresetId, () => {
  if (!isInitialized.value || isSyncing.value) {
    return
  }
  debouncedSavePreset()
})

// 外部跳转（转文字页「去设置」、托盘等）：切到指定分页后清空，下次同一目标仍能触发
watch(() => appStore.settingsSection, (section) => {
  if (!section) {
    return
  }
  goSection(section)
  appStore.settingsSection = ''
}, { immediate: true })

provide(SETTINGS_CTX, {
  form,
  audioInputDevices,
  streamingRuntimeAvailable,
  streamingRuntimeSummary,
  allPresets,
  selectedPresetId,
  openPresetModal,
  openPresetCreator,
  deletePreset,
  hotkeyDisplay,
  isCapturingHotkey,
  panelHotkeyDisplay,
  isCapturingPanelHotkey,
  rewriteHotkeyDisplay,
  isCapturingRewriteHotkey,
  hotkeyError,
  startHotkeyCapture,
  resetHotkey,
  goSection
})

onMounted(async () => {
  // 用 allSettled：任意一项初始化失败（健康探测超时、provider 列表异常等）都不能
  // 让下面的 isInitialized 停在 false —— 那会让此后所有设置保存被守卫静默跳过
  const initResults = await Promise.allSettled([
    settingsStore.loadSettings(),
    settingsStore.loadRuntimeStatus(true),
    loadPresets(),
    loadAudioInputDevices()
  ])

  initResults
    .filter((result) => result.status === 'rejected')
    .forEach((result, index) => {
      console.warn('[Settings] 初始化子步骤失败：', index, result.reason)
    })

  runSyncUpdate(() => {
    normalizeSettings(settings.value)
    selectedPresetId.value = resolveSelectedPresetId()
    lastSavedSettingsSnapshot.value = serializeSettingsPayload(buildSettingsPayload())
    lastSavedPresetId.value = selectedPresetId.value
  })

  isInitialized.value = true
  window.addEventListener('keydown', handleHotkeyKeydown, true)
})

onUnmounted(() => {
  window.removeEventListener('keydown', handleHotkeyKeydown, true)
})
</script>

<style scoped>
/* 两栏：左侧二级导航随外层 .page-view 滚动时吸顶，右侧为当前分页 */
.settings-page {
  display: grid;
  grid-template-columns: 216px minmax(0, 1fr);
  gap: var(--s-6);
  align-items: start;
}

.settings-nav {
  position: sticky;
  top: 0;
  display: flex;
  flex-direction: column;
  max-height: calc(100vh - var(--titlebar-height) - 2 * var(--space-xl));
  background: var(--bg-surface);
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  box-shadow: var(--edge-hi);
  overflow: hidden;
}

.settings-nav__head {
  padding: var(--s-4) var(--s-3) var(--s-3);
}

.settings-nav__title {
  margin: 0 0 var(--s-3);
  padding: 0 var(--s-1);
  font-size: var(--fs-lg);
  font-weight: 600;
  color: var(--text-1);
}

.settings-nav__search {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  height: 34px;
  padding: 0 var(--s-2);
  border-radius: var(--r-sm);
  background: var(--bg-raise);
  border: 1px solid var(--line-strong);
  color: var(--text-3);
  transition: border-color var(--t-fast) var(--ease);
}

.settings-nav__search:focus-within {
  border-color: var(--accent-line-strong);
}

.settings-nav__search svg {
  flex: none;
  width: 14px;
  height: 14px;
  stroke: currentColor;
  stroke-width: 2;
  stroke-linecap: round;
}

.settings-nav__search input {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  color: var(--text-1);
  font: inherit;
  font-size: var(--fs-sm);
}

.settings-nav__groups {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  padding: var(--s-1) var(--s-2) var(--s-4);
}

.settings-nav__group {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.settings-nav__group-label {
  padding: 0 var(--s-2) 6px;
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.08em;
  color: var(--text-3);
}

.settings-nav__item {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  height: 36px;
  padding: 0 var(--s-2);
  border: none;
  border-radius: var(--r-sm);
  background: transparent;
  color: var(--text-2);
  font-size: var(--fs-sm);
  text-align: left;
  cursor: pointer;
  transition: background-color var(--t-fast) var(--ease), color var(--t-fast) var(--ease);
}

.settings-nav__item span {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.settings-nav__item svg {
  flex: none;
  width: 16px;
  height: 16px;
  stroke: currentColor;
  stroke-width: 1.8;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.settings-nav__item:hover {
  background: var(--bg-raise);
  color: var(--text-1);
}

.settings-nav__item:focus-visible {
  outline: none;
  box-shadow: var(--focus-ring);
}

.settings-nav__item[aria-current='page'] {
  background: var(--accent-dim);
  color: var(--accent-hi);
}

.settings-nav__empty {
  padding: var(--s-2);
  font-size: var(--fs-cap);
  color: var(--text-3);
  line-height: 1.6;
}

.settings-nav__foot {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: var(--s-3) var(--s-4);
  border-top: 1px solid var(--line);
  font-size: var(--fs-cap);
  color: var(--text-3);
}

.settings-nav__dot {
  width: 6px;
  height: 6px;
  border-radius: var(--r-xs);
  background: var(--ok);
}

.settings-nav__foot.saving .settings-nav__dot {
  background: var(--accent);
}

.settings-main {
  container: settings-main / inline-size;
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
  min-width: 0;
  max-width: 760px;
}

/* 内容列过窄（800px 窗口 + 展开的主侧栏只剩约 340px）时，
   「说明 | 320px 控件」两列会把说明挤成一字一行：改为上下排，开关仍留在右侧 */
@container settings-main (max-width: 520px) {
  .settings-main :deep(.setting-item) {
    grid-template-columns: minmax(0, 1fr);
    gap: var(--s-2);
  }

  .settings-main :deep(.setting-item:has(> .toggle-switch)) {
    grid-template-columns: minmax(0, 1fr) auto;
    gap: var(--s-3);
  }
}

.settings-main__head {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding-top: var(--s-1);
}

.settings-main__crumb {
  font-size: var(--fs-cap);
  color: var(--text-3);
}

.settings-main__title {
  margin: 0;
  font-size: var(--fs-xl);
  font-weight: 600;
  color: var(--text-1);
}

.settings-main__desc {
  margin: 0;
  font-size: var(--fs-sm);
  line-height: 1.6;
  color: var(--text-2);
}

/* 外壳里的待加载提示不再挂在卡片顶部，改成独立提示条 */
.settings-pending {
  border: 1px solid var(--line);
  border-radius: var(--r-md);
}

/* 最小窗口（800px）下侧栏仍在：导航收窄，内容区留足宽度 */
@media (max-width: 1100px) {
  .settings-page {
    grid-template-columns: 164px minmax(0, 1fr);
    gap: var(--s-4);
  }

  .settings-nav__head {
    padding: var(--s-3) var(--s-2) var(--s-2);
  }

  .settings-nav__groups {
    padding-inline: var(--s-1);
  }
}
</style>
