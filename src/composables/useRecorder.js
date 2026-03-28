import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useAppStore } from '@/stores/app'
import { useRecordsStore } from '@/stores/records'
import { useSettingsStore } from '@/stores/settings'
import { useElectron } from './useElectron'
import { useGlobalWebSocket } from './useWebSocket'
import { createSilenceDetector } from './useSilenceDetector'

const MIN_REALTIME_SEGMENT_DURATION = 2.2
const MAX_REALTIME_SEGMENT_DURATION = 10
const STREAM_SAMPLE_RATE = 16000
const STREAM_FLUSH_SAMPLES = 3200

const duration = ref(0)
const mediaRecorder = ref(null)
const durationTimer = ref(null)
const stream = ref(null)
const audioLevel = ref(0)
const liveTranscript = ref('')
const liveSegments = ref([])
const livePhase = ref('idle')
const activeRecordingMode = ref('single')
const currentSessionId = ref(null)
const processedSegmentCount = ref(0)
const streamingConnectionState = ref('idle')
const sessionStatusText = ref('按住快捷键开始录音')

let pollingTimer = null
let mountedInstances = 0
let wsAttached = false
let desktopHotkeyAttached = false
let startPending = false
let pendingStopAfterStart = false
let pollingInFlight = false
let floatingSessionActive = false
let desktopRecordingSyncActive = false
let silenceDetector = null
let realtimeStopRequested = false
let realtimeFinalizing = false
let realtimePendingUploads = 0
let realtimePendingSegmentIds = new Set()
let realtimeCompletedSegmentIds = new Set()
let realtimeHasCapturedSpeech = false
let realtimeSegmentStartedAt = 0
let realtimeMaxSegmentTimer = null
let realtimeRestartAfterStop = false
let streamingSocket = null
let streamingAudioContext = null
let streamingSourceNode = null
let streamingWorkletNode = null
let streamingPendingSamples = []
let streamingPendingSampleCount = 0
let streamingAwaitingFinish = false
const completedPhases = new Set()

