<template>
  <section class="recorder-stage">
    <div class="stage-shell">
      <div class="stage-header">
        <div class="stage-heading">
          <h1 class="stage-title">录音</h1>
        </div>
        <div class="stage-actions">
          <button
            v-if="showServiceButton"
            class="service-toggle"
            :class="{
              active: serviceReady,
              loading: serviceLoading
            }"
            type="button"
            :disabled="serviceButtonDisabled"
            @click="toggleStreamingService"
          >
            <span class="service-toggle-dot"></span>
            <span>{{ serviceButtonLabel }}</span>
          </button>

          <div class="stage-badges">
            <span class="stage-badge primary">{{ modeBadge }}</span>
            <span class="stage-badge">{{ statusBadge }}</span>
          </div>
        </div>
      </div>

      <div class="stage-grid">
        <article class="recorder-panel" :class="{ active: isRecording }">
          <div class="panel-aura"></div>

          <div class="panel-summary">
            <span class="panel-summary-item">{{ modeBadge }}</span>
            <span class="panel-summary-item">{{ sessionStatusShort }}</span>
          </div>

          <div class="panel-core">
            <button
              class="recorder-toggle"
              :class="{ active: isRecording }"
              type="button"
              :aria-label="buttonText"
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

          <div class="live-content">
            <p v-if="displayTranscript" class="live-transcript">{{ displayTranscript }}</p>
            <p v-else class="live-placeholder">{{ transcriptPlaceholder }}</p>
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
import { computed, onMounted, ref, watch } from 'vue'
import { useRecorder } from '@/composables/useRecorder'
import { useSettingsStore } from '@/stores/settings'

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
  stopRecording
} = useRecorder()
const settingsStore = useSettingsStore()
const serviceActionPending = ref(false)

const buttonText = computed(() => (isRecording.value ? '结束录音' : '开始录音'))
const showServiceButton = computed(() => settingsStore.settings.asr_provider === 'qwen_asr')
const serviceLoading = computed(() => (
  serviceActionPending.value || settingsStore.runtimeStatus.qwen_streaming_service_loading
))
const serviceReady = computed(() => settingsStore.runtimeStatus.qwen_streaming_service_ready)
const serviceRunning = computed(() => settingsStore.runtimeStatus.qwen_streaming_service_running)
const serviceButtonDisabled = computed(() => (
  serviceLoading.value || isRecording.value || isTranscribing.value
))
const serviceButtonLabel = computed(() => {
  if (serviceLoading.value) return '模型服务加载中'
  if (serviceReady.value) return '关闭模型服务'
  if (serviceRunning.value) return '模型服务预热中'
  return '启动模型服务'
})

const modeBadge = computed(() => {
  if (isStreamingModeEnabled.value) return 'Qwen 真流式'
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
  if (isTranscribing.value) return '处理中'
  return '就绪'
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
  return '等待开始录音'
})

const panelHint = computed(() => {
  if (isStreamingModeEnabled.value) {
    return '真流式模式会持续发送 16k PCM 音频，并实时刷新 Qwen 转写结果。'
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
    if (isRecording.value) {
      return 'Qwen 真流式模式已启用，这里会持续刷新当前转写文本。'
    }
    return '切换到 Qwen 真流式模式后，这里会显示连续更新的实时文本。'
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
  if (isRecording.value && isStreamingModeEnabled.value) return 'QWEN STREAM'
  if (isRecording.value && isMultiModeEnabled.value) return 'LIVE AI'
  if (isRecording.value) return 'RECORDING'
  if (isTranscribing.value) return 'PROCESSING'
  return 'STANDBY'
})

async function refreshStreamingServiceStatus(probeRuntime = true) {
  if (settingsStore.settings.asr_provider !== 'qwen_asr') {
    return
  }

  try {
    await settingsStore.loadRuntimeStatus(probeRuntime)
  } catch (error) {
    console.error('[RecorderSection] Failed to refresh streaming service status:', error)
  }
}

async function toggleStreamingService() {
  if (serviceButtonDisabled.value) {
    return
  }

  serviceActionPending.value = true
  try {
    if (serviceReady.value || serviceRunning.value) {
      await settingsStore.stopStreamingService()
      return
    }

    await settingsStore.startStreamingService(true)
  } catch (error) {
    console.error('[RecorderSection] Failed to toggle streaming service:', error)
  } finally {
    serviceActionPending.value = false
  }
}

function toggleRecording() {
  if (isRecording.value) {
    stopRecording()
    return
  }

  startRecording()
}

onMounted(() => {
  if (showServiceButton.value) {
    void refreshStreamingServiceStatus(true)
  }
})

watch(
  () => [settingsStore.settings.asr_provider, settingsStore.settings.recording_mode],
  ([provider, mode]) => {
    if (provider === 'qwen_asr') {
      void refreshStreamingServiceStatus(true)
    }
  },
  { immediate: false }
)
</script>

<style scoped>
.recorder-stage {
  width: 100%;
  margin-bottom: 20px;
}

.stage-shell {
  position: relative;
  overflow: hidden;
  border: 1px solid var(--border);
  border-radius: var(--radius-xl);
  padding: 18px;
  background: var(--bg-secondary);
  box-shadow: var(--shadow-md);
}

.stage-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 14px;
}

