import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useAppStore } from '@/stores/app'
import { useRecordsStore } from '@/stores/records'
import { useSettingsStore } from '@/stores/settings'
import { useOptimizationStreamStore } from '@/stores/optimizationStream'
import { useElectron } from './useElectron'
import * as statusCopy from './recorder/statusCopy.mjs'
import * as nativeSession from './recorder/nativeSession.mjs'

// 录音界面状态（模块级单例，多个组件实例共用）。
//
// 采集、识别、记录落库、复制粘贴、悬浮窗与全局快捷键全部在 Rust 录音 Actor
// （src-tauri/src/recording.rs）内完成。这里只负责：发出开始 / 停止指令，
// 把 recording:* 事件映射成界面状态（映射规则见 recorder/nativeSession.mjs，有 Node 单测）。

const duration = ref(0)
const audioLevel = ref(0)
const liveTranscript = ref('')
const livePhase = ref('idle')
const sessionStatusText = ref(statusCopy.IDLE_SESSION_STATUS_TEXT)

let durationTimer = null
let mountedInstances = 0
// 原生录音事件是否已订阅
let nativeRecordingAttached = false

function clearDurationTimer() {
  if (durationTimer) {
    clearInterval(durationTimer)
    durationTimer = null
  }
}

function startDurationClock() {
  const appStore = useAppStore()

  duration.value = 0
  clearDurationTimer()
  durationTimer = setInterval(() => {
    duration.value += 1
    appStore.incrementDuration()
  }, 1000)
}

