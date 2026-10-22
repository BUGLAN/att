<template>
  <section class="recorder-stage">
    <!-- 首页页头：统一由 PageHeader 提供，卡片内不再重复放标题 -->
    <PageHeader
      title="录音"
      subtitle="按下录音按钮开始语音输入；转写、优化与历史记录都会在下方实时更新。"
    >
      <template #actions>
        <BaseButton
          v-if="showServiceButton"
          size="sm"
          pill
          class="service-toggle"
          :active="serviceReady"
          :loading="serviceLoading"
          :disabled="serviceButtonDisabled"
          @click="toggleModelService"
        >
          <template #icon>
            <span class="service-toggle-dot"></span>
          </template>
          {{ serviceButtonLabel }}
        </BaseButton>

        <div class="stage-badges">
          <BaseBadge tone="accent" uppercase>{{ modeBadge }}</BaseBadge>
          <BaseBadge :tone="statusTone" uppercase>{{ statusBadge }}</BaseBadge>
        </div>
      </template>
    </PageHeader>

    <div class="stage-shell">
      <div class="stage-grid">
        <article class="recorder-panel" :class="{ active: isRecording }">
          <div class="panel-aura"></div>

          <div class="panel-core">
            <button
              class="recorder-toggle"
              :class="{ active: isRecording }"
              type="button"
              :aria-label="buttonText"
              :disabled="recordingButtonDisabled"
              @click="toggleRecording"
            >
              <span class="toggle-ring"></span>
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path
                  d="M12 15.5a3.5 3.5 0 0 0 3.5-3.5V7a3.5 3.5 0 1 0-7 0v5a3.5 3.5 0 0 0 3.5 3.5Z"
                  fill="currentColor"
                />
                <path
                  d="M7 11.75a.75.75 0 0 1 .75.75 4.25 4.25 0 0 0 8.5 0 .75.75 0 0 1 1.5 0 5.76 5.76 0 0 1-5 5.69V20h2a.75.75 0 0 1 0 1.5h-6.5a.75.75 0 0 1 0-1.5h3v-1.81a5.76 5.76 0 0 1-5-5.69.75.75 0 0 1 .75-.75Z"
                  fill="currentColor"
                />
              </svg>
            </button>

            <div class="stage-waveform" aria-hidden="true">
              <span
                v-for="(barHeight, index) in waveformHeights"
                :key="index"
                class="waveform-bar"
                :style="{ height: `${barHeight}px` }"
              ></span>
            </div>

            <div class="panel-meta">
              <strong class="panel-time">{{ formattedDuration }}</strong>
              <p class="panel-status">{{ panelStatus }}</p>
              <p class="panel-hint">{{ panelHint }}</p>
            </div>
          </div>
        </article>

        <article class="live-panel">
          <div class="live-header">
            <div class="live-heading">
              <h2 class="live-title">实时转录</h2>
            </div>
            <div class="live-indicator">
              <span class="live-dot"></span>
              <span>{{ liveIndicatorText }}</span>
            </div>
          </div>

          <div ref="liveScroll" class="live-content" @scroll="handleLiveScroll">
            <template v-if="showOptimizationStream">
              <div class="stream-status">
                <span v-if="optimizationStream.isWaitingForFirstToken" class="stream-spinner" aria-hidden="true"></span>
                <span>{{ optimizationStatusText }}</span>
                <BaseButton
                  v-if="canCancelOptimization"
                  variant="ghost"
                  size="sm"
                  class="stream-cancel"
                  @click="cancelOptimization(optimizationStream.recordId)"
                >
                  取消
                </BaseButton>
              </div>

              <p v-if="optimizationStream.text" class="live-transcript streaming-transcript">
                {{ optimizationStream.text }}<span
                  v-if="optimizationStream.phase === 'streaming'"
                  class="typing-cursor"
                  aria-hidden="true"
                ></span>
              </p>
              <p v-else class="live-placeholder">{{ optimizationEmptyText }}</p>

              <p v-if="optimizationStream.phase === 'error'" class="stream-error">
                {{ optimizationStream.error }}
              </p>
              <p
                v-if="optimizationStream.phase === 'waiting' && displayTranscript"
                class="stream-raw-preview"
              >
                原始转写：{{ displayTranscript }}
              </p>
            </template>

            <template v-else>
              <p v-if="displayTranscript" class="live-transcript">{{ displayTranscript }}</p>
              <p v-else class="live-placeholder">{{ transcriptPlaceholder }}</p>
            </template>
          </div>

          <div class="live-footer">
            <span class="live-footer-item">状态 {{ sessionStatusShort }}</span>
            <span class="live-footer-item">片段 {{ segmentCountText }}</span>
            <span class="live-footer-item">模式 {{ liveModeText }}</span>
          </div>
        </article>
      </div>
    </div>
  </section>
