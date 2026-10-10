<template>
  <!-- 连接状态：调试用的 WS 客户端数只在开发模式显示，正式 UI 只保留连接与否 -->
  <div class="connection-status">
    <span class="status-indicator" :class="{ connected: isConnected }"></span>
    <span class="status-text">{{ statusText }}</span>
  </div>
  <div v-if="showDebugInfo" class="client-count">
    <span class="client-icon">WS</span>
    <span class="client-text">{{ appStore.connectedClients }} 个客户端在线（开发模式）</span>
  </div>
</template>

<script setup>
import { computed } from 'vue'
import { useAppStore } from '@/stores/app'

const appStore = useAppStore()

const isConnected = computed(() => appStore.isConnected)
const showDebugInfo = import.meta.env.DEV

const statusText = computed(() => {
  if (appStore.isRecording) return '录音中...'
  if (appStore.isTranscribing) return '转写中...'
  return isConnected.value ? '已连接' : '未连接'
})
</script>
