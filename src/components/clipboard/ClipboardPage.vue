<template>
  <section class="ui-page clipboard-page">
    <PageHeader crumb="首页 / 剪贴板" title="剪贴板" />

    <div class="ui-toolbar">
      <label class="ui-search">
        <span class="ui-search-glyph" aria-hidden="true">{{ G.search }}</span>
        <input v-model.trim="query" type="search" placeholder="搜索剪贴板" aria-label="搜索剪贴板">
      </label>
      <span class="ui-meta"><span class="ui-dot ok"></span>今天 {{ stats.today }} 条</span>
      <button class="ui-tbtn" type="button" :disabled="isLoading" @click="refreshCurrentView">
        <span class="ui-tbtn-glyph">{{ G.refresh }}</span>刷新
      </button>
      <button class="ui-tbtn" :class="{ 'is-gold': selectMode }" type="button" @click="toggleSelectMode">
        <span class="ui-tbtn-glyph">{{ G.select }}</span>{{ selectMode ? '退出选择' : '选择' }}
      </button>
      <button class="ui-tbtn is-danger" type="button" :disabled="!stats.total" @click="isClearConfirmVisible = true">
        <span class="ui-tbtn-glyph">{{ G.delete }}</span>清空历史
      </button>
    </div>

    <section class="settings-section clipboard-section">
      <div class="settings-group">
        <div class="ui-tabs clipboard-tabs" role="tablist" aria-label="筛选剪贴板">
          <button
            v-for="tab in tabs"
            :key="tab.value"
            type="button"
            role="tab"
            :aria-selected="activeQuickFilter === tab.value"
            :class="{ 'is-on': activeQuickFilter === tab.value }"
            @click="setQuickFilter(tab.value)"
          >
            {{ tab.label }}<span v-if="tab.count != null" class="ui-tabs-count">{{ tab.count }}</span>
          </button>
        </div>

        <div v-if="isLoading && !items.length" class="ui-empty">
          <span class="ui-meta"><span class="ui-dot blue pulse"></span>正在加载…</span>
        </div>
        <div v-else-if="!items.length" class="ui-empty">
          <span class="ui-glyph-tile ui-glyph-tile--lg">{{ NAV_GLYPHS.clipboard }}</span>
          {{ activeQuickFilter === 'all' && !query ? '还没有剪贴板记录，复制点什么试试' : '这个分类下还没有内容' }}
        </div>
        <template v-else>
          <ClipboardRow
            v-for="item in items"
            :key="item.id"
            :item="item"
            :item-action-state="getItemActionState(item.id)"
            :select-mode="selectMode"
            :selected="selectedIds.has(item.id)"
            @select="toggleSelected(item.id)"
            @copy="handleCopy"
            @delete="requestDelete"
            @toggle-favorite="handleToggleFavorite"
            @toggle-pin="handleTogglePin"
            @view-image="viewer = $event"
          />
        </template>
      </div>
    </section>

    <!-- 选择模式：底部一条操作栏。放在卡片外面：.settings-section 的 overflow: hidden 会让 sticky 失效 -->
    <div v-if="selectMode" class="clipboard-select-bar">
      <span class="ui-meta">已选 {{ selectedIds.size }} 条</span>
      <button class="ui-tbtn" type="button" :disabled="!items.length" @click="toggleSelectAll">
        {{ isAllSelected ? '取消全选' : '全选本页' }}
      </button>
      <button class="ui-tbtn is-danger" type="button" :disabled="!selectedIds.size" @click="isBulkConfirmVisible = true">
        <span class="ui-tbtn-glyph">{{ G.delete }}</span>删除选中（{{ selectedIds.size }}）
      </button>
    </div>

    <ImageViewer
      :src="viewer?.src || ''"
      :title="viewer?.title || ''"
      :meta="viewer?.meta || ''"
      copyable
      @copy="handleCopy(viewer.item)"
      @close="viewer = null"
    />

    <ConfirmDialog
      v-model="isDeleteConfirmVisible"
      title="删除这条剪贴板记录？"
      description="删除后无法恢复，再次复制会重新产生记录。"
      confirm-text="删除"
      @confirm="confirmDelete"
    />

    <ConfirmDialog
      v-model="isBulkConfirmVisible"
      :title="`删除选中的 ${selectedIds.size} 条剪贴板记录？`"
      description="图片会一并删除，删除后无法恢复。"
      confirm-text="删除"
      @confirm="confirmBulkDelete"
    />

    <ConfirmDialog
      v-model="isClearConfirmVisible"
      title="清空剪贴板历史？"
      description="置顶和收藏的记录会保留，其余记录（含图片）全部删除，无法恢复。"
      confirm-text="清空"
      @confirm="confirmClear"
    />
  </section>
