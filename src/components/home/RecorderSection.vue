<template>
  <div class="recorder-section">
    <div class="recorder-controls">
      <button
        class="record-btn"
        :class="{ recording: isRecording }"
        type="button"
        @click="toggleRecording"
      >
        <span class="btn-icon">●</span>
        <span class="btn-text">{{ buttonText }}</span>
      </button>

      <div class="recording-status" :class="{ recording: isRecording }">
        <span class="status-dot"></span>
        <span class="status-text status-ready" :class="{ hidden: isRecording }">准备就绪</span>
        <span class="status-text status-recording" :class="{ visible: isRecording }">录音中</span>
        <span class="duration" :class="{ visible: isRecording }">{{ formattedDuration }}</span>
      </div>
    </div>
  </div>
</template>

<script setup>
import { computed } from 'vue'
import { useRecorder } from '@/composables/useRecorder'

const { isRecording, duration, startRecording, stopRecording } = useRecorder()

const buttonText = computed(() => {
  if (isRecording.value) return '停止录音'
  return '开始录音'
})

const formattedDuration = computed(() => {
  const mins = Math.floor(duration.value / 60)
  const secs = duration.value % 60
  return `${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`
})

function toggleRecording() {
  if (isRecording.value) {
    stopRecording()
    return
  }

  startRecording()
}
</script>