</template>

<script setup>
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useRecorder } from '@/composables/useRecorder'
import { deriveStreamingProviderLabel } from '@/composables/recorder/streamProtocol.mjs'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import { useOptimizationStreamStore } from '@/stores/optimizationStream'
import { shouldAutoScroll } from '@/composables/recorder/streamScroll.mjs'
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import PageHeader from '@/components/common/PageHeader.vue'

const {
  isRecording,
  isTranscribing,
  duration,
  audioLevel,
  liveTranscript,
  livePhase,
  processedSegmentCount,
  sessionStatusText,
  isMultiModeEnabled,
  isStreamingModeEnabled,
  startRecording,
  stopRecording,
  cancelOptimization
} = useRecorder()
const appStore = useAppStore()
const settingsStore = useSettingsStore()
const optimizationStream = useOptimizationStreamStore()
const serviceActionPending = ref(false)
const liveScroll = ref(null)
const autoScrollEnabled = ref(true)

// 真流式 provider 的展示名（Qwen 本地 / 豆包在线），文案不再写死 Qwen
const streamingProviderLabel = computed(() => (
  deriveStreamingProviderLabel(settingsStore.settings.asr_provider)
))

const showServiceButton = computed(() => true)
const serviceLoading = computed(() => (
  serviceActionPending.value || settingsStore.runtimeStatus.model_service_loading
))
const serviceReady = computed(() => settingsStore.runtimeStatus.model_service_ready)
const serviceRunning = computed(() => settingsStore.runtimeStatus.model_service_running)
const serviceSupported = computed(() => settingsStore.runtimeStatus.model_service_supported)
const streamingAvailable = computed(() => settingsStore.runtimeStatus.streaming_available)
// 在线 provider（豆包）：流式不可用时缺的是凭据，不是「模型没开」，
// 按钮文案要给用户正确的下一步（去设置页填 Token）
const streamingNeedsCredentials = computed(() => (
  isStreamingModeEnabled.value &&
  !streamingAvailable.value &&
  settingsStore.runtimeStatus.model_service_mode === 'online'
))
const serviceButtonDisabled = computed(() => (
  serviceLoading.value || isRecording.value || isTranscribing.value
))
const recordingButtonDisabled = computed(() => (
  !isRecording.value &&
  (!serviceReady.value || (isStreamingModeEnabled.value && !streamingAvailable.value))
))
const buttonText = computed(() => {
  if (isRecording.value) return '结束录音'
  if (serviceLoading.value) return '模型加载中'
  if (!serviceSupported.value) return '当前模式不支持开启模型'
  if (streamingNeedsCredentials.value) return '请先配置 ASR 密钥'
  if (isStreamingModeEnabled.value && serviceReady.value && !streamingAvailable.value) return '当前环境暂不支持流式'
  if (recordingButtonDisabled.value) return '请先开启模型'
  return '开始录音'
})
const serviceButtonLabel = computed(() => {
  if (!serviceSupported.value) return '当前模式不可用'
  if (serviceLoading.value) return '模型加载中'
  if (serviceReady.value) return '模型已开启'
  if (serviceRunning.value) return '模型预热中'
  return '开启模型'
})

const modeBadge = computed(() => {
  if (isStreamingModeEnabled.value) return streamingProviderLabel.value
  if (isMultiModeEnabled.value) return '多段准实时'
  return '单段录音'
})

const liveModeText = computed(() => {
  if (isStreamingModeEnabled.value) return '连续流式'
  if (isMultiModeEnabled.value) return '自动切段'
  return '完整录音'
})

const statusBadge = computed(() => {
  if (isRecording.value) return '录音中'
  if (optimizationStream.isActive) return '优化中'
  if (isTranscribing.value) return '处理中'
  return '就绪'
})

