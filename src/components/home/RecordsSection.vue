<template>
  <!-- 首页「最近」：只放最新几条，完整列表、搜索与批量操作在记录集 -->
  <section class="settings-section">
    <div class="settings-group">
      <h3 class="settings-group-title">
        最近
        <button
          v-if="records.length"
          class="ui-tbtn is-gold ui-title-aside"
          type="button"
          @click="appStore.setCurrentPage('records')"
        >
          查看全部
        </button>
      </h3>

      <RecordRow
        v-for="record in recentRecords"
        :key="record.id"
        :record="record"
        :expanded="expandedId === record.id"
        :is-playing="isPlaying(record.id)"
        @toggle="expandedId = expandedId === record.id ? null : record.id"
        @edit="openEditModal(record)"
        @copy="copyText(record)"
        @export="(format) => exportRecord(record, format)"
        @delete="pendingDelete = record"
        @toggle-audio="toggleAudio(record.id)"
        @optimize="optimizeRecord(record.id)"
        @cancel-optimize="cancelOptimize(record.id)"
        @toggle-favorite="toggleFavorite(record)"
      />

      <div v-if="!records.length" class="ui-empty">
        <span class="ui-glyph-tile ui-glyph-tile--lg">{{ NAV_GLYPHS.home }}</span>
        完成一次录音后，结果会出现在这里
      </div>
    </div>
  </section>

  <ConfirmDialog
    :model-value="Boolean(pendingDelete)"
    title="删除这条记录？"
    description="将同时删除它的音频文件，该操作不可撤销。"
    confirm-text="删除"
    @update:model-value="(open) => { if (!open) pendingDelete = null }"
    @confirm="confirmDelete"
  />
</template>

<script setup>
import { computed, ref } from 'vue'
import { useRecords } from '@/composables/useRecords'
import { useAppStore } from '@/stores/app'
import { NAV_GLYPHS } from '@/components/common/glyphs.mjs'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import RecordRow from '@/components/records/RecordRow.vue'

/** 首页只展示最新的几条 */
const RECENT_LIMIT = 6

const appStore = useAppStore()
const {
  records,
  deleteRecord,
  copyText,
  toggleFavorite,
  optimizeRecord,
  cancelOptimize,
  exportRecord,
  toggleAudio,
  isPlaying
} = useRecords()

const expandedId = ref(null)
const pendingDelete = ref(null)
const recentRecords = computed(() => records.value.slice(0, RECENT_LIMIT))

function openEditModal(record) {
  appStore.setCurrentRecord(record)
  appStore.openModal('recordEdit')
}

async function confirmDelete() {
  const record = pendingDelete.value
  pendingDelete.value = null
  if (record) {
    await deleteRecord(record.id).catch(() => {})
  }
}
</script>
