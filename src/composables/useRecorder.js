import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useAppStore } from '@/stores/app'
import { useRecordsStore } from '@/stores/records'
import { useSettingsStore } from '@/stores/settings'
import { useOptimizationStreamStore } from '@/stores/optimizationStream'
import { useElectron } from './useElectron'
import { useGlobalWebSocket } from './useWebSocket'
import { createSilenceDetector } from './useSilenceDetector'
import { derivePostStopState, deriveStartAbortState, shouldQueueStopDuringStart } from '@/utils/recordingStopState.mjs'
// 第 5 轮"安全抽取"出的纯逻辑模块（不依赖 Vue / 浏览器全局对象，可被 Node 直接单测）。
// 本文件保留全部时序编排：startRecording / stopRecording 主流程、AudioContext、
// WebSocket 生命周期、热键与悬浮窗同步，只把纯计算与纯判定委托给这些模块。
import * as floatingSession from './recorder/floatingSession.mjs'
import * as pcm from './recorder/pcm.mjs'
import * as segmentPolicy from './recorder/segmentPolicy.mjs'
import * as pollPolicy from './recorder/pollPolicy.mjs'
import * as streamProtocol from './recorder/streamProtocol.mjs'
import * as statusCopy from './recorder/statusCopy.mjs'

const STREAM_SAMPLE_RATE = 16000
// 流式连接必须在限定时间内 settle，否则调用方的 finally 永不执行，startPending 会永久为真
const STREAM_CONNECT_TIMEOUT_MS = 8000
// 后台优化看门狗：迟迟收不到 optimization_completed / optimization_failed 时按原文收尾，
// 避免悬浮窗永久停在“正在优化”。取 60s，与 LLM 模型默认 timeout_s 对齐。
const OPTIMIZATION_WATCHDOG_MS = 60000

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
// 当前轮询绑定的 record_id：旧录音的后台优化事件不能停掉新录音的轮询
let pollingRecordId = null
// 连续轮询失败次数：超过失败上限（见 recorder/pollPolicy.mjs）必须停止并提示，避免静默永久轮询
let pollingFailureCount = 0
let mountedInstances = 0
let wsAttached = false
let desktopHotkeyAttached = false
let startPending = false
let pendingStopAfterStart = false
let pollingInFlight = false
let floatingSessionActive = false
// 当前悬浮窗绑定的记录 id（空串 = 新会话尚未拿到 record_id）。
// 后台优化事件可能晚于下一次录音到达，收尾前必须核对它属于哪个会话。
let floatingSessionRecordId = ''
let optimizationWatchdog = null
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
let realtimeSegmentHasSpeech = false
let realtimeSpeechActive = false
let streamingSocket = null
let streamingAudioContext = null
let streamingSourceNode = null
let streamingWorkletNode = null
let streamingPendingSamples = []
let streamingPendingSampleCount = 0
let streamingAwaitingFinish = false
// 采集代次：cleanupStreamingAudio 每次收口时自增。采集与建连并行后，setupStreamingAudioCapture
// 必须在 await 之后核对它，才能发现「本次启动已被放弃 / 已收尾」，从而回滚自己刚建的节点——
// 否则采集会晚于收口落地，留下无人清理的 AudioContext + worklet（幽灵采集、内存持续增长）。
// 前置条件：任何新增的流式收口入口都必须调用 cleanupStreamingAudio()，只关 socket / 只停轨
// 不会让代次自增，守卫会失效（看门狗与 stream_finished 分支目前安全，仅因 stop 已先收口）。
let streamingCaptureGeneration = 0
// 启动放弃标志：连接/采集就绪前用户已松手时置位（见 stopStreamingRecording），
// startStreamingRecording 在每个 await 之后检查它，避免开启幽灵录音
let streamingStartAborted = false
const completedPhases = new Set()

function logRecorderHotkey(message, details = {}) {
  const appStore = useAppStore()
  const { debugLog } = useElectron()

  debugLog('renderer.recorder', message, {
    isRecording: appStore.isRecording,
    isTranscribing: appStore.isTranscribing,
    startPending,
    pendingStopAfterStart,
    activeRecordingMode: activeRecordingMode.value,
    mediaRecorderState: mediaRecorder.value?.state || 'none',
    duration: duration.value,
    ...details
  })
}

function applyAbortedStartState(appStore, tray) {
  const nextState = deriveStartAbortState()
  appStore.setRecording(nextState.recording)
  appStore.setTranscribing(nextState.transcribing)
  tray.setIcon(nextState.trayIcon)
}

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

/**
 * 悬浮窗会话收尾：仅在会话活跃时同步一次终态并复位活跃标志。
 * 等价于原先在终止 / 失败路径上重复了十几遍的
 * `if (floatingSessionActive) { syncFloatingWindow({ phase, duration: 0, text }); floatingSessionActive = false }`，
 * 同步顺序与文案完全不变，只是去掉了重复代码。
 */
function completeFloatingSession(phase, text) {
  if (!floatingSessionActive) {
    return
  }

  syncFloatingWindow({
    phase,
    duration: 0,
    text
  })
  floatingSessionActive = false
  floatingSessionRecordId = ''
  clearOptimizationWatchdog()
}

function clearOptimizationWatchdog() {
  if (!optimizationWatchdog) {
    return
  }

  clearTimeout(optimizationWatchdog)
  optimizationWatchdog = null
}

/**
 * 后台优化迟迟没有回包时按原文收尾：宁可让用户先拿到转写文本，
 * 也不要把悬浮窗永久钉在“正在优化”。
 */
function armOptimizationWatchdog(recordId, fallbackText) {
  clearOptimizationWatchdog()
  optimizationWatchdog = setTimeout(() => {
    optimizationWatchdog = null

    if (!ownsFloatingRecord(recordId)) {
      return
    }

    const appStore = useAppStore()
    if (appStore.isRecording || startPending) {
      // 用户已经开了下一段录音：上一段的超时不能动新会话的状态
      return
    }

    const { tray } = useElectron()
    appStore.setTranscribing(false)
    tray.setIcon('idle')
    livePhase.value = 'completed'
    sessionStatusText.value = '优化超时，已保留转写原文'
    completeFloatingSession('complete', fallbackText)
    appStore.showToast('优化超时，已保留转写原文')
  }, OPTIMIZATION_WATCHDOG_MS)
}

function ownsFloatingRecord(recordId) {
  return floatingSession.ownsFloatingRecord({
    active: floatingSessionActive,
    currentRecordId: floatingSessionRecordId,
    eventRecordId: recordId
  })
}

/**
 * 流式 ASR 已收尾、后台优化仍在跑：把悬浮窗切到 optimizing 并放进看门狗。
 * 这里不调用 completeFloatingSession——那会让悬浮窗在优化完成前自动隐藏。
 */
function awaitBackgroundOptimization(recordId, text) {
  floatingSessionRecordId = recordId
  syncFloatingWindow({
    phase: 'optimizing',
    duration: 0,
    text
  })
  armOptimizationWatchdog(recordId, text)
}

