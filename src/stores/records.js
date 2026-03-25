/**
 * 记录状态管理
 * 管理录音记录列表
 */

import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { useElectron } from '@/composables/useElectron'

export const useRecordsStore = defineStore('records', () => {
  const { api } = useElectron()

  // === 状态 ===
  const records = ref([])
  const currentRecord = ref(null)
  const isLoading = ref(false)
  const error = ref(null)

  // === 计算属性 ===
  const sortedRecords = computed(() => {
    return [...records.value].sort((a, b) => {
      return new Date(b.created_at) - new Date(a.created_at)
    })
  })

  const recordCount = computed(() => records.value.length)

  // === 操作 ===
  async function loadRecords() {
    isLoading.value = true
    error.value = null

    try {
      const result = await api.getRecords()
      records.value = Object.values(result)
    } catch (e) {
      error.value = e.message
      console.error('Failed to load records:', e)
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
      return null
    }
  }

  async function updateRecord(id, data) {
    try {
      const updated = await api.updateRecord(id, data)
      // 更新本地列表
      const index = records.value.findIndex(r => r.id === id)
      if (index !== -1) {
        records.value[index] = { ...records.value[index], ...updated }
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
      // 从本地列表移除
      records.value = records.value.filter(r => r.id !== id)
    } catch (e) {
      error.value = e.message
      console.error('Failed to delete record:', e)
      throw e
    }
  }

  async function deleteRecords(ids) {
    try {
      await Promise.all(ids.map(id => api.deleteRecord(id)))
      // 从本地列表移除
      records.value = records.value.filter(r => !ids.includes(r.id))
    } catch (e) {
      error.value = e.message
      console.error('Failed to delete records:', e)
      throw e
    }
  }

  async function optimizeRecord(id) {
    try {
      const result = await api.optimizeRecord(id)
      // 更新本地记录
      const index = records.value.findIndex(r => r.id === id)
      if (index !== -1) {
        records.value[index] = { ...records.value[index], ...result }
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
      const result = await api.exportRecord(id, format)
      return result
    } catch (e) {
      error.value = e.message
      console.error('Failed to export record:', e)
      throw e
    }
  }

  // 添加新记录（上传后）
  function addRecord(record) {
    records.value.unshift(record)
  }

  // 更新记录状态（轮询时）
  function updateRecordStatus(id, status) {
    const index = records.value.findIndex(r => r.id === id)
    if (index !== -1) {
      records.value[index] = { ...records.value[index], ...status }
    }
  }

  return {
    // 状态
    records,
    currentRecord,
    isLoading,
    error,
    sortedRecords,
    recordCount,

    // 操作
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