.stage-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 10px;
  flex-wrap: wrap;
}

.stage-heading {
  min-width: 0;
}

.stage-title {
  margin: 0;
  font-size: clamp(1.05rem, 2vw, 1.35rem);
  font-weight: 700;
  letter-spacing: -0.03em;
  color: var(--text-primary);
  line-height: 1.2;
}

.stage-badges {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  justify-content: flex-end;
}

.service-toggle {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 7px 12px;
  border-radius: 999px;
  border: 1px solid var(--border);
  background: var(--bg-elevated);
  color: var(--text-secondary);
  font-size: 12px;
  font-weight: 700;
  cursor: pointer;
  transition:
    border-color 180ms ease,
    color 180ms ease,
    background 180ms ease,
    transform 180ms ease;
}

.service-toggle:hover:not(:disabled) {
  transform: translateY(-1px);
  border-color: rgba(201, 162, 39, 0.28);
  color: var(--text-primary);
}

.service-toggle:disabled {
  opacity: 0.72;
  cursor: not-allowed;
}

.service-toggle.active {
  color: var(--accent);
  border-color: rgba(201, 162, 39, 0.32);
  background: rgba(201, 162, 39, 0.08);
}

.service-toggle-dot {
  width: 8px;
  height: 8px;
  border-radius: 999px;
  background: rgba(161, 161, 170, 0.85);
  box-shadow: 0 0 0 4px rgba(113, 113, 122, 0.08);
}

.service-toggle.active .service-toggle-dot {
  background: var(--accent);
  box-shadow: 0 0 0 4px rgba(201, 162, 39, 0.12);
}

.service-toggle.loading .service-toggle-dot {
  background: var(--accent);
  animation: blinkDot 1.2s ease-in-out infinite;
}

.stage-badge {
  padding: 5px 10px;
  border: 1px solid var(--border);
  border-radius: 999px;
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--text-secondary);
  background: var(--bg-elevated);
}

.stage-badge.primary {
  color: var(--accent);
  background: var(--accent-dim);
  border-color: rgba(201, 162, 39, 0.3);
}

.stage-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.35fr) minmax(320px, 1fr);
  gap: 14px;
  align-items: stretch;
}

.recorder-panel,
.live-panel {
  position: relative;
  overflow: hidden;
  min-height: 340px;
  border-radius: var(--radius-lg);
}

.recorder-panel {
  display: flex;
  flex-direction: column;
  padding: 18px;
  border: 1px solid var(--border);
  background:
    linear-gradient(180deg, rgba(31, 31, 35, 0.98), rgba(24, 24, 27, 0.98));
}

.recorder-panel.active {
  box-shadow:
    inset 0 0 0 1px rgba(201, 162, 39, 0.2),
    0 10px 22px rgba(0, 0, 0, 0.22);
  border-color: rgba(201, 162, 39, 0.24);
}

.panel-aura {
  position: absolute;
  inset: 0;
  background: radial-gradient(circle at center, rgba(201, 162, 39, 0.08), transparent 62%);
  opacity: 0;
  transition: opacity 220ms ease;
  pointer-events: none;
}

.recorder-panel.active .panel-aura {
  opacity: 1;
}

.panel-summary {
  position: relative;
  z-index: 1;
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  margin-bottom: 6px;
}

.panel-summary-item {
  padding: 4px 9px;
  border-radius: 999px;
  background: var(--bg-elevated);
  color: var(--text-secondary);
  border: 1px solid var(--border);
  font-size: 11px;
  font-weight: 700;
}

.panel-core {
  position: relative;
  z-index: 1;
  display: flex;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 12px 0 6px;
}

.recorder-toggle {
  position: relative;
  z-index: 1;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 104px;
  height: 104px;
  border: 1px solid rgba(201, 162, 39, 0.2);
  border-radius: 999px;
  color: var(--text-primary);
  background:
    radial-gradient(circle at 30% 30%, rgba(201, 162, 39, 0.24), rgba(31, 31, 35, 0.96) 72%);
  box-shadow:
    0 10px 24px rgba(0, 0, 0, 0.24),
    inset 0 1px 0 rgba(255, 255, 255, 0.06);
  cursor: pointer;
  transition:
    transform 180ms ease,
    box-shadow 180ms ease,
    border-color 180ms ease,
    color 180ms ease;
}

