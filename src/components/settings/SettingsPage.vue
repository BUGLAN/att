<template>
  <div class="settings-page">
    <PageHeader title="设置" subtitle="配置你的录音、提示词和 LLM 服务" />

    <section class="settings-section">
        <div class="settings-group">
          <h3 class="settings-group-title">语音识别</h3>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">ASR Provider</span>
              <span class="setting-desc">切换转写实现层。切换后会自动卸载当前模型服务，需重新开启模型。</span>
            </div>
            <BaseSelect v-model="form.asr_provider">
              <option value="whisper">Whisper / faster-whisper</option>
              <option value="qwen_asr">Qwen3-ASR</option>
              <option value="doubao_asr">豆包语音（火山引擎 · 在线流式）</option>
            </BaseSelect>
          </div>

          <div v-if="showWhisperSettings" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">Whisper 模型</span>
              <span class="setting-desc">模型越大识别越准，但速度也会更慢。</span>
            </div>
            <BaseSelect v-model="form.whisper_model">
              <option value="tiny">tiny</option>
              <option value="base">base</option>
              <option value="small">small</option>
              <option value="medium">medium</option>
              <option value="large-v3">large-v3</option>
            </BaseSelect>
          </div>

          <div v-if="showWhisperSettings" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">Whisper 设备</span>
              <span class="setting-desc">选择用于 Whisper 推理的设备。</span>
            </div>
            <BaseSelect v-model="form.whisper_device">
              <option value="auto">自动</option>
              <option value="cuda">CUDA</option>
              <option value="cpu">CPU</option>
            </BaseSelect>
          </div>

          <div v-if="showQwenSettings" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">Qwen3-ASR 模型</span>
              <span class="setting-desc">支持 Hugging Face 模型名或本地模型目录。</span>
            </div>
            <BaseInput v-model.trim="form.qwen_asr_model" type="text" placeholder="Qwen/Qwen3-ASR-1.7B" />
          </div>

          <div v-if="showQwenSettings" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">Qwen 推理设备</span>
              <span class="setting-desc">`auto` 会优先尝试 CUDA，否则回退到 CPU。</span>
            </div>
            <BaseSelect v-model="form.qwen_asr_device">
              <option value="auto">自动</option>
              <option value="cuda:0">CUDA:0</option>
              <option value="cpu">CPU</option>
            </BaseSelect>
          </div>

          <div v-if="showQwenSettings" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">Qwen dtype</span>
              <span class="setting-desc">默认自动推断；显存紧张时可手动调整。</span>
            </div>
            <BaseSelect v-model="form.qwen_asr_dtype">
              <option value="auto">自动</option>
              <option value="bfloat16">bfloat16</option>
              <option value="float16">float16</option>
              <option value="float32">float32</option>
            </BaseSelect>
          </div>

          <div v-if="showQwenSettings" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">最大输出 token</span>
              <span class="setting-desc">限制单次生成长度，避免极端长音频时输出失控。</span>
            </div>
            <BaseInput v-model.number="form.qwen_asr_max_new_tokens" type="number" min="32" max="2048" step="32" />
          </div>

          <div v-if="showQwenSettings" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">返回时间戳</span>
              <span class="setting-desc">开启后会尝试返回更细粒度的时间信息，代价是更重的推理链路。</span>
            </div>
            <label class="toggle-switch">
              <input v-model="form.qwen_asr_return_timestamps" type="checkbox">
              <span class="toggle-slider"></span>
            </label>
          </div>

          <div v-if="showQwenSettings" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">音频长度上限</span>
              <span class="setting-desc">用于记录 Qwen3-ASR 的单次音频设计上限，当前先保留为配置项。</span>
            </div>
            <BaseInput v-model.number="form.qwen_asr_max_audio_seconds" type="number" min="5" max="600" step="5" />
          </div>

          <div v-if="showQwenSettings && form.qwen_asr_return_timestamps" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">启用强制对齐器</span>
              <span class="setting-desc">进一步细化时间戳，但会显著增加显存和加载时间。</span>
            </div>
            <label class="toggle-switch">
              <input v-model="form.qwen_asr_use_forced_aligner" type="checkbox">
              <span class="toggle-slider"></span>
            </label>
          </div>

          <div v-if="showQwenSettings && shouldShowForcedAlignerModel" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">强制对齐器模型</span>
              <span class="setting-desc">默认使用 `Qwen/Qwen3-ForcedAligner-0.6B`，也可改为本地目录。</span>
            </div>
            <BaseInput v-model.trim="form.qwen_asr_forced_aligner_model" type="text" placeholder="Qwen/Qwen3-ForcedAligner-0.6B" />
          </div>

          <!-- 豆包语音（火山引擎 · 在线流式）：Access Token 只写不读，后端加密落盘 -->
          <div v-if="showDoubaoSettings" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">Access Token</span>
              <span class="setting-desc">
                {{ form.doubao_asr_api_key_set ? '已保存（留空表示不修改）' : '火山语音技术控制台的 Access Token，注意与方舟大模型 key 不通用。' }}
              </span>
            </div>
            <BaseInput
              v-model.trim="form.doubao_asr_api_key"
              type="password"
              autocomplete="new-password"
              placeholder="请输入 Access Token"
            />
          </div>

          <div v-if="showDoubaoSettings && form.doubao_asr_api_key_set" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">清除已保存的 Token</span>
              <span class="setting-desc">开启后保存设置会删除已加密存储的 Access Token。</span>
            </div>
            <label class="toggle-switch">
              <input v-model="form.doubao_asr_api_key_clear" type="checkbox">
              <span class="toggle-slider"></span>
            </label>
          </div>

          <div v-if="showDoubaoSettings" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">Resource ID</span>
              <span class="setting-desc">必须与账号实际开通的产品一致，否则握手直接 403。ASR 1.0 用 bigasr，2.0 用 seedasr。</span>
            </div>
            <BaseSelect v-model="form.doubao_asr_resource_id">
              <option value="volc.seedasr.sauc.duration">volc.seedasr.sauc.duration（ASR 2.0 · 时长）</option>
              <option value="volc.seedasr.sauc.concurrent">volc.seedasr.sauc.concurrent（ASR 2.0 · 并发）</option>
              <option value="volc.bigasr.sauc.duration">volc.bigasr.sauc.duration（ASR 1.0 · 时长）</option>
              <option value="volc.bigasr.sauc.concurrent">volc.bigasr.sauc.concurrent（ASR 1.0 · 并发）</option>
            </BaseSelect>
          </div>

          <div v-if="showDoubaoSettings" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">接入点</span>
              <span class="setting-desc">双向流式 WebSocket 地址，默认无需修改。</span>
            </div>
            <BaseInput
              v-model.trim="form.doubao_asr_endpoint"
              type="text"
              placeholder="wss://openspeech.bytedance.com/api/v3/sauc/bigmodel"
            />
          </div>

          <div v-if="showDoubaoSettings" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">强制断句静音时长</span>
              <span class="setting-desc">毫秒。0 表示跟随服务端语义分段；设为 200–1000 可让句子更早定稿，实时字幕更跟手。</span>
            </div>
            <BaseInput v-model.number="form.doubao_asr_end_window_size" type="number" min="0" max="10000" step="100" />
          </div>
        </div>

        <div class="settings-group">
          <h3 class="settings-group-title">录音设置</h3>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">录音模式</span>
              <span class="setting-desc">支持单段录音、多段录音与流式录音。</span>
            </div>
            <div class="setting-actions-inline setting-actions-block">
              <BaseSelect v-model="form.recording_mode">
              <option value="single">单段录音</option>
              <option value="multi">多段录音</option>
              <option value="streaming">流式录音</option>
              </BaseSelect>
            </div>
          </div>

          <div class="setting-item setting-item-runtime">
            <div class="setting-info">
              <span class="setting-label">流式运行时状态</span>
              <span class="setting-desc">仅在流式录音模式下生效，用来说明当前后端是否可直接进入流式识别。</span>
            </div>
            <div class="setting-runtime-card" :class="{ unavailable: !streamingRuntimeAvailable }">
              <BaseBadge :tone="streamingRuntimeAvailable ? 'ok' : 'neutral'" uppercase>
                {{ streamingRuntimeAvailable ? '可用' : '不可用' }}
              </BaseBadge>
              <span class="setting-runtime-text">{{ streamingRuntimeSummary }}</span>
            </div>
          </div>

          <div v-if="form.recording_mode === 'multi'" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">静音分段阈值</span>
              <span class="setting-desc">静音持续超过该秒数时自动切段。</span>
            </div>
            <BaseInput v-model.number="form.silence_threshold" type="number" min="0.5" max="5" step="0.5" />
          </div>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">麦克风设备</span>
              <span class="setting-desc">选择录音使用的输入设备。</span>
            </div>
            <BaseSelect v-model="form.audio_input_device">
              <option value="default">系统默认</option>
              <option v-for="device in audioInputDevices" :key="device.deviceId" :value="device.deviceId">
                {{ device.label || `麦克风 ${device.deviceId.slice(0, 8)}` }}
              </option>
            </BaseSelect>
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
              <BaseSelect v-model="selectedPresetId" :disabled="!builtinPresets.length">
                <option v-if="!builtinPresets.length" value="">未发现 prompts/*.txt</option>
                <option v-for="preset in builtinPresets" :key="preset.id" :value="preset.id">
                  {{ preset.name }}
                </option>
              </BaseSelect>
              <BaseButton variant="outline" :disabled="!builtinPresets.length" @click="openPresetModal">
                查看提示词
              </BaseButton>
            </div>
          </div>

          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">录音快捷键</span>
              <span class="setting-desc">点击输入框后按下组合键。现在由桌面端监听热键，长按后开始录音，松开结束；支持 `Win+Alt` 这类无主键组合。</span>
            </div>
            <div class="hotkey-input-wrapper">
              <BaseInput
                :value="hotkeyDisplay"
                class="hotkey-input"
                :class="{ listening: isCapturingHotkey }"
                type="text"
                readonly
                placeholder="点击录入快捷键"
                @click="startHotkeyCapture"
              />
              <BaseButton variant="ghost" icon title="恢复默认快捷键" @click="resetHotkey">
                <template #icon>
                  <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
                    <path d="M4 10.5A8 8 0 1 1 6.6 17" />
                    <path d="M4 5v5.5h5.5" />
                  </svg>
                </template>
              </BaseButton>
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
            <BaseSelect v-model="selectedLLMConfigId">
              <option value="">未设置</option>
              <option v-for="config in llmConfigs" :key="config.id" :value="config.id">
                {{ config.name }}
              </option>
            </BaseSelect>
            <BaseButton variant="outline" @click="openLLMModal">管理配置</BaseButton>
          </div>
        </div>
      </div>

      <!-- 诊断区：把原先常驻侧边栏的调试信息（WS 客户端数、模型服务状态）折叠到这里 -->
      <div class="settings-group">
        <h3 class="settings-group-title">诊断</h3>

        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">后端连接</span>
            <span class="setting-desc">WebSocket 与后端服务的实时连接状态。</span>
          </div>
          <div class="setting-actions-inline">
            <BaseBadge :tone="appStore.isConnected ? 'ok' : 'err'">
              {{ appStore.isConnected ? '已连接' : '未连接' }}
            </BaseBadge>
            <BaseBadge tone="neutral" mono>{{ appStore.connectedClients }} 个客户端</BaseBadge>
          </div>
        </div>

        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">运行环境</span>
            <span class="setting-desc">后端上报的运行时环境与模型服务状态。</span>
          </div>
          <div class="setting-actions-inline">
            <BaseBadge tone="neutral">{{ runtimeEnvironment }}</BaseBadge>
            <BaseBadge :tone="settingsStore.runtimeStatus.model_service_ready ? 'ok' : 'warn'">
              {{ settingsStore.runtimeStatus.model_service_ready ? '模型已开启' : '模型未开启' }}
            </BaseBadge>
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
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseInput from '@/components/common/BaseInput.vue'
import BaseSelect from '@/components/common/BaseSelect.vue'
import PageHeader from '@/components/common/PageHeader.vue'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { settings, saveSettings } = useSettings()
const { presets, defaultPresetId, loadPresets, setDefaultPreset } = usePresets()
const { llmConfigs, loadLLMConfigs, loadLLMProviders, setDefaultLLMConfig, defaultConfigId } = useLLMConfig()
const { syncDesktopSettings, syncInputSettings, platform } = useElectron()

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
const showWhisperSettings = computed(() => form.value.asr_provider === 'whisper')
const showQwenSettings = computed(() => form.value.asr_provider === 'qwen_asr')
const showDoubaoSettings = computed(() => form.value.asr_provider === 'doubao_asr')
const streamingRuntimeAvailable = computed(() => settingsStore.runtimeStatus.qwen_streaming_available === true)
// 诊断区展示：后端上报的运行时环境
const runtimeEnvironment = computed(() => settingsStore.runtimeStatus.runtime_environment || 'unknown')
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
const shouldShowForcedAlignerModel = computed(() => (
  showQwenSettings.value &&
  form.value.qwen_asr_return_timestamps &&
  form.value.qwen_asr_use_forced_aligner
))

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

function normalizeHotkeyConfig(hotkey) {
  const fallback = getDefaultHotkey()
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
    doubao_asr_endpoint: value.doubao_asr_endpoint || 'wss://openspeech.bytedance.com/api/v3/sauc/bigmodel',
    doubao_asr_end_window_size: value.doubao_asr_end_window_size ?? 0,
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
  const enableQwenTimestamps = !!form.value.qwen_asr_return_timestamps
  const payload = {
    asr_provider: form.value.asr_provider,
    asr_language: form.value.asr_language,
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
    pause_audio_on_recording: form.value.pause_audio_on_recording,
    auto_optimize: form.value.auto_optimize,
    auto_copy_to_clipboard: form.value.auto_copy_to_clipboard,
    audio_input_device: form.value.audio_input_device,
    hotkey: normalizeHotkeyConfig(form.value.hotkey),
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

  return payload
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

  form.value.hotkey = normalizeHotkeyConfig({
    modifiers,
    key: key && key.length === 1 ? key : null
  })
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
    settingsStore.loadRuntimeStatus(true),
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