// 状态徽章的语义色：录音=错误红（醒目）、转写=警告黄、就绪=成功绿
const statusTone = computed(() => {
  if (isRecording.value) return 'err'
  if (isTranscribing.value || livePhase.value === 'optimizing') return 'warn'
  return 'ok'
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

const panelHint = computed(() => {
  if (isStreamingModeEnabled.value) {
    return `真流式模式会持续发送 16k PCM 音频，并实时刷新${streamingProviderLabel.value}转写结果。`
  }

  if (isMultiModeEnabled.value) {
    return '说完一句停顿片刻，系统会自动切段并实时追加文本。'
  }

  return '单段模式会在结束录音后统一上传并转写。'
})

const waveformHeights = computed(() => {
  const base = [10, 22, 34, 48, 60, 46, 30, 20, 12]
  const amplitude = isRecording.value
    ? ((isMultiModeEnabled.value || isStreamingModeEnabled.value) ? audioLevel.value : 0.72)
    : 0.18

  return base.map((height, index) => {
    const pulse = ((index % 3) + 1) * 4
    return Math.max(8, Math.round(height * (0.42 + amplitude * 0.85) + pulse))
  })
})

const displayTranscript = computed(() => liveTranscript.value.trim())

const sessionStatusShort = computed(() => sessionStatusText.value || '待开始')

const segmentCountText = computed(() => {
  if (isStreamingModeEnabled.value) {
    if (isRecording.value) return '流式会话进行中'
    if (livePhase.value === 'optimizing') return '后台优化中'
    if (isTranscribing.value) return '流式结果收尾中'
    return '流式模式'
  }

  if (!isMultiModeEnabled.value) {
    if (isRecording.value) return '整段采集中'
    if (isTranscribing.value) return '整段处理中'
    return '整段模式'
  }

  return `${processedSegmentCount.value} 段`
})

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

const liveIndicatorText = computed(() => {
  if (isRecording.value && isStreamingModeEnabled.value) return 'LIVE STREAM'
  if (isRecording.value && isMultiModeEnabled.value) return 'LIVE AI'
  if (isRecording.value) return 'RECORDING'
  if (optimizationStream.isActive) return 'STREAMING'
  if (isTranscribing.value) return 'PROCESSING'
  return 'STANDBY'
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

async function toggleModelService() {
  if (serviceButtonDisabled.value) {
    return
  }

  if (!serviceSupported.value) {
    appStore.showToast(
      settingsStore.runtimeStatus.model_service_error ||
      settingsStore.runtimeStatus.streaming_error ||
      '当前模式不支持开启模型'
    )
    return
  }

  serviceActionPending.value = true
  try {
    if (serviceReady.value || serviceRunning.value) {
      await settingsStore.stopStreamingService()
      return
    }

    await settingsStore.startStreamingService(false)
  } catch (error) {
    appStore.showToast(error.message || '模型服务启动失败')
    console.error('[RecorderSection] Failed to toggle streaming service:', error)
  } finally {
    serviceActionPending.value = false
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
.recorder-stage {
  width: 100%;
  margin-bottom: var(--s-6);
}

.stage-shell {
  position: relative;
  overflow: hidden;
  border: 1px solid var(--line);
  border-radius: var(--r-xl);
  padding: var(--s-4);
  background: var(--bg-surface);
  box-shadow: var(--edge-hi);
}

.stage-badges {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  flex-wrap: wrap;
  justify-content: flex-end;
}

/* 服务开关：在原语基线上保留胶囊 + 状态点 */
.service-toggle-dot {
  width: 8px;
  height: 8px;
  border-radius: var(--r-full);
  background: var(--gray-300);
  box-shadow: 0 0 0 4px var(--line-soft);
}

.service-toggle.is-active .service-toggle-dot {
  background: var(--accent);
  box-shadow: 0 0 0 4px var(--accent-dim);
}

.stage-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.35fr) minmax(320px, 1fr);
  gap: var(--s-3);
  align-items: stretch;
}

.recorder-panel,
.live-panel {
  position: relative;
  overflow: hidden;
  min-height: 340px;
  border-radius: var(--r-lg);
}

.recorder-panel {
  display: flex;
  flex-direction: column;
  padding: var(--s-4);
  border: 1px solid var(--line);
  background: var(--grad-card);
}

.recorder-panel.active {
  box-shadow:
    inset 0 0 0 1px var(--accent-tint),
    var(--e-hover);
  border-color: var(--accent-line);
}

.panel-aura {
  position: absolute;
  inset: 0;
  background: radial-gradient(circle at center, var(--accent-halo), transparent 62%);
  opacity: 0;
  transition: opacity var(--t-base) var(--ease);
  pointer-events: none;
}

.recorder-panel.active .panel-aura {
  opacity: 1;
}

.panel-core {
  position: relative;
  z-index: 1;
  display: flex;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: var(--s-3) 0 var(--s-1);
}

.recorder-toggle {
  position: relative;
  z-index: 1;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 104px;
  height: 104px;
  border: 1px solid var(--accent-tint);
  border-radius: var(--r-full);
  color: var(--text-1);
  background:
    radial-gradient(circle at 30% 30%, var(--accent-tint), var(--bg-raise) 72%);
  box-shadow:
    var(--e-hover),
    var(--edge-hi);
  cursor: pointer;
  transition:
    transform var(--t-base) var(--ease),
    box-shadow var(--t-base) var(--ease),
    border-color var(--t-base) var(--ease),
    color var(--t-base) var(--ease);
}

.recorder-toggle:hover:not(:disabled) {
  transform: translateY(-2px);
  border-color: var(--accent-line);
  color: var(--accent-hi);
}

.recorder-toggle:disabled {
  cursor: not-allowed;
  opacity: 0.55;
  transform: none;
  box-shadow: var(--e-1);
}

.recorder-toggle:disabled:hover {
  transform: none;
  border-color: var(--accent-tint);
  color: var(--text-1);
}

.recorder-toggle:active:not(:disabled) {
  transform: translateY(1px);
}

.recorder-toggle:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 4px;
}

.toggle-ring {
  position: absolute;
  inset: -10px;
  border: 1px solid var(--line-strong);
  border-radius: inherit;
  opacity: 0.5;
  transition: transform var(--t-base) var(--ease), opacity var(--t-base) var(--ease);
}

.recorder-toggle.active {
  color: var(--accent);
  border-color: var(--accent-line-strong);
  box-shadow:
    0 0 0 8px var(--accent-halo),
    var(--e-hover);
}

.recorder-toggle.active .toggle-ring {
  transform: scale(1.04);
  opacity: 1;
  border-color: var(--accent-line);
}

.recorder-toggle svg {
  width: 38px;
  height: 38px;
}

.stage-waveform {
  display: flex;
  align-items: end;
  justify-content: center;
  gap: var(--s-1);
  width: 100%;
  max-width: 220px;
  height: 44px;
  margin-top: var(--s-3);
}

.waveform-bar {
  width: 3px;
  border-radius: var(--r-full);
  background: linear-gradient(180deg, var(--gray-400), var(--accent));
  transition: height var(--t-fast) var(--ease), opacity var(--t-fast) var(--ease);
}

.panel-meta {
  position: relative;
  z-index: 1;
  margin-top: var(--s-3);
  text-align: center;
}

/* 计时器是这一屏的视觉主角：等宽数字 + 收紧字距，
   读数变化时不会左右跳动。 */
.panel-time {
  display: block;
  font-family: var(--font-mono);
  font-size: clamp(var(--fs-2xl), 3vw, 2.75rem);
  font-weight: 700;
  letter-spacing: -0.04em;
  color: var(--text-1);
  font-variant-numeric: tabular-nums;
}

.panel-status {
  margin: var(--s-2) 0 var(--s-1);
  font-size: var(--fs-cap);
  font-weight: 700;
  letter-spacing: 0.16em;
  text-transform: uppercase;
  color: var(--accent);
}

.panel-hint {
  margin: 0;
  max-width: 320px;
  color: var(--text-3);
  font-size: var(--fs-cap);
  line-height: 1.55;
}

.live-panel {
  display: flex;
  flex-direction: column;
  padding: var(--s-4);
  border: 1px solid var(--line);
  background: var(--grad-card);
}

.live-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-2);
}

.live-heading {
  min-width: 0;
}

.live-title {
  margin: 0;
  font-size: var(--fs-md);
  font-weight: 700;
  letter-spacing: -0.02em;
  color: var(--text-1);
}

.live-indicator {
  display: inline-flex;
  align-items: center;
  gap: var(--s-2);
  font-size: var(--fs-cap);
  font-weight: 700;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--text-3);
}

