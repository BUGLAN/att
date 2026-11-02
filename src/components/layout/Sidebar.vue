<template>
  <aside
    class="sidebar"
    :class="{
      collapsed,
      resizing: isResizing,
      'collapse-intent': collapseIntent
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

    <!-- 底部只留一行总体状态：正常时一句「已就绪」，出问题才写原因；明细放在 title 里，点击去对应设置页 -->
    <div class="sidebar-status">
      <button
        class="sidebar-health"
        :class="{ 'is-quiet': health.level === 'ok' }"
        type="button"
        :title="health.detail"
        @click="appStore.openSettings(health.section)"
      >
        <span class="sidebar-health-slot" aria-hidden="true">
          <span class="ui-dot" :class="health.dot"></span>
        </span>
        <span v-if="!collapsed" class="sidebar-health-text">{{ health.text }}</span>
      </button>
    </div>

    <!-- 拖拽把手就是右边线本身：不放悬浮按钮，指针落到边线热区上停一下，整条边线才亮起 -->
    <button
      ref="resizeHandle"
      class="sidebar-resizer"
      type="button"
      aria-label="调整侧边栏宽度"
      title="拖动调整宽度"
      @pointerdown="startResize"
    >
      <span class="sidebar-resizer-rail" aria-hidden="true"></span>
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
// 调试信息开关：仅开发模式显示，打包后的正式界面不再暴露 WS 连接数
const showDebugInfo = import.meta.env.DEV
// 后端连接 + 识别引擎合成一条总体状态：断开最严重，其次引擎不可用、加载中，都正常才是「已就绪」
const health = computed(() => {
  const runtime = settingsStore.runtimeStatus
  const settings = settingsStore.settings
  const engine = `${providerLabel(settings.asr_provider)} · ${MODE_LABELS[settings.recording_mode] || '单段'}`
  const streaming = settings.recording_mode === 'streaming'
  const ready = runtime.model_service_ready && (!streaming || runtime.streaming_available)

  let state
  if (!isConnected.value) {
    state = { level: 'err', dot: 'err', text: '未连接后端', section: 'diagnostics', engineText: '未知' }
  } else if (runtime.model_service_loading) {
    state = { level: 'loading', dot: 'gold pulse', text: '模型加载中', section: 'asr', engineText: '加载中' }
  } else if (!ready) {
    const reason = runtime.streaming_error || runtime.model_service_error || '不可用'
    state = { level: 'warn', dot: 'warn', text: '识别引擎不可用', section: 'asr', engineText: reason }
  } else {
    state = { level: 'ok', dot: 'ok', text: '已就绪', section: 'asr', engineText: '已就绪' }
  }

  const lines = [
    `后端：${isConnected.value ? '已连接' : '未连接'}`,
    `识别引擎：${engine}（${state.engineText}）`
  ]
  if (showDebugInfo) {
    lines.push(`WS：${connectedClients.value} 个客户端`)
  }
  return { ...state, detail: lines.join('\n') }
})
const collapseThreshold = computed(() => appStore.sidebarCollapseThreshold)
const minWidth = computed(() => appStore.sidebarMinWidth)
const maxWidth = computed(() => appStore.sidebarMaxWidth)

// 拖到收起阈值以内：松手就会收起，边线换成实金提示
const collapseIntent = computed(() => isResizing.value && liveWidth.value <= collapseThreshold.value)

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
