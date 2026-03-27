import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useAppStore } from '@/stores/app'
import { useRecordsStore } from '@/stores/records'
import { useSettingsStore } from '@/stores/settings'
import { useElectron } from './useElectron'
import { useGlobalWebSocket } from './useWebSocket'

const duration = ref(0)
const mediaRecorder = ref(null)
const chunks = ref([])
const durationTimer = ref(null)
const stream = ref(null)

let pollingTimer = null
let mountedInstances = 0
let wsAttached = false
let startPending = false
let pendingStopAfterStart = false
let pollingInFlight = false
const completedPhases = new Set()

function syncFloatingWindow(state, options = {}) {
  const { floatingWindow } = useElectron()

  floatingWindow.updateState(state)

  if (options.show) {
    floatingWindow.show()
  }

  if (options.hide) {
    floatingWindow.hide()
  }
}

function clearDurationTimer() {
  if (durationTimer.value) {
    clearInterval(durationTimer.value)
    durationTimer.value = null
  }
}

function stopMediaTracks() {
  if (stream.value) {
    stream.value.getTracks().forEach((track) => track.stop())
    stream.value = null
  }
}

function stopPolling() {
  if (pollingTimer) {
    clearInterval(pollingTimer)
    pollingTimer = null
  }
  pollingInFlight = false
}

async function maybeCopyText(text) {
  if (!text) {
    return { attempted: false, copied: false, pasted: false }
  }

  const settingsStore = useSettingsStore()
  const { clipboard } = useElectron()

  if (!settingsStore.settings.auto_copy_to_clipboard) {
    return { attempted: false, copied: false, pasted: false }
  }

  try {
    await clipboard.writeText(text)
    await clipboard.pasteActive()
    return { attempted: true, copied: true, pasted: true }
  } catch (error) {
    console.error('[Clipboard] Failed to copy/paste text:', error)
    return { attempted: true, copied: false, pasted: false }
  }
}

function markCompletion(recordId, phase) {
  const key = `${recordId}:${phase}`
  if (completedPhases.has(key)) {
    return false
  }
  completedPhases.add(key)
  return true
}

function clearCompletion(recordId) {
  completedPhases.delete(`${recordId}:transcription`)
  completedPhases.delete(`${recordId}:optimization`)
}

function handleRecordCompleted(recordId, patch, options = {}) {
  const appStore = useAppStore()
  const recordsStore = useRecordsStore()
  const { tray } = useElectron()

  recordsStore.updateRecordStatus(recordId, patch)
  appStore.setCurrentRecordId(recordId)
  appStore.setRecordStatus({ record_id: recordId, ...patch })

  const existingRecord = Array.isArray(recordsStore.records)
    ? recordsStore.records.find((record) => record.id === recordId)
    : null

  appStore.setCurrentRecord(existingRecord ? { ...existingRecord, ...patch } : { id: recordId, ...patch })

  if (!options.keepBusy) {
    appStore.setTranscribing(false)
    tray.setIcon('idle')
  }
}

