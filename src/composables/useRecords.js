import { computed, onMounted, ref } from 'vue'
import { useRecordsStore } from '@/stores/records'
import { useAppStore } from '@/stores/app'
import { useOptimizationStreamStore } from '@/stores/optimizationStream'
import { useElectron } from './useElectron'
import { buildExportFilename, downloadFromUrl } from '@/utils/exportDownload.mjs'

// 当前播放的音频对象
const currentAudio = ref(null)
// 当前播放的记录 ID
const playingRecordId = ref(null)
// 手动优化的 AbortController：取消按钮 / 组件卸载时中断 fetch 流
let activeOptimizationController = null

export function useRecords() {
  const recordsStore = useRecordsStore()
  const appStore = useAppStore()
  const { api, clipboard } = useElectron()

  const records = computed(() => recordsStore.sortedRecords)
  const isLoading = computed(() => recordsStore.isLoading)
  const selectMode = computed(() => appStore.selectMode)
  const selectedRecords = computed(() => appStore.selectedRecords)

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

  async function deleteSelected() {
    const ids = Array.from(appStore.selectedRecords)
    if (ids.length === 0) {
      return
    }

    try {
      await recordsStore.deleteRecords(ids)
      appStore.clearSelection()
      appStore.showToast(`已删除 ${ids.length} 条记录`)
    } catch (error) {
      appStore.showToast('批量删除失败')
      throw error
    }
  }

  function patchOptimizationResult(id, patch) {
    recordsStore.updateRecordStatus(id, patch)
    if (!appStore.currentRecordId || appStore.currentRecordId === id) {
      appStore.setRecordStatus({ record_id: id, ...patch })
    }
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
      patchOptimizationResult(id, {
        status: 'optimized',
        optimized_text: doneEvent.optimized_text || '',
        processing_time: doneEvent.processing_time
      })

      if (doneEvent.degraded) {
        // 服务端在首条增量前发现上游不支持 stream，已回退一次性优化
        appStore.showToast('当前上游不支持流式，已自动降级为一次性返回')
      } else if (doneEvent.truncated) {
        // 输出达到 Max Tokens 上限被截断（思考 token 也计入），文本可能不完整
        appStore.showToast('优化完成，但输出已达 Max Tokens 上限被截断，可在设置 → LLM 服务里调优', {
          tone: 'warn',
          duration: 8000
        })
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

  function toggleSelectMode() {
    appStore.toggleSelectMode()
  }

  function toggleSelection(recordId) {
    appStore.toggleRecordSelection(recordId)
  }

  function selectAll() {
    appStore.selectAllRecords(records.value.map((record) => record.id))
  }

  function clearSelection() {
    appStore.clearSelection()
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
    isLoading,
    selectMode,
    selectedRecords,
    loadRecords,
    deleteRecord,
    deleteSelected,
    optimizeRecord,
    cancelOptimize,
    copyText,
    toggleFavorite,
    exportRecord,
    toggleSelectMode,
    toggleSelection,
    selectAll,
    clearSelection,
    toggleAudio,
    isPlaying
  }
}