</template>

<script setup>
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useClipboard } from '@/composables/useClipboard'
import { useElectron } from '@/composables/useElectron'
import ClipboardRow from '@/components/clipboard/ClipboardRow.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import ImageViewer from '@/components/common/ImageViewer.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import { ACTION_GLYPHS as G, NAV_GLYPHS } from '@/components/common/glyphs.mjs'
import { buildClipboardTogglePatch, patchClipboardItems } from '@/utils/clipboardCardActions.mjs'

const appStore = useAppStore()
const { onClipboardChanged, removeAllListeners } = useElectron()
const {
  items,
  stats,
  isLoading,
  refresh,
  refreshStats,
  toggleFavorite,
  togglePin,
  removeItem,
  removeItems,
  clearHistory,
  copyItemContent
} = useClipboard()

const query = ref('')
const activeQuickFilter = ref('all')
const itemActionState = ref({})
// 下划线标签：全部 / 收藏 / 置顶带数量，其后按类型筛选
const tabs = computed(() => [
  { value: 'all', label: '全部' },
  { value: 'favorite', label: '收藏', count: stats.value.favorites },
  { value: 'pinned', label: '置顶', count: stats.value.pinned },
  { value: 'text', label: '文本' },
  { value: 'code', label: '长文本' },
  { value: 'image', label: '图片' },
  { value: 'link', label: '链接' },
  { value: 'file', label: '文件' }
])
const pendingDeleteItem = ref(null)
const isDeleteConfirmVisible = ref(false)
const isBulkConfirmVisible = ref(false)
const isClearConfirmVisible = ref(false)

// ---- 选择模式 ----
const selectMode = ref(false)
const selectedIds = ref(new Set())
const isAllSelected = computed(() => items.value.length > 0 && items.value.every((item) => selectedIds.value.has(item.id)))

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
  selectedIds.value = isAllSelected.value ? new Set() : new Set(items.value.map((item) => item.id))
}

// 列表刷新（筛选、自动清理、别处删除）后，已经不在列表里的条目不再算选中
watch(items, (list) => {
  const visible = new Set(list.map((item) => item.id))
  const kept = [...selectedIds.value].filter((id) => visible.has(id))
  if (kept.length !== selectedIds.value.size) {
    selectedIds.value = new Set(kept)
  }
})

async function confirmBulkDelete() {
  const ids = [...selectedIds.value]
  if (!ids.length) {
    return
  }
  try {
    const deleted = await removeItems(ids)
    appStore.showToast(`已删除 ${deleted} 条剪贴板记录`, { tone: 'success' })
    selectedIds.value = new Set()
    selectMode.value = false
  } catch (error) {
    console.error('[Clipboard] Failed to delete items:', error)
    appStore.showToast('批量删除失败', { tone: 'error' })
  }
  await refreshCurrentView()
}

async function confirmClear() {
  try {
    const { deleted, kept } = await clearHistory()
    appStore.showToast(kept ? `已清空 ${deleted} 条，保留置顶与收藏 ${kept} 条` : `已清空 ${deleted} 条`, { tone: 'success' })
    selectedIds.value = new Set()
    selectMode.value = false
  } catch (error) {
    console.error('[Clipboard] Failed to clear history:', error)
    appStore.showToast('清空剪贴板历史失败', { tone: 'error' })
  }
  await refreshCurrentView()
}
// 看图器当前展示的图片：{ item, src, title, meta }，null 表示关闭
const viewer = ref(null)

