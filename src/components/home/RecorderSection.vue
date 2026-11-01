<template>
  <section class="recorder-page">
    <PageHeader crumb="首页" title="录音" subtitle="按下印章或快捷键开始说话，转写与优化结果会自动复制。" />

    <!-- 录音：一枚印章 + 计时 / 状态 + 声纹 + 引擎状态；进行中底边流动细线（录音朱红 / 处理靛蓝） -->
    <section class="settings-section recorder-card">
      <div class="settings-group">
        <h3 class="settings-group-title">录音</h3>
        <div class="recorder-row">
          <button
            class="ui-seal recorder-seal"
            :data-seal="sealState"
            type="button"
            :title="buttonText"
            :aria-label="buttonText"
            :disabled="recordingButtonDisabled"
            @click="toggleRecording"
          >
            <Transition name="seal-stamp" mode="out-in">
              <span :key="sealGlyph" class="ui-seal-ink">{{ sealGlyph }}</span>
            </Transition>
          </button>

          <div class="recorder-readout">
            <span class="recorder-timer ui-mono">{{ formattedDuration }}</span>
            <span class="ui-meta recorder-status">
              <span class="ui-dot" :class="statusDot"></span>
              <span>{{ statusLine }}</span>
            </span>
          </div>

          <div class="recorder-wave" :class="{ live: isRecording }" aria-hidden="true">
            <i v-for="(height, index) in waveformHeights" :key="index" :style="{ height: `${height}px` }"></i>
          </div>

          <!-- 引擎 / 优化状态：两枚小标签，点了直接去对应的设置页 -->
          <div class="recorder-side">
            <button
              class="recorder-chip"
              type="button"
              :title="`${engineStatus.title}，点击调整识别引擎`"
              @click="appStore.openSettings('asr')"
            >
              <span class="ui-dot" :class="engineStatus.dot"></span>
              <span>{{ engineStatus.text }}</span>
            </button>
            <button
              class="recorder-chip"
              type="button"
              title="点击调整自动优化"
              @click="appStore.openSettings('output')"
            >
              <span class="ui-dot" :class="optimizeStatus.dot"></span>
              <span>{{ optimizeStatus.text }}</span>
            </button>
          </div>
        </div>

        <!-- 引擎不可用：一行原因 + 去设置 -->
        <div v-if="notReadyReason" class="settings-note warn">
          <span>{{ notReadyReason }}</span>
          <button class="ui-tbtn" type="button" @click="appStore.openSettings('asr')">
            <span class="ui-tbtn-glyph">{{ ACTION_GLYPHS.settings }}</span>去设置
          </button>
        </div>

        <!-- 实时文本：录音 / 转写 / 优化期间才出现 -->
        <div v-if="showLivePanel" ref="liveScroll" class="recorder-live" @scroll="handleLiveScroll">
          <template v-if="showOptimizationStream">
            <div class="recorder-live-head">
              <span class="ui-meta">
                <span class="ui-dot" :class="optimizationDot"></span>
                <span>{{ optimizationStatusText }}</span>
              </span>
              <button
                v-if="canCancelOptimization"
                class="ui-tbtn"
                type="button"
                @click="cancelOptimization(optimizationStream.recordId)"
              >
                <span class="ui-tbtn-glyph">{{ ACTION_GLYPHS.cancel }}</span>取消优化
              </button>
            </div>
            <p v-if="optimizationStream.text" class="recorder-live-text">
              {{ optimizationStream.text }}<span
                v-if="optimizationStream.phase === 'streaming'"
                class="typing-cursor"
                aria-hidden="true"
              ></span>
            </p>
            <p v-else class="recorder-live-placeholder">{{ optimizationEmptyText }}</p>
            <p v-if="optimizationStream.phase === 'error'" class="recorder-live-error">{{ optimizationStream.error }}</p>
            <p v-if="optimizationStream.phase === 'waiting' && displayTranscript" class="recorder-live-raw">
              原始转写：{{ displayTranscript }}
            </p>
          </template>
          <template v-else>
            <p v-if="displayTranscript" class="recorder-live-text" :class="{ converting: !isRecording }">{{ displayTranscript }}</p>
            <p v-else class="recorder-live-placeholder">{{ transcriptPlaceholder }}</p>
          </template>
        </div>
      </div>
      <div v-if="flowColor" class="ui-flow" :style="{ '--flow': flowColor }"></div>
    </section>
  </section>