function syncFloatingWindow(state, options = {}) {
  const { floatingWindow } = useElectron()
  const payload = {
    mode: activeRecordingMode.value,
    ...state
  }

  floatingWindow.updateState(payload)

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

function startDurationClock() {
  const appStore = useAppStore()

  duration.value = 0
  clearDurationTimer()
  durationTimer.value = setInterval(() => {
    duration.value += 1
    appStore.incrementDuration()

    if (floatingSessionActive) {
      syncFloatingWindow({
        phase: 'recording',
        duration: duration.value,
        text: ['multi', 'streaming'].includes(activeRecordingMode.value) ? liveTranscript.value : ''
      })
    }
  }, 1000)
}

function stopMediaTracks() {
  if (stream.value) {
    stream.value.getTracks().forEach((track) => track.stop())
    stream.value = null
  }
}

function resetStreamingBuffer() {
  streamingPendingSamples = []
  streamingPendingSampleCount = 0
}

function cleanupStreamingAudio() {
  if (streamingWorkletNode) {
    streamingWorkletNode.port.onmessage = null
    streamingWorkletNode.disconnect()
    streamingWorkletNode = null
  }

  if (streamingSourceNode) {
    streamingSourceNode.disconnect()
    streamingSourceNode = null
  }

  if (streamingAudioContext) {
    streamingAudioContext.close().catch(() => {})
    streamingAudioContext = null
  }

  resetStreamingBuffer()
  audioLevel.value = 0
}

function cleanupStreamingSocket() {
  if (streamingSocket) {
    streamingSocket.onopen = null
    streamingSocket.onmessage = null
    streamingSocket.onerror = null
    streamingSocket.onclose = null
    if (streamingSocket.readyState === WebSocket.OPEN || streamingSocket.readyState === WebSocket.CONNECTING) {
      streamingSocket.close()
    }
    streamingSocket = null
  }

  streamingConnectionState.value = 'idle'
  streamingAwaitingFinish = false
}

function updateStreamingTranscript(text) {
  if (!text && liveTranscript.value) {
    return
  }

  liveTranscript.value = text || ''

  if (floatingSessionActive && activeRecordingMode.value === 'streaming') {
    syncFloatingWindow({
      phase: 'recording',
      duration: duration.value,
      text: liveTranscript.value
    })
  }
}

function calculateAudioLevel(samples) {
  if (!samples?.length) {
    return 0
  }

  let sumSquares = 0
  for (let index = 0; index < samples.length; index += 1) {
    sumSquares += samples[index] * samples[index]
  }

  const rms = Math.sqrt(sumSquares / samples.length)
  return Math.max(0, Math.min(1, rms * 6))
}

function takeStreamingSamples(count) {
  const result = new Float32Array(count)
  let offset = 0

  while (offset < count && streamingPendingSamples.length > 0) {
    const current = streamingPendingSamples[0]
    const remaining = count - offset

    if (current.length <= remaining) {
      result.set(current, offset)
      offset += current.length
      streamingPendingSamples.shift()
      continue
    }

    result.set(current.subarray(0, remaining), offset)
    streamingPendingSamples[0] = current.subarray(remaining)
    offset += remaining
  }

  streamingPendingSampleCount -= count
  return result
}

function convertFloat32ToPcm16Buffer(samples) {
  const pcm16 = new Int16Array(samples.length)
  for (let index = 0; index < samples.length; index += 1) {
    const sample = Math.max(-1, Math.min(1, samples[index]))
    pcm16[index] = sample < 0 ? sample * 0x8000 : sample * 0x7fff
  }
  return pcm16.buffer
}

function sendStreamingSamples(force = false) {
  if (!streamingSocket || streamingSocket.readyState !== WebSocket.OPEN || !currentSessionId.value) {
    return
  }

  if (!force) {
    while (streamingPendingSampleCount >= STREAM_FLUSH_SAMPLES) {
      const chunk = takeStreamingSamples(STREAM_FLUSH_SAMPLES)
      streamingSocket.send(convertFloat32ToPcm16Buffer(chunk))
    }
    return
  }

  if (streamingPendingSampleCount > 0) {
    const chunk = takeStreamingSamples(streamingPendingSampleCount)
    streamingSocket.send(convertFloat32ToPcm16Buffer(chunk))
  }
}

function queueStreamingSamples(samples) {
  if (!samples?.length) {
    return
  }

  audioLevel.value = calculateAudioLevel(samples)
  streamingPendingSamples.push(samples)
  streamingPendingSampleCount += samples.length
  sendStreamingSamples(false)
}

function cleanupRealtimeDetector() {
  silenceDetector?.stop?.()
  silenceDetector = null
  audioLevel.value = 0
}

function clearRealtimeMaxSegmentTimer() {
  if (realtimeMaxSegmentTimer) {
    clearTimeout(realtimeMaxSegmentTimer)
    realtimeMaxSegmentTimer = null
  }
}

function stopPolling() {
  if (pollingTimer) {
    clearInterval(pollingTimer)
    pollingTimer = null
  }
  pollingInFlight = false
}

function resetStreamingState() {
  cleanupStreamingAudio()
  cleanupStreamingSocket()
  streamingConnectionState.value = 'idle'
  currentSessionId.value = null
  liveTranscript.value = ''
}

function resetRealtimeState() {
  cleanupRealtimeDetector()
  clearRealtimeMaxSegmentTimer()
  realtimeStopRequested = false
  realtimeFinalizing = false
  realtimePendingUploads = 0
  realtimePendingSegmentIds = new Set()
  realtimeCompletedSegmentIds = new Set()
  realtimeHasCapturedSpeech = false
  realtimeSegmentStartedAt = 0
  realtimeRestartAfterStop = false
  currentSessionId.value = null
  processedSegmentCount.value = 0
  liveTranscript.value = ''
  liveSegments.value = []
  livePhase.value = 'idle'
  sessionStatusText.value = '按住快捷键开始录音'
}

function updateLiveTranscriptPreview() {
  const orderedSegments = [...liveSegments.value].sort((left, right) => {
    const leftSequence = typeof left.sequence === 'number' ? left.sequence : Number.MAX_SAFE_INTEGER
    const rightSequence = typeof right.sequence === 'number' ? right.sequence : Number.MAX_SAFE_INTEGER
    return leftSequence - rightSequence
  })

  liveSegments.value = orderedSegments
  liveTranscript.value = orderedSegments.map((segment) => segment.text).join('\n\n')

  if (floatingSessionActive && activeRecordingMode.value === 'multi') {
    syncFloatingWindow({
      phase: 'recording',
      duration: duration.value,
      text: liveTranscript.value
    })
  }
}

function syncRecordIntoStore(record) {
  const recordsStore = useRecordsStore()
  const existing = recordsStore.records.find((item) => item.id === record.id)

  if (existing) {
    recordsStore.updateRecordStatus(record.id, record)
    return
  }

  recordsStore.addRecord(record)
}

function setRealtimePhase(phase, text) {
  livePhase.value = phase

  if (text) {
    sessionStatusText.value = text
    return
  }

  if (phase === 'listening') {
    sessionStatusText.value = '静音切段已启用，停顿后会自动出字'
  } else if (phase === 'speaking') {
    sessionStatusText.value = '正在监听语音片段...'
  } else if (phase === 'segment-processing') {
    sessionStatusText.value = '上一段正在转写，继续说话即可'
  } else if (phase === 'finalizing') {
    sessionStatusText.value = '正在合并整场录音结果'
  } else if (phase === 'completed') {
    sessionStatusText.value = '多段录音处理完成'
  } else if (phase === 'error') {
    sessionStatusText.value = '本次录音未形成有效结果'
  } else {
    sessionStatusText.value = '按住快捷键开始录音'
  }
}

async function syncDesktopRecordingState(syncRecordingState, recording) {
  if (recording && desktopRecordingSyncActive) {
    return { success: true, skipped: true, reason: 'already-recording' }
  }

  if (!recording && !desktopRecordingSyncActive) {
    return { success: true, skipped: true, reason: 'already-restored' }
  }

  try {
    const result = await syncRecordingState({ recording })
    if (result?.success === false) {
      console.error('[Desktop] Failed to sync recording state:', result.error)
      return result
    }

    desktopRecordingSyncActive = recording
    return result || { success: true }
  } catch (error) {
    console.error('[Desktop] Failed to sync recording state:', error)
    return { success: false, error: error.message }
  }
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

async function prepareRecordingEnvironment() {
  const settingsStore = useSettingsStore()
  const appStore = useAppStore()
  const { tray, syncRecordingState } = useElectron()

  startPending = true
  pendingStopAfterStart = false
  floatingSessionActive = true
  syncFloatingWindow({
    phase: 'recording',
    duration: 0,
    text: ''
  }, { show: true })

  const desktopSyncResult = await syncDesktopRecordingState(syncRecordingState, true)
  if (desktopSyncResult?.success === false) {
    pendingStopAfterStart = false
    duration.value = 0
    appStore.setRecording(false)
    appStore.setTranscribing(false)
    tray.setIcon('idle')
    floatingSessionActive = false
    syncFloatingWindow({
      phase: 'error',
      duration: 0,
      text: '静音失败'
    })
    appStore.showToast('静音失败')
    startPending = false
    return null
  }

  const deviceId = settingsStore.settings.audio_input_device || 'default'
  const constraints = {
    audio: {
      deviceId: deviceId === 'default' ? undefined : { exact: deviceId }
    }
  }

  stream.value = await navigator.mediaDevices.getUserMedia(constraints)

  if (pendingStopAfterStart) {
    stopMediaTracks()
    await syncDesktopRecordingState(syncRecordingState, false)
    floatingSessionActive = false
    syncFloatingWindow({
      phase: 'error',
      duration: 0,
      text: '录音时间过短'
    })
    appStore.showToast('录音时间过短')
    startPending = false
    pendingStopAfterStart = false
    return null
  }

  return {
    appStore,
    settingsStore,
    tray,
    syncRecordingState
  }
}

function createRecorder(targetStream, onStop) {
  const preferredMimeType = MediaRecorder.isTypeSupported('audio/webm;codecs=opus')
    ? 'audio/webm;codecs=opus'
    : undefined

  const recorder = preferredMimeType
    ? new MediaRecorder(targetStream, { mimeType: preferredMimeType })
    : new MediaRecorder(targetStream)

  const localChunks = []
  recorder.ondataavailable = (event) => {
    if (event.data.size > 0) {
      localChunks.push(event.data)
    }
  }

  recorder.onstop = async () => {
    const blob = new Blob(localChunks, { type: recorder.mimeType || 'audio/webm' })
    await onStop(blob)
  }

  return recorder
}

async function ensureStreamingRuntimeAvailable() {
  const settingsStore = useSettingsStore()

  if (settingsStore.settings.asr_provider !== 'qwen_asr') {
    return {
      available: false,
      reason: '真流式模式仅支持 Qwen3-ASR provider'
    }
  }

  try {
    const runtimeStatus = await settingsStore.loadRuntimeStatus(true)
    if (runtimeStatus.qwen_streaming_available) {
      return {
        available: true,
        runtimeStatus
      }
    }

    return {
      available: false,
      reason: runtimeStatus.qwen_streaming_error ||
        `当前后端环境 ${runtimeStatus.runtime_environment || 'unknown'} 不支持真流式`
    }
  } catch (error) {
    return {
      available: false,
      reason: error.message || '无法探测真流式运行时状态'
    }
  }
}

async function connectStreamingSocket(language) {
  const { api } = useElectron()

  streamingConnectionState.value = 'connecting'
  const url = await api.getStreamingWebSocketUrl()

  await new Promise((resolve, reject) => {
    let settled = false
    streamingSocket = new WebSocket(url)
    streamingSocket.binaryType = 'arraybuffer'

    const fail = (error) => {
      if (settled) {
        return
      }
      settled = true
      reject(error instanceof Error ? error : new Error(String(error)))
    }

    streamingSocket.onopen = () => {
      streamingSocket.send(JSON.stringify({
        type: 'stream_start',
        language: language || 'zh'
      }))
    }

    streamingSocket.onerror = () => {
      fail(new Error('流式连接建立失败'))
    }

    streamingSocket.onclose = () => {
      if (!settled && streamingConnectionState.value === 'connecting') {
        fail(new Error('流式连接已关闭'))
      } else if (!streamingAwaitingFinish && streamingConnectionState.value !== 'idle') {
        streamingConnectionState.value = 'error'
      }
    }

    streamingSocket.onmessage = async (event) => {
      try {
        const message = JSON.parse(event.data)

        if (message.type === 'stream_ready') {
          currentSessionId.value = message.data?.session_id || null
          streamingConnectionState.value = 'streaming'
          if (!settled) {
            settled = true
            resolve(message)
          }
          return
        }

        if (message.type === 'stream_partial') {
          updateStreamingTranscript(message.data?.text || '')
          sessionStatusText.value = 'Qwen 真流式转写中'
          livePhase.value = 'streaming'
          return
        }

        if (message.type === 'stream_finished') {
          const appStore = useAppStore()
          const recordsStore = useRecordsStore()
          const { tray } = useElectron()
          const payload = message.data || {}
          const record = payload.record || null

          if (record) {
            syncRecordIntoStore(record)
            appStore.setCurrentRecordId(record.id)
            appStore.setCurrentRecord(record)
            appStore.setRecordStatus({ record_id: record.id, status: record.status })
          }

          const finalText = payload.optimized_text || payload.raw_text || record?.optimized_text || record?.raw_text || ''
          updateStreamingTranscript(finalText)
          livePhase.value = 'completed'
          sessionStatusText.value = 'Qwen 真流式录音已完成'
          appStore.setRecording(false)
          appStore.setTranscribing(false)
          tray.setIcon('idle')

          if (floatingSessionActive) {
            syncFloatingWindow({
              phase: 'complete',
              duration: 0,
              text: finalText
            })
            floatingSessionActive = false
          }

          const copyResult = await maybeCopyText(finalText)
          appStore.showToast(
            copyResult.pasted
              ? '真流式转写完成，已复制并粘贴'
              : copyResult.attempted
                ? '真流式转写完成，复制或粘贴失败'
                : '真流式转写完成'
          )

          streamingAwaitingFinish = false
          cleanupStreamingSocket()
          currentSessionId.value = null
          return
        }

        if (message.type === 'stream_cancelled') {
          streamingAwaitingFinish = false
          cleanupStreamingSocket()
          currentSessionId.value = null
          return
        }

        if (message.type === 'stream_error') {
          throw new Error(message.data?.error || '流式转写失败')
        }
      } catch (error) {
        if (!settled) {
          fail(error)
          return
        }

        const appStore = useAppStore()
        const { tray } = useElectron()
        cleanupStreamingAudio()
        cleanupStreamingSocket()
        stopMediaTracks()
        clearDurationTimer()
        appStore.setRecording(false)
        appStore.setTranscribing(false)
        tray.setIcon('idle')
        livePhase.value = 'error'
        sessionStatusText.value = error.message || '流式转写失败'
        if (floatingSessionActive) {
          syncFloatingWindow({
            phase: 'error',
            duration: 0,
            text: error.message || '流式转写失败'
          })
          floatingSessionActive = false
        }
        appStore.showToast(error.message || '流式转写失败')
      }
    }
  })
}

async function setupStreamingAudioCapture(targetStream) {
  const workletUrl = new URL('../worklets/pcm-capture-worklet.js', import.meta.url)

  streamingAudioContext = new AudioContext({ sampleRate: STREAM_SAMPLE_RATE })
  if (streamingAudioContext.sampleRate !== STREAM_SAMPLE_RATE) {
    throw new Error(`当前浏览器返回的采样率为 ${streamingAudioContext.sampleRate}Hz，无法启用真流式`)
  }
  await streamingAudioContext.audioWorklet.addModule(workletUrl.href)

  streamingSourceNode = streamingAudioContext.createMediaStreamSource(targetStream)
  streamingWorkletNode = new AudioWorkletNode(streamingAudioContext, 'pcm-capture-processor')
  streamingWorkletNode.port.onmessage = (event) => {
    if (!(event.data instanceof Float32Array)) {
      return
    }
    queueStreamingSamples(event.data)
  }

  streamingSourceNode.connect(streamingWorkletNode)
  const silentGainNode = streamingAudioContext.createGain()
  silentGainNode.gain.value = 0
  streamingWorkletNode.connect(silentGainNode)
  silentGainNode.connect(streamingAudioContext.destination)
}

async function handleShortPressCancellation(syncRecordingState) {
  const appStore = useAppStore()
  const { tray } = useElectron()

  pendingStopAfterStart = false
  clearDurationTimer()
  duration.value = 0
  appStore.setRecording(false)
  appStore.setTranscribing(false)
  tray.setIcon('idle')
  await syncDesktopRecordingState(syncRecordingState, false)

  if (floatingSessionActive) {
    syncFloatingWindow({
      phase: 'error',
      duration: 0,
      text: '录音时间过短'
    })
  }

  floatingSessionActive = false
  resetRealtimeState()
  appStore.showToast('录音时间过短')
}

async function uploadSingleRecording(blob) {
  const appStore = useAppStore()
  const recordsStore = useRecordsStore()
  const { api, tray } = useElectron()

  try {
    if (duration.value < 1) {
      appStore.showToast('录音时间过短')
      appStore.setTranscribing(false)
      tray.setIcon('idle')
      if (floatingSessionActive) {
        syncFloatingWindow({
          phase: 'error',
          duration: 0,
          text: '录音时间过短'
        })
        floatingSessionActive = false
      }
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
    if (floatingSessionActive) {
      syncFloatingWindow({
        phase: 'error',
        duration: 0,
        text: '上传录音失败'
      })
      floatingSessionActive = false
    }
    appStore.showToast('上传录音失败')
  }
}

async function uploadRealtimeSegment(blob, segmentDuration, options = {}) {
  const { api } = useElectron()
  const { force = false } = options

  if (!currentSessionId.value || !blob.size) {
    return
  }

  if (!force && segmentDuration < MIN_REALTIME_SEGMENT_DURATION) {
    return
  }

  if (force && segmentDuration < 0.8 && !realtimeHasCapturedSpeech) {
    return
  }

  realtimePendingUploads += 1
  setRealtimePhase(realtimeStopRequested ? 'finalizing' : 'segment-processing')

  try {
    const result = await api.uploadSessionSegment(
      currentSessionId.value,
      blob,
      `segment_${Date.now()}.webm`
    )

    if (result?.segment_id) {
      realtimePendingSegmentIds.add(result.segment_id)
      processedSegmentCount.value += 1
      realtimeHasCapturedSpeech = true
    }
  } finally {
    realtimePendingUploads = Math.max(0, realtimePendingUploads - 1)
  }
}

async function finalizeRealtimeSession() {
  const appStore = useAppStore()
  const settingsStore = useSettingsStore()
  const { api, tray } = useElectron()

  if (!currentSessionId.value || realtimeFinalizing) {
    return
  }

  if (mediaRecorder.value || realtimePendingUploads > 0 || realtimePendingSegmentIds.size > 0) {
    return
  }

  const sessionId = currentSessionId.value
  realtimeFinalizing = true
  appStore.setTranscribing(true)
  tray.setIcon('transcribing')
  setRealtimePhase('finalizing')

  if (floatingSessionActive) {
    syncFloatingWindow({
      phase: settingsStore.settings.auto_optimize ? 'optimizing' : 'transcribing',
      duration: 0,
      text: ''
    })
  }

  try {
    const noSpeechCaptured = !realtimeHasCapturedSpeech || processedSegmentCount.value === 0
    if (noSpeechCaptured) {
      await api.deleteSession(sessionId).catch(() => {})
      throw new Error(duration.value >= 1 ? '未检测到有效语音' : '录音时间过短')
    }

    const merged = await api.mergeSession(sessionId)
    liveTranscript.value = merged.merged_text || liveTranscript.value

    let finalRecord = merged.record || null
    if (!finalRecord && merged.record_id) {
      finalRecord = await api.getRecord(merged.record_id).catch(() => null)
    }

    if (finalRecord) {
      syncRecordIntoStore(finalRecord)
      appStore.setCurrentRecordId(finalRecord.id)
      appStore.setCurrentRecord(finalRecord)
      appStore.setRecordStatus({ record_id: finalRecord.id, status: finalRecord.status })
    }

    const finalText = merged.optimized_text || finalRecord?.optimized_text || finalRecord?.raw_text || merged.merged_text || liveTranscript.value
    liveTranscript.value = finalText || liveTranscript.value
    setRealtimePhase('completed')

    if (floatingSessionActive) {
      syncFloatingWindow({
        phase: 'complete',
        duration: 0,
        text: finalText || ''
      })
      floatingSessionActive = false
    }

    const copyResult = await maybeCopyText(finalText)
    appStore.showToast(
      copyResult.pasted
        ? '多段录音完成，已复制并粘贴'
        : copyResult.attempted
          ? '多段录音完成，复制并粘贴失败'
          : '多段录音完成'
    )
  } catch (error) {
    console.error('[Recorder] Failed to finalize realtime session:', error)
    setRealtimePhase('error', error.message || '多段录音处理失败')
    if (floatingSessionActive) {
      syncFloatingWindow({
        phase: 'error',
        duration: 0,
        text: error.message || '多段录音处理失败'
      })
      floatingSessionActive = false
    }
    appStore.showToast(error.message || '多段录音处理失败')
  } finally {
    appStore.setTranscribing(false)
    tray.setIcon('idle')
    stopMediaTracks()
    clearRealtimeMaxSegmentTimer()
    cleanupRealtimeDetector()
    mediaRecorder.value = null
    currentSessionId.value = null
    realtimePendingSegmentIds.clear()
    realtimeCompletedSegmentIds.clear()
    realtimeFinalizing = false
  }
}

async function maybeFinalizeRealtimeSession() {
  if (!realtimeStopRequested) {
    return
  }

  await finalizeRealtimeSession()
}

function startRealtimeSegment() {
  if (!stream.value || mediaRecorder.value || realtimeFinalizing) {
    return
  }

  realtimeSegmentStartedAt = performance.now()
  realtimeRestartAfterStop = false
  mediaRecorder.value = createRecorder(stream.value, async (blob) => {
    const shouldRestart = realtimeRestartAfterStop
    realtimeRestartAfterStop = false
    clearRealtimeMaxSegmentTimer()
    mediaRecorder.value = null
    const segmentDuration = (performance.now() - realtimeSegmentStartedAt) / 1000
    realtimeSegmentStartedAt = 0
    const shouldContinueCapture = shouldRestart && !realtimeStopRequested && !realtimeFinalizing

    if (shouldContinueCapture) {
      startRealtimeSegment()
    }

    await uploadRealtimeSegment(blob, segmentDuration, { force: realtimeStopRequested || shouldRestart })

    if (shouldContinueCapture) {
      return
    }

    if (!realtimeStopRequested && !realtimeFinalizing) {
      setRealtimePhase('listening')
    }

    await maybeFinalizeRealtimeSession()
  })

  mediaRecorder.value.start(250)
  setRealtimePhase('speaking')
  clearRealtimeMaxSegmentTimer()
  realtimeMaxSegmentTimer = setTimeout(() => {
    if (!mediaRecorder.value || mediaRecorder.value.state === 'inactive') {
      return
    }

    realtimeRestartAfterStop = true
    setRealtimePhase('segment-processing', '当前语句较长，正在拆分为较大的片段')
    stopRealtimeSegment({ force: true })
  }, MAX_REALTIME_SEGMENT_DURATION * 1000)
}

function stopRealtimeSegment(options = {}) {
  const { force = false } = options

  if (!mediaRecorder.value || mediaRecorder.value.state === 'inactive') {
    return
  }

  const segmentDuration = realtimeSegmentStartedAt
    ? (performance.now() - realtimeSegmentStartedAt) / 1000
    : 0

  if (!force && !realtimeStopRequested && segmentDuration < MIN_REALTIME_SEGMENT_DURATION) {
    setRealtimePhase('speaking', '停顿过短，继续合并当前语句')
    return
  }

  mediaRecorder.value.stop()
}

async function startStreamingRecording() {
  const context = await prepareRecordingEnvironment()
  if (!context) {
    return
  }

  const { appStore, tray } = context
  const { sendRecordingStatus, sendWebRecording } = useGlobalWebSocket()

  try {
    activeRecordingMode.value = 'streaming'
    resetRealtimeState()
    resetStreamingState()
    livePhase.value = 'connecting'
    sessionStatusText.value = '正在连接 Qwen 真流式服务'

    if (context.settingsStore.settings.asr_provider !== 'qwen_asr') {
      throw new Error('真流式模式仅支持 Qwen3-ASR provider')
    }

    await connectStreamingSocket(context.settingsStore.settings.asr_language || 'zh')
    await setupStreamingAudioCapture(stream.value)

    if (pendingStopAfterStart) {
      pendingStopAfterStart = false
      await stopStreamingRecording()
      return
    }

    appStore.setRecording(true)
    tray.setIcon('recording')
    sendWebRecording('start')
    sendRecordingStatus({
      recording: true,
      transcribing: false,
      duration: 0
    })
    startDurationClock()
    livePhase.value = 'streaming'
    sessionStatusText.value = 'Qwen 真流式转写中'
  } catch (error) {
    console.error('[Recorder] Failed to start streaming recording:', error)
    await syncDesktopRecordingState(context.syncRecordingState, false)
    cleanupStreamingAudio()
    cleanupStreamingSocket()
    stopMediaTracks()
    mediaRecorder.value = null
    appStore.setRecording(false)
    if (floatingSessionActive) {
      syncFloatingWindow({
        phase: 'error',
        duration: 0,
        text: error.message || '无法启动真流式录音'
      })
      floatingSessionActive = false
    }
    appStore.showToast(error.message || '无法启动真流式录音')
  } finally {
    startPending = false
  }
}

async function stopStreamingRecording() {
  const appStore = useAppStore()
  const { tray, syncRecordingState } = useElectron()
  const { sendRecordingStatus, sendWebRecording } = useGlobalWebSocket()

  if (startPending && !appStore.isRecording) {
    pendingStopAfterStart = true
    return
  }

  if (streamingAwaitingFinish || (!appStore.isRecording && !currentSessionId.value)) {
    return
  }

  pendingStopAfterStart = false
  streamingAwaitingFinish = true
  clearDurationTimer()
  appStore.setRecording(false)
  appStore.setTranscribing(true)
  syncDesktopRecordingState(syncRecordingState, false).catch(() => {})
  tray.setIcon('transcribing')
  sendWebRecording('stop')
  sendRecordingStatus({
    recording: false,
    transcribing: true,
    duration: duration.value
  })

  if (floatingSessionActive) {
    syncFloatingWindow({
      phase: 'transcribing',
      duration: 0,
      text: ''
    })
  }

  livePhase.value = 'finalizing'
  sessionStatusText.value = '正在完成 Qwen 真流式转写'
  cleanupStreamingAudio()
  stopMediaTracks()

  if (streamingSocket && streamingSocket.readyState === WebSocket.OPEN) {
    sendStreamingSamples(true)
    streamingSocket.send(JSON.stringify({ type: 'stream_finish' }))
    return
  }

  streamingAwaitingFinish = false
  cleanupStreamingSocket()
  appStore.setTranscribing(false)
  tray.setIcon('idle')
  appStore.showToast('流式连接已断开，无法完成转写')
}

async function startSingleRecording() {
  const context = await prepareRecordingEnvironment()
  if (!context) {
    return
  }

  const { appStore, tray } = context
  const { sendRecordingStatus, sendWebRecording } = useGlobalWebSocket()

  try {
    activeRecordingMode.value = 'single'
    mediaRecorder.value = createRecorder(stream.value, async (blob) => {
      mediaRecorder.value = null
      stopMediaTracks()
      await uploadSingleRecording(blob)
    })

    if (pendingStopAfterStart) {
      mediaRecorder.value = null
      stopMediaTracks()
      await handleShortPressCancellation(context.syncRecordingState)
      return
    }

    mediaRecorder.value.start(1000)
    appStore.setRecording(true)
    tray.setIcon('recording')
    sendWebRecording('start')
    sendRecordingStatus({
      recording: true,
      transcribing: false,
      duration: 0
    })
    startDurationClock()
    resetRealtimeState()
    resetStreamingState()
  } catch (error) {
    console.error('[Recorder] Failed to start recording:', error)
    await syncDesktopRecordingState(context.syncRecordingState, false)
    stopMediaTracks()
    mediaRecorder.value = null
    appStore.setRecording(false)
    if (floatingSessionActive) {
      syncFloatingWindow({
        phase: 'error',
        duration: 0,
        text: '无法访问麦克风'
      })
      floatingSessionActive = false
    }
    appStore.showToast('无法访问麦克风')
  } finally {
    startPending = false
  }
}

async function startMultiRecording() {
  const context = await prepareRecordingEnvironment()
  if (!context) {
    return
  }

  const { appStore, tray } = context
  const { api } = useElectron()
  const { sendRecordingStatus, sendWebRecording } = useGlobalWebSocket()
  const settingsStore = useSettingsStore()

  try {
    activeRecordingMode.value = 'multi'
    resetRealtimeState()
    const session = await api.createSession()
    currentSessionId.value = session.session_id

    if (pendingStopAfterStart) {
      await api.deleteSession(currentSessionId.value).catch(() => {})
      stopMediaTracks()
      await handleShortPressCancellation(context.syncRecordingState)
      return
    }

    setRealtimePhase('listening')

    appStore.setRecording(true)
    tray.setIcon('recording')
    sendWebRecording('start')
    sendRecordingStatus({
      recording: true,
      transcribing: false,
      duration: 0
    })
    startDurationClock()
    resetStreamingState()

    silenceDetector = createSilenceDetector({
      stream: stream.value,
      silenceLevel: settingsStore.settings.silence_level ?? -40,
      silenceThreshold: Math.max((settingsStore.settings.silence_threshold ?? 1.5) * 1000, 400),
      onLevelChange: ({ normalizedLevel }) => {
        audioLevel.value = normalizedLevel
      },
      onSpeechStart: () => {
        if (!realtimeStopRequested) {
          startRealtimeSegment()
        }
      },
      onSpeechEnd: () => {
        stopRealtimeSegment()
      }
    })
  } catch (error) {
    console.error('[Recorder] Failed to start realtime recording:', error)
    await syncDesktopRecordingState(context.syncRecordingState, false)
    cleanupRealtimeDetector()
    stopMediaTracks()
    mediaRecorder.value = null
    appStore.setRecording(false)
    if (floatingSessionActive) {
      syncFloatingWindow({
        phase: 'error',
        duration: 0,
        text: '无法启动多段录音'
      })
      floatingSessionActive = false
    }
    appStore.showToast('无法启动多段录音')
    resetRealtimeState()
  } finally {
    startPending = false
  }
}

async function startRecording() {
  const appStore = useAppStore()
  const settingsStore = useSettingsStore()

  if (
    startPending ||
    appStore.isRecording ||
    appStore.isTranscribing ||
    mediaRecorder.value?.state === 'recording'
  ) {
    return
  }

  if (settingsStore.settings.recording_mode === 'multi') {
    await startMultiRecording()
    return
  }

  if (settingsStore.settings.recording_mode === 'streaming') {
    const streamingRuntime = await ensureStreamingRuntimeAvailable()
    if (!streamingRuntime.available) {
      appStore.showToast(`当前后端不支持真流式，已自动降级为单段录音：${streamingRuntime.reason}`)
      await startSingleRecording()
      return
    }

    await startStreamingRecording()
    return
  }

  await startSingleRecording()
}

function stopSingleRecording() {
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
  syncDesktopRecordingState(syncRecordingState, false).catch(() => {})
  tray.setIcon('idle')
  sendWebRecording('stop')
  sendRecordingStatus({
    recording: false,
    transcribing: false,
    duration: duration.value
  })
  if (floatingSessionActive && duration.value >= 1) {
    syncFloatingWindow({
      phase: 'transcribing',
      duration: 0,
      text: ''
    })
  }
  mediaRecorder.value.stop()
}

function stopMultiRecording() {
  const appStore = useAppStore()
  const { tray, syncRecordingState } = useElectron()
  const { sendRecordingStatus, sendWebRecording } = useGlobalWebSocket()

  if (startPending && !appStore.isRecording) {
    pendingStopAfterStart = true
    return
  }

  if (!appStore.isRecording && !currentSessionId.value) {
    return
  }

  pendingStopAfterStart = false
  realtimeStopRequested = true
  clearDurationTimer()
  appStore.setRecording(false)
  syncDesktopRecordingState(syncRecordingState, false).catch(() => {})
  cleanupRealtimeDetector()
  tray.setIcon('idle')
  sendWebRecording('stop')
  sendRecordingStatus({
    recording: false,
    transcribing: false,
    duration: duration.value
  })

  if (floatingSessionActive) {
    syncFloatingWindow({
      phase: 'transcribing',
      duration: 0,
      text: ''
    })
  }

  if (mediaRecorder.value && mediaRecorder.value.state !== 'inactive') {
    stopRealtimeSegment({ force: true })
    return
  }

  stopMediaTracks()
  void maybeFinalizeRealtimeSession()
}

function stopRecording() {
  if (activeRecordingMode.value === 'streaming') {
    void stopStreamingRecording()
    return
  }

  if (activeRecordingMode.value === 'multi') {
    stopMultiRecording()
    return
  }

  stopSingleRecording()
}

async function handleWebSocketMessage(message) {
  const appStore = useAppStore()
  const settingsStore = useSettingsStore()
  const recordsStore = useRecordsStore()
  const { tray } = useElectron()

  if (message.type === 'recording_command') {
    if (message.action === 'start') {
      await startRecording()
    } else if (message.action === 'stop') {
      stopRecording()
    }
    return
  }

  if (message.type === 'segment_transcribed') {
    const sessionId = message.data?.session_id
    const segmentId = message.data?.segment_id
    if (!sessionId || sessionId !== currentSessionId.value || !segmentId || realtimeCompletedSegmentIds.has(segmentId)) {
      return
    }

    realtimeCompletedSegmentIds.add(segmentId)
    realtimePendingSegmentIds.delete(segmentId)

    const segmentText = message.data?.text?.trim()
    if (segmentText) {
      liveSegments.value = [
        ...liveSegments.value,
        {
          id: segmentId,
          sequence: message.data?.sequence,
          text: segmentText,
          duration: message.data?.duration || 0
        }
      ]
      updateLiveTranscriptPreview()
    }

    if (!realtimeStopRequested && !realtimeFinalizing) {
      setRealtimePhase('listening')
    }

    await maybeFinalizeRealtimeSession()
    return
  }

  if (message.type === 'segment_failed') {
    const sessionId = message.data?.session_id
    const segmentId = message.data?.segment_id
    if (!sessionId || sessionId !== currentSessionId.value || !segmentId) {
      return
    }

    realtimePendingSegmentIds.delete(segmentId)

    if (!realtimeStopRequested && !realtimeFinalizing) {
      setRealtimePhase('listening', '有一个片段转写失败，已继续监听后续语音')
    }

    await maybeFinalizeRealtimeSession()
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
    if (floatingSessionActive) {
      syncFloatingWindow({
        phase: 'transcribing',
        duration: 0,
        text: ''
      })
    }
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
      if (floatingSessionActive) {
        syncFloatingWindow({
          phase: 'complete',
          duration: 0,
          text: message.data?.text || ''
        })
        floatingSessionActive = false
      }
      const copyResult = await maybeCopyText(message.data?.text)
      appStore.showToast(
        copyResult.pasted
          ? '转写完成，已复制并粘贴'
          : copyResult.attempted
            ? '转写完成，复制并粘贴失败'
            : '转写完成'
      )
    } else {
      if (floatingSessionActive) {
        syncFloatingWindow({
          phase: 'optimizing',
          duration: 0,
          text: ''
        })
      }
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
    if (floatingSessionActive) {
      syncFloatingWindow({
        phase: 'optimizing',
        duration: 0,
        text: ''
      })
    }
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
    if (floatingSessionActive) {
      syncFloatingWindow({
        phase: 'complete',
        duration: 0,
        text: message.data?.optimized_text || ''
      })
      floatingSessionActive = false
    }
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
    if (floatingSessionActive) {
      syncFloatingWindow({
        phase: 'error',
        duration: 0,
        text: message.data?.error || '处理失败'
      })
      floatingSessionActive = false
    }
    appStore.showToast(message.data?.error || '处理失败')
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
      if (floatingSessionActive) {
        syncFloatingWindow({
          phase: 'complete',
          duration: 0,
          text: status.optimized_text || ''
        })
        floatingSessionActive = false
      }
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
        if (floatingSessionActive) {
          syncFloatingWindow({
            phase: 'complete',
            duration: 0,
            text: status.raw_text || ''
          })
          floatingSessionActive = false
        }
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
      if (floatingSessionActive) {
        syncFloatingWindow({
          phase: 'error',
          duration: 0,
          text: '转写失败'
        })
        floatingSessionActive = false
      }
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

export function useRecorder() {
  const appStore = useAppStore()
  const settingsStore = useSettingsStore()
  const { connect, addMessageHandler, removeMessageHandler } = useGlobalWebSocket()
  const { onHotkeyStart, onHotkeyStop, removeAllListeners } = useElectron()

  onMounted(async () => {
    mountedInstances += 1
    await connect()

    if (!wsAttached) {
      addMessageHandler(handleWebSocketMessage)
      wsAttached = true
    }

    if (!desktopHotkeyAttached) {
      onHotkeyStart(() => {
        void startRecording()
      })
      onHotkeyStop(() => {
        stopRecording()
      })
      desktopHotkeyAttached = true
    }
  })

  onUnmounted(() => {
    mountedInstances -= 1

    if (mountedInstances <= 0) {
      if (wsAttached) {
        removeMessageHandler(handleWebSocketMessage)
        wsAttached = false
      }

      removeAllListeners('hotkey:start')
      removeAllListeners('hotkey:stop')
      desktopHotkeyAttached = false
      mountedInstances = 0
    }
  })

  return {
    isRecording: computed(() => appStore.isRecording),
    isTranscribing: computed(() => appStore.isTranscribing),
    duration,
    audioLevel,
    liveTranscript,
    liveSegments,
    livePhase,
    activeRecordingMode,
    processedSegmentCount,
    sessionStatusText,
    isMultiModeEnabled: computed(() => settingsStore.settings.recording_mode === 'multi'),
    isStreamingModeEnabled: computed(() => settingsStore.settings.recording_mode === 'streaming'),
    streamingConnectionState,
    currentSessionId,
    startRecording,
    stopRecording,
    startPolling,
    stopPolling
  }
}
