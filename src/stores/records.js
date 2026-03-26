import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { useElectron } from '@/composables/useElectron'

export const useRecordsStore = defineStore('records', () => {
  const { api, tray } = useElectron()

  const records = ref([])
  const currentRecord = ref(null)
  const isLoading = ref(false)
  const error = ref(null)

  const sortedRecords = computed(() => {
    return [...records.value].sort((a, b) => new Date(b.created_at) - new Date(a.created_at))
  })

  const recordCount = computed(() => records.value.length)

  function syncTrayMenu() {
    tray.updateMenu(sortedRecords.value.map((record) => ({
      id: record.id,
      created_at: record.created_at,
      raw_text: record.raw_text,
      optimized_text: record.optimized_text,
      status: record.status
    })))
  }

  async function loadRecords() {
    isLoading.value = true
    error.value = null

    try {
      const result = await api.getRecords()
      records.value = Array.isArray(result) ? result : []
      syncTrayMenu()
      return records.value
    } catch (e) {
      error.value = e.message
      console.error('Failed to load records:', e)
      throw e
    } finally {
      isLoading.value = false
    }
  }

  async function getRecord(id) {
    try {
      const record = await api.getRecord(id)
      currentRecord.value = record
      return record
    } catch (e) {
      error.value = e.message
      console.error('Failed to get record:', e)
      throw e
    }
  }

  async function updateRecord(id, data) {
    try {
      const updated = await api.updateRecord(id, data)
      const index = records.value.findIndex((record) => record.id === id)
      if (index !== -1) {
        records.value[index] = { ...records.value[index], ...updated }
        syncTrayMenu()
      }
      return updated
    } catch (e) {
      error.value = e.message
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
      error.value = e.message
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
      error.value = e.message
      console.error('Failed to delete records:', e)
      throw e
    }
  }

  async function optimizeRecord(id) {
    try {
      const result = await api.optimizeRecord(id)
      const index = records.value.findIndex((record) => record.id === id)
      if (index !== -1) {
        records.value[index] = {
          ...records.value[index],
          ...result,
          status: 'optimized'
        }
        syncTrayMenu()
      }
      return result
    } catch (e) {
      error.value = e.message
      console.error('Failed to optimize record:', e)
      throw e
    }
  }

  async function exportRecord(id, format = 'txt') {
    try {
      return await api.exportRecord(id, format)
    } catch (e) {
      error.value = e.message
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
    currentRecord,
    isLoading,
    error,
    sortedRecords,
    recordCount,
    loadRecords,
    getRecord,
    updateRecord,
    deleteRecord,
    deleteRecords,
    optimizeRecord,
    exportRecord,
    addRecord,
    updateRecordStatus
  }
})