</template>

<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useRecorder } from '@/composables/useRecorder'
import { deriveStreamingProviderLabel } from '@/composables/recorder/streamProtocol.mjs'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import { useOptimizationStreamStore } from '@/stores/optimizationStream'
import { shouldAutoScroll } from '@/composables/recorder/streamScroll.mjs'
import PageHeader from '@/components/common/PageHeader.vue'
import { ACTION_GLYPHS, SEALS, recorderSealState } from '@/components/common/glyphs.mjs'
import { FAILURE_HOLD_MS, SUCCESS_HOLD_MS } from '@/components/floating/floatingView.mjs'
import { providerLabel } from '@/components/settings/asrEngineStatus.mjs'

const {
  isRecording,
  isTranscribing,
  duration,
  audioLevel,
  liveTranscript,
  livePhase,
  isMultiModeEnabled,
  isStreamingModeEnabled,
  startRecording,
  stopRecording,
  cancelOptimization
} = useRecorder()
const appStore = useAppStore()
const settingsStore = useSettingsStore()
const optimizationStream = useOptimizationStreamStore()
const liveScroll = ref(null)
const autoScrollEnabled = ref(true)

// 真流式 provider 的展示名（Qwen 本地 / 豆包在线），文案不再写死 Qwen
const streamingProviderLabel = computed(() => (
  deriveStreamingProviderLabel(settingsStore.settings.asr_provider)
))

const serviceLoading = computed(() => settingsStore.runtimeStatus.model_service_loading)
const serviceReady = computed(() => settingsStore.runtimeStatus.model_service_ready)
const serviceSupported = computed(() => settingsStore.runtimeStatus.model_service_supported)
const streamingAvailable = computed(() => settingsStore.runtimeStatus.streaming_available)
// 在线 provider（豆包）：流式不可用时缺的是凭据，不是「模型没开」，
// 按钮文案要给用户正确的下一步（去设置页填 Token）
const streamingNeedsCredentials = computed(() => (
  isStreamingModeEnabled.value &&
  !streamingAvailable.value &&
  settingsStore.runtimeStatus.model_service_mode === 'online'
))
const recordingButtonDisabled = computed(() => (
  !isRecording.value &&
  (!serviceReady.value || (isStreamingModeEnabled.value && !streamingAvailable.value))
))
const buttonText = computed(() => {
  if (isRecording.value) return '结束录音'
  if (serviceLoading.value) return '模型加载中'
  if (!serviceSupported.value) return '当前模式不可用'
  if (streamingNeedsCredentials.value) return '请先配置 ASR 密钥'
  if (isStreamingModeEnabled.value && serviceReady.value && !streamingAvailable.value) return '当前环境暂不支持流式'
  if (recordingButtonDisabled.value) return '识别引擎未就绪'
  return '开始录音'
})
const formattedDuration = computed(() => {
  const mins = Math.floor(duration.value / 60)
  const secs = duration.value % 60
  return `${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`
})

const panelStatus = computed(() => {
  if (isRecording.value && isStreamingModeEnabled.value) {
    if (livePhase.value === 'connecting') return '正在连接流式转写服务'
    if (livePhase.value === 'finalizing') return '正在生成最终转写结果'
    return '正在连续流式转写'
  }

  if (isRecording.value && isMultiModeEnabled.value) {
    if (livePhase.value === 'speaking') return '正在捕获当前语音片段'
    if (livePhase.value === 'segment-processing') return '上一段已送去转写'
    if (livePhase.value === 'finalizing') return '正在合并整场录音结果'
    return '静音切段监听中'
  }

  if (isRecording.value) return '正在录制整段音频'
  if (isTranscribing.value) return '正在处理录音结果'
  if (livePhase.value === 'optimizing') return '正在后台优化文本'
  return '等待开始录音'
})

