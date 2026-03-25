<template>
  <button
    class="record-btn"
    :class="{ recording: isRecording }"
    :disabled="disabled"
    @click="$emit('click')"
  >
    <span class="btn-icon">{{ icon }}</span>
    <span class="btn-text">{{ text }}</span>
  </button>
</template>

<script setup>
import { computed } from 'vue'

const props = defineProps({
  isRecording: {
    type: Boolean,
    default: false
  },
  disabled: {
    type: Boolean,
    default: false
  }
})

defineEmits(['click'])

const icon = computed(() => {
  if (props.isRecording) return '⏹'
  return '🎙'
})

const text = computed(() => {
  if (props.isRecording) return '停止'
  return '开始'
})
</script>
