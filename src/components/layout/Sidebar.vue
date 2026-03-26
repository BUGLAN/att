<template>
  <aside class="sidebar" :class="{ collapsed }">
    <div class="sidebar-header">
      <button class="sidebar-toggle" type="button" @click="$emit('toggle')">
        <span class="toggle-icon">▸</span>
      </button>
      <div class="sidebar-logo">
        <span class="logo-icon">ATT</span>
        <span v-if="!collapsed" class="logo-text">Audio To Text</span>
      </div>
    </div>

    <nav class="sidebar-nav">
      <button class="nav-item" :class="{ active: currentPage === 'home' }" type="button" @click="navigateTo('home')">
        <span class="nav-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24" fill="none">
            <path d="M12 2.5a3.5 3.5 0 0 0-3.5 3.5v6a3.5 3.5 0 1 0 7 0V6A3.5 3.5 0 0 0 12 2.5Z" />
            <path d="M18.5 11.5V12a6.5 6.5 0 0 1-13 0v-.5" />
            <path d="M12 18.5v3" />
            <path d="M9 21.5h6" />
          </svg>
        </span>
        <span v-if="!collapsed" class="nav-text">首页</span>
      </button>
      <button class="nav-item" :class="{ active: currentPage === 'records' }" type="button" @click="navigateTo('records')">
        <span class="nav-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24" fill="none">
            <rect x="4" y="5" width="16" height="4" rx="1.5" />
            <rect x="4" y="10" width="16" height="4" rx="1.5" />
            <rect x="4" y="15" width="16" height="4" rx="1.5" />
          </svg>
        </span>
        <span v-if="!collapsed" class="nav-text">记录集</span>
      </button>
      <button class="nav-item" :class="{ active: currentPage === 'settings' }" type="button" @click="navigateTo('settings')">
        <span class="nav-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24" fill="none">
            <circle cx="12" cy="12" r="3" />
            <path d="M12 2.75v2.5" />
            <path d="M12 18.75v2.5" />
            <path d="m4.93 4.93 1.77 1.77" />
            <path d="m17.3 17.3 1.77 1.77" />
            <path d="M2.75 12h2.5" />
            <path d="M18.75 12h2.5" />
            <path d="m4.93 19.07 1.77-1.77" />
            <path d="m17.3 6.7 1.77-1.77" />
          </svg>
        </span>
        <span v-if="!collapsed" class="nav-text">设置</span>
      </button>
    </nav>

    <div class="sidebar-status">
      <div class="connection-status">
        <span class="status-indicator" :class="isConnected ? 'connected' : 'disconnected'"></span>
        <span class="status-text">{{ isConnected ? '已连接后端' : '未连接后端' }}</span>
      </div>
      <div v-if="!collapsed" class="client-count">
        <span class="client-icon">WS</span>
        <span class="client-text">{{ connectedClients }} 个客户端在线</span>
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
const connectedClients = computed(() => appStore.connectedClients)

function navigateTo(page) {
  appStore.setCurrentPage(page)
}
</script>