// 声纹：36 根细条，空闲时收成一条 3px 的静线；录音时按音量起伏（单段模式没有实时电平，用固定幅度）
const WAVE_SHAPE = [0.35, 0.6, 0.45, 0.8, 0.55, 0.3, 0.7, 0.95, 0.5, 0.4, 0.75, 0.6,
  0.85, 0.45, 0.3, 0.65, 1, 0.55, 0.4, 0.7, 0.5, 0.9, 0.6, 0.35, 0.8, 0.45, 0.65, 0.3,
  0.55, 0.75, 0.4, 0.6, 0.85, 0.5, 0.35, 0.45]
const waveformHeights = computed(() => {
  if (!isRecording.value) {
    return WAVE_SHAPE.map(() => 3)
  }
  const amplitude = (isMultiModeEnabled.value || isStreamingModeEnabled.value) ? audioLevel.value : 0.6
  return WAVE_SHAPE.map((shape) => Math.max(3, Math.round(4 + shape * 22 * Math.min(1, 0.25 + amplitude))))
})

const displayTranscript = computed(() => liveTranscript.value.trim())

const transcriptPlaceholder = computed(() => {
  if (isStreamingModeEnabled.value) {
    // 不可用时优先把后端给出的可读原因摆出来（在线 provider 通常是「未配置 Access Token」）
    if (!streamingAvailable.value && settingsStore.runtimeStatus.streaming_error) {
      return settingsStore.runtimeStatus.streaming_error
    }
    if (serviceReady.value && !streamingAvailable.value) {
      return '当前环境暂不支持流式录音，请切换单段或多段模式。'
    }
    if (isRecording.value) {
      return `${streamingProviderLabel.value}模式已启用，这里会持续刷新当前转写文本。`
    }
    return `切换到${streamingProviderLabel.value}模式后，这里会显示连续更新的实时文本。`
  }

  if (isMultiModeEnabled.value) {
    return '启用多段准实时模式后，这里会按静音自动切段，持续累积转写结果。'
  }

  if (isRecording.value) {
    return '单段模式正在录音中，结束后会统一生成完整转写文本。'
  }

  return '按下录音按钮开始一次新的语音输入。'
})

const showOptimizationStream = computed(() => (
  !isRecording.value &&
  Boolean(optimizationStream.recordId) &&
  (
    optimizationStream.isActive ||
    optimizationStream.phase === 'error' ||
    optimizationStream.phase === 'cancelled'
  )
))

const optimizationStatusText = computed(() => {
  if (optimizationStream.phase === 'waiting') return '正在等待模型返回首字…'
  if (optimizationStream.phase === 'streaming') return '正在流式优化'
  if (optimizationStream.phase === 'error') return '优化失败'
  if (optimizationStream.phase === 'cancelled') return '已取消优化'
  return '正在优化'
})

const optimizationEmptyText = computed(() => {
  if (optimizationStream.phase === 'waiting') return '首字到达前会先显示加载状态，请稍候…'
  if (optimizationStream.phase === 'error') return '没有收到可用的优化文本'
  if (optimizationStream.phase === 'cancelled') return '已保留原始转写'
  return '正在生成优化文本…'
})

const canCancelOptimization = computed(() => (
  optimizationStream.isActive &&
  optimizationStream.source === 'auto' &&
  Boolean(optimizationStream.recordId)
))

// ---- 印章：錄 待命 → 聽 录音中 → 轉 转写 / 优化中 → 成 / 敗（停留片刻）→ 錄 ----
const sealFlash = ref(null)
let sealFlashTimer = null

const isProcessing = computed(() => (
  isTranscribing.value || livePhase.value === 'optimizing' || optimizationStream.isActive
))
const sealState = computed(() => recorderSealState({
  recording: isRecording.value,
  processing: isProcessing.value,
  flash: sealFlash.value
}))
const sealGlyph = computed(() => SEALS[sealState.value])

watch(livePhase, (phase) => {
  if (phase !== 'completed' && phase !== 'error') {
    return
  }
  sealFlash.value = phase === 'completed' ? 'success' : 'failure'
  clearTimeout(sealFlashTimer)
  sealFlashTimer = setTimeout(() => {
    sealFlash.value = null
  }, phase === 'completed' ? SUCCESS_HOLD_MS : FAILURE_HOLD_MS)
})

onUnmounted(() => clearTimeout(sealFlashTimer))

const hotkeyDisplay = computed(() => settingsStore.settings.hotkey?.display || '')

