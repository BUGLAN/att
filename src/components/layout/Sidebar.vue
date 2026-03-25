<template>
  <aside class="sidebar" :class="{ collapsed }">
    <!-- 头部 - 支持窗口拖拽 -->
    <div class="sidebar-header" style="-webkit-app-region: drag;">
      <button class="sidebar-toggle" style="-webkit-app-region: no-drag;" @click="$emit('toggle')">
        <span class="toggle-icon">◀</span>
      </button>
      <div class="sidebar-logo">
        <span class="logo-icon">🎤</span>
        <span v-if="!collapsed" class="logo-text">ATT</span>
      </div>
    </div>

    <!-- 导航 -->
    <nav class="sidebar-nav">
      <a
        class="nav-item"
        :class="{ active: currentPage === 'home' }"
        @click="navigateTo('home')"
      >
        <span class="nav-icon">🏠</span>
        <span v-if="!collapsed" class="nav-text">首页</span>
      </a>
      <a
        class="nav-item"
        :class="{ active: currentPage === 'settings' }"
        @click="navigateTo('settings')"
      >
        <span class="nav-icon">⚙️</span>
        <span v-if="!collapsed" class="nav-text">设置</span>
      </a>
    </nav>

    <!-- 连接状态 -->
    <div v-if="!collapsed" class="sidebar-status">
      <div class="connection-status">
        <span class="status-indicator" :class="{ connected: isConnected }"></span>
        <span class="status-text">{{ isConnected ? '已连接' : '未连接' }}</span>
      </div>
    </div>
  </aside>
</template>

<script setup>
import { computed } from 'vue'
import { useAppStore } from '@/stores/app'

defineProps({
  collapsed: {
    type: Boolean,
    default: false
  }
})

defineEmits(['toggle'])

const appStore = useAppStore()

const currentPage = computed(() => appStore.currentPage)
const isConnected = computed(() => appStore.isConnected)

function navigateTo(page) {
  appStore.setCurrentPage(page)
}
</script>
