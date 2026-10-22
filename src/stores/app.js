import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

const SIDEBAR_LAYOUT_STORAGE_KEY = 'att.sidebar.layout'
const SIDEBAR_DEFAULT_WIDTH = 220
const SIDEBAR_COLLAPSED_WIDTH = 60
const SIDEBAR_MIN_WIDTH = SIDEBAR_COLLAPSED_WIDTH
const SIDEBAR_MAX_WIDTH = 280
const SIDEBAR_COLLAPSE_THRESHOLD = 110

// Toast 默认展示时长与最大同时条数（队列上限）
const TOAST_DEFAULT_DURATION = 3000
const TOAST_MAX_VISIBLE = 3

function clampSidebarWidth(width) {
  return Math.min(Math.max(width, SIDEBAR_MIN_WIDTH), SIDEBAR_MAX_WIDTH)
}

function sanitizeExpandedWidth(width) {
  return Math.min(
    Math.max(width, SIDEBAR_COLLAPSE_THRESHOLD + 1),
    SIDEBAR_MAX_WIDTH
  )
}

function loadSidebarLayout() {
  if (typeof window === 'undefined') {
    return {
      collapsed: false,
      width: SIDEBAR_DEFAULT_WIDTH
    }
  }

  try {
    const rawLayout = window.localStorage.getItem(SIDEBAR_LAYOUT_STORAGE_KEY)

    if (!rawLayout) {
      return {
        collapsed: false,
        width: SIDEBAR_DEFAULT_WIDTH
      }
    }

    const parsedLayout = JSON.parse(rawLayout)

    return {
      collapsed: Boolean(parsedLayout?.collapsed),
      width: sanitizeExpandedWidth(parsedLayout?.width ?? SIDEBAR_DEFAULT_WIDTH)
    }
  } catch (error) {
    console.warn('[AppStore] Failed to load sidebar layout:', error)
    return {
      collapsed: false,
      width: SIDEBAR_DEFAULT_WIDTH
    }
  }
}