const statusLine = computed(() => {
  if (isRecording.value || isProcessing.value) {
    return panelStatus.value
  }
  if (sealState.value === 'success') return '完成'
  if (sealState.value === 'failure') return '这次没有成功，可以再试一次'
  if (recordingButtonDisabled.value) return buttonText.value
  return hotkeyDisplay.value ? `待命 · 快捷键 ${hotkeyDisplay.value}` : '待命 · 点按印章开始'
})

const statusDot = computed(() => {
  if (isRecording.value) return 'red pulse'
  if (isProcessing.value) return 'blue pulse'
  if (sealState.value === 'success') return 'ok'
  if (sealState.value === 'failure') return 'err'
  if (recordingButtonDisabled.value) return 'warn'
  return 'gold'
})

// 进行中细线的颜色跟印章走：录音朱红、处理靛蓝；空闲时不显示
const flowColor = computed(() => {
  if (isRecording.value) return 'var(--seal-listening-line)'
  if (isProcessing.value) return 'var(--seal-converting-line)'
  return ''
})

const MODE_LABELS = { single: '单段', multi: '多段', streaming: '流式' }

const engineStatus = computed(() => {
  const text = `${providerLabel(settingsStore.settings.asr_provider)} · ${MODE_LABELS[settingsStore.settings.recording_mode] || '单段'}`
  if (serviceLoading.value) {
    return { text: `${text} · 模型加载中`, dot: 'gold pulse', title: '模型加载中' }
  }
  if (recordingButtonDisabled.value) {
    return { text, dot: 'warn', title: buttonText.value }
  }
  return { text, dot: 'ok', title: '识别引擎已就绪' }
})

const optimizeStatus = computed(() => {
  if (!settingsStore.settings.auto_optimize) {
    return { text: '优化已关闭', dot: '' }
  }
  const preset = settingsStore.defaultPreset?.name ||
    String(settingsStore.settings.default_preset_id || '').replace(/^preset-/, '')
  return { text: preset ? `优化 · ${preset}` : '自动优化', dot: 'ok' }
})

// 引擎不可用时给出可照做的原因（加载中不算不可用）
const notReadyReason = computed(() => {
  if (isRecording.value || isProcessing.value || serviceLoading.value || !recordingButtonDisabled.value) {
    return ''
  }
  const runtime = settingsStore.runtimeStatus
  return runtime.streaming_error || runtime.model_service_error || buttonText.value
})

const showLivePanel = computed(() => (
  showOptimizationStream.value ||
  (isRecording.value && (isMultiModeEnabled.value || isStreamingModeEnabled.value)) ||
  ((isRecording.value || isTranscribing.value) && Boolean(displayTranscript.value))
))

const optimizationDot = computed(() => {
  if (optimizationStream.phase === 'error') return 'err'
  if (optimizationStream.phase === 'cancelled') return ''
  return 'blue pulse'
})

function handleLiveScroll() {
  const element = liveScroll.value
  if (!element) {
    return
  }

  autoScrollEnabled.value = shouldAutoScroll({
    scrollTop: element.scrollTop,
    scrollHeight: element.scrollHeight,
    clientHeight: element.clientHeight
  })
}

watch(
  () => optimizationStream.phase,
  (phase, previous) => {
    if (phase === 'waiting' && previous !== 'waiting') {
      autoScrollEnabled.value = true
    }
  }
)

watch(
  () => [optimizationStream.text, optimizationStream.phase, showOptimizationStream.value],
  async () => {
    await nextTick()
    const element = liveScroll.value
    if (!element || !autoScrollEnabled.value) {
      return
    }
    element.scrollTop = element.scrollHeight
  },
  { flush: 'post' }
)

async function refreshModelServiceStatus(probeRuntime = true) {
  try {
    await settingsStore.loadRuntimeStatus(probeRuntime)
  } catch (error) {
    console.error('[RecorderSection] Failed to refresh model service status:', error)
  }
}

function toggleRecording() {
  if (recordingButtonDisabled.value) {
    return
  }

  if (isRecording.value) {
    stopRecording()
    return
  }

  startRecording()
}

onMounted(() => {
  void refreshModelServiceStatus(true)
})

watch(
  () => [settingsStore.settings.asr_provider, settingsStore.settings.recording_mode],
  () => {
    void refreshModelServiceStatus(true)
  },
  { immediate: false }
)
</script>

