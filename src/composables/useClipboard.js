import { ref } from 'vue'
import { useAppStore } from '@/stores/app'
import { useElectron } from '@/composables/useElectron'
import { createLatestRequestGuard, runLatest } from '@/utils/latestRequest.mjs'

export function useClipboard() {
  const appStore = useAppStore()
  const { api, clipboard } = useElectron()

  const items = ref([])
  const stats = ref({
    total: 0,
    today: 0,
    favorites: 0,
    pinned: 0
  })
  const isLoading = ref(false)
  const itemsRequest = createLatestRequestGuard()
  const statsRequest = createLatestRequestGuard()

  async function loadItems(filters = {}) {
    return runLatest(itemsRequest, () => api.getClipboardItems(filters), {
      onStart: () => {
        isLoading.value = true
      },
      onResult: (result) => {
        items.value = Array.isArray(result) ? result : []
      },
      onError: (error) => {
        console.error('[Clipboard] Failed to load items:', error)
        appStore.showToast('加载剪切板记录失败')
      },
      onFinally: () => {
        isLoading.value = false
      }
    })
  }

  async function loadStats() {
    return runLatest(statsRequest, () => api.getClipboardStats(), {
      onResult: (result) => {
        stats.value = result
      },
      onError: (error) => {
        console.error('[Clipboard] Failed to load stats:', error)
      }
    })
  }

  async function refresh(filters = {}) {
    await Promise.all([
      loadItems(filters),
      loadStats()
    ])
  }

  async function toggleFavorite(item) {
    return api.updateClipboardItem(item.id, {
      is_favorite: !item.is_favorite
    })
  }

  async function togglePin(item) {
    return api.updateClipboardItem(item.id, {
      is_pinned: !item.is_pinned
    })
  }

  async function removeItem(itemId) {
    await api.deleteClipboardItem(itemId)
  }

  /** 批量删除，返回删掉的条数 */
  async function removeItems(itemIds) {
    const result = await api.deleteClipboardItems(itemIds)
    return result?.deleted ?? 0
  }

  /** 清空历史（置顶与收藏保留），返回 { deleted, kept } */
  async function clearHistory() {
    const result = await api.clearClipboardHistory()
    return { deleted: result?.deleted ?? 0, kept: result?.kept ?? 0 }
  }

  async function copyItemContent(item) {
    try {
      if (item?.item_type === 'image' && item?.metadata?.image_path) {
        await clipboard.writeImage({
          imagePath: item.metadata.image_path,
          imageToken: item.content || ''
        })
        appStore.showToast('已复制图片到系统剪切板')
        return
      }

      await clipboard.writeText(item.content || '')
      appStore.showToast('已复制到系统剪切板')
    } catch (error) {
      // 剪贴板被其他程序占用、图片文件丢失等：说清原因，不要静默失败
      console.error('[Clipboard] Failed to copy item:', error)
      appStore.showToast(typeof error === 'string' ? error : '复制失败，剪贴板可能正被其他程序占用', { tone: 'error' })
    }
  }

  return {
    items,
    stats,
    isLoading,
    refresh,
    refreshStats: loadStats,
    toggleFavorite,
    togglePin,
    removeItem,
    removeItems,
    clearHistory,
    copyItemContent
  }
}
