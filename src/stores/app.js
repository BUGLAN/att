/**
 * 应用状态管理
 * 管理页面、侧边栏、录音状态、模态框等全局状态
 */

import { defineStore } from 'pinia'
import { ref, computed } from 'vue'

export const useAppStore = defineStore('app', () => {
  // === 页面状态 ===
  const currentPage = ref('home')
  const sidebarCollapsed = ref(false)

  // === 录音状态 ===
  const isRecording = ref(false)
  const isTranscribing = ref(false)
  const currentRecordId = ref(null)
  const recordStatus = ref(null)
  const recordingDuration = ref(0)

  // === 模态框状态 ===
  const modals = ref({
    recordEdit: false,
    preset: false,
    presetEdit: false,
    llm: false,
    llmEdit: false
  })

  // === Toast 状态 ===
  const toastMessage = ref('')
  const toastVisible = ref(false)
  let toastTimeout = null

  // === 选择模式 ===
  const selectMode = ref(false)
  const selectedRecords = ref(new Set())

  // === 计算属性 ===
  const isConnected = computed(() => true) // Electron 始终连接

  // === 页面操作 ===
  function setCurrentPage(page) {
    currentPage.value = page
  }

  function toggleSidebar() {
    sidebarCollapsed.value = !sidebarCollapsed.value
  }

  // === 录音操作 ===
  function setRecording(value) {
    isRecording.value = value
    if (value) {
      recordingDuration.value = 0
    }
  }

  function startRecording() {
    setRecording(true)
  }

  function stopRecording() {
    setRecording(false)
  }

  function setTranscribing(value) {
    isTranscribing.value = value
  }

  function setCurrentRecordId(id) {
    currentRecordId.value = id
  }

  function setRecordStatus(status) {
    recordStatus.value = status
  }

  function incrementDuration() {
    recordingDuration.value++
  }

  // === 模态框操作 ===
  function openModal(modalName) {
    modals.value[modalName] = true
  }

  function closeModal(modalName) {
    modals.value[modalName] = false
  }

  function closeAllModals() {
    for (const key in modals.value) {
      modals.value[key] = false
    }
  }

  // === Toast 操作 ===
  function showToast(message, duration = 3000) {
    toastMessage.value = message
    toastVisible.value = true

    if (toastTimeout) {
      clearTimeout(toastTimeout)
    }

    toastTimeout = setTimeout(() => {
      toastVisible.value = false
    }, duration)
  }

  function hideToast() {
    toastVisible.value = false
    if (toastTimeout) {
      clearTimeout(toastTimeout)
    }
  }

  // === 选择模式操作 ===
  function toggleSelectMode() {
    selectMode.value = !selectMode.value
    if (!selectMode.value) {
      selectedRecords.value.clear()
    }
  }

  function toggleRecordSelection(recordId) {
    if (selectedRecords.value.has(recordId)) {
      selectedRecords.value.delete(recordId)
    } else {
      selectedRecords.value.add(recordId)
    }
  }

  function selectAllRecords(recordIds) {
    selectedRecords.value = new Set(recordIds)
  }

  function clearSelection() {
    selectedRecords.value.clear()
  }

  return {
    // 状态
    currentPage,
    sidebarCollapsed,
    isRecording,
    isTranscribing,
    currentRecordId,
    recordStatus,
    recordingDuration,
    modals,
    toastMessage,
    toastVisible,
    selectMode,
    selectedRecords,
    isConnected,

    // 操作
    setCurrentPage,
    toggleSidebar,
    setRecording,
    startRecording,
    stopRecording,
    setTranscribing,
    setCurrentRecordId,
    setRecordStatus,
    incrementDuration,
    openModal,
    closeModal,
    closeAllModals,
    showToast,
    hideToast,
    toggleSelectMode,
    toggleRecordSelection,
    selectAllRecords,
    clearSelection
  }
})
