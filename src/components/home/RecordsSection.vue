<template>
  <div class="records-section">
    <div class="section-header">
      <div class="section-header-left">
        <h2>历史记录</h2>
        <BaseBadge tone="neutral" mono>{{ records.length }} 条</BaseBadge>
      </div>
      <div class="section-header-right">
        <BaseButton variant="outline" @click="toggleSelectMode">
          {{ selectMode ? '退出选择' : '选择模式' }}
        </BaseButton>
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
        @export="(format) => exportRecord(record, format)"
        @toggle-audio="toggleAudio(record.id)"
        @optimize="optimizeRecord(record.id)"
        @toggle-favorite="toggleFavorite(record)"
      />

      <EmptyState
        v-if="!records.length"
        size="sm"
        title="暂无录音记录"
        description="完成一次录音后，转写结果会自动出现在这里。"
      />
    </div>

    <div v-if="selectMode" class="select-toolbar">
      <label class="select-all-label">
        <input :checked="isAllSelected" type="checkbox" @change="toggleSelectAll">
        全选
      </label>
      <BaseButton
        variant="danger"
        size="sm"
        :disabled="!selectedRecords.size"
        @click="isDeleteConfirmVisible = true"
      >
        删除选中 ({{ selectedRecords.size }})
      </BaseButton>
      <BaseButton variant="outline" size="sm" @click="toggleSelectMode">
        取消
      </BaseButton>
    </div>

    <ConfirmDialog
      v-model="isDeleteConfirmVisible"
      title="删除选中的记录？"
      :description="`将删除 ${selectedRecords.size} 条记录及其音频文件，该操作不可撤销。`"
      confirm-text="删除"
      @confirm="deleteSelected"
    />
  </div>
</template>

<script setup>
import { computed, ref } from 'vue'
import { useRecords } from '@/composables/useRecords'
import { useAppStore } from '@/stores/app'
import RecordCard from './RecordCard.vue'
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import EmptyState from '@/components/common/EmptyState.vue'

const appStore = useAppStore()
const {
  records,
  selectMode,
  selectedRecords,
  deleteSelected,
  copyText,
  toggleFavorite,
  optimizeRecord,
  exportRecord,
  toggleSelectMode,
  toggleSelection,
  selectAll,
  clearSelection,
  toggleAudio,
  isPlaying
} = useRecords()

const isDeleteConfirmVisible = ref(false)

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
