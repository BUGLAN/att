import { computed, onMounted, ref } from 'vue'
import { useRecordsStore } from '@/stores/records'
import { useAppStore } from '@/stores/app'
import { useElectron } from './useElectron'
import { buildExportFilename, downloadFromUrl } from '@/utils/exportDownload.mjs'

// 当前播放的音频对象
const currentAudio = ref(null)
// 当前播放的记录 ID
const playingRecordId = ref(null)

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

  async function optimizeRecord(id) {
    try {
      await recordsStore.optimizeRecord(id)
      appStore.showToast('优化完成')
    } catch (error) {
      appStore.showToast('优化失败')
      throw error
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
