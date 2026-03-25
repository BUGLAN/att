/**
 * 录音 Composable
 * 处理录音逻辑和状态管理
 */

import { ref, computed, onMounted, onUnmounted } from 'vue'
import { useAppStore } from '@/stores/app'
import { useRecordsStore } from '@/stores/records'
import { useSettingsStore } from '@/stores/settings'
import { useElectron } from './useElectron'

export function useRecorder() {
  const appStore = useAppStore()
  const recordsStore = useRecordsStore()
  const settingsStore = useSettingsStore()
  const { api, startStatusPolling, stopStatusPolling, onRecordStatus, onRecordCompleted, tray } = useElectron()

  const isRecording = computed(() => appStore.isRecording)
  const isTranscribing = computed(() => appStore.isTranscribing)
  const duration = ref(0)
  const mediaRecorder = ref(null)
  const chunks = ref([])
  const durationTimer = ref(null)
  const stream = ref(null)

  /**
   * 开始录音
   */
  async function startRecording() {
    try {
      // 使用用户选择的麦克风设备
      const deviceId = settingsStore.settings.audio_input_device || 'default'
      console.log('[Recorder] Using audio device:', deviceId)

      stream.value = await navigator.mediaDevices.getUserMedia({
        audio: {
          deviceId: deviceId === 'default' ? 'default' : { exact: deviceId }
        }
      })

      // 打印当前使用的麦克风设备信息
      const audioTrack = stream.value.getAudioTracks()[0]
      console.log('[Recorder] Using microphone:', audioTrack?.label || 'Unknown device')
      console.log('[Recorder] Audio track settings:', audioTrack?.getSettings())

      mediaRecorder.value = new MediaRecorder(stream.value, {
        mimeType: 'audio/webm;codecs=opus'
      })

      chunks.value = []

      mediaRecorder.value.ondataavailable = (e) => {
        if (e.data.size > 0) {
          chunks.value.push(e.data)
        }
      }

      mediaRecorder.value.onstop = async () => {
        const blob = new Blob(chunks.value, { type: 'audio/webm' })
        console.log('[Recorder] Recording stopped, blob size:', blob.size)
        await uploadRecording(blob)
        stream.value.getTracks().forEach(track => track.stop())
      }

      mediaRecorder.value.start(1000) // 每秒收集一次
      appStore.setRecording(true)
      tray.setIcon('recording')

      // 开始计时
      duration.value = 0
      durationTimer.value = setInterval(() => {
        duration.value++
        appStore.incrementDuration()
      }, 1000)

    } catch (error) {
      console.error('Failed to start recording:', error)
      appStore.showToast('无法访问麦克风')
    }
  }

  /**
   * 停止录音
   */
  function stopRecording() {
    if (mediaRecorder.value && mediaRecorder.value.state !== 'inactive') {
      mediaRecorder.value.stop()
      appStore.setRecording(false)
      tray.setIcon('transcribing')

      if (durationTimer.value) {
        clearInterval(durationTimer.value)
        durationTimer.value = null
      }
    }
  }

  /**
   * 上传录音
   */
  async function uploadRecording(blob) {
    try {
      // 检查最短时长
      if (duration.value < 1) {
        appStore.showToast('录音时间太短')
        tray.setIcon('idle')
        return
      }

      appStore.setTranscribing(true)
      const result = await api.uploadAudio(blob, `recording_${Date.now()}.webm`)

      if (result.record_id) {
        appStore.setCurrentRecordId(result.record_id)

        // 添加临时记录到列表
        recordsStore.addRecord({
          id: result.record_id,
          created_at: new Date().toISOString(),
          duration: result.duration || duration.value,
          status: 'transcribing',
          raw_text: '',
          optimized_text: ''
        })

        // 开始轮询状态
        startStatusPolling(result.record_id)
      }
    } catch (error) {
      console.error('Upload failed:', error)
      appStore.showToast('上传失败')
      appStore.setTranscribing(false)
      tray.setIcon('idle')
    }
  }

  // 监听状态更新
  onMounted(() => {
    onRecordStatus((status) => {
      console.log('[Renderer] Received record:status:', status)
      appStore.setRecordStatus(status)
      recordsStore.updateRecordStatus(status.record_id, status)
    })

    onRecordCompleted((data) => {
      console.log('[Renderer] Received record:completed:', data)
      appStore.setTranscribing(false)
      stopStatusPolling()
      tray.setIcon('idle')

      // 更新记录
      recordsStore.updateRecordStatus(data.record_id, data)

      // 自动复制粘贴
      if (settingsStore.settings.auto_copy_to_clipboard) {
        const text = data.raw_text || data.optimized_text
        if (text) {
          // 复制到剪贴板（通过 Electron）
          navigator.clipboard.writeText(text)
          appStore.showToast('已复制到剪贴板')
        }
      }

      // 自动优化
      if (settingsStore.settings.auto_optimize && data.status === 'transcribed') {
        optimizeRecord(data.record_id)
      }

      appStore.showToast('转写完成')
    })
  })

  // 清理
  onUnmounted(() => {
    if (durationTimer.value) {
      clearInterval(durationTimer.value)
    }
    if (stream.value) {
      stream.value.getTracks().forEach(track => track.stop())
    }
  })

  /**
   * 优化记录
   */
  async function optimizeRecord(id) {
    try {
      tray.setIcon('transcribing')
      const result = await api.optimizeRecord(id)
      recordsStore.updateRecordStatus(id, result)
      appStore.showToast('优化完成')
    } catch (error) {
      console.error('Optimize failed:', error)
      appStore.showToast('优化失败')
    } finally {
      tray.setIcon('idle')
    }
  }

  return {
    isRecording,
    isTranscribing,
    duration,
    startRecording,
    stopRecording
  }
}
