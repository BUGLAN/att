<template>
  <div class="floating-shell">
    <section class="floating-pill" :data-phase="phase">
      <div class="status-indicator">
        <div class="status-icon" :class="{ pulsing: phase === 'recording' }" :data-phase="phase">
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
        </div>
      </div>

      <div
        v-if="showWaveform"
        class="waveform"
        :class="{ active: phase === 'recording' }"
        aria-hidden="true"
      >
        <span v-for="bar in 5" :key="bar" class="wave-bar"></span>
      </div>

      <div v-else class="spinner" aria-hidden="true"></div>

      <div class="content">
        <p v-if="showScrollingTranscript" class="primary-line primary-line-streaming">
          <span class="prefix-text">{{ prefixText }}</span>
          <span ref="streamingViewport" class="streaming-viewport">
            <span class="highlight streaming-text">{{ contentText }}</span>
          </span>
        </p>
        <p v-else class="primary-line">
          {{ prefixText }}<span class="highlight">{{ contentText }}</span>
        </p>
      </div>

      <div class="actions">
        <BaseButton
          variant="ghost"
          icon
          size="sm"
          class="action-button"
          style="--ui-icon-size: 24px"
          title="打开设置"
          @click="openSettings"
        >
          <template #icon>
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path
                d="m19.14 12.94.04-.94-.04-.94 2.03-1.58a.5.5 0 0 0 .12-.64l-1.92-3.32a.5.5 0 0 0-.6-.22l-2.39.96a7.4 7.4 0 0 0-1.63-.94l-.36-2.54a.5.5 0 0 0-.49-.42h-3.84a.5.5 0 0 0-.49.42l-.36 2.54c-.58.23-1.13.54-1.63.94l-2.39-.96a.5.5 0 0 0-.6.22L2.67 8.84a.5.5 0 0 0 .12.64l2.03 1.58-.04.94.04.94-2.03 1.58a.5.5 0 0 0-.12.64l1.92 3.32a.5.5 0 0 0 .6.22l2.39-.96c.5.4 1.05.71 1.63.94l.36 2.54a.5.5 0 0 0 .49.42h3.84a.5.5 0 0 0 .49-.42l.36-2.54c.58-.23 1.13-.54 1.63-.94l2.39.96a.5.5 0 0 0 .6-.22l1.92-3.32a.5.5 0 0 0-.12-.64l-2.03-1.58ZM12 15.5A3.5 3.5 0 1 1 12 8a3.5 3.5 0 0 1 0 7.5Z"
                fill="currentColor"
              />
            </svg>
          </template>
        </BaseButton>
        <BaseButton
          variant="ghost"
          icon
          size="sm"
          class="action-button close"
          style="--ui-icon-size: 24px"
          title="关闭悬浮窗"
          @click="closeWindow"
        >
          <template #icon>
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path
                d="M6.53 5.47a.75.75 0 0 1 1.06 0L12 9.88l4.41-4.41a.75.75 0 1 1 1.06 1.06L13.06 11l4.41 4.41a.75.75 0 0 1-1.06 1.06L12 12.06l-4.41 4.41a.75.75 0 1 1-1.06-1.06L10.94 11 6.53 6.59a.75.75 0 0 1 0-1.12Z"
                fill="currentColor"
              />
            </svg>
          </template>
        </BaseButton>
      </div>
    </section>
  </div>
</template>

<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import BaseButton from '@/components/common/BaseButton.vue'

const AUTO_HIDE_MS = 800

const phase = ref('idle')
const text = ref('')
const mode = ref('single')
const streamingViewport = ref(null)
let hideTimer = null
let scrollAnimationFrame = null

const showWaveform = computed(() => (
  phase.value === 'recording'
  || phase.value === 'idle'
  || phase.value === 'complete'
))