function debounce(fn, delay) {
  let timer = null
  return function (...args) {
    if (timer) {
      clearTimeout(timer)
    }
    timer = setTimeout(() => fn.apply(this, args), delay)
  }
}

function buildFilters() {
  return {
    query: query.value,
    item_type: ['text', 'image', 'file', 'link', 'code'].includes(activeQuickFilter.value)
      ? activeQuickFilter.value
      : undefined,
    favorite_only: activeQuickFilter.value === 'favorite',
    pinned_only: activeQuickFilter.value === 'pinned',
    limit: 120
  }
}

function setQuickFilter(filterValue) {
  activeQuickFilter.value = filterValue
}

async function refreshCurrentView() {
  await refresh(buildFilters())
}

async function handleCopy(item) {
  await copyItemContent(item)
}

// 危险操作先走二次确认，确认后才真正删除
function requestDelete(item) {
  pendingDeleteItem.value = item
  isDeleteConfirmVisible.value = true
}

async function confirmDelete() {
  const item = pendingDeleteItem.value
  pendingDeleteItem.value = null

  if (!item) {
    return
  }

  try {
    await removeItem(item.id)
    appStore.showToast('已删除剪贴板记录', { tone: 'success' })
    await refreshCurrentView()
  } catch (error) {
    console.error('[Clipboard] Failed to delete item:', error)
    appStore.showToast('删除剪贴板记录失败', { tone: 'error' })
  }
}

function getItemActionState(itemId) {
  return itemActionState.value[itemId] || {
    favorite: false,
    pin: false
  }
}

function setItemActionState(itemId, actionKey, isBusy) {
  itemActionState.value = {
    ...itemActionState.value,
    [itemId]: {
      ...getItemActionState(itemId),
      [actionKey]: isBusy
    }
  }
}

async function handleToggleFavorite(item) {
  if (getItemActionState(item.id).favorite) {
    return
  }

  const previousItems = items.value
  items.value = patchClipboardItems(items.value, buildClipboardTogglePatch(item, 'is_favorite'))
  setItemActionState(item.id, 'favorite', true)

  try {
    const updatedItem = await toggleFavorite(item)
    items.value = patchClipboardItems(items.value, updatedItem)
    await refreshStats()
    appStore.showToast(updatedItem.is_favorite ? '已加入收藏' : '已取消收藏', { tone: 'success' })
  } catch (error) {
    items.value = previousItems
    console.error('[Clipboard] Failed to toggle favorite:', error)
    appStore.showToast('更新收藏状态失败', { tone: 'error' })
  } finally {
    setItemActionState(item.id, 'favorite', false)
  }
}

async function handleTogglePin(item) {
  if (getItemActionState(item.id).pin) {
    return
  }

  const previousItems = items.value
  items.value = patchClipboardItems(items.value, buildClipboardTogglePatch(item, 'is_pinned'))
  setItemActionState(item.id, 'pin', true)

  try {
    const updatedItem = await togglePin(item)
    items.value = patchClipboardItems(items.value, updatedItem)
    await refreshStats()
    appStore.showToast(updatedItem.is_pinned ? '已置顶该记录' : '已取消置顶', { tone: 'success' })
  } catch (error) {
    items.value = previousItems
    console.error('[Clipboard] Failed to toggle pin:', error)
    appStore.showToast('更新置顶状态失败', { tone: 'error' })
  } finally {
    setItemActionState(item.id, 'pin', false)
  }
}

const debouncedRefresh = debounce(refreshCurrentView, 250)

watch([query, activeQuickFilter], () => {
  debouncedRefresh()
})

onMounted(async () => {
  onClipboardChanged(() => {
    refreshCurrentView()
  })

  await refreshCurrentView()
})

onUnmounted(() => {
  removeAllListeners('clipboard:changed')
})
</script>

<style scoped>
.clipboard-tabs {
  margin-bottom: var(--s-4);
}

.clipboard-select-bar {
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

.clipboard-select-bar .ui-meta {
  margin-right: auto;
}
</style>