function updateStreamingTranscript(text) {
  if (!text && liveTranscript.value) {
    return
  }
  liveTranscript.value = text || ''
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

function applyNativeSessionState(payload) {
  const patch = nativeSession.deriveNativeSessionPatch(payload)
  if (!patch) {
    return
  }

  const appStore = useAppStore()
  const { tray } = useElectron()

  if (patch.resetTranscript) {
    liveTranscript.value = ''
  }
  if (patch.clock === 'start') {
    startDurationClock()
  } else if (patch.clock === 'stop') {
    clearDurationTimer()
    audioLevel.value = 0
  }

  livePhase.value = patch.livePhase
  sessionStatusText.value = patch.statusText || statusCopy.IDLE_SESSION_STATUS_TEXT
  appStore.setRecording(patch.recording)
  appStore.setTranscribing(patch.transcribing)

  if (patch.transcript) {
    liveTranscript.value = patch.transcript
  }

  if (patch.record) {
    syncRecordIntoStore(patch.record)
    appStore.setCurrentRecordId(patch.record.id)
    appStore.setCurrentRecord(patch.record)
    appStore.setRecordStatus({ record_id: patch.record.id, status: patch.record.status })
  }

  applyNativeOptimization(patch, payload)

  if (patch.toast) {
    appStore.showToast(patch.toast, patch.toastTone ? { tone: patch.toastTone } : undefined)
  }

  tray.setIcon(patch.trayIcon)
}

// 自动优化的流式面板（与手动优化共用 optimizationStream，取消按钮走 cancelOptimization → Rust 取消）
function applyNativeOptimization(patch, payload) {
  if (!patch.optimization) {
    return
  }
  const optimizationStream = useOptimizationStreamStore()
  const recordId = patch.record?.id || optimizationStream.recordId
  if (patch.optimization === 'begin') {
    if (!(optimizationStream.isActive && optimizationStream.recordId === recordId)) {
      optimizationStream.begin(recordId, { source: 'auto' })
    }
    return
  }
  if (optimizationStream.recordId !== recordId) {
    return
  }
  if (patch.optimization === 'finish') {
    optimizationStream.finish(payload.text || '')
  } else if (patch.optimization === 'cancel') {
    // 取消后展示原文 + 「已取消」，不保留半截优化文本
    optimizationStream.begin(recordId, { source: 'auto' })
    if (payload.raw_text) {
      optimizationStream.append(payload.raw_text)
    }
    optimizationStream.cancel()
  } else if (patch.optimization === 'fail') {
    optimizationStream.begin(recordId, { source: 'auto' })
    if (payload.raw_text) {
      optimizationStream.append(payload.raw_text)
    }
    optimizationStream.fail(patch.optimizeError || '优化失败')
  }
}

function applyNativeOptimizationDelta(payload) {
  const optimizationStream = useOptimizationStreamStore()
  const recordId = payload?.record_id
  const chunk = payload?.text || ''
  if (!recordId || !chunk || optimizationStream.recordId !== recordId) {
    return
  }
  optimizationStream.append(chunk)
  if (livePhase.value === 'optimizing') {
    liveTranscript.value = optimizationStream.text
    sessionStatusText.value = '正在优化'
  }
}

// 用户已开始下一段录音：上一段的优化结果只入库，不打断当前会话
function applyNativeBackgroundOptimization(payload) {
  const appStore = useAppStore()
  const record = payload?.record
  if (record?.id) {
    syncRecordIntoStore(record)
    const optimizationStream = useOptimizationStreamStore()
    if (optimizationStream.recordId === record.id) {
      if (payload.optimized) {
        optimizationStream.finish(record.optimized_text || '')
      } else if (payload.cancelled) {
        optimizationStream.cancel()
      } else {
        optimizationStream.fail(payload.error || '优化失败')
      }
    }
  }
  if (payload?.message) {
    appStore.showToast(payload.message, payload.optimized ? undefined : { tone: 'error' })
  }
}

function attachNativeRecording() {
  if (nativeRecordingAttached) {
    return
  }
  nativeRecordingAttached = true

  const { recording } = useElectron()
  recording.onState(applyNativeSessionState)
  recording.onPartial((payload) => updateStreamingTranscript(payload?.text || ''))
  recording.onLevel((payload) => {
    audioLevel.value = Number(payload?.level) || 0
  })
  recording.onOptimization(applyNativeOptimizationDelta)
  recording.onBackgroundOptimization(applyNativeBackgroundOptimization)

  // 页面重载时 Actor 可能正在录音：同步一次当前状态
  recording.getState()
    .then((state) => {
      if (state && ['connecting', 'recording', 'finalizing', 'optimizing'].includes(state.phase)) {
        applyNativeSessionState(state)
      }
    })
    .catch((error) => console.warn('[Recorder] Failed to read native recording state:', error))
}

function detachNativeRecording() {
  if (!nativeRecordingAttached) {
    return
  }
  const { removeAllListeners } = useElectron()
  removeAllListeners('recording:state')
  removeAllListeners('recording:partial')
  removeAllListeners('recording:level')
  removeAllListeners('recording:optimization')
  removeAllListeners('recording:background-optimization')
  nativeRecordingAttached = false
}

async function startRecording() {
  const appStore = useAppStore()
  const { isDesktop, recording } = useElectron()

  if (!isDesktop) {
    appStore.showToast('录音只能在桌面应用里使用')
    return
  }

  // 状态由 recording:state 事件驱动；Actor 自己会吞掉录音中/收尾中的重复开始
  await recording.start().catch((error) => {
    appStore.showToast(error?.message || String(error) || '无法开始录音')
  })
}

function stopRecording() {
  const { isDesktop, recording } = useElectron()
  if (isDesktop) {
    void recording.stop()
  }
}

export function useRecorder() {
  const appStore = useAppStore()
  const settingsStore = useSettingsStore()
  const { isDesktop } = useElectron()

  onMounted(() => {
    mountedInstances += 1
    if (isDesktop) {
      attachNativeRecording()
    }
  })

  onUnmounted(() => {
    mountedInstances -= 1
    if (mountedInstances <= 0) {
      detachNativeRecording()
      clearDurationTimer()
      mountedInstances = 0
    }
  })

  return {
    isRecording: computed(() => appStore.isRecording),
    isTranscribing: computed(() => appStore.isTranscribing),
    duration,
    audioLevel,
    liveTranscript,
    livePhase,
    sessionStatusText,
    isMultiModeEnabled: computed(() => settingsStore.settings.recording_mode === 'multi'),
    isStreamingModeEnabled: computed(() => settingsStore.settings.recording_mode === 'streaming'),
    startRecording,
    stopRecording,
    cancelOptimization
  }
}
