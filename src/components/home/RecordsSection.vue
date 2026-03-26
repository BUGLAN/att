<template>
  <div class="records-section">
    <div class="section-header">
      <div class="section-header-left">
        <h2>历史记录</h2>
        <span class="record-count">{{ records.length }} 条</span>
      </div>
      <div class="section-header-right">
        <button class="btn btn-secondary" type="button" @click="toggleSelectMode">
          {{ selectMode ? '退出选择' : '选择模式' }}
        </button>
      </div>
    </div>

    <div class="records-list">
      <RecordCard
        v-for="record in records"
        :key="record.id"
        :record="record"
        :select-mode="selectMode"
        :selected="selectedRecords.has(record.id)"
        :is-playing="isPlaying(record.id)"
        @select="toggleSelection(record.id)"
        @edit="openEditModal(record)"
        @copy="copyText(record)"
        @toggle-audio="toggleAudio(record.id)"
        @optimize="optimizeRecord(record.id)"
        @toggle-favorite="toggleFavorite(record)"
      />

      <div v-if="!records.length" class="empty-state">
        <p>暂无录音记录</p>
      </div>
    </div>

    <div v-if="selectMode" class="select-toolbar">
      <label class="select-all-label">
        <input :checked="isAllSelected" type="checkbox" @change="toggleSelectAll">
        全选
      </label>
      <button class="btn-delete-selected" type="button" @click="deleteSelected">
        删除选中 ({{ selectedRecords.size }})
      </button>
      <button class="btn-cancel-select" type="button" @click="toggleSelectMode">
        取消
      </button>
    </div>
  </div>
</template>

<script setup>
import { computed } from 'vue'
import { useRecords } from '@/composables/useRecords'
import { useAppStore } from '@/stores/app'
import RecordCard from './RecordCard.vue'

const appStore = useAppStore()
const {
  records,
  selectMode,
  selectedRecords,
  deleteSelected,
  copyText,
  toggleFavorite,
  optimizeRecord,
  toggleSelectMode,
  toggleSelection,
  selectAll,
  clearSelection,
  toggleAudio,
  isPlaying
} = useRecords()

const isAllSelected = computed(() => {
  return records.value.length > 0 && selectedRecords.value.size === records.value.length
})

function toggleSelectAll() {
  if (isAllSelected.value) {
    clearSelection()
  } else {
    selectAll()
  }
}

function openEditModal(record) {
  appStore.setCurrentRecord(record)
  appStore.openModal('recordEdit')
}
</script>

<style scoped>
.empty-state {
  text-align: center;
  padding: var(--space-xl);
  color: var(--text-muted);
}
</style>
