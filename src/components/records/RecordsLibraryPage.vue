<template>
  <div class="ui-page records-library-page">
    <PageHeader crumb="首页 / 记录集" title="记录集" />

    <!-- 用量统计：今天 / 近 7 天一行，点开看按天的柱状图 -->
    <RecordsUsageStats />

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

    <!-- 第二行：日期 / 状态筛选与「导出当前结果」（导出全部筛选结果，不受分页限制） -->
    <div class="ui-toolbar records-filters">
      <BaseSelect v-model="dateRange" class="records-filter" aria-label="按日期筛选">
        <option v-for="option in DATE_RANGE_OPTIONS" :key="option.value" :value="option.value">{{ option.label }}</option>
      </BaseSelect>
      <template v-if="dateRange === 'custom'">
        <input v-model="customFrom" class="records-date" type="date" aria-label="开始日期">
        <span class="ui-meta">至</span>
        <input v-model="customTo" class="records-date" type="date" aria-label="结束日期">
      </template>
      <BaseSelect v-model="statusFilter" class="records-filter" aria-label="按状态筛选">
        <option v-for="option in STATUS_FILTER_OPTIONS" :key="option.value" :value="option.value">{{ option.label }}</option>
      </BaseSelect>
      <button v-if="hasExtraFilters" class="ui-tbtn" type="button" @click="clearExtraFilters">清空筛选</button>
      <span class="records-filters-spacer"></span>
      <ExportFormatMenu label="导出当前结果" :disabled="!total || exporting" @select="exportCurrent" />
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
              :asr-engines="asrEngines"
              @toggle="toggleExpanded(record.id)"
              @select="toggleSelected(record.id)"
              @edit="openRecord(record)"
              @copy="copyText(record)"
              @export="(format) => exportRecord(record, format)"
              @delete="pendingDelete = [record.id]"
              @toggle-audio="toggleAudio(record.id)"
              @optimize="optimizeRecord(record.id)"
              @cancel-optimize="cancelOptimize(record.id)"
              @toggle-favorite="handleToggleFavorite(record)"
              @retranscribe="(provider) => requestRetranscribe(record, provider)"
            />
          </template>
        </template>

        <div v-if="totalPages > 1" class="records-pagination">
          <button class="ui-tbtn" type="button" :disabled="currentPage <= 1 || isLoading" @click="goToPage(currentPage - 1)">上一页</button>
          <span class="ui-meta ui-mono">{{ currentPage }} / {{ totalPages }}</span>
          <button class="ui-tbtn" type="button" :disabled="currentPage >= totalPages || isLoading" @click="goToPage(currentPage + 1)">下一页</button>
        </div>
      </div>

    </section>

    <!-- 选择模式：底部一条操作栏。放在卡片外面：.settings-section 的 overflow: hidden 会让 sticky 失效 -->
    <div v-if="selectMode" class="records-select-bar">
      <span class="ui-meta">已选 {{ selectedIds.size }} 条</span>
      <button class="ui-tbtn" type="button" @click="toggleSelectAll">{{ isAllSelected ? '取消全选' : '全选本页' }}</button>
      <ExportFormatMenu
        :label="`导出选中（${selectedIds.size}）`"
        placement="up"
        :disabled="!selectedIds.size || exporting"
        @select="exportSelected"
      />
      <button
        class="ui-tbtn is-danger"
        type="button"
        :disabled="!selectedIds.size"
        @click="pendingDelete = [...selectedIds]"
      >
        <span class="ui-tbtn-glyph">{{ G.delete }}</span>删除选中（{{ selectedIds.size }}）
      </button>
    </div>

    <ConfirmDialog
      :model-value="pendingDelete.length > 0"
      :title="pendingDelete.length > 1 ? `删除选中的 ${pendingDelete.length} 条记录？` : '删除这条记录？'"
      description="将同时删除音频文件，该操作不可撤销。"
      confirm-text="删除"
      @update:model-value="(open) => { if (!open) pendingDelete = [] }"
      @confirm="confirmDelete"
    />

    <ConfirmDialog
      :model-value="Boolean(pendingRetranscribe)"
      title="重新识别这条记录？"
      :description="RETRANSCRIBE_CONFIRM"
      confirm-text="重新识别"
      tone="default"
      @update:model-value="(open) => { if (!open) pendingRetranscribe = null }"
      @confirm="confirmRetranscribe"
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
import ExportFormatMenu from './ExportFormatMenu.vue'
import RecordsUsageStats from './RecordsUsageStats.vue'
import { DATE_RANGE_OPTIONS, STATUS_FILTER_OPTIONS, dateRangeFor, groupRecordsByDay } from './recordListModel.mjs'
import { buildBatchExportFilename, downloadFromUrl } from '@/utils/exportDownload.mjs'
import { RETRANSCRIBE_CONFIRM, needsRetranscribeConfirm } from './retranscribeModel.mjs'
import { createLatestRequestGuard, runLatest } from '@/utils/latestRequest.mjs'

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
  isPlaying,
  asrEngines,
  loadAsrEngines,
  retranscribeRecord
} = useRecords()

