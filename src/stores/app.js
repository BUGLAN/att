import { defineStore } from 'pinia'
import { ref } from 'vue'

export const useAppStore = defineStore('app', () => {
  const currentPage = ref('home')
  const sidebarCollapsed = ref(false)

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
    llm: false,
    llmEdit: false
  })

  const toastMessage = ref('')
  const toastVisible = ref(false)
  let toastTimeout = null

  const selectMode = ref(false)
  const selectedRecords = ref(new Set())

  function setCurrentPage(page) {
    currentPage.value = page
  }

  function toggleSidebar() {
    sidebarCollapsed.value = !sidebarCollapsed.value
  }

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

  function closeAllModals() {
    Object.keys(modals.value).forEach((key) => {
      modals.value[key] = false
    })
  }

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
    isRecording,
    isTranscribing,
    currentRecordId,
    currentRecord,
    recordStatus,
    recordingDuration,
    isConnected,
    connectedClients,
    modals,
    toastMessage,
    toastVisible,
    selectMode,
    selectedRecords,
    setCurrentPage,
    toggleSidebar,
    setRecording,
    startRecording,
    stopRecording,
    setTranscribing,
    setCurrentRecordId,
    setCurrentRecord,
    setRecordStatus,
    incrementDuration,
    setConnected,
    setConnectedClients,
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
