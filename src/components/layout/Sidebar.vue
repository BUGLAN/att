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
    <!-- 导航图标是「文字图标」（字表见 common/glyphs.mjs）：选中项是一枚金色小印章 -->
    <nav class="sidebar-nav">
      <template v-for="item in NAV_ITEMS" :key="item.page">
        <hr v-if="item.page === 'settings'" class="sidebar-divider">
        <button
          class="nav-item"
          :class="{ active: currentPage === item.page }"
          type="button"
          :title="collapsed ? item.label : undefined"
          :aria-current="currentPage === item.page ? 'page' : undefined"
          @click="navigateTo(item.page)"
        >
          <span class="nav-glyph" aria-hidden="true">{{ NAV_GLYPHS[item.page] }}</span>
          <span v-if="!collapsed" class="nav-text">{{ item.label }}</span>
        </button>
      </template>
    </nav>

    <!-- 状态只用小圆点 + 灰字 -->
    <div class="sidebar-status">
      <span class="ui-meta" :title="isConnected ? '已连接后端' : '未连接后端'">
        <span class="ui-dot" :class="isConnected ? 'ok' : 'err'"></span>
        <span v-if="!collapsed">{{ isConnected ? '已连接' : '未连接后端' }}</span>
      </span>
      <span v-if="!collapsed" class="ui-meta" :title="engineStatus.title">
        <span class="ui-dot" :class="engineStatus.dot"></span>
        <span>{{ engineStatus.text }}</span>
      </span>
      <!-- 调试信息不再常驻正式 UI：仅在开发模式（vite dev）下显示 -->
      <span v-if="showDebugInfo && !collapsed" class="ui-meta">
        <span class="ui-dot"></span><span>WS {{ connectedClients }} 个客户端</span>
      </span>
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
import { useSettingsStore } from '@/stores/settings'
import { NAV_GLYPHS } from '@/components/common/glyphs.mjs'
import { providerLabel } from '@/components/settings/asrEngineStatus.mjs'

const NAV_ITEMS = [
  { page: 'home', label: '首页' },
  { page: 'transcribe', label: '转文字' },
  { page: 'records', label: '记录集' },
  { page: 'clipboard', label: '剪贴板' },
  { page: 'settings', label: '设置' }
]
const MODE_LABELS = { single: '单段', multi: '多段', streaming: '流式' }

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
const settingsStore = useSettingsStore()
const resizeHandle = ref(null)
const isResizing = ref(false)
const liveWidth = ref(props.width)

const currentPage = computed(() => appStore.currentPage)
const isConnected = computed(() => appStore.isConnected)
const connectedClients = computed(() => appStore.connectedClients)
// 当前识别引擎是否可用：就绪绿点；加载中金点闪烁；不可用黄点（原因放在 title 里）
const engineStatus = computed(() => {
  const runtime = settingsStore.runtimeStatus
  const settings = settingsStore.settings
  const text = `${providerLabel(settings.asr_provider)} · ${MODE_LABELS[settings.recording_mode] || '单段'}`
  if (runtime.model_service_loading) {
    return { text, dot: 'gold pulse', title: '模型加载中' }
  }
  const streaming = settings.recording_mode === 'streaming'
  const ready = runtime.model_service_ready && (!streaming || runtime.streaming_available)
  if (ready) {
    return { text, dot: 'ok', title: '识别引擎已就绪' }
  }
  return { text, dot: 'warn', title: runtime.streaming_error || runtime.model_service_error || '识别引擎不可用' }
})
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

// 拖动时每帧最多改一次宽度：pointermove 往往比刷新率还密，每次都改会让整页反复重排而卡顿
let pendingWidth = null
let frameId = 0

function flushWidth() {
  frameId = 0
  if (pendingWidth == null) {
    return
  }
  liveWidth.value = pendingWidth
  appStore.setSidebarWidth(pendingWidth)
  pendingWidth = null
}

function handlePointerMove(event) {
  pendingWidth = clampWidth(event.clientX)
  if (!frameId) {
    frameId = window.requestAnimationFrame(flushWidth)
  }
}

function stopResize() {
  if (!isResizing.value) {
    return
  }

  // 松手前把最后一帧的宽度落下，免得最终宽度比指针慢一拍
  if (frameId) {
    window.cancelAnimationFrame(frameId)
    flushWidth()
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