export const useAppStore = defineStore('app', () => {
  const currentPage = ref('home')
  const initialSidebarLayout = loadSidebarLayout()
  const sidebarCollapsed = ref(initialSidebarLayout.collapsed)
  const sidebarWidth = ref(initialSidebarLayout.width)
  const isSidebarResizing = ref(false)
  const sidebarDragWidth = ref(null)
  const sidebarDisplayCollapsed = computed(() => (
    isSidebarResizing.value && sidebarDragWidth.value != null
      ? sidebarDragWidth.value <= SIDEBAR_COLLAPSE_THRESHOLD
      : sidebarCollapsed.value
  ))
  const currentSidebarWidth = computed(() => (
    isSidebarResizing.value && sidebarDragWidth.value != null
      ? sidebarDragWidth.value
      : (sidebarCollapsed.value ? SIDEBAR_COLLAPSED_WIDTH : sidebarWidth.value)
  ))

  const isRecording = ref(false)
  const isTranscribing = ref(false)
  const currentRecordId = ref(null)
  const currentRecord = ref(null)
  const recordStatus = ref(null)
  const recordingDuration = ref(0)
  const isConnected = ref(false)
  const connectedClients = ref(0)

  const modals = ref({
    recordEdit: false,
    preset: false,
    presetEdit: false,
    llmWizard: false,
    llmModelEdit: false,
    llmProviderEdit: false
  })

  // Toast 队列：每条消息有独立 tone 与独立计时，最多同时展示 3 条
  const toasts = ref([])
  const toastTimers = new Map()
  let toastSeq = 0

  const selectMode = ref(false)
  const selectedRecords = ref(new Set())

  function persistSidebarLayout() {
    if (typeof window === 'undefined') {
      return
    }

    window.localStorage.setItem(SIDEBAR_LAYOUT_STORAGE_KEY, JSON.stringify({
      collapsed: sidebarCollapsed.value,
      width: sidebarWidth.value
    }))
  }

  function setCurrentPage(page) {
    currentPage.value = page
  }

  function setSidebarWidth(width) {
    sidebarDragWidth.value = clampSidebarWidth(width)
  }

  function startSidebarResize() {
    isSidebarResizing.value = true
    sidebarDragWidth.value = currentSidebarWidth.value
  }

  function stopSidebarResize() {
    const finalWidth = sidebarDragWidth.value

    if (finalWidth != null) {
      if (finalWidth <= SIDEBAR_COLLAPSE_THRESHOLD) {
        sidebarCollapsed.value = true
      } else {
        sidebarWidth.value = sanitizeExpandedWidth(finalWidth)
        sidebarCollapsed.value = false
      }

      persistSidebarLayout()
    }

    isSidebarResizing.value = false
    sidebarDragWidth.value = null
  }

  function setRecording(value) {
    isRecording.value = value
    if (value) {
      recordingDuration.value = 0
    }
  }

  function setTranscribing(value) {
    isTranscribing.value = value
  }

  function setCurrentRecordId(id) {
    currentRecordId.value = id
  }

  function setCurrentRecord(record) {
    currentRecord.value = record
  }

  function setRecordStatus(status) {
    recordStatus.value = status
  }

  function incrementDuration() {
    recordingDuration.value += 1
  }

  function setConnected(value) {
    isConnected.value = value
  }

  function setConnectedClients(count) {
    connectedClients.value = count
  }

  function openModal(modalName) {
    modals.value[modalName] = true
  }

  function closeModal(modalName) {
    modals.value[modalName] = false
  }

  function removeToast(id) {
    toasts.value = toasts.value.filter((item) => item.id !== id)

    const timer = toastTimers.get(id)
    if (timer) {
      clearTimeout(timer)
      toastTimers.delete(id)
    }
  }

  /**
   * 显示一条 Toast（保持向后兼容）。
   * - 旧签名：showToast('文案') / showToast('文案', 3000)
   * - 新签名：showToast('文案', { tone: 'success' | 'error' | 'info', duration: 3000 })
   */
  function showToast(message, options = {}) {
    const normalized = typeof options === 'number' ? { duration: options } : (options || {})
    const tone = ['success', 'error', 'info'].includes(normalized.tone) ? normalized.tone : 'info'
    const duration = Number.isFinite(normalized.duration) ? normalized.duration : TOAST_DEFAULT_DURATION
    const id = ++toastSeq

    toasts.value = [...toasts.value, {
      id,
      tone,
      duration,
      message: String(message ?? '')
    }].slice(-TOAST_MAX_VISIBLE)

    // 被挤出队列的旧条目要同步清理计时器，避免定时器泄漏
    const aliveIds = new Set(toasts.value.map((item) => item.id))
    toastTimers.forEach((timer, timerId) => {
      if (!aliveIds.has(timerId)) {
        clearTimeout(timer)
        toastTimers.delete(timerId)
      }
    })

    toastTimers.set(id, setTimeout(() => removeToast(id), duration))
    return id
  }

  function toggleSelectMode() {
    selectMode.value = !selectMode.value
    if (!selectMode.value) {
      selectedRecords.value.clear()
    }
  }

  function toggleRecordSelection(recordId) {
    if (selectedRecords.value.has(recordId)) {
      selectedRecords.value.delete(recordId)
      return
    }

    selectedRecords.value.add(recordId)
  }

  function selectAllRecords(recordIds) {
    selectedRecords.value = new Set(recordIds)
  }

  function clearSelection() {
    selectedRecords.value.clear()
  }

  return {
    currentPage,
    sidebarCollapsed,
    sidebarDisplayCollapsed,
    sidebarWidth,
    currentSidebarWidth,
    isSidebarResizing,
    sidebarCollapseThreshold: SIDEBAR_COLLAPSE_THRESHOLD,
    sidebarCollapsedWidth: SIDEBAR_COLLAPSED_WIDTH,
    sidebarMinWidth: SIDEBAR_MIN_WIDTH,
    sidebarMaxWidth: SIDEBAR_MAX_WIDTH,
    isRecording,
    isTranscribing,
    currentRecordId,
    currentRecord,
    recordStatus,
    recordingDuration,
    isConnected,
    connectedClients,
    modals,
    toasts,
    selectMode,
    selectedRecords,
    setCurrentPage,
    setSidebarWidth,
    startSidebarResize,
    stopSidebarResize,
    setRecording,
    setTranscribing,
    setCurrentRecordId,
    setCurrentRecord,
    setRecordStatus,
    incrementDuration,
    setConnected,
    setConnectedClients,
    openModal,
    closeModal,
    showToast,
    toggleSelectMode,
    toggleRecordSelection,
    selectAllRecords,
    clearSelection
  }
})
