<template>
  <div class="connection-status">
    <span class="status-indicator" :class="{ connected: isConnected }"></span>
    <span class="status-text">{{ statusText }}</span>
  </div>
</template>

<script setup>
import { computed } from 'vue'
import { useAppStore } from '@/stores/app'

const appStore = useAppStore()

const isConnected = computed(() => appStore.isConnected)

const statusText = computed(() => {
  if (appStore.isRecording) return '录音中...'
  if (appStore.isTranscribing) return '转写中...'
  return isConnected.value ? '已连接' : '未连接'
})
</script>