const recordSearch = ref('')
const recordSearchType = ref('all')
const favoriteOnly = ref(false)
// 日期（预设 / 自定义）与状态筛选
const dateRange = ref('all')
const customFrom = ref('')
const customTo = ref('')
const statusFilter = ref('')
const exporting = ref(false)
const hasExtraFilters = computed(() => dateRange.value !== 'all' || Boolean(statusFilter.value))

function clearExtraFilters() {
  dateRange.value = 'all'
  customFrom.value = ''
  customTo.value = ''
  statusFilter.value = ''
}

/** 当前筛选条件（与 /records/search 同名参数；导出当前结果也用它） */
function currentFilter() {
  return {
    q: recordSearch.value,
    search_type: recordSearchType.value,
    favorite_only: favoriteOnly.value,
    status: statusFilter.value,
    ...dateRangeFor(dateRange.value, new Date(), { from: customFrom.value, to: customTo.value })
  }
}

async function runExport(options, count) {
  exporting.value = true
  try {
    const url = await api.getRecordsExportUrl(options)
    downloadFromUrl(url, buildBatchExportFilename(options.format, count))
    appStore.showToast(`已导出 ${count} 条记录（${options.format.toUpperCase()}）`, { tone: 'success' })
  } catch (error) {
    console.error('Failed to export records:', error)
    appStore.showToast('导出失败', { tone: 'error' })
  } finally {
    exporting.value = false
  }
}

function exportCurrent(format) {
  void runExport({ filter: currentFilter(), format }, total.value)
}

function exportSelected(format) {
  const ids = [...selectedIds.value]
  if (ids.length) {
    void runExport({ ids, format }, ids.length)
  }
}
const records = ref([])
const isLoading = ref(false)
const currentPage = ref(1)
const total = ref(0)
const totalPages = ref(1)
const pageSize = 100
const recordsRequest = createLatestRequestGuard()

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
  const outcome = await runLatest(recordsRequest, () => api.searchRecords({
    ...currentFilter(),
    page: currentPage.value,
    page_size: pageSize
  }), {
    onStart: () => {
      isLoading.value = true
    },
    onResult: (result) => {
      records.value = Array.isArray(result?.items) ? result.items.map(mergeRecordWithStore) : []
      total.value = result?.total || 0
      totalPages.value = result?.total_pages || 1
    },
    onError: (error) => {
      console.error('Failed to search records:', error)
      appStore.showToast('加载记录失败', { tone: 'error' })
    },
    onFinally: () => {
      isLoading.value = false
    }
  })

  // 当前页被删空后回到最后一页；新请求会接管 loading，旧请求不会复位它
  if (outcome.applied && currentPage.value > totalPages.value) {
    currentPage.value = totalPages.value
    return loadRecordsPage()
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

function toggleExpanded(id) {
  expandedId.value = expandedId.value === id ? null : id
  if (expandedId.value) {
    // 展开后才用得上「重新识别」的引擎列表
    loadAsrEngines()
  }
}

// ---- 重新识别：会覆盖已有文字的先确认 ----
const pendingRetranscribe = ref(null)

function requestRetranscribe(record, provider) {
  if (needsRetranscribeConfirm(record)) {
    pendingRetranscribe.value = { record, provider }
    return
  }
  runRetranscribe(record, provider)
}

function confirmRetranscribe() {
  const pending = pendingRetranscribe.value
  pendingRetranscribe.value = null
  if (pending) {
    runRetranscribe(pending.record, pending.provider)
  }
}

function runRetranscribe(record, provider) {
  // 记录集是服务端分页：不在 store 最近列表里的记录，结果直接回填到本页
  retranscribeRecord(record, provider, {
    onUpdate: (updated) => {
      records.value = records.value.map((item) => (item.id === updated.id ? { ...item, ...updated } : item))
    }
  })
}
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

watch([recordSearch, recordSearchType, favoriteOnly, dateRange, customFrom, customTo, statusFilter], () => {
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

.records-filters {
  flex-wrap: wrap;
}

.records-filter {
  width: 128px;
  flex: none;
}

.records-date {
  height: 36px;
  padding: 0 var(--s-2);
  border: 1px solid var(--line);
  border-radius: var(--r-sm);
  background: var(--bg-raise);
  color: var(--text-1);
  font: inherit;
  font-size: var(--fs-sm);
  color-scheme: dark;
}

.records-filters-spacer {
  flex: 1;
}

.records-select-bar {
  position: sticky;
  bottom: 0;
  z-index: 2;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: var(--s-2);
  margin-top: var(--s-3);
  padding: var(--s-3) var(--s-4);
  border: 1px solid var(--line);
  border-radius: var(--r-md, var(--r-sm));
  background: var(--bg-surface);
  box-shadow: 0 -8px 24px rgba(0, 0, 0, 0.25);
}

.records-select-bar .ui-meta {
  margin-right: auto;
}
</style>