async function handleWebSocketMessage(message) {
  const appStore = useAppStore()
  const settingsStore = useSettingsStore()
  const recordsStore = useRecordsStore()
  const { tray } = useElectron()

  if (message.type === 'recording_command') {
    if (message.action === 'start') {
      syncFloatingWindow({
        phase: 'recording',
        duration: 0,
        text: ''
      }, { show: true })
      await startRecording()
    } else if (message.action === 'stop') {
      stopRecording()
    }
    return
  }

  if (message.type === 'transcription_started') {
    const recordId = message.data?.record_id
    if (!recordId) {
      return
    }

    clearCompletion(recordId)

    appStore.setCurrentRecordId(recordId)
    appStore.setTranscribing(true)
    appStore.setRecordStatus({ record_id: recordId, status: 'transcribing' })
    tray.setIcon('transcribing')
    recordsStore.updateRecordStatus(recordId, { status: 'transcribing' })
    syncFloatingWindow({
      phase: 'transcribing',
      duration: 0,
      text: ''
    })
    return
  }

  if (message.type === 'transcription_completed') {
    const recordId = message.data?.record_id
    if (!recordId) {
      return
    }
    if (!markCompletion(recordId, 'transcription')) {
      return
    }

    stopPolling()
    handleRecordCompleted(recordId, {
      status: 'transcribed',
      raw_text: message.data?.text || '',
      duration: message.data?.duration,
      processing_time: message.data?.processing_time
    })

    if (!settingsStore.settings.auto_optimize) {
      syncFloatingWindow({
        phase: 'complete',
        duration: 0,
        text: message.data?.text || ''
      })
      const copyResult = await maybeCopyText(message.data?.text)
      appStore.showToast(
        copyResult.pasted
          ? '转写完成，已复制并粘贴'
          : copyResult.attempted
            ? '转写完成，复制并粘贴失败'
            : '转写完成'
      )
    } else {
      syncFloatingWindow({
        phase: 'optimizing',
        duration: 0,
        text: ''
      })
      appStore.showToast('转写完成，正在优化')
    }
    return
  }

  if (message.type === 'optimization_started') {
    const recordId = message.data?.record_id
    if (!recordId) {
      return
    }
    completedPhases.delete(`${recordId}:optimization`)

    appStore.setTranscribing(true)
    appStore.setRecordStatus({ record_id: recordId, status: 'optimizing' })
    tray.setIcon('transcribing')
    recordsStore.updateRecordStatus(recordId, { status: 'optimizing' })
    syncFloatingWindow({
      phase: 'optimizing',
      duration: 0,
      text: ''
    })
    return
  }

  if (message.type === 'optimization_completed') {
    const recordId = message.data?.record_id
    if (!recordId) {
      return
    }
    if (!markCompletion(recordId, 'optimization')) {
      return
    }

    stopPolling()
    handleRecordCompleted(recordId, {
      status: 'optimized',
      optimized_text: message.data?.optimized_text || ''
    })
    syncFloatingWindow({
      phase: 'complete',
      duration: 0,
      text: message.data?.optimized_text || ''
    })
    const copyResult = await maybeCopyText(message.data?.optimized_text)
    appStore.showToast(
      copyResult.pasted
        ? '优化完成，已复制并粘贴'
        : copyResult.attempted
          ? '优化完成，复制并粘贴失败'
          : '优化完成'
    )
    return
  }

  if (message.type === 'error') {
    const recordId = message.data?.record_id
    if (recordId) {
      stopPolling()
      recordsStore.updateRecordStatus(recordId, { status: 'failed' })
      appStore.setRecordStatus({ record_id: recordId, status: 'failed' })
    }

    appStore.setTranscribing(false)
    tray.setIcon('idle')
    syncFloatingWindow({
      phase: 'error',
      duration: 0,
      text: message.data?.error || '处理失败'
    })
    appStore.showToast(message.data?.error || '处理失败')
    return
  }

  if (message.type === 'segment_transcribed') {
    appStore.showToast('收到新的分段转写结果')
  }
}