<style scoped>
.recorder-page {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}

.recorder-card {
  position: relative;
}

.recorder-row {
  display: flex;
  align-items: center;
  gap: var(--s-6);
}

.recorder-seal {
  width: 64px;
  height: 64px;
  border-radius: 8px;
  font-size: 36px;
  cursor: pointer;
  transition: transform var(--t-fast) var(--ease), background-color 280ms ease, color 280ms ease, box-shadow var(--t-base) var(--ease);
}

.recorder-seal[data-seal='idle'] {
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.18), inset 0 -3px 0 rgba(0, 0, 0, 0.18), 0 0 0 6px var(--accent-faint);
}

.recorder-seal:hover:not(:disabled) {
  transform: translateY(-1px);
}

.recorder-seal:active:not(:disabled) {
  transform: scale(0.96);
}

.recorder-seal:focus-visible {
  outline: none;
  box-shadow: var(--focus-ring);
}

.recorder-seal:disabled {
  cursor: not-allowed;
  filter: saturate(0.35) brightness(0.7);
}

/* 录音中：印章外一圈朱红波纹 */
.recorder-seal[data-seal='listening']::after {
  content: '';
  position: absolute;
  inset: -6px;
  border-radius: 12px;
  border: 1px solid var(--seal-listening-line);
  animation: recorder-ring 1.6s var(--ease) infinite;
}

@keyframes recorder-ring {
  from { transform: scale(0.94); opacity: 0.8; }
  to { transform: scale(1.22); opacity: 0; }
}

.recorder-readout {
  display: grid;
  gap: 8px;
  min-width: 160px;
}

.recorder-timer {
  font-size: 28px;
  line-height: 1;
  letter-spacing: 0.04em;
  color: var(--text-1);
}

.recorder-status {
  white-space: normal;
}

.recorder-wave {
  display: flex;
  align-items: center;
  gap: 3px;
  height: 28px;
  min-width: 0;
  overflow: hidden;
}

.recorder-wave i {
  width: 2px;
  flex: none;
  border-radius: 2px;
  background: var(--gray-600);
  transition: height 120ms ease-out;
}

.recorder-wave.live i {
  background: var(--seal-listening-line);
}

.recorder-side {
  margin-left: auto;
  display: flex;
  flex-wrap: wrap;
  justify-content: flex-end;
  gap: 8px;
  min-width: 0;
}

.recorder-chip {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  height: 28px;
  padding: 0 12px 0 10px;
  border: 1px solid var(--line-strong);
  border-radius: 999px;
  background: transparent;
  color: var(--text-2);
  font: inherit;
  font-size: var(--fs-cap);
  white-space: nowrap;
  cursor: pointer;
  transition: background-color var(--t-fast) var(--ease), color var(--t-fast) var(--ease), border-color var(--t-fast) var(--ease);
}

.recorder-chip:hover {
  background: var(--bg-raise);
  color: var(--text-1);
}

.recorder-chip:focus-visible {
  outline: none;
  box-shadow: var(--focus-ring);
}

.recorder-live {
  max-height: 220px;
  margin-top: var(--s-4);
  padding: var(--s-4);
  overflow-y: auto;
  background: var(--bg-card);
  border: 1px solid var(--line);
  border-radius: var(--r-md);
}

.recorder-live-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-3);
  margin-bottom: var(--s-2);
}

.recorder-live-text {
  margin: 0;
  font-size: var(--fs-md);
  line-height: 1.75;
  color: var(--text-1);
  white-space: pre-wrap;
  word-break: break-word;
}

.recorder-live-text.converting {
  color: var(--text-3);
}

.recorder-live-placeholder,
.recorder-live-raw {
  margin: 0;
  font-size: var(--fs-sm);
  line-height: 1.7;
  color: var(--text-3);
}

.recorder-live-raw {
  margin-top: var(--s-2);
}

.recorder-live-error {
  margin: var(--s-2) 0 0;
  font-size: var(--fs-sm);
  color: var(--err);
}

@media (max-width: 860px) {
  .recorder-wave {
    display: none;
  }
}

@media (prefers-reduced-motion: reduce) {
  .recorder-seal[data-seal='listening']::after {
    animation: none;
  }
}
</style>