const prefixText = computed(() => {
  if (phase.value === 'recording' && ['multi', 'streaming'].includes(mode.value)) return '实时记录：'
  if (phase.value === 'recording') return '正在录音：'
  if (phase.value === 'transcribing') return '正在转写：'
  if (phase.value === 'optimizing') return '正在优化：'
  if (phase.value === 'complete') return '转写完成：'
  if (phase.value === 'error') return '处理失败：'
  return '正在录音：'
})

const contentText = computed(() => {
  if (phase.value === 'recording' && ['multi', 'streaming'].includes(mode.value)) return text.value || '正在聆听...'
  if (phase.value === 'recording') return '结束录音后将开始转写...'
  if (phase.value === 'transcribing') return '正在生成转写结果...'
  if (phase.value === 'optimizing') return 'AI 正在整理文本...'
  if (phase.value === 'complete') return text.value || '文本已生成'
  if (phase.value === 'error') return text.value || '请稍后重试'
  return '实时内容正在生成中...'
})

const showScrollingTranscript = computed(() => (
  phase.value === 'recording' && ['multi', 'streaming'].includes(mode.value) && Boolean(text.value)
))

function clearHideTimer() {
  if (!hideTimer) {
    return
  }

  clearTimeout(hideTimer)
  hideTimer = null
}

function clearScrollAnimation() {
  if (scrollAnimationFrame !== null) {
    cancelAnimationFrame(scrollAnimationFrame)
    scrollAnimationFrame = null
  }
}

function scheduleHide() {
  clearHideTimer()
  hideTimer = setTimeout(() => {
    window.floatingAPI?.close()
  }, AUTO_HIDE_MS)
}

function applyState(payload = {}) {
  phase.value = payload.phase || 'idle'
  text.value = payload.text || ''
  mode.value = payload.mode || 'single'

  if (phase.value === 'complete' || phase.value === 'error') {
    scheduleHide()
    return
  }

  clearHideTimer()
}

function closeWindow() {
  window.floatingAPI?.close()
}

function openSettings() {
  window.floatingAPI?.openSettings()
}

function syncStreamingViewport() {
  const viewport = streamingViewport.value
  if (!viewport) {
    return
  }

  const targetScrollLeft = Math.max(0, viewport.scrollWidth - viewport.clientWidth)
  const startScrollLeft = viewport.scrollLeft

  if (Math.abs(targetScrollLeft - startScrollLeft) < 1) {
    viewport.scrollLeft = targetScrollLeft
    return
  }

  clearScrollAnimation()

  const startTime = performance.now()
  const duration = 220

  const step = (timestamp) => {
    const elapsed = Math.min((timestamp - startTime) / duration, 1)
    const eased = 1 - ((1 - elapsed) ** 3)

    viewport.scrollLeft = startScrollLeft + ((targetScrollLeft - startScrollLeft) * eased)

    if (elapsed < 1) {
      scrollAnimationFrame = requestAnimationFrame(step)
      return
    }

    scrollAnimationFrame = null
  }

  scrollAnimationFrame = requestAnimationFrame(step)
}

watch([text, phase, mode], async () => {
  if (!showScrollingTranscript.value) {
    return
  }

  await nextTick()
  syncStreamingViewport()
})

onMounted(() => {
  window.floatingAPI?.onStateChange((payload) => {
    applyState(payload)
  })

  nextTick(() => {
    syncStreamingViewport()
  })
})

onUnmounted(() => {
  clearHideTimer()
  clearScrollAnimation()
  window.floatingAPI?.removeAllListeners('floating:state')
})
</script>

<style scoped>
/* 关闭按钮保留"悬停变红"的语义（用足够高的复合选择器覆盖原语幽灵态） */
.floating-pill .action-button.close:hover:not(:disabled) {
  color: var(--err);
  background: var(--err-soft);
}

/* 悬浮窗图标按钮使用实心图标，需要覆盖原语的描边式 svg 规则 */
.floating-pill .action-button :deep(svg) {
  fill: currentColor;
  stroke: none;
}
</style>
