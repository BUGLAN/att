import { ref } from 'vue'
import { useAppStore } from '@/stores/app'
import { useElectron } from '@/composables/useElectron'

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
  const isImporting = ref(false)

  async function loadItems(filters = {}) {
    isLoading.value = true

    try {
      const result = await api.getClipboardItems(filters)
      items.value = Array.isArray(result) ? result : []
    } catch (error) {
      console.error('[Clipboard] Failed to load items:', error)
      appStore.showToast('加载剪切板记录失败')
    } finally {
      isLoading.value = false
    }
  }

  async function loadStats() {
    try {
      stats.value = await api.getClipboardStats()
    } catch (error) {
      console.error('[Clipboard] Failed to load stats:', error)
    }
  }

  async function refresh(filters = {}) {
    await Promise.all([
      loadItems(filters),
      loadStats()
    ])
  }

  async function importCurrentClipboard() {
    isImporting.value = true

    try {
      const content = await clipboard.readText()
      if (!content || !content.trim()) {
        appStore.showToast('当前系统剪切板没有可导入的文本')
        return null
      }

      const created = await api.createClipboardItem({
        content,
        source: 'manual_import'
      })
      appStore.showToast('已导入当前剪切板')
      return created
    } catch (error) {
      console.error('[Clipboard] Failed to import current clipboard:', error)
      appStore.showToast('导入当前剪切板失败')
      return null
    } finally {
      isImporting.value = false
    }
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

  async function copyItemContent(item) {
    await clipboard.writeText(item.content || '')
    appStore.showToast('已复制到系统剪切板')
  }

  return {
    items,
    stats,
    isLoading,
    isImporting,
    refresh,
    importCurrentClipboard,
    toggleFavorite,
    togglePin,
    removeItem,
    copyItemContent
  }
}