.live-dot {
  width: 8px;
  height: 8px;
  border-radius: var(--r-full);
  background: var(--accent);
  box-shadow: 0 0 12px var(--accent-glow);
  animation: blinkDot 1.6s ease-in-out infinite;
}

.live-content {
  flex: 1;
  margin: var(--s-3) 0;
  padding: var(--s-4);
  border-radius: var(--r-md);
  background: var(--bg-base);
  border: 1px solid var(--line);
  overflow: auto;
  box-shadow: var(--edge-hi);
}

.live-transcript,
.live-placeholder {
  margin: 0;
  font-size: var(--fs-base);
  line-height: 1.7;
  white-space: pre-wrap;
  word-break: break-word;
}

.live-transcript {
  color: var(--text-1);
}

.live-placeholder {
  color: var(--text-3);
}

.stream-status {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  margin-bottom: var(--s-3);
  padding-bottom: var(--s-2);
  border-bottom: 1px dashed var(--line);
  color: var(--accent);
  font-size: var(--fs-cap);
  font-weight: 600;
  letter-spacing: 0.04em;
}

.stream-cancel {
  margin-left: auto;
}

.stream-spinner {
  width: 12px;
  height: 12px;
  border: 1.5px solid var(--line-soft-strong);
  border-top-color: var(--accent);
  border-radius: var(--r-full);
  animation: streamSpin 0.9s linear infinite;
}

