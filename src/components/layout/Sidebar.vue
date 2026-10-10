<template>
  <aside
    class="sidebar"
    :class="{
      collapsed,
      resizing: isResizing,
      'collapse-intent': resizeIntent === 'collapse',
      'expand-intent': resizeIntent === 'expand'
    }"
    :style="{ width: `${width}px` }"
  >
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
      <button class="nav-item" :class="{ active: currentPage === 'clipboard' }" type="button" @click="navigateTo('clipboard')">
        <span class="nav-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24" fill="none">
            <path d="M9 4.75h6" />
            <path d="M9 3.5h6a1.5 1.5 0 0 1 1.5 1.5v1.25H7.5V5A1.5 1.5 0 0 1 9 3.5Z" />
            <path d="M7.25 6.25h9.5A2.25 2.25 0 0 1 19 8.5v9.25A2.25 2.25 0 0 1 16.75 20h-9.5A2.25 2.25 0 0 1 5 17.75V8.5a2.25 2.25 0 0 1 2.25-2.25Z" />
            <path d="M8.5 11h7" />
            <path d="M8.5 14.5h5.5" />
          </svg>
        </span>
        <span v-if="!collapsed" class="nav-text">剪切板</span>
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
      <!-- 调试信息不再常驻正式 UI：仅在开发模式（vite dev）下显示 -->
      <div v-if="showDebugInfo && !collapsed" class="client-count">
        <span class="client-icon">WS</span>
        <span class="client-text">{{ connectedClients }} 个客户端在线</span>
      </div>
    </div>

    <button
      ref="resizeHandle"
      class="sidebar-resizer"
      type="button"
      aria-label="调整侧边栏宽度"
      @pointerdown="startResize"
    >
      <span class="sidebar-resizer-rail"></span>
      <span class="sidebar-resizer-grip" aria-hidden="true">
        <template v-if="resizeIntent === 'collapse'">
          <svg viewBox="0 0 16 16" fill="none">
            <path d="M10.5 3.5 6 8l4.5 4.5" />
          </svg>
        </template>
        <template v-else-if="resizeIntent === 'expand'">
          <svg viewBox="0 0 16 16" fill="none">
            <path d="M5.5 3.5 10 8l-4.5 4.5" />
          </svg>
        </template>
        <template v-else>
          <svg viewBox="0 0 16 16" fill="none">
            <path d="M6 3v10" />
            <path d="M10 3v10" />
          </svg>
        </template>
      </span>
    </button>
  </aside>
</template>

<script setup>
import { computed, onBeforeUnmount, ref } from 'vue'
import { useAppStore } from '@/stores/app'

const props = defineProps({
  collapsed: {
    type: Boolean,
    default: false
  },
  width: {
    type: Number,
    default: 220
  }
})

const appStore = useAppStore()
const resizeHandle = ref(null)
const isResizing = ref(false)
const liveWidth = ref(props.width)

const currentPage = computed(() => appStore.currentPage)
const isConnected = computed(() => appStore.isConnected)
const connectedClients = computed(() => appStore.connectedClients)
// 调试信息开关：仅开发模式显示，打包后的正式界面不再暴露 WS 连接数
const showDebugInfo = import.meta.env.DEV
const collapseThreshold = computed(() => appStore.sidebarCollapseThreshold)
const minWidth = computed(() => appStore.sidebarMinWidth)
const maxWidth = computed(() => appStore.sidebarMaxWidth)

const resizeIntent = computed(() => {
  if (props.collapsed) {
    return 'expand'
  }

  if (isResizing.value && liveWidth.value <= collapseThreshold.value) {
    return 'collapse'
  }

  return 'resize'
})

function navigateTo(page) {
  appStore.setCurrentPage(page)
}

function clampWidth(width) {
  return Math.min(Math.max(width, minWidth.value), maxWidth.value)
}

function handlePointerMove(event) {
  const nextWidth = clampWidth(event.clientX)
  liveWidth.value = nextWidth
  appStore.setSidebarWidth(nextWidth)
}

function stopResize() {
  if (!isResizing.value) {
    return
  }

  isResizing.value = false
  appStore.stopSidebarResize()
  window.removeEventListener('pointermove', handlePointerMove)
  window.removeEventListener('pointerup', stopResize)
  window.removeEventListener('pointercancel', stopResize)
}

function startResize(event) {
  event.preventDefault()
  liveWidth.value = props.width
  isResizing.value = true
  appStore.startSidebarResize()
  resizeHandle.value?.setPointerCapture?.(event.pointerId)
  window.addEventListener('pointermove', handlePointerMove)
  window.addEventListener('pointerup', stopResize)
  window.addEventListener('pointercancel', stopResize)
}

onBeforeUnmount(() => {
  stopResize()
})
</script>
