<template>
  <div class="records-section">
    <!-- 标题栏 -->
    <div class="section-header">
      <h2>历史记录</h2>
      <div class="section-header-right">
        <span class="record-count">{{ records.length }} 条</span>
        <button class="header-settings-btn" @click="toggleSelectMode">
          {{ selectMode ? '取消' : '⋮' }}
        </button>
      </div>
    </div>

    <!-- 记录列表 -->
    <div class="records-list">
      <RecordCard
        v-for="record in records"
        :key="record.id"
        :record="record"
        :select-mode="selectMode"
        :selected="selectedRecords.has(record.id)"
        @select="toggleSelection(record.id)"
        @edit="openEditModal(record)"
        @delete="deleteRecord(record.id)"
        @optimize="optimizeRecord(record.id)"
        @copy="copyText(record)"
        @play="playAudio(record.id)"
      />

      <!-- 空状态 -->
      <div v-if="records.length === 0" class="empty-state">
        <p>暂无录音记录</p>
      </div>
    </div>

    <!-- 选择工具栏 -->
    <div v-if="selectMode" class="select-toolbar">
      <label class="select-all-label">
        <input
          type="checkbox"
          :checked="isAllSelected"
          @change="toggleSelectAll"
        />
        全选
      </label>
      <button class="btn-delete-selected" @click="deleteSelected">
        删除选中 ({{ selectedRecords.size }})
      </button>
      <button class="btn-cancel-select" @click="toggleSelectMode">
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
  deleteRecord,
  deleteSelected,
  optimizeRecord,
  copyText,
  toggleSelectMode,
  toggleSelection,
  selectAll,
  clearSelection,
  playAudio
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
  appStore.currentRecord = record
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