function logRecordingTiming(stage, startedAt) {
  const elapsedMs = Math.round(performance.now() - startedAt)
  // 走主进程 debugLog 落 logs/electron.log：原先的 console.log 只留在渲染进程控制台，
  // 排查"按下热键到开始采集"这类跨进程时序问题时拿不到任何落盘证据。
  const { debugLog } = useElectron()
  debugLog('renderer.recorder', `[RecorderTiming] ${stage}`, { elapsedMs })
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
  // 自增采集代次：正在飞行的 setupStreamingAudioCapture 据此发现自己已被收口并回滚
  streamingCaptureGeneration += 1

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
  clearFinishWatchdog()
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

// 收尾看门狗：发出 stream_finish 后后端最长会在 ASR finish（6s 硬超时）内回包，
// 优化已改为后台任务不再阻塞收尾；15s 仍无回包基本可断定回包永远来不了
// （后端异常 / 连接已死且 close 未送达）。不收口的话界面会永久停在「正在完成流式转写」。
const STREAM_FINISH_WATCHDOG_MS = 15000
let finishWatchdog = null

function clearFinishWatchdog() {
  if (finishWatchdog) {
    clearTimeout(finishWatchdog)
    finishWatchdog = null
  }
}

function armFinishWatchdog() {
  clearFinishWatchdog()
  finishWatchdog = setTimeout(() => {
    finishWatchdog = null
    if (!streamingAwaitingFinish) {
      return
    }

    const appStore = useAppStore()
    const { tray } = useElectron()

    streamingAwaitingFinish = false
    cleanupStreamingSocket()
    currentSessionId.value = null
    appStore.setRecording(false)
    appStore.setTranscribing(false)
    tray.setIcon('idle')
    livePhase.value = 'error'
    sessionStatusText.value = '流式收尾超时，记录可能仍在后台处理'
    completeFloatingSession('error', '流式收尾超时')
    appStore.showToast('流式收尾超时，请稍后到记录集查看结果')
  }, STREAM_FINISH_WATCHDOG_MS)
}

/**
 * 流式连接在录音过程中意外断开时的完整收口。
 * 原实现只把内部状态置为 error，导致麦克风继续采集、计时器继续跑、
 * 悬浮窗永久置顶显示"正在完成流式转写"，只能重启应用。
 */
function handleUnexpectedStreamingClose() {
  const appStore = useAppStore()
  const { tray } = useElectron()

  clearDurationTimer()
  cleanupStreamingAudio()
  stopMediaTracks()
  mediaRecorder.value = null
  streamingSocket = null
  startPending = false
  streamingAwaitingFinish = false
  appStore.setRecording(false)
  appStore.setTranscribing(false)
  tray.setIcon('idle')
  livePhase.value = 'error'
  sessionStatusText.value = '流式连接已断开，请重新开始录音'

  completeFloatingSession('error', '流式连接已断开')

  appStore.showToast('流式转写连接已断开，请重新开始录音')
}

/**
 * 收尾等待中连接断开（stream_finished 永远不会到达）时的收口。
 * 与意外断开不同：此刻录音已停止、音频已推完，记录可能已由后端落库，
 * 所以文案引导去记录集确认，而不是简单要求重新录音。
 */
function handleFinishInterruptedClose() {
  const appStore = useAppStore()
  const { tray } = useElectron()

  clearDurationTimer()
  cleanupStreamingAudio()
  stopMediaTracks()
  mediaRecorder.value = null
  streamingSocket = null
  startPending = false
  streamingAwaitingFinish = false
  clearFinishWatchdog()
  currentSessionId.value = null
  appStore.setRecording(false)
  appStore.setTranscribing(false)
  tray.setIcon('idle')
  livePhase.value = 'error'
  sessionStatusText.value = '流式连接在收尾时中断，请到记录集确认结果'

  completeFloatingSession('error', '流式收尾连接中断')

  appStore.showToast('流式连接在收尾时中断，请稍后到记录集查看结果')
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

// 仅保留"改模块级缓冲状态"的部分，纯切分计算在 recorder/pcm.mjs 里。
// 注意：原实现在队列不足时也会执行 `streamingPendingSampleCount -= count`（计数器可能为负），
// 该历史行为本次不修改，逐字保留。
function takeStreamingSamples(count) {
  const { samples, remaining } = pcm.takeSamplesFromQueue(streamingPendingSamples, count)
  streamingPendingSamples = remaining
  streamingPendingSampleCount -= count
  return samples
}

function sendStreamingSamples(force = false) {
  if (!streamingSocket || streamingSocket.readyState !== WebSocket.OPEN || !currentSessionId.value) {
    return
  }

  if (!force) {
    // 分片尺寸由 planFlushChunkSizes 预先算好，等价于原来的 while 循环
    for (const chunkSize of pcm.planFlushChunkSizes(streamingPendingSampleCount, pcm.STREAM_FLUSH_SAMPLES)) {
      streamingSocket.send(pcm.convertFloat32ToPcm16Buffer(takeStreamingSamples(chunkSize)))
    }
    return
  }

  if (streamingPendingSampleCount > 0) {
    const chunk = takeStreamingSamples(streamingPendingSampleCount)
    streamingSocket.send(pcm.convertFloat32ToPcm16Buffer(chunk))
  }
}

function queueStreamingSamples(samples) {
  if (!pcm.hasSamples(samples)) {
    return
  }

  audioLevel.value = pcm.calculateAudioLevel(samples)
  streamingPendingSamples = pcm.appendSamples(streamingPendingSamples, samples)
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
  pollingFailureCount = 0
  pollingRecordId = null
}

/**
 * 只停止属于该 record 的轮询。后台优化事件可能晚于下一次录音到达，
 * 无差别 stopPolling() 会把新录音的轮询一并停掉。
 */
function stopPollingForRecord(recordId) {
  if (!recordId || pollingRecordId !== recordId) {
    return
  }

  stopPolling()
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
  realtimeSegmentHasSpeech = false
  realtimeSpeechActive = false
  currentSessionId.value = null
  processedSegmentCount.value = 0
  liveTranscript.value = ''
  liveSegments.value = []
  livePhase.value = 'idle'
  sessionStatusText.value = '按住快捷键开始录音'
}

function updateLiveTranscriptPreview() {
  // 排序与拼接是纯计算，见 recorder/segmentPolicy.mjs
  const orderedSegments = segmentPolicy.sortSegmentsBySequence(liveSegments.value)

  liveSegments.value = orderedSegments
  liveTranscript.value = segmentPolicy.joinSegmentTexts(orderedSegments)

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
  // 阶段文案映射是纯查表，见 recorder/statusCopy.mjs
  sessionStatusText.value = text || statusCopy.deriveRealtimePhaseText(phase)
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
  const { background = false } = options

  recordsStore.updateRecordStatus(recordId, patch)

  // background：上一段录音的后台优化完成时，用户可能已经开始录下一段。
  // 此时只更新记录集，不能把主界面当前记录 / 忙碌状态切回旧记录。
  if (!background) {
    appStore.setCurrentRecordId(recordId)
    appStore.setRecordStatus({ record_id: recordId, ...patch })

    const existingRecord = Array.isArray(recordsStore.records)
      ? recordsStore.records.find((record) => record.id === recordId)
      : null

    appStore.setCurrentRecord(existingRecord ? { ...existingRecord, ...patch } : { id: recordId, ...patch })
  }

  if (!options.keepBusy && !background) {
    appStore.setTranscribing(false)
    tray.setIcon('idle')
  }
}

async function prepareRecordingEnvironment() {
  const startedAt = performance.now()
  const settingsStore = useSettingsStore()
  const appStore = useAppStore()
  const { tray, syncRecordingState } = useElectron()

  startPending = true
  pendingStopAfterStart = false
  floatingSessionActive = true
  // 新会话尚未拿到 record_id：清掉上一段的值，避免旧优化事件误收新悬浮窗
  floatingSessionRecordId = ''
  clearOptimizationWatchdog()
  appStore.setRecording(true)
  syncFloatingWindow({
    phase: 'recording',
    duration: 0,
    text: ''
  }, { show: true })
  logRecordingTiming('floating-shown', startedAt)

  const deviceId = settingsStore.settings.audio_input_device || 'default'
  const constraints = {
    audio: {
      deviceId: deviceId === 'default' ? undefined : { exact: deviceId }
    }
  }

  let desktopSyncResult = null
  let mediaStream = null

  // 分别持有两个 Promise：任一方失败时都要能收口另一方，避免拿到却没释放的麦克风轨道
  const desktopSyncPromise = syncDesktopRecordingState(syncRecordingState, true)
  const mediaPromise = navigator.mediaDevices.getUserMedia(constraints)

  try {
    ;[desktopSyncResult, mediaStream] = await Promise.all([desktopSyncPromise, mediaPromise])
  } catch (error) {
    // 关键修复：本函数在 await 之前已经置位 startPending / isRecording / floatingSessionActive，
    // 若此处直接抛出，热键与录音按钮会被永久静默拦截，用户只能重启应用（docs/todo.md 相关 bug 的根因之一）。
    console.error('[Recorder] Failed to prepare recording environment:', error)

    const leakedStream = await mediaPromise.catch(() => null)
    leakedStream?.getTracks?.().forEach((track) => track.stop())

    await syncDesktopRecordingState(syncRecordingState, false).catch(() => {})

    clearDurationTimer()
    stopMediaTracks()
    cleanupStreamingAudio()
    cleanupStreamingSocket()
    appStore.setRecording(false)
    appStore.setTranscribing(false)
    tray.setIcon('idle')
    startPending = false
    pendingStopAfterStart = false
    floatingSessionActive = false
    streamingAwaitingFinish = false
    livePhase.value = 'idle'
    sessionStatusText.value = '按住快捷键开始录音'

    syncFloatingWindow({
      phase: 'error',
      duration: 0,
      text: '无法访问麦克风'
    })
    appStore.showToast('无法访问麦克风，请检查系统权限或输入设备')
    return null
  }

  logRecordingTiming('desktop-sync-and-media-ready', startedAt)

  if (desktopSyncResult?.success === false) {
    pendingStopAfterStart = false
    duration.value = 0
    appStore.setRecording(false)
    appStore.setTranscribing(false)
    mediaStream?.getTracks?.().forEach((track) => track.stop())
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

  stream.value = mediaStream

  if (pendingStopAfterStart) {
    stopMediaTracks()
    applyAbortedStartState(appStore, tray)
    duration.value = 0
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

  if (settingsStore.runtimeStatus.streaming_available) {
    return {
      available: true,
      runtimeStatus: settingsStore.runtimeStatus
    }
  }

  if (!streamProtocol.isStreamingProviderSupported(settingsStore.settings.asr_provider)) {
    return {
      available: false,
      reason: '真流式模式仅支持 Qwen3-ASR 或豆包在线 ASR provider'
    }
  }

  try {
    const runtimeStatus = await settingsStore.loadRuntimeStatus(false)
    if (runtimeStatus.streaming_available) {
      return {
        available: true,
        runtimeStatus
      }
    }

    return {
      available: false,
      reason: runtimeStatus.model_service_loading
        ? '模型服务仍在加载，请稍候'
        : runtimeStatus.streaming_error ||
          runtimeStatus.model_service_error ||
          runtimeStatus.qwen_streaming_service_error ||
          runtimeStatus.qwen_streaming_error ||
          '请先启动模型服务'
    }
  } catch (error) {
    return {
      available: false,
      reason: error.message || '无法探测模型服务状态'
    }
  }
}

async function ensureModelServiceAvailable() {
  const settingsStore = useSettingsStore()

  if (settingsStore.runtimeStatus.model_service_ready) {
    return {
      available: true,
      runtimeStatus: settingsStore.runtimeStatus
    }
  }

  try {
    const runtimeStatus = await settingsStore.loadRuntimeStatus(false)
    if (runtimeStatus.model_service_ready) {
      return {
        available: true,
        runtimeStatus
      }
    }

    return {
      available: false,
      reason: runtimeStatus.model_service_loading
        ? '模型服务仍在加载，请稍候'
        : runtimeStatus.model_service_error ||
          runtimeStatus.qwen_streaming_service_error ||
          runtimeStatus.qwen_streaming_error ||
          '请先开启模型'
    }
  } catch (error) {
    return {
      available: false,
      reason: error.message || '无法探测模型服务状态'
    }
  }
}

async function connectStreamingSocket(language) {
  const { api } = useElectron()
  const connectStartedAt = performance.now()

  streamingConnectionState.value = 'connecting'
  const url = await api.getStreamingWebSocketUrl()

  await new Promise((resolve, reject) => {
    let settled = false
    streamingSocket = new WebSocket(url)
    streamingSocket.binaryType = 'arraybuffer'

    // 连接超时兜底：无论发生什么（含被 cleanupStreamingSocket 摘掉 handler），Promise 都必须 settle
    const connectTimer = setTimeout(() => {
      fail(new Error('流式连接超时（8 秒内未就绪）'))
    }, STREAM_CONNECT_TIMEOUT_MS)

    const fail = (error) => {
      if (settled) {
        return
      }
      settled = true
      clearTimeout(connectTimer)
      reject(error instanceof Error ? error : new Error(String(error)))
    }

    streamingSocket.onopen = () => {
      streamingSocket.send(JSON.stringify(streamProtocol.deriveStreamStartPayload(language)))
    }

    streamingSocket.onerror = () => {
      fail(new Error('流式连接建立失败'))
    }

    streamingSocket.onclose = () => {
      // 关闭动作的判定抽到 recorder/streamProtocol.mjs，这里只负责执行副作用
      const action = streamProtocol.deriveStreamCloseAction({
        settled,
        awaitingFinish: streamingAwaitingFinish,
        connectionState: streamingConnectionState.value
      })

      if (action === streamProtocol.STREAM_CLOSE_ACTIONS.FAIL) {
        fail(new Error('流式连接已关闭'))
        return
      }

      if (action === streamProtocol.STREAM_CLOSE_ACTIONS.FINISH_INTERRUPTED) {
        // 收尾等待中连接断开：stream_finished 永远不会再来（后端异常或网络中断），
        // 记录可能已由后端落库。完整收口解锁界面，提示去记录集确认结果。
        streamingConnectionState.value = 'error'
        handleFinishInterruptedClose()
        return
      }

      if (action === streamProtocol.STREAM_CLOSE_ACTIONS.UNEXPECTED) {
        // 录音过程中意外断开：原实现只改内部状态，导致麦克风继续采集、时长无限增长、
        // 悬浮窗永久置顶显示"正在完成"，必须在此完整收口。
        streamingConnectionState.value = 'error'
        handleUnexpectedStreamingClose()
      }
    }

    streamingSocket.onmessage = async (event) => {
      try {
        const parsed = streamProtocol.parseStreamMessage(event.data)
        if (!parsed.ok) {
          throw parsed.error
        }

        const message = parsed.message

        switch (streamProtocol.classifyStreamMessage(message)) {
          case streamProtocol.STREAM_MESSAGE_TYPES.READY: {
            currentSessionId.value = streamProtocol.deriveStreamReadySessionId(message)
            streamingConnectionState.value = 'streaming'
            if (!settled) {
              settled = true
              clearTimeout(connectTimer)
              logRecordingTiming('connect-ready', connectStartedAt)
              resolve(message)
            }
            return
          }

          case streamProtocol.STREAM_MESSAGE_TYPES.PARTIAL: {
            updateStreamingTranscript(streamProtocol.deriveStreamPartialText(message))
            sessionStatusText.value = `${streamProtocol.deriveStreamingProviderLabel(
              useSettingsStore().settings.asr_provider
            )}转写中`
            livePhase.value = 'streaming'
            return
          }

          case streamProtocol.STREAM_MESSAGE_TYPES.FINISHED: {
            const appStore = useAppStore()
            const recordsStore = useRecordsStore()
            const { tray } = useElectron()
            const { record, finalText } = streamProtocol.deriveStreamFinishedPayload(message)

            if (!record) {
              // 后端对「极短点击 / 没有有效语音」回 record=null（音频不留档），
              // 这是正常业务结果，轻提示收尾即可。
              // 状态必须完整复位：否则 isTranscribing 卡在 true，
              // 托盘一直显示「转写中」且热键全被吞（历史真实缺陷）。
              livePhase.value = 'completed'
              sessionStatusText.value = '没有识别到语音内容'
              appStore.setRecording(false)
              appStore.setTranscribing(false)
              tray.setIcon('idle')
              completeFloatingSession('complete', '')
              appStore.showToast('没有识别到语音内容')
              streamingAwaitingFinish = false
              cleanupStreamingSocket()
              currentSessionId.value = null
              return
            }

            syncRecordIntoStore(record)
            appStore.setCurrentRecordId(record.id)
            appStore.setCurrentRecord(record)
            appStore.setRecordStatus({ record_id: record.id, status: record.status })

            updateStreamingTranscript(finalText)
            appStore.setRecording(false)
            appStore.setTranscribing(false)

            const awaitOptimization = floatingSession.shouldAwaitBackgroundOptimization({
              autoOptimize: useSettingsStore().settings.auto_optimize,
              recordId: record.id,
              recordStatus: record.status,
              // 极端竞态：optimization_completed 可能比 stream_finished 先到
              optimizationCompleted: completedPhases.has(`${record.id}:optimization`)
            })

            if (awaitOptimization) {
              // ASR 已收尾、后台优化还在跑：这里绝不能 complete ——
              // FloatingWindow 收到 complete 后 2s 就会自动隐藏，优化期间会全无提示。
              livePhase.value = 'optimizing'
              sessionStatusText.value = '转写完成，正在后台优化'
              tray.setIcon('transcribing')
              const optimizationStream = useOptimizationStreamStore()
              // 极端竞态：后台 optimization_started/delta 可能先于 stream_finished 到达，
              // 此时不能重置已经收到的增量。
              if (!(optimizationStream.isActive && optimizationStream.recordId === record.id)) {
                optimizationStream.begin(record.id, { source: 'auto' })
              }
              awaitBackgroundOptimization(record.id, optimizationStream.text || finalText)
              // 自动优化开启时不在 transcribed 阶段粘贴：后台优化完成后
              // optimization_completed 会粘贴最终文本（与单段模式一致）。
              // 否则原文、优化文各粘贴一次，输入框出现两遍几乎相同的内容。
              appStore.showToast('转写完成，正在后台优化')
            } else {
              livePhase.value = 'completed'
              sessionStatusText.value = `${streamProtocol.deriveStreamingProviderLabel(
                useSettingsStore().settings.asr_provider
              )}录音已完成`
              tray.setIcon('idle')
              completeFloatingSession('complete', finalText)
              appStore.showToast(
                statusCopy.deriveCompletionToast('streaming', await maybeCopyText(finalText))
              )
            }

            streamingAwaitingFinish = false
            cleanupStreamingSocket()
            currentSessionId.value = null
            return
          }

          case streamProtocol.STREAM_MESSAGE_TYPES.CANCELLED: {
            streamingAwaitingFinish = false
            cleanupStreamingSocket()
            currentSessionId.value = null
            return
          }

          case streamProtocol.STREAM_MESSAGE_TYPES.ERROR: {
            throw new Error(streamProtocol.deriveStreamErrorMessage(message))
          }

          default:
            // 未识别的消息类型：与原实现一致，静默忽略
            return
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
        completeFloatingSession('error', error.message || '流式转写失败')
        appStore.showToast(error.message || '流式转写失败')
      }
    }
  })
}

async function setupStreamingAudioCapture(targetStream) {
  const workletUrl = new URL('../worklets/pcm-capture-worklet.js', import.meta.url)
  const captureStartedAt = performance.now()
  // 记下本次采集的代次：期间任何收口（松手放弃 / 连接失败 / 意外断开 / 组件卸载）
  // 都会让 cleanupStreamingAudio 把它自增，下面的核对就是靠这个发现"自己已被放弃"。
  const generation = streamingCaptureGeneration

  // 用局部变量持有 AudioContext：并行的收口路径可能已经把模块级引用置空，
  // 继续用模块变量会在 addModule 之后炸成 null 解引用。
  const audioContext = new AudioContext({ sampleRate: STREAM_SAMPLE_RATE })
  streamingAudioContext = audioContext

  if (audioContext.sampleRate !== STREAM_SAMPLE_RATE) {
    throw new Error(`当前浏览器返回的采样率为 ${audioContext.sampleRate}Hz，无法启用真流式`)
  }

  await audioContext.audioWorklet.addModule(workletUrl.href)

  // addModule（首次加载约 50~150ms）期间可能已经完成收口：此刻再连图就会留下
  // 没人 cleanup 的采集节点，必须自己关掉刚建的 AudioContext 并退出。
  if (generation !== streamingCaptureGeneration) {
    audioContext.close().catch(() => {})
    logRecordingTiming('capture-aborted-after-cleanup', captureStartedAt)
    return
  }

  streamingSourceNode = audioContext.createMediaStreamSource(targetStream)
  streamingWorkletNode = new AudioWorkletNode(audioContext, 'pcm-capture-processor')
  streamingWorkletNode.port.onmessage = (event) => {
    if (!(event.data instanceof Float32Array)) {
      return
    }
    queueStreamingSamples(event.data)
  }

  streamingSourceNode.connect(streamingWorkletNode)
  const silentGainNode = audioContext.createGain()
  silentGainNode.gain.value = 0
  streamingWorkletNode.connect(silentGainNode)
  silentGainNode.connect(audioContext.destination)

  // 正常情况下该标记早于 connect-ready（建连要 0.3~0.7s，挂 worklet 只要 50~150ms），
  // 两者差值就是"按下后多久开始采集"的直接量化；worklet 模块已缓存时两者会非常接近。
  logRecordingTiming('capture-started', captureStartedAt)
}

async function handleShortPressCancellation(syncRecordingState) {
  const appStore = useAppStore()
  const { tray } = useElectron()

  pendingStopAfterStart = false
  clearDurationTimer()
  duration.value = 0
  applyAbortedStartState(appStore, tray)
  await syncDesktopRecordingState(syncRecordingState, false)

  completeFloatingSession('error', '录音时间过短')
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
      completeFloatingSession('error', '录音时间过短')
      return
    }

    appStore.setTranscribing(true)
    const result = await api.uploadAudio(blob, `recording_${Date.now()}.webm`)

    if (!result.record_id) {
      throw new Error('Missing record id from upload response')
    }

    appStore.setCurrentRecordId(result.record_id)
    if (floatingSessionActive) {
      floatingSessionRecordId = result.record_id
    }
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
    completeFloatingSession('error', '上传录音失败')
    appStore.showToast('上传录音失败')
  }
}

async function uploadRealtimeSegment(blob, segmentDuration, options = {}) {
  const { api } = useElectron()
  const { force = false, hadSpeech = false } = options

  // 上传门槛判定委托给 recorder/segmentPolicy.mjs（判定顺序与原实现逐字一致）
  const shouldUpload = segmentPolicy.shouldUploadRealtimeSegment({
    hasSession: currentSessionId.value,
    hasContent: blob.size,
    hadSpeech,
    force,
    segmentDuration,
    hasCapturedSpeech: realtimeHasCapturedSpeech
  })

  if (!shouldUpload) {
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

  const canFinalize = segmentPolicy.shouldFinalizeRealtimeSession({
    hasSessionId: currentSessionId.value,
    finalizing: realtimeFinalizing,
    hasRecorder: mediaRecorder.value,
    pendingUploads: realtimePendingUploads,
    pendingSegmentCount: realtimePendingSegmentIds.size
  })

  if (!canFinalize) {
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
      text: liveTranscript.value
    })
  }

  try {
    const noSpeechCaptured = segmentPolicy.isMissingRealtimeSpeech({
      hasCapturedSpeech: realtimeHasCapturedSpeech,
      processedSegmentCount: processedSegmentCount.value
    })
    if (noSpeechCaptured) {
      await api.deleteSession(sessionId).catch(() => {})
      throw new Error(segmentPolicy.deriveMissingSpeechErrorMessage(duration.value))
    }

    const merged = await api.mergeSession(sessionId)
    liveTranscript.value = merged.merged_text || liveTranscript.value

    let finalRecord = merged.record || null
    if (!finalRecord && merged.record_id) {
      finalRecord = await api.getRecord(merged.record_id).catch(() => null)
    }

    if (finalRecord) {
      if (floatingSessionActive) {
        floatingSessionRecordId = finalRecord.id
      }
      syncRecordIntoStore(finalRecord)
      appStore.setCurrentRecordId(finalRecord.id)
      appStore.setCurrentRecord(finalRecord)
      appStore.setRecordStatus({ record_id: finalRecord.id, status: finalRecord.status })
    }

    const finalText = merged.optimized_text || finalRecord?.optimized_text || finalRecord?.raw_text || merged.merged_text || liveTranscript.value
    liveTranscript.value = finalText || liveTranscript.value
    setRealtimePhase('completed')

    // 多段 merge 的流式优化只广播增量、不广播 terminal（避免与这里重复粘贴），
    // 因此由 finalize 负责把流式状态收成 done。
    const optimizationStream = useOptimizationStreamStore()
    if (optimizationStream.recordId === (finalRecord?.id || merged.record_id)) {
      optimizationStream.finish(finalText || '')
    }

    completeFloatingSession('complete', finalText || '')

    appStore.showToast(statusCopy.deriveCompletionToast('multi', await maybeCopyText(finalText)))
  } catch (error) {
    console.error('[Recorder] Failed to finalize realtime session:', error)
    const optimizationStream = useOptimizationStreamStore()
    if (floatingSessionRecordId && optimizationStream.recordId === floatingSessionRecordId) {
      optimizationStream.fail(error.message || '多段录音处理失败')
    }
    setRealtimePhase('error', error.message || '多段录音处理失败')
    completeFloatingSession('error', error.message || '多段录音处理失败')
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
  if (!segmentPolicy.canStartRealtimeSegment({
    hasStream: stream.value,
    hasRecorder: mediaRecorder.value,
    finalizing: realtimeFinalizing
  })) {
    return
  }

  realtimeSegmentStartedAt = performance.now()
  realtimeRestartAfterStop = false
  realtimeSegmentHasSpeech = realtimeSpeechActive
  mediaRecorder.value = createRecorder(stream.value, async (blob) => {
    const shouldRestart = realtimeRestartAfterStop
    const hadSpeech = realtimeSegmentHasSpeech
    realtimeRestartAfterStop = false
    realtimeSegmentHasSpeech = false
    clearRealtimeMaxSegmentTimer()
    mediaRecorder.value = null
    const segmentDuration = segmentPolicy.computeRealtimeSegmentDurationSeconds(
      realtimeSegmentStartedAt,
      performance.now()
    )
    realtimeSegmentStartedAt = 0
    const shouldContinueCapture = segmentPolicy.shouldContinueRealtimeCapture({
      stopRequested: realtimeStopRequested,
      finalizing: realtimeFinalizing
    })

    if (shouldContinueCapture) {
      startRealtimeSegment()
    }

    await uploadRealtimeSegment(blob, segmentDuration, {
      force: realtimeStopRequested || shouldRestart,
      hadSpeech
    })

    if (shouldContinueCapture) {
      return
    }

    if (segmentPolicy.shouldContinueRealtimeCapture({
      stopRequested: realtimeStopRequested,
      finalizing: realtimeFinalizing
    })) {
      setRealtimePhase('listening')
    }

    await maybeFinalizeRealtimeSession()
  })

  mediaRecorder.value.start(250)
  setRealtimePhase(realtimeSpeechActive ? 'speaking' : 'listening')
  clearRealtimeMaxSegmentTimer()
  realtimeMaxSegmentTimer = setTimeout(() => {
    if (segmentPolicy.isRecorderInactive(mediaRecorder.value)) {
      return
    }

    realtimeRestartAfterStop = true
    setRealtimePhase('segment-processing', '当前语句较长，正在拆分为较大的片段')
    stopRealtimeSegment({ force: true })
  }, segmentPolicy.resolveRealtimeMaxSegmentDelayMs())
}

function stopRealtimeSegment(options = {}) {
  const { force = false } = options

  if (segmentPolicy.isRecorderInactive(mediaRecorder.value)) {
    return
  }

  const segmentDuration = segmentPolicy.computeRealtimeSegmentDurationSeconds(
    realtimeSegmentStartedAt,
    performance.now()
  )

  if (!segmentPolicy.shouldStopRealtimeSegment({
    force,
    segmentHasSpeech: realtimeSegmentHasSpeech,
    stopRequested: realtimeStopRequested,
    segmentDuration
  })) {
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
  const { sendRecordingStatus } = useGlobalWebSocket()

  // 启动放弃标志：stopStreamingRecording 在连接/采集就绪前被触发时置位，
  // 用于关闭「松手竞争窗口」——否则 start 的后半段会把一个用户已经放弃的
  // 会话正式开启（幽灵录音：无人停止、一直推流、永远停在「转写中」）。
  streamingStartAborted = false

  try {
    activeRecordingMode.value = 'streaming'
    resetRealtimeState()
    resetStreamingState()
    livePhase.value = 'connecting'

    if (!streamProtocol.isStreamingProviderSupported(context.settingsStore.settings.asr_provider)) {
      throw new Error('真流式模式仅支持 Qwen3-ASR 或豆包在线 ASR provider')
    }

    const streamingLabel = streamProtocol.deriveStreamingProviderLabel(
      context.settingsStore.settings.asr_provider
    )
    sessionStatusText.value = `正在连接${streamingLabel}服务`

    // 采集与建连并行：麦克风在 prepareRecordingEnvironment 里已经打开，这里必须尽早挂上
    // worklet —— WebAudio 是实时的，不会为稍后才出现的消费者回放采样，所以「按下热键后立刻
    // 说话」时串行等待建连（实测 0.5~0.7s）的那段语音会永久丢失（开头丢字）。
    // 并行期间采到的样本只进 streamingPendingSamples，READY 之后由 queueStreamingSamples →
    // sendStreamingSamples 统一冲刷；协议顺序不变（sendStreamingSamples 有 currentSessionId 守卫，
    // 音频永远在 stream_ready 之后才上行）。
    await Promise.all([
      connectStreamingSocket(context.settingsStore.settings.asr_language || 'zh'),
      setupStreamingAudioCapture(stream.value)
    ])

    if (pendingStopAfterStart) {
      pendingStopAfterStart = false
      // fromStart：此处是「启动完成后消费排队停止」的唯一入口，
      // 必须绕过 stopStreamingRecording 里的 startPending 重入守卫——
      // 否则守卫会把 pendingStopAfterStart 原样置回，消费永远无效，
      // 表现为快速点按热键时录音再也起不来。
      await stopStreamingRecording({ fromStart: true })
      return
    }

    // 松手竞争窗口收口：上面的 await 期间用户已松手、stopStreamingRecording
    // 已经发出 finish（awaitingFinish）或已经放弃本次连接（startAborted）。
    // 此刻绝不能再进入录音状态，等 stream_finished / 看门狗完成收尾。
    if (streamingAwaitingFinish || streamingStartAborted) {
      logRecorderHotkey('Streaming start aborted: stop arrived during setup', {
        awaitingFinish: streamingAwaitingFinish,
        startAborted: streamingStartAborted
      })
      if (streamingStartAborted) {
        // 放弃路径：后端会话可能已建立（ready 晚到），关连接让后端释放会话
        cleanupStreamingSocket()
        currentSessionId.value = null
      }
      return
    }

    appStore.setRecording(true)
    tray.setIcon('recording')
    sendRecordingStatus({
      recording: true,
      transcribing: false,
      duration: 0
    })
    startDurationClock()
    livePhase.value = 'streaming'
    sessionStatusText.value = `${streamingLabel}转写中`
  } catch (error) {
    console.error('[Recorder] Failed to start streaming recording:', error)
    await syncDesktopRecordingState(context.syncRecordingState, false)
    cleanupStreamingAudio()
    cleanupStreamingSocket()
    stopMediaTracks()
    mediaRecorder.value = null
    appStore.setRecording(false)
    completeFloatingSession('error', error.message || '无法启动真流式录音')

    // 用户已在启动阶段松手放弃：这是预期路径（连接被主动关闭后 8s 超时
    // reject 也会走到这里），静默收口，不要弹错误吓用户
    if (streamingStartAborted) {
      return
    }

    // 后端进程繁忙 / 重启窗口期的 fetch 失败会被浏览器报成英文
    // "Failed to fetch"，特化成可读中文
    if (error instanceof TypeError && error.message === 'Failed to fetch') {
      appStore.showToast('无法连接本地后端服务，请稍后重试')
      return
    }

    appStore.showToast(error.message || '无法启动真流式录音')
  } finally {
    startPending = false
    streamingStartAborted = false
  }
}

async function stopStreamingRecording({ fromStart = false } = {}) {
  const appStore = useAppStore()
  const { tray, syncRecordingState } = useElectron()
  const { sendRecordingStatus } = useGlobalWebSocket()

  // 启动进行中收到停止请求：排队等 start 完成后消费。
  // fromStart=true 表示已经是「启动完成后消费」的路径，不能再排队（否则死循环）。
  // 注意不能用 isRecording 当判据：prepareRecordingEnvironment 开麦克风之前就把它置真了，
  // 而启动窗口内它恒为真 → 判据失效会让松手落进下面的放弃分支，最终变成幽灵录音。
  if (shouldQueueStopDuringStart({ startPending, fromStart })) {
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
  sendRecordingStatus({
    recording: false,
    transcribing: true,
    duration: duration.value
  })

  if (floatingSessionActive) {
    syncFloatingWindow({
      phase: 'transcribing',
      duration: 0,
      text: liveTranscript.value
    })
  }

  livePhase.value = 'finalizing'
  sessionStatusText.value = '正在完成流式转写'

  // 尾包必须赶在 cleanupStreamingAudio() 之前冲刷：后者内部的 resetStreamingBuffer() 会清空
  // 积压队列，顺序反了则每次停止都会丢掉不足一个 STREAM_FLUSH_SAMPLES 块的尾部（最多 ≈200ms，
  // 与"开头丢字"对称的缺陷）。这里到 cleanup 之间没有 await，工作线程不可能再塞进新样本。
  const socketReadyForFinish = Boolean(
    streamingSocket && streamingSocket.readyState === WebSocket.OPEN
  )
  if (socketReadyForFinish) {
    sendStreamingSamples(true)
  }

  cleanupStreamingAudio()
  stopMediaTracks()

  if (socketReadyForFinish) {
    streamingSocket.send(JSON.stringify(streamProtocol.deriveStreamFinishPayload()))
    armFinishWatchdog()
    return
  }

  // socket 尚未就绪（还卡在连接/采集阶段）就松手：本次启动直接放弃。
  // 置放弃标志让仍在飞行的 startStreamingRecording 后半段不要进入录音状态；
  // 此时后端会话尚未建立（stream_ready 未回），没有任何需要保存的内容。
  // 前置条件：上面的 cleanupStreamingAudio() / stopMediaTracks() 必须保持无条件执行，
  // 本分支才能不重复收口——调整顺序时务必一起改。
  streamingAwaitingFinish = false
  streamingStartAborted = true
  cleanupStreamingSocket()
  appStore.setTranscribing(false)
  tray.setIcon('idle')
  appStore.showToast('已取消本次录音')
}

async function startSingleRecording() {
  const startedAt = performance.now()
  const context = await prepareRecordingEnvironment()
  if (!context) {
    return
  }

  const { appStore, tray } = context
  const { sendRecordingStatus } = useGlobalWebSocket()

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
    logRecordingTiming('media-recorder-started', startedAt)
    appStore.setRecording(true)
    tray.setIcon('recording')
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
    completeFloatingSession('error', '无法访问麦克风')
    appStore.showToast('无法访问麦克风')
  } finally {
    startPending = false
  }
}

async function startMultiRecording() {
  const startedAt = performance.now()
  const context = await prepareRecordingEnvironment()
  if (!context) {
    return
  }

  const { appStore, tray } = context
  const { api } = useElectron()
  const { sendRecordingStatus } = useGlobalWebSocket()
  const settingsStore = useSettingsStore()

  try {
    activeRecordingMode.value = 'multi'
    resetRealtimeState()
    resetStreamingState()
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
    sendRecordingStatus({
      recording: true,
      transcribing: false,
      duration: 0
    })
    startDurationClock()

    silenceDetector = createSilenceDetector({
      stream: stream.value,
      silenceLevel: settingsStore.settings.silence_level ?? -40,
      silenceThreshold: Math.max((settingsStore.settings.silence_threshold ?? 1.5) * 1000, 400),
      onLevelChange: ({ normalizedLevel }) => {
        audioLevel.value = normalizedLevel
      },
      onSpeechStart: () => {
        if (!realtimeStopRequested) {
          realtimeSpeechActive = true
          realtimeSegmentHasSpeech = true
          setRealtimePhase('speaking')
          if (!mediaRecorder.value || mediaRecorder.value.state === 'inactive') {
            startRealtimeSegment()
          }
        }
      },
      onSpeechEnd: () => {
        realtimeSpeechActive = false
        if (realtimeSegmentHasSpeech) {
          stopRealtimeSegment()
        }
      }
    })
    startRealtimeSegment()
    logRecordingTiming('multi-recording-started', startedAt)
  } catch (error) {
    console.error('[Recorder] Failed to start realtime recording:', error)
    await syncDesktopRecordingState(context.syncRecordingState, false)
    cleanupRealtimeDetector()
    stopMediaTracks()
    mediaRecorder.value = null
    appStore.setRecording(false)
    completeFloatingSession('error', '无法启动多段录音')
    appStore.showToast('无法启动多段录音')
    resetRealtimeState()
  } finally {
    startPending = false
  }
}

/**
 * 转写中状态泄漏自愈。
 *
 * 「转写中」本应只存在于有活跃会话 / 收尾等待的窗口内；历史缺陷（收尾
 * 无超时、状态复位遗漏、HMR 状态残留等）会让它永久卡住，表现为托盘
 * 一直「转写中」且热键全部失效，只能重启应用。判定：流式模式下既没有
 * 收尾等待、也没有活跃会话，却停在转写中——必然是泄漏，直接复位。
 *
 * 返回 true 表示可以继续开始录音；false 表示真的在收尾中，应保持吞掉。
 */
function recoverLeakedTranscribingState() {
  const appStore = useAppStore()
  if (!appStore.isTranscribing) {
    return true
  }

  const leaked = activeRecordingMode.value === 'streaming'
    && !streamingAwaitingFinish
    && !currentSessionId.value

  if (!leaked) {
    return false
  }

  const { tray } = useElectron()
  appStore.setTranscribing(false)
  tray.setIcon('idle')
  logRecorderHotkey('Recovered leaked transcribing state on record entry', {})
  return true
}

async function cancelOptimization(recordId) {
  if (!recordId) {
    return
  }

  const { api } = useElectron()
  const optimizationStream = useOptimizationStreamStore()
  const result = await api.cancelOptimizeRecord(recordId).catch(() => null)
  if (result?.cancelled) {
    if (optimizationStream.recordId === recordId) {
      optimizationStream.cancel()
    }
    useAppStore().showToast('正在取消优化…')
  }
}

async function startRecording() {
  const startedAt = performance.now()
  const appStore = useAppStore()
  const settingsStore = useSettingsStore()

  if (
    startPending ||
    appStore.isRecording ||
    mediaRecorder.value?.state === 'recording'
  ) {
    return
  }

  // 真在收尾中保持吞掉；检测到泄漏的转写状态则复位后照常开始
  if (appStore.isTranscribing && !recoverLeakedTranscribingState()) {
    return
  }

  logRecordingTiming('start-recording-entered', startedAt)

  if (settingsStore.settings.recording_mode === 'multi') {
    await startMultiRecording()
    return
  }

  if (settingsStore.settings.recording_mode === 'streaming') {
    await startStreamingRecording()
    return
  }

  await startSingleRecording()
}

function stopSingleRecording() {
  const appStore = useAppStore()
  const { tray, syncRecordingState } = useElectron()
  const { sendRecordingStatus } = useGlobalWebSocket()

  // 启动窗口判据这里用 mediaRecorder 状态即可：单段的 recorder 尚未 start 时它必为 null/inactive，
  // 是"还没开始录"的直接依据，不像 isRecording 那样会被 prepare 提前置真。
  if (startPending && (!mediaRecorder.value || mediaRecorder.value.state === 'inactive')) {
    pendingStopAfterStart = true
    return
  }

  if (!mediaRecorder.value || mediaRecorder.value.state === 'inactive') {
    return
  }

  pendingStopAfterStart = false
  clearDurationTimer()
  const nextState = derivePostStopState({
    mode: 'single',
    durationSeconds: duration.value
  })
  appStore.setRecording(nextState.recording)
  appStore.setTranscribing(nextState.transcribing)
  syncDesktopRecordingState(syncRecordingState, false).catch(() => {})
  tray.setIcon(nextState.trayIcon)
  sendRecordingStatus({
    recording: nextState.recording,
    transcribing: nextState.transcribing,
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
  const { sendRecordingStatus } = useGlobalWebSocket()

  // 与 stopStreamingRecording 同源：启动窗口内 isRecording 已由 prepare 置真，不能用它当判据，
  // 否则短按松手会带着 realtimeStopRequested 落进下面的常规停止路径，而 startMultiRecording
  // 开头的 resetRealtimeState() 又会清掉这个标志 → 会话照建、录音照开（幽灵录音）。
  if (shouldQueueStopDuringStart({ startPending })) {
    pendingStopAfterStart = true
    return
  }

  if (!appStore.isRecording && !currentSessionId.value) {
    return
  }

  pendingStopAfterStart = false
  realtimeStopRequested = true
  clearDurationTimer()
  const nextState = derivePostStopState({
    mode: 'multi',
    durationSeconds: duration.value
  })
  appStore.setRecording(nextState.recording)
  appStore.setTranscribing(nextState.transcribing)
  syncDesktopRecordingState(syncRecordingState, false).catch(() => {})
  cleanupRealtimeDetector()
  tray.setIcon(nextState.trayIcon)
  sendRecordingStatus({
    recording: nextState.recording,
    transcribing: nextState.transcribing,
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
  const optimizationStream = useOptimizationStreamStore()
  const { tray } = useElectron()

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
      if (activeRecordingMode.value === 'single') {
        floatingSessionRecordId = recordId
      }
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

    stopPollingForRecord(recordId)
    handleRecordCompleted(recordId, {
      status: 'transcribed',
      raw_text: message.data?.text || '',
      duration: message.data?.duration,
      processing_time: message.data?.processing_time
    })

    if (!settingsStore.settings.auto_optimize) {
      if (ownsFloatingRecord(recordId)) {
        completeFloatingSession('complete', message.data?.text || '')
      }
      appStore.showToast(
        statusCopy.deriveCompletionToast('transcription', await maybeCopyText(message.data?.text))
      )
    } else {
      livePhase.value = 'optimizing'
      sessionStatusText.value = '转写完成，正在优化'
      if (floatingSessionActive) {
        if (activeRecordingMode.value === 'single') {
          floatingSessionRecordId = recordId
        }
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

    const recordingInProgress = appStore.isRecording || startPending
    // 多段模式的记录在 merge 内部才创建；finalize 期间允许 started 事件认领悬浮窗
    if (
      floatingSessionActive &&
      activeRecordingMode.value === 'multi' &&
      realtimeFinalizing &&
      !floatingSessionRecordId
    ) {
      floatingSessionRecordId = recordId
    }

    // 用户已经开始下一段录音：上一段的 started 只更新记录集，不打断新会话
    if (recordingInProgress && !ownsFloatingRecord(recordId)) {
      recordsStore.updateRecordStatus(recordId, { status: 'optimizing' })
      return
    }

    if (!(optimizationStream.isActive && optimizationStream.recordId === recordId)) {
      optimizationStream.begin(recordId, { source: 'auto' })
    }

    // 流式模式的后台优化不应把热键锁死；单段/多段保持原有「处理中」语义
    if (activeRecordingMode.value !== 'streaming') {
      appStore.setTranscribing(true)
    }
    appStore.setRecordStatus({ record_id: recordId, status: 'optimizing' })
    tray.setIcon('transcribing')
    recordsStore.updateRecordStatus(recordId, { status: 'optimizing' })
    livePhase.value = 'optimizing'
    if (floatingSessionActive) {
      if (['single', 'streaming'].includes(activeRecordingMode.value)) {
        floatingSessionRecordId = recordId
      }
      syncFloatingWindow({
        phase: 'optimizing',
        duration: 0,
        text: optimizationStream.text
      })
    }
    return
  }

  if (message.type === 'optimization_delta') {
    const recordId = message.data?.record_id
    const chunk = message.data?.text || ''
    if (!recordId || !chunk) {
      return
    }

    if (optimizationStream.recordId !== recordId) {
      // 兼容没有 started 的旧后端；新录音期间不认领旧记录
      if (!ownsFloatingRecord(recordId)) {
        return
      }
      optimizationStream.begin(recordId, { source: 'auto' })
    }

    optimizationStream.append(chunk)

    if (floatingSessionActive && ownsFloatingRecord(recordId)) {
      liveTranscript.value = optimizationStream.text
      sessionStatusText.value = '正在优化'
      syncFloatingWindow({
        phase: 'optimizing',
        duration: 0,
        text: optimizationStream.text
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

    stopPollingForRecord(recordId)
    const recordingInProgress = appStore.isRecording || startPending
    if (optimizationStream.recordId === recordId) {
      // 即使新录音已经开始，也要把旧记录的状态收成 done，避免旧 store 卡在 active
      optimizationStream.finish(message.data?.optimized_text || '')
    } else if (!recordingInProgress && !optimizationStream.isActive) {
      // 兼容没有 started 的旧后端
      optimizationStream.begin(recordId, { source: 'auto' })
      optimizationStream.finish(message.data?.optimized_text || '')
    }
    handleRecordCompleted(recordId, {
      status: 'optimized',
      optimized_text: message.data?.optimized_text || ''
    }, { keepBusy: recordingInProgress, background: recordingInProgress })
    if (ownsFloatingRecord(recordId)) {
      completeFloatingSession('complete', message.data?.optimized_text || '')
    }

    if (!recordingInProgress) {
      livePhase.value = 'completed'
      sessionStatusText.value = '优化完成'
    }

    // 新录音已经开始：上一段的优化结果只入库，不要粘贴进当前输入框
    if (recordingInProgress) {
      appStore.showToast('上一段录音优化完成，文本已保存到记录集')
      return
    }

    appStore.showToast(
      statusCopy.deriveCompletionToast('optimization', await maybeCopyText(message.data?.optimized_text))
    )
    return
  }

  if (message.type === 'optimization_cancelled') {
    const recordId = message.data?.record_id
    const rawText = message.data?.raw_text || ''
    const recordingInProgress = appStore.isRecording || startPending

    if (recordId) {
      recordsStore.updateRecordStatus(recordId, { status: 'transcribed' })
      if (!recordingInProgress) {
        appStore.setRecordStatus({ record_id: recordId, status: 'transcribed' })
      }
    }
    if (optimizationStream.recordId === recordId) {
      // 取消后展示原文 + “已取消”，不要保留半截优化文本
      optimizationStream.begin(recordId, { source: 'auto' })
      if (rawText) {
        optimizationStream.append(rawText)
      }
      optimizationStream.cancel()
    }
    if (!recordingInProgress) {
      appStore.setTranscribing(false)
      tray.setIcon('idle')
      livePhase.value = 'completed'
      sessionStatusText.value = '已取消优化'
    }
    if (ownsFloatingRecord(recordId)) {
      completeFloatingSession('complete', rawText)
    }
    appStore.showToast('已取消优化，保留原始文本')
    return
  }

  if (message.type === 'optimization_failed') {
    const recordId = message.data?.record_id
    const fallbackText = message.data?.text || ''

    const recordingInProgress = appStore.isRecording || startPending
    stopPollingForRecord(recordId)
    if (recordId) {
      if (recordingInProgress && optimizationStream.recordId === recordId) {
        optimizationStream.fail(message.data?.error || '优化失败')
      } else if (!recordingInProgress && (
        optimizationStream.recordId === recordId || !optimizationStream.isActive
      )) {
        // 把失败状态收成“原文 + 错误”，避免界面停在半截优化文本上
        optimizationStream.begin(recordId, { source: 'auto' })
        if (fallbackText) {
          optimizationStream.append(fallbackText)
        }
        optimizationStream.fail(message.data?.error || '优化失败')
      }
    }
    if (recordId) {
      recordsStore.updateRecordStatus(recordId, { status: message.data?.status || 'transcribed' })
      if (!recordingInProgress) {
        appStore.setRecordStatus({ record_id: recordId, status: message.data?.status || 'transcribed' })
      }
    }
    if (!recordingInProgress) {
      appStore.setTranscribing(false)
      tray.setIcon('idle')
      livePhase.value = 'completed'
      sessionStatusText.value = '优化失败，已保留转写原文'
    }
    if (ownsFloatingRecord(recordId)) {
      completeFloatingSession('complete', fallbackText)
    }

    // 用户已经开始下一段录音：不要把上一段失败兜底的原文粘贴进新输入框
    if (recordingInProgress) {
      appStore.showToast('上一段录音优化失败，原文已保留在记录集中', { tone: 'error' })
      return
    }

    // 自动优化模式下 transcribed 阶段没有粘贴过，失败时兜底粘贴原文，
    // 保证「总有输出」；优化失败的完整原因在记录详情的 error 字段里
    const copyResult = await maybeCopyText(fallbackText)
    appStore.showToast(
      copyResult.pasted ? '优化失败，已粘贴转写文本' : '优化失败，转写文本已保留在记录集中',
      { tone: 'error' }
    )
    return
  }

  if (message.type === 'error') {
    const recordId = message.data?.record_id
    if (recordId) {
      stopPollingForRecord(recordId)
      recordsStore.updateRecordStatus(recordId, { status: 'failed' })
      appStore.setRecordStatus({ record_id: recordId, status: 'failed' })
    }

    appStore.setTranscribing(false)
    tray.setIcon('idle')
    completeFloatingSession('error', message.data?.error || '处理失败')
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
      if (ownsFloatingRecord(recordId)) {
        completeFloatingSession('complete', status.optimized_text || '')
      }
      appStore.showToast(
        statusCopy.deriveCompletionToast('optimization', await maybeCopyText(status.optimized_text))
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
        if (ownsFloatingRecord(recordId)) {
          completeFloatingSession('complete', status.raw_text || '')
        }
        appStore.showToast(
          statusCopy.deriveCompletionToast('transcription', await maybeCopyText(status.raw_text))
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
      if (ownsFloatingRecord(recordId)) {
        completeFloatingSession('error', '转写失败')
      }
      appStore.showToast('转写失败')
      return true
    }

    return false
  } catch (error) {
    pollingFailureCount = pollPolicy.nextPollFailureCount(pollingFailureCount)
    console.error('[Recorder] Failed to poll status:', error)

    // 超过失败上限就停止轮询并提示，避免后端异常时永久 1Hz 静默轮询、UI 卡在"转写中"
    if (pollPolicy.hasReachedPollFailureLimit(pollingFailureCount)) {
      stopPolling()
      appStore.setTranscribing(false)
      tray.setIcon('idle')

      completeFloatingSession('error', '查询转写状态失败')

      appStore.showToast('查询转写状态失败，请稍后在记录集里刷新查看')
      return true
    }

    return false
  }
}

function startPolling(recordId) {
  stopPolling()
  pollingRecordId = recordId
  pollingFailureCount = 0
  pollingTimer = setInterval(async () => {
    if (pollPolicy.shouldSkipPollTick(pollingInFlight)) {
      return
    }
    pollingInFlight = true
    const completed = await pollRecordStatus(recordId)
    pollingInFlight = false
    if (pollPolicy.shouldStopPollingAfterTick(completed)) {
      stopPolling()
    }
  }, pollPolicy.POLL_INTERVAL_MS)
}

export function useRecorder() {
  const appStore = useAppStore()
  const settingsStore = useSettingsStore()
  const { connect, addMessageHandler, removeMessageHandler } = useGlobalWebSocket()
  const { onHotkeyPress, onHotkeyRelease, removeAllListeners } = useElectron()

  onMounted(async () => {
    mountedInstances += 1
    await connect()

    if (!wsAttached) {
      addMessageHandler(handleWebSocketMessage)
      wsAttached = true
    }

    if (!desktopHotkeyAttached) {
      onHotkeyPress((payload) => {
        logRecorderHotkey('Received desktop hotkey press', {
          payload
        })

        if (appStore.isRecording || startPending) {
          return
        }

        // 真在收尾中保持吞掉；检测到泄漏的转写状态则复位后照常开始
        if (appStore.isTranscribing && !recoverLeakedTranscribingState()) {
          return
        }

        void startRecording()
      })

      onHotkeyRelease((payload) => {
        logRecorderHotkey('Received desktop hotkey release', {
          payload
        })

        if (!appStore.isRecording && !startPending) {
          return
        }

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

      removeAllListeners('hotkey:press')
      removeAllListeners('hotkey:release')
      desktopHotkeyAttached = false

      // 原实现只摘 WS 与热键，定时器 / WebSocket / AudioContext / 麦克风轨道全部泄漏，
      // 组件卸载后仍会继续轮询与采集。
      clearDurationTimer()
      clearRealtimeMaxSegmentTimer()
      stopPolling()
      cleanupStreamingSocket()
      cleanupStreamingAudio()
      cleanupRealtimeDetector()
      stopMediaTracks()
      mediaRecorder.value = null
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
    cancelOptimization,
    startPolling,
    stopPolling
  }
}
