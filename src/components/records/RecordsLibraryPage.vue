<template>
  <div class="ui-page records-library-page">
    <PageHeader crumb="首页 / 记录集" title="记录集" />

    <div class="ui-toolbar">
      <label class="ui-search">
        <span class="ui-search-glyph" aria-hidden="true">{{ G.search }}</span>
        <input v-model.trim="recordSearch" type="search" placeholder="搜索转写或优化文本" aria-label="搜索记录">
      </label>
      <BaseSelect v-model="recordSearchType" class="records-scope" aria-label="搜索范围">
        <option value="all">全部文本</option>
        <option value="optimized">仅优化文本</option>
        <option value="raw">仅原始文本</option>
      </BaseSelect>
      <button class="ui-tbtn" :class="{ 'is-gold': favoriteOnly }" type="button" @click="favoriteOnly = !favoriteOnly">
        <span class="ui-tbtn-glyph">{{ G.favorite }}</span>{{ favoriteOnly ? '仅收藏' : '收藏' }}
      </button>
      <button class="ui-tbtn" :class="{ 'is-gold': selectMode }" type="button" @click="toggleSelectMode">
        <span class="ui-tbtn-glyph">{{ G.select }}</span>{{ selectMode ? '退出选择' : '选择' }}
      </button>
    </div>

    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">
          {{ total }} 条记录
          <span class="ui-title-aside ui-meta">{{ selectMode ? `已选 ${selectedIds.size} 条` : '点开查看原始转写' }}</span>
        </h3>

        <div v-if="isLoading && !records.length" class="ui-empty">
          <span class="ui-meta"><span class="ui-dot blue pulse"></span>正在加载…</span>
        </div>
        <div v-else-if="!records.length" class="ui-empty">
          <span class="ui-glyph-tile ui-glyph-tile--lg">{{ NAV_GLYPHS.records }}</span>
          没有匹配的记录，换个关键词或清空筛选再试
        </div>
        <template v-else>
          <template v-for="group in groups" :key="group.label">
            <div class="ui-day">{{ group.label }}</div>
            <RecordRow
              v-for="record in group.records"
              :key="record.id"
              :record="record"
              :expanded="expandedId === record.id"
              :is-playing="isPlaying(record.id)"
              :select-mode="selectMode"
              :selected="selectedIds.has(record.id)"
              @toggle="expandedId = expandedId === record.id ? null : record.id"
              @select="toggleSelected(record.id)"
              @edit="openRecord(record)"
              @copy="copyText(record)"
              @export="(format) => exportRecord(record, format)"
              @delete="pendingDelete = [record.id]"
              @toggle-audio="toggleAudio(record.id)"
              @optimize="optimizeRecord(record.id)"
              @cancel-optimize="cancelOptimize(record.id)"
              @toggle-favorite="handleToggleFavorite(record)"
            />
          </template>
        </template>

        <div v-if="totalPages > 1" class="records-pagination">
          <button class="ui-tbtn" type="button" :disabled="currentPage <= 1 || isLoading" @click="goToPage(currentPage - 1)">上一页</button>
          <span class="ui-meta ui-mono">{{ currentPage }} / {{ totalPages }}</span>
          <button class="ui-tbtn" type="button" :disabled="currentPage >= totalPages || isLoading" @click="goToPage(currentPage + 1)">下一页</button>
        </div>
      </div>

      <!-- 选择模式：底部一条操作栏 -->
      <div v-if="selectMode" class="records-select-bar">
        <button class="ui-tbtn" type="button" @click="toggleSelectAll">{{ isAllSelected ? '取消全选' : '全选本页' }}</button>
        <button
          class="ui-tbtn is-danger"
          type="button"
          :disabled="!selectedIds.size"
          @click="pendingDelete = [...selectedIds]"
        >
          <span class="ui-tbtn-glyph">{{ G.delete }}</span>删除选中（{{ selectedIds.size }}）
        </button>
      </div>
    </section>

    <ConfirmDialog
      :model-value="pendingDelete.length > 0"
      :title="pendingDelete.length > 1 ? `删除选中的 ${pendingDelete.length} 条记录？` : '删除这条记录？'"
      description="将同时删除音频文件，该操作不可撤销。"
      confirm-text="删除"
      @update:model-value="(open) => { if (!open) pendingDelete = [] }"
      @confirm="confirmDelete"
    />
  </div>
</template>

<script setup>
import { computed, onMounted, ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useRecordsStore } from '@/stores/records'
import { useElectron } from '@/composables/useElectron'
import { useRecords } from '@/composables/useRecords'
import { ACTION_GLYPHS as G, NAV_GLYPHS } from '@/components/common/glyphs.mjs'
import BaseSelect from '@/components/common/BaseSelect.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import RecordRow from './RecordRow.vue'
import { groupRecordsByDay } from './recordListModel.mjs'