.recorder-toggle:hover {
  transform: translateY(-2px) scale(1.01);
  border-color: rgba(201, 162, 39, 0.34);
  color: var(--accent);
}

.recorder-toggle:active {
  transform: scale(0.97);
}

.toggle-ring {
  position: absolute;
  inset: -10px;
  border: 1px solid rgba(113, 113, 122, 0.22);
  border-radius: inherit;
  opacity: 0.5;
  transition: transform 180ms ease, opacity 180ms ease;
}

.recorder-toggle.active {
  color: var(--accent);
  border-color: rgba(201, 162, 39, 0.42);
  box-shadow:
    0 0 0 8px rgba(201, 162, 39, 0.08),
    0 14px 30px rgba(0, 0, 0, 0.28);
}

.recorder-toggle.active .toggle-ring {
  transform: scale(1.04);
  opacity: 1;
}

.recorder-toggle svg {
  width: 38px;
  height: 38px;
}

.stage-waveform {
  display: flex;
  align-items: end;
  justify-content: center;
  gap: 6px;
  width: 100%;
  max-width: 220px;
  height: 44px;
  margin-top: 14px;
}

.waveform-bar {
  width: 3px;
  border-radius: 999px;
  background: linear-gradient(180deg, rgba(161, 161, 170, 0.42), rgba(201, 162, 39, 0.92));
  transition: height 120ms ease, opacity 120ms ease;
}

.panel-meta {
  position: relative;
  z-index: 1;
  margin-top: 14px;
  text-align: center;
}

.panel-time {
  display: block;
  font-size: clamp(1.8rem, 3vw, 2.5rem);
  font-weight: 800;
  letter-spacing: -0.06em;
  color: var(--text-primary);
}

.panel-status {
  margin: 8px 0 4px;
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.16em;
  text-transform: uppercase;
  color: var(--accent);
}

.panel-hint {
  margin: 0;
  max-width: 320px;
  color: var(--text-muted);
  font-size: 12px;
  line-height: 1.55;
}

.live-panel {
  display: flex;
  flex-direction: column;
  padding: 18px;
  border: 1px solid var(--border);
  background: linear-gradient(180deg, rgba(24, 24, 27, 0.98), rgba(17, 17, 19, 0.98));
  backdrop-filter: blur(16px);
}

.live-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}

.live-heading {
  min-width: 0;
}

.live-title {
  margin: 0;
  font-size: 0.98rem;
  font-weight: 700;
  letter-spacing: -0.02em;
  color: var(--text-primary);
}

.live-indicator {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  font-size: 9px;
  font-weight: 700;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--text-muted);
}

.live-dot {
  width: 8px;
  height: 8px;
  border-radius: 999px;
  background: var(--accent);
  box-shadow: 0 0 12px rgba(201, 162, 39, 0.35);
  animation: blinkDot 1.6s ease-in-out infinite;
}

.live-content {
  flex: 1;
  margin: 14px 0 12px;
  padding: 16px;
  border-radius: 14px;
  background: var(--bg-card);
  border: 1px solid var(--border);
  overflow: auto;
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.03);
}

.live-transcript,
.live-placeholder {
  margin: 0;
  font-size: clamp(0.92rem, 1.4vw, 1rem);
  line-height: 1.66;
  white-space: pre-wrap;
  word-break: break-word;
}

.live-transcript {
  color: var(--text-primary);
}

.live-placeholder {
  color: var(--text-muted);
}

.live-footer {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  padding-top: 10px;
  border-top: 1px solid var(--border);
}

.live-footer-item {
  color: var(--text-secondary);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.02em;
  padding: 4px 9px;
  border-radius: 999px;
  background: var(--bg-elevated);
  border: 1px solid var(--border);
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
    padding: 16px;
    border-radius: 18px;
  }

  .stage-header,
  .stage-actions,
  .live-header {
    flex-direction: column;
    align-items: flex-start;
  }

  .recorder-panel {
    padding: 16px;
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
    font-size: 1.9rem;
  }

  .live-content {
    padding: 16px;
  }
}

@media (max-width: 560px) {
  .stage-header {
    align-items: flex-start;
    flex-direction: column;
  }

  .stage-badges {
    justify-content: flex-start;
  }

  .stage-waveform {
    max-width: 100%;
  }

  .live-footer {
    gap: 8px;
  }
}
</style>
