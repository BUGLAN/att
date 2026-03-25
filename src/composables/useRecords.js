/**
 * 记录管理 Composable
 * 处理记录列表操作
 */

import { computed, onMounted } from 'vue'
import { useRecordsStore } from '@/stores/records'
import { useAppStore } from '@/stores/app'
import { useElectron } from './useElectron'

export function useRecords() {
  const recordsStore = useRecordsStore()
  const appStore = useAppStore()
  const { api } = useElectron()

  const records = computed(() => recordsStore.sortedRecords)
  const isLoading = computed(() => recordsStore.isLoading)
  const selectMode = computed(() => appStore.selectMode)
  const selectedRecords = computed(() => appStore.selectedRecords)

  /**
   * 加载记录列表
   */
  async function loadRecords() {
    await recordsStore.loadRecords()
  }

  /**
   * 删除记录
   */
  async function deleteRecord(id) {
    try {
      await recordsStore.deleteRecord(id)
      appStore.showToast('已删除')
    } catch (error) {
      appStore.showToast('删除失败')
    }
  }

  /**
   * 批量删除选中记录
   */
  async function deleteSelected() {
    const ids = Array.from(appStore.selectedRecords)
    if (ids.length === 0) return

    try {
      await recordsStore.deleteRecords(ids)
      appStore.clearSelection()
      appStore.showToast(`已删除 ${ids.length} 条记录`)
    } catch (error) {
      appStore.showToast('删除失败')
    }
  }

  /**
   * 优化记录
   */
  async function optimizeRecord(id) {
    try {
      await recordsStore.optimizeRecord(id)
      appStore.showToast('优化完成')
    } catch (error) {
      appStore.showToast('优化失败')
    }
  }

  /**
   * 复制记录文本
   */
  async function copyText(record) {
    const text = record.optimized_text || record.raw_text
    if (text) {
      await navigator.clipboard.writeText(text)
      appStore.showToast('已复制')
    }
  }

  /**
   * 导出记录
   */
  async function exportRecord(id, format = 'txt') {
    try {
      const result = await recordsStore.exportRecord(id, format)
      // TODO: 处理下载
      appStore.showToast('导出成功')
      return result
    } catch (error) {
      appStore.showToast('导出失败')
    }
  }

  /**
   * 切换选择模式
   */
  function toggleSelectMode() {
    appStore.toggleSelectMode()
  }

  /**
   * 切换记录选择
   */
  function toggleSelection(recordId) {
    appStore.toggleRecordSelection(recordId)
  }

  /**
   * 全选
   */
  function selectAll() {
    appStore.selectAllRecords(records.value.map(r => r.id))
  }

  /**
   * 取消选择
   */
  function clearSelection() {
    appStore.clearSelection()
  }

  /**
   * 播放音频
   */
  async function playAudio(recordId) {
    try {
      // 在 Electron 中使用 IPC 获取完整 URL
      const audioUrl = await api.getRecordAudio(recordId)
      const audio = new Audio(audioUrl)
      audio.play()
    } catch (error) {
      console.error('Failed to play audio:', error)
      appStore.showToast('播放失败')
    }
  }

  // 初始化时加载记录
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
    exportRecord,
    toggleSelectMode,
    toggleSelection,
    selectAll,
    clearSelection,
    playAudio
  }
}