async function pollRecordStatus(recordId) {
  const appStore = useAppStore()
  const recordsStore = useRecordsStore()
  const settingsStore = useSettingsStore()
  const { api, tray } = useElectron()

  try {
    const status = await api.getRecordStatus(recordId)
    recordsStore.updateRecordStatus(recordId, status)
    appStore.setRecordStatus(status)

    if (status.status === 'optimized') {
      if (!markCompletion(recordId, 'optimization')) {
        return true
      }
      stopPolling()
      handleRecordCompleted(recordId, status)
      syncFloatingWindow({
        phase: 'complete',
        duration: 0,
        text: status.optimized_text || ''
      })
      const copyResult = await maybeCopyText(status.optimized_text)
      appStore.showToast(
        copyResult.pasted
          ? '优化完成，已复制并粘贴'
        : copyResult.attempted
            ? '优化完成，复制并粘贴失败'
            : '优化完成'
      )
      return true
    }

    if (status.status === 'transcribed') {
      if (!settingsStore.settings.auto_optimize) {
        if (!markCompletion(recordId, 'transcription')) {
          return true
        }
        stopPolling()
        handleRecordCompleted(recordId, status)
        syncFloatingWindow({
          phase: 'complete',
          duration: 0,
          text: status.raw_text || ''
        })
        const copyResult = await maybeCopyText(status.raw_text)
        appStore.showToast(
          copyResult.pasted
            ? '转写完成，已复制并粘贴'
          : copyResult.attempted
              ? '转写完成，复制并粘贴失败'
              : '转写完成'
        )
        return true
      }

      appStore.setTranscribing(false)
      tray.setIcon('idle')
      return false
    }

    if (status.status === 'failed') {
      stopPolling()
      appStore.setTranscribing(false)
      tray.setIcon('idle')
      syncFloatingWindow({
        phase: 'error',
        duration: 0,
        text: '转写失败'
      })
      appStore.showToast('转写失败')
      return true
    }

    return false
  } catch (error) {
    console.error('[Recorder] Failed to poll status:', error)
    return false
  }
}

function startPolling(recordId) {
  stopPolling()
  pollingTimer = setInterval(async () => {
    if (pollingInFlight) {
      return
    }
    pollingInFlight = true
    const completed = await pollRecordStatus(recordId)
    pollingInFlight = false
    if (completed) {
      stopPolling()
    }
  }, 1000)
}

async function uploadRecording(blob) {
  const appStore = useAppStore()
  const recordsStore = useRecordsStore()
  const { api, tray } = useElectron()

  try {
    if (duration.value < 1) {
      appStore.showToast('录音时间过短')
      appStore.setTranscribing(false)
      tray.setIcon('idle')
      syncFloatingWindow({
        phase: 'error',
        duration: 0,
        text: '录音时间过短'
      })
      return
    }

    appStore.setTranscribing(true)
    const result = await api.uploadAudio(blob, `recording_${Date.now()}.webm`)

    if (!result.record_id) {
      throw new Error('Missing record id from upload response')
    }

    appStore.setCurrentRecordId(result.record_id)
    clearCompletion(result.record_id)
    appStore.setRecordStatus({ record_id: result.record_id, status: result.transcription_status || 'transcribing' })
    recordsStore.addRecord({
      id: result.record_id,
      created_at: new Date().toISOString(),
      duration: result.duration || duration.value,
      status: result.transcription_status || 'transcribing',
      raw_text: '',
      optimized_text: '',
      processing_time: 0,
      is_modified: false,
      is_favorite: false
    })

    const { sendRecordingStatus } = useGlobalWebSocket()
    sendRecordingStatus({
      recording: false,
      transcribing: true,
      record_id: result.record_id,
      duration: duration.value
    })

    startPolling(result.record_id)
  } catch (error) {
    console.error('[Recorder] Upload failed:', error)
    appStore.setTranscribing(false)
    tray.setIcon('idle')
    syncFloatingWindow({
      phase: 'error',
      duration: 0,
      text: '上传录音失败'
    })
    appStore.showToast('上传录音失败')
  }
}