.streaming-transcript {
  min-height: 1.7em;
}

.typing-cursor {
  display: inline-block;
  width: 2px;
  height: 1.1em;
  margin-left: 2px;
  vertical-align: text-bottom;
  border-radius: 1px;
  background: var(--accent);
  animation: cursorBlink 0.9s steps(1) infinite;
}

.stream-error {
  margin: var(--s-3) 0 0;
  color: var(--err);
  font-size: var(--fs-cap);
  line-height: 1.5;
}

.stream-raw-preview {
  margin: var(--s-3) 0 0;
  padding-top: var(--s-3);
  border-top: 1px dashed var(--line);
  color: var(--text-3);
  font-size: var(--fs-cap);
  line-height: 1.6;
  white-space: pre-wrap;
}

@keyframes streamSpin {
  to {
    transform: rotate(360deg);
  }
}

@keyframes cursorBlink {
  0%,
  49% {
    opacity: 1;
  }

  50%,
  100% {
    opacity: 0;
  }
}

.live-footer {
  display: flex;
  flex-wrap: wrap;
  gap: var(--s-2);
  padding-top: var(--s-3);
  border-top: 1px solid var(--line);
}

.live-footer-item {
  color: var(--text-2);
  font-size: var(--fs-cap);
  font-weight: 500;
  padding: 3px var(--s-2);
  border-radius: var(--r-full);
  background: var(--bg-raise);
  border: 1px solid var(--line);
}

@keyframes blinkDot {
  0%,
  100% {
    opacity: 1;
  }
  50% {
    opacity: 0.4;
  }
}

@media (prefers-reduced-motion: reduce) {
  .live-dot {
    animation: none;
  }
}

@media (max-width: 1040px) {
  .stage-grid {
    grid-template-columns: 1fr;
  }

  .recorder-panel,
  .live-panel {
    min-height: auto;
  }
}

@media (max-width: 720px) {
  .stage-shell {
    padding: var(--s-3);
    border-radius: var(--r-lg);
  }

  .live-header {
    flex-direction: column;
    align-items: flex-start;
  }

  .recorder-panel {
    padding: var(--s-3);
  }

  .recorder-toggle {
    width: 92px;
    height: 92px;
  }

  .recorder-toggle svg {
    width: 34px;
    height: 34px;
  }

  .panel-time {
    font-size: var(--fs-2xl);
  }

  .live-content {
    padding: var(--s-3);
  }
}

@media (max-width: 560px) {
  .stage-badges {
    justify-content: flex-start;
  }

  .stage-waveform {
    max-width: 100%;
  }

  .live-footer {
    gap: var(--s-1);
  }
}
</style>
