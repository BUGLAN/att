import { computed, onMounted, ref } from 'vue'
import { useRecordsStore } from '@/stores/records'
import { useAppStore } from '@/stores/app'
import { useOptimizationStreamStore } from '@/stores/optimizationStream'
import { useElectron } from './useElectron'
import { buildExportFilename, downloadFromUrl } from '@/utils/exportDownload.mjs'
import {
  RETRANSCRIBE_MAX_WAIT_MS,
  RETRANSCRIBE_POLL_INTERVAL_MS,
  retranscribeResultToast
} from '@/components/records/retranscribeModel.mjs'

// 当前播放的音频对象
const currentAudio = ref(null)
// 当前播放的记录 ID
const playingRecordId = ref(null)
// 手动优化的 AbortController：取消按钮 / 组件卸载时中断 fetch 流
let activeOptimizationController = null
// 「重新识别」可选的引擎（GET /api/asr/engines）：首页与记录集共用，展开记录时按需刷新
const asrEngines = ref({ current: '', engines: [], loaded: false })
let asrEnginesLoadedAt = 0
const ASR_ENGINES_TTL_MS = 10_000
// 正在轮询重新识别结果的记录 id（同一条不重复轮询）
const retranscribingIds = new Set()

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

export function useRecords() {
  const recordsStore = useRecordsStore()
  const appStore = useAppStore()
  const { api, clipboard } = useElectron()

  const records = computed(() => recordsStore.sortedRecords)

  async function loadRecords() {
    await recordsStore.loadRecords()
  }

  async function deleteRecord(id) {
    try {
      await recordsStore.deleteRecord(id)
      appStore.showToast('记录已删除')
    } catch (error) {
      appStore.showToast('删除记录失败')
      throw error
    }
  }

  function patchOptimizationResult(id, patch) {
    recordsStore.updateRecordStatus(id, patch)
    if (appStore.currentRecord?.id === id) {
      appStore.setCurrentRecord({ ...appStore.currentRecord, ...patch })
    }
  }

  async function optimizeRecord(id) {
    const optimizationStream = useOptimizationStreamStore()

    // 同一记录重复点击：复用正在跑的流；不同记录则先中断上一条，避免两段文本交错
    if (optimizationStream.isActive) {
      if (optimizationStream.recordId === id) {
        return
      }
      activeOptimizationController?.abort()
      await api.cancelOptimizeRecord(optimizationStream.recordId).catch(() => {})
    }

    const controller = new AbortController()
    activeOptimizationController = controller
    optimizationStream.begin(id, { source: 'manual' })
    // 不立刻清空旧 optimized_text：流式失败时记录卡仍可回退显示上一次结果
    patchOptimizationResult(id, { status: 'optimizing' })

    let receivedDelta = false
    let streamError = null
    let doneEvent = null

    try {
      await api.optimizeRecordStream(id, {
        signal: controller.signal,
        onEvent(event) {
          if (event.type === 'delta' && event.text) {
            receivedDelta = true
            optimizationStream.append(event.text)
          } else if (event.type === 'done') {
            doneEvent = event
          } else if (event.type === 'error') {
            streamError = new Error(event.error || '优化失败')
          }
        }
      })

      if (streamError) {
        throw streamError
      }

      if (!doneEvent) {
        throw new Error('流式优化未返回最终结果')
      }

      optimizationStream.finish(doneEvent.optimized_text || '')
      // doneEvent.processing_time 是本次优化耗时，只写 optimize_time，不能覆盖识别阶段的处理耗时
      patchOptimizationResult(id, {
        status: 'optimized',
        optimized_text: doneEvent.optimized_text || '',
        optimize_time: Number.isFinite(doneEvent.processing_time) ? doneEvent.processing_time : null
      })

      if (doneEvent.degraded) {
        // 服务端在首条增量前发现上游不支持 stream，已回退一次性优化
        appStore.showToast('当前上游不支持流式，已自动降级为一次性返回')
      } else {
        appStore.showToast('优化完成')
      }
    } catch (error) {
      // 已被下一条记录的手动优化取代：旧请求的错误/取消不能覆盖新状态
      if (optimizationStream.recordId !== id || activeOptimizationController !== controller) {
        return
      }

      const rawText = recordsStore.records.find((item) => item.id === id)?.raw_text || ''

      if (error?.cancelled) {
        optimizationStream.begin(id, { source: 'manual' })
        if (rawText) {
          optimizationStream.append(rawText)
        }
        optimizationStream.cancel()
        patchOptimizationResult(id, { status: 'transcribed' })
        appStore.showToast('已取消优化，保留原始文本')
        return
      }

      // 首条增量之前失败才降级到非流式端点；已经渲染过增量时不再重试，
      // 否则用户会先看到半段、再看到整段重复文本。
      if (!receivedDelta && !streamError) {
        try {
          const result = await recordsStore.optimizeRecord(id)
          optimizationStream.begin(id, { source: 'manual' })
          optimizationStream.append(result?.optimized_text || '')
          optimizationStream.finish(result?.optimized_text || '')
          patchOptimizationResult(id, {
            status: 'optimized',
            optimized_text: result?.optimized_text || ''
          })
          appStore.showToast('优化完成（流式不可用，已自动降级）')
          return
        } catch (fallbackError) {
          error = fallbackError
        }
      }

      optimizationStream.begin(id, { source: 'manual' })
      if (rawText) {
        optimizationStream.append(rawText)
      }
      optimizationStream.fail(error?.message || '优化失败')
      patchOptimizationResult(id, { status: 'transcribed' })
      appStore.showToast(error?.message || '优化失败', { tone: 'error' })
      throw error
    } finally {
      if (activeOptimizationController === controller) {
        activeOptimizationController = null
      }
    }
  }

  async function cancelOptimize(id) {
    const optimizationStream = useOptimizationStreamStore()
    if (optimizationStream.recordId === id) {
      activeOptimizationController?.abort()
    }
    const result = await api.cancelOptimizeRecord(id).catch(() => null)
    if (result?.cancelled && optimizationStream.recordId === id) {
      optimizationStream.cancel()
    }
  }

  /** 拉取「重新识别」可选引擎；10 秒内拉过就不再请求（force 时总是请求） */
  async function loadAsrEngines({ force = false } = {}) {
    if (!force && Date.now() - asrEnginesLoadedAt < ASR_ENGINES_TTL_MS) {
      return asrEngines.value
    }
    try {
      const result = await api.getAsrEngines()
      asrEngines.value = {
        current: result?.current || '',
        engines: Array.isArray(result?.engines) ? result.engines : [],
        loaded: true
      }
      asrEnginesLoadedAt = Date.now()
    } catch (error) {
      console.warn('Failed to load ASR engines:', error)
    }
    return asrEngines.value
  }

  /** 同步一条记录的最新内容：store（首页 / 托盘）、编辑弹窗、调用方自己的列表（记录集分页） */
  function syncRecord(record, onUpdate) {
    if (!record?.id) {
      return
    }
    recordsStore.updateRecordStatus(record.id, record)
    if (appStore.currentRecord?.id === record.id) {
      appStore.setCurrentRecord({ ...appStore.currentRecord, ...record })
    }
    onUpdate?.(record)
  }

  /** 轮询到记录离开「识别中」，返回最终记录；超时抛错 */
  async function waitForTranscription(id) {
    const deadline = Date.now() + RETRANSCRIBE_MAX_WAIT_MS
    while (Date.now() < deadline) {
      await sleep(RETRANSCRIBE_POLL_INTERVAL_MS)
      let record
      try {
        record = await api.getRecord(id)
      } catch (error) {
        // 单次网络抖动不改判识别任务失败，继续轮询到终态或总等待上限。
        continue
      }
      if (record?.status !== 'transcribing') {
        return record
      }
    }
    throw new Error('重新识别超过 15 分钟仍未结束，稍后刷新记录查看结果')
  }

  /**
   * 用记录的录音重新识别（provider 为空 = 当前设置的引擎）。
   * 成功覆盖原文并清空优化结果；失败时记录标为失败、原有文字不动。onUpdate 收到每次的最新记录。
   */
  async function retranscribeRecord(record, provider = '', { onUpdate } = {}) {
    if (!record?.id || retranscribingIds.has(record.id)) {
      return
    }
    retranscribingIds.add(record.id)
    try {
      const accepted = await api.retranscribeRecord(record.id, provider)
      syncRecord(accepted, onUpdate)
      appStore.showToast('正在重新识别…')
      const finished = await waitForTranscription(record.id)
      syncRecord(finished, onUpdate)
      const { message, tone } = retranscribeResultToast(finished)
      appStore.showToast(message, { tone, duration: tone === 'success' ? undefined : 6000 })
      return finished
    } catch (error) {
      appStore.showToast(error?.message || '重新识别失败', { tone: 'error', duration: 6000 })
      return null
    } finally {
      retranscribingIds.delete(record.id)
    }
  }

  async function copyText(record) {
    const text = record.optimized_text || record.raw_text
    if (!text) {
      return
    }

    await clipboard.writeText(text)
    appStore.showToast('文本已复制到剪贴板')
  }

  async function toggleFavorite(record) {
    try {
      const updated = await recordsStore.updateRecord(record.id, {
        is_favorite: !record.is_favorite
      })
      appStore.showToast(updated.is_favorite ? '已收藏记录' : '已取消收藏')
      return updated
    } catch (error) {
      appStore.showToast('更新收藏状态失败')
      throw error
    }
  }

  async function exportRecord(record, format = 'txt') {
    try {
      const url = await recordsStore.exportRecord(record.id, format)
      downloadFromUrl(url, buildExportFilename(record, format))
      appStore.showToast(`已导出 ${format.toUpperCase()} 文件`)
      return url
    } catch (error) {
      appStore.showToast('导出失败')
      throw error
    }
  }

  /**
   * 切换音频播放/暂停
   * 播放新音频时自动停止上一条
   */
  async function toggleAudio(recordId) {
    // 如果正在播放同一条，则暂停
    if (playingRecordId.value === recordId && currentAudio.value) {
      currentAudio.value.pause()
      currentAudio.value = null
      playingRecordId.value = null
      return
    }

    // 停止当前播放
    if (currentAudio.value) {
      currentAudio.value.pause()
      currentAudio.value = null
      playingRecordId.value = null
    }

    try {
      const audioUrl = await api.getRecordAudio(recordId)
      const audio = new Audio(audioUrl)

      audio.onended = () => {
        currentAudio.value = null
        playingRecordId.value = null
      }

      audio.onerror = () => {
        currentAudio.value = null
        playingRecordId.value = null
        appStore.showToast('播放音频失败')
      }

      currentAudio.value = audio
      playingRecordId.value = recordId
      await audio.play()
    } catch (error) {
      console.error('Failed to play audio:', error)
      currentAudio.value = null
      playingRecordId.value = null
      appStore.showToast('播放音频失败')
    }
  }

  /**
   * 检查指定记录是否正在播放
   */
  function isPlaying(recordId) {
    return playingRecordId.value === recordId
  }

  onMounted(() => {
    loadRecords()
  })

  return {
    records,
    loadRecords,
    deleteRecord,
    optimizeRecord,
    cancelOptimize,
    asrEngines,
    loadAsrEngines,
    retranscribeRecord,
    copyText,
    toggleFavorite,
    exportRecord,
    toggleAudio,
    isPlaying
  }
}