async function startRecording() {
  const appStore = useAppStore()
  const settingsStore = useSettingsStore()
  const { tray, syncRecordingState } = useElectron()
  const { sendRecordingStatus, sendWebRecording } = useGlobalWebSocket()

  if (
    startPending ||
    appStore.isRecording ||
    appStore.isTranscribing ||
    mediaRecorder.value?.state === 'recording'
  ) {
    return
  }

  startPending = true
  pendingStopAfterStart = false

  try {
    const deviceId = settingsStore.settings.audio_input_device || 'default'
    const constraints = {
      audio: {
        deviceId: deviceId === 'default' ? undefined : { exact: deviceId }
      }
    }

    stream.value = await navigator.mediaDevices.getUserMedia(constraints)

    const preferredMimeType = MediaRecorder.isTypeSupported('audio/webm;codecs=opus')
      ? 'audio/webm;codecs=opus'
      : undefined

    mediaRecorder.value = preferredMimeType
      ? new MediaRecorder(stream.value, { mimeType: preferredMimeType })
      : new MediaRecorder(stream.value)

    chunks.value = []

    mediaRecorder.value.ondataavailable = (event) => {
      if (event.data.size > 0) {
        chunks.value.push(event.data)
      }
    }

    mediaRecorder.value.onstop = async () => {
      const blob = new Blob(chunks.value, { type: mediaRecorder.value?.mimeType || 'audio/webm' })
      mediaRecorder.value = null
      stopMediaTracks()
      await uploadRecording(blob)
    }

    if (pendingStopAfterStart) {
      mediaRecorder.value = null
      stopMediaTracks()
      syncFloatingWindow({
        phase: 'idle',
        duration: 0,
        text: ''
      }, { hide: true })
      return
    }

    duration.value = 0
    clearDurationTimer()
    mediaRecorder.value.start(1000)
    appStore.setRecording(true)
    tray.setIcon('recording')
    await syncRecordingState({ recording: true })
    sendWebRecording('start')
    sendRecordingStatus({
      recording: true,
      transcribing: false,
      duration: 0
    })
    syncFloatingWindow({
      phase: 'recording',
      duration: 0,
      text: ''
    }, { show: true })

    durationTimer.value = setInterval(() => {
      duration.value += 1
      appStore.incrementDuration()
      syncFloatingWindow({
        phase: 'recording',
        duration: duration.value,
        text: ''
      })
    }, 1000)
  } catch (error) {
    console.error('[Recorder] Failed to start recording:', error)
    await syncRecordingState({ recording: false }).catch(() => {})
    stopMediaTracks()
    mediaRecorder.value = null
    appStore.setRecording(false)
    syncFloatingWindow({
      phase: 'error',
      duration: 0,
      text: '无法访问麦克风'
    })
    appStore.showToast('无法访问麦克风')
  } finally {
    startPending = false
  }
}

function stopRecording() {
  const appStore = useAppStore()
  const { tray, syncRecordingState } = useElectron()
  const { sendRecordingStatus, sendWebRecording } = useGlobalWebSocket()

  if (startPending && (!mediaRecorder.value || mediaRecorder.value.state === 'inactive')) {
    pendingStopAfterStart = true
    return
  }

  if (!mediaRecorder.value || mediaRecorder.value.state === 'inactive') {
    return
  }

  pendingStopAfterStart = false
  clearDurationTimer()
  appStore.setRecording(false)
  syncRecordingState({ recording: false }).catch(() => {})
  // 停止录音后立即恢复空闲状态，不设置转写中状态
  // 转写状态只在历史记录卡片中显示
  tray.setIcon('idle')
  sendWebRecording('stop')
  sendRecordingStatus({
    recording: false,
    transcribing: false,
    duration: duration.value
  })
  syncFloatingWindow({
    phase: 'transcribing',
    duration: 0,
    text: ''
  })
  mediaRecorder.value.stop()
}

export function useRecorder() {
  const appStore = useAppStore()
  const { connect, addMessageHandler, removeMessageHandler } = useGlobalWebSocket()

  onMounted(async () => {
    mountedInstances += 1
    await connect()

    if (!wsAttached) {
      addMessageHandler(handleWebSocketMessage)
      wsAttached = true
    }
  })

  onUnmounted(() => {
    mountedInstances -= 1

    if (mountedInstances <= 0 && wsAttached) {
      removeMessageHandler(handleWebSocketMessage)
      wsAttached = false
      mountedInstances = 0
    }
  })

  return {
    isRecording: computed(() => appStore.isRecording),
    isTranscribing: computed(() => appStore.isTranscribing),
    duration,
    startRecording,
    stopRecording,
    startPolling,
    stopPolling
  }
}
