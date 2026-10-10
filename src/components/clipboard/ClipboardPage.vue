<template>
  <section class="clipboard-page">
    <PageHeader
      title="剪切板"
      subtitle="自动记录复制的文本与图片，支持收藏、置顶、筛选与悬停预览。"
    />

    <section class="clipboard-toolbar">
      <div class="clipboard-search">
        <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <circle cx="11" cy="11" r="6" />
          <path d="m16 16 4 4" />
        </svg>
        <BaseInput
          v-model.trim="query"
          bare
          type="search"
          placeholder="搜索剪切板记录..."
        />
      </div>

      <div class="clipboard-actions">
        <BaseButton variant="outline" size="sm" :disabled="isLoading" @click="refreshCurrentView">
          刷新
        </BaseButton>
      </div>
    </section>

    <section class="clipboard-summary-row">
      <BaseButton
        size="sm"
        pill
        :active="activeQuickFilter === 'all'"
        @click="resetAllFilters"
      >
        今天 {{ stats.today }} 条记录
      </BaseButton>
      <BaseButton
        size="sm"
        pill
        :active="activeQuickFilter === 'favorite'"
        @click="setQuickFilter('favorite')"
      >
        收藏 {{ stats.favorites }}
      </BaseButton>
      <BaseButton
        size="sm"
        pill
        :active="activeQuickFilter === 'pinned'"
        @click="setQuickFilter('pinned')"
      >
        置顶 {{ stats.pinned }}
      </BaseButton>
      <BaseButton
        v-for="filter in visibleTypeFilters"
        :key="filter.value"
        size="sm"
        pill
        :active="activeQuickFilter === filter.value"
        @click="setQuickFilter(filter.value)"
      >
        {{ filter.label }}
      </BaseButton>
    </section>

    <section v-if="isLoading" class="clipboard-loading">
      <div v-for="index in 6" :key="index" class="clipboard-loading-card">
        <Skeleton :lines="3" inline />
      </div>
    </section>

    <EmptyState
      v-else-if="!items.length"
      size="lg"
      title="还没有剪切板记录"
      description="当前还没有可展示的剪切板历史记录，等待系统监听产生新内容。"
    />

    <section v-else class="clipboard-grid">
      <ClipboardCard
        v-for="item in items"
        :key="item.id"
        :item="item"
        :item-action-state="getItemActionState(item.id)"
        @copy="handleCopy"
        @delete="requestDelete"
        @toggle-favorite="handleToggleFavorite"
        @toggle-pin="handleTogglePin"
      />
    </section>

    <ConfirmDialog
      v-model="isDeleteConfirmVisible"
      title="删除这条剪切板记录？"
      description="删除后无法恢复，后续仍可通过再次复制重新产生记录。"
      confirm-text="删除"
      @confirm="confirmDelete"
    />
  </section>
</template>

<script setup>
import { onMounted, onUnmounted, ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useClipboard } from '@/composables/useClipboard'
import { useElectron } from '@/composables/useElectron'
import ClipboardCard from '@/components/clipboard/ClipboardCard.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseInput from '@/components/common/BaseInput.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import EmptyState from '@/components/common/EmptyState.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import Skeleton from '@/components/common/Skeleton.vue'
import { PAGE_CLIPBOARD_FILTERS } from '@/utils/clipboardPresentation'
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
  copyItemContent
} = useClipboard()

const query = ref('')
const activeQuickFilter = ref('all')
const filters = PAGE_CLIPBOARD_FILTERS
const itemActionState = ref({})
const visibleTypeFilters = filters.filter((filter) => filter.value !== 'all')
const pendingDeleteItem = ref(null)
const isDeleteConfirmVisible = ref(false)

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

function resetAllFilters() {
  activeQuickFilter.value = 'all'
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
    appStore.showToast('已删除剪切板记录', { tone: 'success' })
    await refreshCurrentView()
  } catch (error) {
    console.error('[Clipboard] Failed to delete item:', error)
    appStore.showToast('删除剪切板记录失败', { tone: 'error' })
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
/* 页面级 padding 归零：外层 .container 已经提供统一留白，避免内容套内容 */
.clipboard-page {
  width: 100%;
  max-width: 100%;
}

.clipboard-toolbar {
  display: grid;
  grid-template-columns: minmax(240px, 1fr) auto;
  gap: var(--s-4);
  align-items: center;
  margin-bottom: var(--s-4);
}

/* 搜索框：外层容器负责底色与 focus 环，内部 BaseInput 用 bare 形态 */
.clipboard-search {
  position: relative;
  display: flex;
  align-items: center;
  height: 44px;
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  background: var(--bg-raise);
  transition: border-color var(--t-fast) var(--ease), box-shadow var(--t-fast) var(--ease);
}

.clipboard-search:focus-within {
  border-color: var(--accent);
  box-shadow: var(--focus-halo);
}

.clipboard-search svg {
  position: absolute;
  left: var(--s-4);
  width: 18px;
  height: 18px;
  stroke: var(--text-3);
  stroke-width: 1.8;
  stroke-linecap: round;
  stroke-linejoin: round;
  pointer-events: none;
}

.clipboard-search :deep(.ui-input) {
  padding-left: calc(var(--s-8) + var(--s-2));
  padding-right: var(--s-4);
  font-size: var(--fs-base);
}

.clipboard-actions,
.clipboard-summary-row {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  flex-wrap: wrap;
}

.clipboard-summary-row {
  margin-bottom: var(--s-6);
  row-gap: var(--s-3);
}

.clipboard-loading,
.clipboard-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
  gap: var(--s-4);
}

.clipboard-loading-card {
  padding: var(--s-4);
  border: 1px solid var(--line);
  border-radius: var(--r-lg);
  background: var(--bg-card);
}

@media (max-width: 1080px) {
  .clipboard-toolbar {
    grid-template-columns: 1fr;
  }
}
</style>
