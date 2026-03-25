<template>
  <div class="recording-status" :class="{ recording: isRecording }">
    <span class="status-dot"></span>
    <span class="status-text status-ready" :class="{ hidden: isRecording }">
      {{ readyText }}
    </span>
    <span class="status-text status-recording" :class="{ visible: isRecording }">
      {{ recordingText }}
    </span>
    <span class="duration" :class="{ visible: showDuration }">
      {{ formattedDuration }}
    </span>
  </div>
</template>

<script setup>
import { computed } from 'vue'

const props = defineProps({
  isRecording: {
    type: Boolean,
    default: false
  },
  duration: {
    type: Number,
    default: 0
  },
  showDuration: {
    type: Boolean,
    default: false
  },
  readyText: {
    type: String,
    default: '准备就绪'
  },
  recordingText: {
    type: String,
    default: '录音中'
  }
})

const formattedDuration = computed(() => {
  const mins = Math.floor(props.duration / 60)
  const secs = props.duration % 60
  return `${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`
})
</script>
