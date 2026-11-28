import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { useElectron } from '@/composables/useElectron'

export const useRecordsStore = defineStore('records', () => {
  const { api, tray } = useElectron()

  const records = ref([])
  const isLoading = ref(false)

  // 记录集页面走 /records/search 服务端分页；store 只保留最近这么多条，供托盘菜单和首页「最近」使用。
  const RECENT_LIMIT = 200

  const sortedRecords = computed(() => {
    return [...records.value].sort((a, b) => new Date(b.created_at) - new Date(a.created_at))
  })

  function syncTrayMenu() {
    tray.updateMenu()
  }

  async function loadRecords() {
    isLoading.value = true

    try {
      const result = await api.getRecords({ limit: RECENT_LIMIT })
      records.value = Array.isArray(result) ? result : []
      syncTrayMenu()
      return records.value
    } catch (e) {
      console.error('Failed to load records:', e)
      throw e
    } finally {
      isLoading.value = false
    }
  }

  async function updateRecord(id, data) {
    try {
      const updated = await api.updateRecord(id, data)
      const index = records.value.findIndex((record) => record.id === id)
      if (index !== -1) {
        records.value[index] = { ...records.value[index], ...updated }
      } else if (updated?.id) {
        records.value.unshift(updated)
      }
      syncTrayMenu()
      return updated
    } catch (e) {
      console.error('Failed to update record:', e)
      throw e
    }
  }

  async function deleteRecord(id) {
    try {
      await api.deleteRecord(id)
      records.value = records.value.filter((record) => record.id !== id)
      syncTrayMenu()
    } catch (e) {
      console.error('Failed to delete record:', e)
      throw e
    }
  }

  async function deleteRecords(ids) {
    try {
      await Promise.all(ids.map((id) => api.deleteRecord(id)))
      records.value = records.value.filter((record) => !ids.includes(record.id))
      syncTrayMenu()
    } catch (e) {
      console.error('Failed to delete records:', e)
      throw e
    }
  }

  async function optimizeRecord(id) {
    try {
      const result = await api.optimizeRecord(id)
      const index = records.value.findIndex((record) => record.id === id)
      if (index !== -1) {
        // 非流式接口的 processing_time 是优化耗时，不是记录的处理耗时
        const optimizeTime = Number.isFinite(result?.processing_time) ? result.processing_time : null
        records.value[index] = {
          ...records.value[index],
          optimized_text: result?.optimized_text ?? records.value[index].optimized_text,
          optimize_time: optimizeTime,
          status: 'optimized'
        }
        syncTrayMenu()
      }
      return result
    } catch (e) {
      console.error('Failed to optimize record:', e)
      throw e
    }
  }

  async function exportRecord(id, format = 'txt') {
    try {
      return await api.exportRecord(id, format)
    } catch (e) {
      console.error('Failed to export record:', e)
      throw e
    }
  }

  function addRecord(record) {
    records.value.unshift(record)
    syncTrayMenu()
  }

  function updateRecordStatus(id, status) {
    const index = records.value.findIndex((record) => record.id === id)
    if (index !== -1) {
      records.value[index] = { ...records.value[index], ...status }
      syncTrayMenu()
    }
  }

  return {
    records,
    isLoading,
    sortedRecords,
    loadRecords,
    updateRecord,
    deleteRecord,
    deleteRecords,
    optimizeRecord,
    exportRecord,
    addRecord,
    updateRecordStatus
  }
})