const appStore = useAppStore()
const recordsStore = useRecordsStore()
const { api } = useElectron()
// 复制 / 导出 / 优化 / 试听与首页共用同一套动作（含 toast 文案）
const {
  copyText,
  exportRecord,
  optimizeRecord,
  cancelOptimize,
  toggleAudio,
  isPlaying
} = useRecords()

const recordSearch = ref('')
const recordSearchType = ref('all')
const favoriteOnly = ref(false)
const records = ref([])
const isLoading = ref(false)
const currentPage = ref(1)
const total = ref(0)
const totalPages = ref(1)
const pageSize = 100

function debounce(fn, delay) {
  let timer = null
  return function(...args) {
    if (timer) {
      clearTimeout(timer)
    }
    timer = setTimeout(() => fn.apply(this, args), delay)
  }
}

function mergeRecordWithStore(record) {
  const existing = recordsStore.records.find((item) => item.id === record.id)
  return existing ? { ...record, ...existing } : record
}

async function loadRecordsPage() {
  isLoading.value = true

  try {
    const result = await api.searchRecords({
      q: recordSearch.value,
      search_type: recordSearchType.value,
      favorite_only: favoriteOnly.value,
      page: currentPage.value,
      page_size: pageSize
    })
    records.value = Array.isArray(result?.items) ? result.items.map(mergeRecordWithStore) : []
    total.value = result?.total || 0
    totalPages.value = result?.total_pages || 1

    if (currentPage.value > totalPages.value) {
      currentPage.value = totalPages.value
      return await loadRecordsPage()
    }
  } catch (error) {
    console.error('Failed to search records:', error)
    appStore.showToast('加载记录失败', { tone: 'error' })
  } finally {
    isLoading.value = false
  }
}

async function handleToggleFavorite(record) {
  try {
    const updated = await recordsStore.updateRecord(record.id, {
      is_favorite: !record.is_favorite
    })

    if (favoriteOnly.value && !updated.is_favorite) {
      records.value = records.value.filter((item) => item.id !== record.id)
    } else {
      records.value = records.value.map((item) => (
        item.id === record.id ? { ...item, ...updated } : item
      ))
    }

    if (appStore.currentRecord?.id === record.id) {
      appStore.setCurrentRecord({ ...appStore.currentRecord, ...updated })
    }

    appStore.showToast(updated.is_favorite ? '已收藏记录' : '已取消收藏', { tone: 'success' })
  } catch (error) {
    appStore.showToast('更新收藏状态失败', { tone: 'error' })
  }
}

// ---- 展开 / 选择 / 删除 ----
const expandedId = ref(null)
const selectMode = ref(false)
const selectedIds = ref(new Set())
const pendingDelete = ref([])

const groups = computed(() => groupRecordsByDay(records.value))
const isAllSelected = computed(() => (
  records.value.length > 0 && records.value.every((record) => selectedIds.value.has(record.id))
))

function toggleSelectMode() {
  selectMode.value = !selectMode.value
  selectedIds.value = new Set()
}

function toggleSelected(id) {
  const next = new Set(selectedIds.value)
  if (next.has(id)) {
    next.delete(id)
  } else {
    next.add(id)
  }
  selectedIds.value = next
}

function toggleSelectAll() {
  selectedIds.value = isAllSelected.value ? new Set() : new Set(records.value.map((record) => record.id))
}

async function confirmDelete() {
  const ids = pendingDelete.value
  pendingDelete.value = []
  if (!ids.length) {
    return
  }
  try {
    await recordsStore.deleteRecords(ids)
    appStore.showToast(ids.length > 1 ? `已删除 ${ids.length} 条记录` : '记录已删除', { tone: 'success' })
    selectedIds.value = new Set()
    if (selectMode.value && ids.length > 1) {
      selectMode.value = false
    }
  } catch (error) {
    console.error('Failed to delete records:', error)
    appStore.showToast('删除记录失败', { tone: 'error' })
  }
  await loadRecordsPage()
}

function openRecord(record) {
  appStore.setCurrentRecord(mergeRecordWithStore(record))
  appStore.openModal('recordEdit')
}

function goToPage(page) {
  currentPage.value = Math.max(1, Math.min(page, totalPages.value))
  loadRecordsPage()
}

const debouncedLoadRecords = debounce(loadRecordsPage, 300)

watch([recordSearch, recordSearchType, favoriteOnly], () => {
  currentPage.value = 1
  debouncedLoadRecords()
})

watch(() => recordsStore.records, () => {
  debouncedLoadRecords()
}, { deep: true })

onMounted(async () => {
  await loadRecordsPage()
})
</script>

<style scoped>
.records-scope {
  width: 140px;
  flex: none;
}

.records-pagination {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: var(--s-3);
  margin-top: var(--s-4);
}

.records-select-bar {
  position: sticky;
  bottom: 0;
  display: flex;
  justify-content: flex-end;
  gap: var(--s-2);
  padding: var(--s-3) var(--s-6);
  border-top: 1px solid var(--line);
  background: var(--bg-surface);
}
</style>
