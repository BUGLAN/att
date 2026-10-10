<template>
  <div class="overlay-shell">
    <section ref="panelRef" class="overlay-panel">
      <header class="overlay-header">
        <div class="overlay-title-group">
          <span class="overlay-title-icon">O</span>
          <h1 class="overlay-title">最近记录</h1>
        </div>
        <div class="overlay-header-actions">
          <BaseButton
            variant="ghost"
            icon
            size="sm"
            :active="pinned"
            :title="pinned ? '取消钉住' : '钉住悬浮窗'"
            @click="emit('pin', !pinned)"
          >
            <template #icon>
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path
                  d="M8.5 4.5h7l-1 5 3 3v1h-4v5l-1.5 1.5-1.5-1.5v-5h-4v-1l3-3-1-5Z"
                  fill="currentColor"
                />
              </svg>
            </template>
          </BaseButton>
          <BaseButton variant="ghost" icon size="sm" title="关闭" @click="emit('close')">
            <template #icon>
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path
                  d="M6.53 5.47a.75.75 0 0 1 1.06 0L12 9.88l4.41-4.41a.75.75 0 1 1 1.06 1.06L13.06 11l4.41 4.41a.75.75 0 0 1-1.06 1.06L12 12.06l-4.41 4.41a.75.75 0 1 1-1.06-1.06L10.94 11 6.53 6.59a.75.75 0 0 1 0-1.12Z"
                  fill="currentColor"
                />
              </svg>
            </template>
          </BaseButton>
        </div>
      </header>

      <div class="overlay-filter-row">
        <BaseButton
          v-for="filter in OVERLAY_CLIPBOARD_FILTERS"
          :key="filter.value"
          size="sm"
          pill
          :active="selectedFilter === filter.value"
          @click="selectedFilter = filter.value"
        >
          {{ filter.label }}
        </BaseButton>
      </div>

      <div class="overlay-search">
        <BaseInput
          v-model="query"
          type="text"
          placeholder="查找剪贴板记录..."
        />
      </div>

      <main class="overlay-list" role="listbox" :aria-activedescendant="activeItemId">
        <div v-if="isLoading" class="overlay-loading">
          <Skeleton :lines="3" inline />
        </div>
        <EmptyState
          v-else-if="!items.length"
          class="overlay-empty-state"
          size="sm"
          title="暂无可用记录"
          description="复制任意文本或图片后会自动出现在这里。"
        />
        <div
          v-for="(item, index) in items"
          v-else
          :id="`clipboard-item-${item.id}`"
          :key="item.id"
          class="overlay-item"
          :class="{ active: hasKeyboardSelection && index === selectedIndex }"
          role="option"
          tabindex="-1"
          @contextmenu.prevent="handleFavoriteToggle(item)"
          @click="handleItemClick(item)"
          @mouseenter="handleItemMouseEnter(item, $event)"
          @mouseleave="clearHoveredPreview"
        >
          <span class="overlay-item-icon">{{ iconFor(item.item_type) }}</span>
          <span class="overlay-item-content">
            <span class="overlay-item-title">{{ primaryText(item) }}</span>
            <span class="overlay-item-meta">
              <span>{{ relativeTime(item.updated_at) }}</span>
              <span v-if="item.item_type">{{ typeLabel(item.item_type) }}</span>
            </span>
          </span>
          <BaseButton
            variant="ghost"
            icon
            size="sm"
            :active="item.is_favorite"
            :title="item.is_favorite ? '取消收藏' : '收藏'"
            @click.stop="handleFavoriteToggle(item)"
          >
            <template #icon>
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path
                  d="m12 3.75 2.55 5.17 5.7.83-4.13 4.03.98 5.67L12 16.78 6.9 19.45l.97-5.67-4.12-4.03 5.7-.83L12 3.75Z"
                  fill="currentColor"
                />
              </svg>
            </template>
          </BaseButton>
        </div>
      </main>

      <div
        v-if="hoveredPreview"
        class="overlay-image-preview"
        :style="hoveredPreviewStyle"
      >
        <div class="overlay-image-preview-header">{{ displayTitle(hoveredPreview) }}</div>
        <div v-if="previewUnavailable" class="overlay-image-preview-empty">图片不可用</div>
        <img
          v-else
          class="overlay-image-preview-image"
          :src="hoveredPreview.preview_url"
          :alt="displayTitle(hoveredPreview)"
          @error="markPreviewFailed(hoveredPreview)"
        >
        <div class="overlay-image-preview-meta">{{ getClipboardSummaryText(hoveredPreview) || '图片条目' }}</div>
      </div>

      <footer class="overlay-footer">
        <span class="overlay-status">{{ pinned ? 'PINNED' : 'LIVE' }}</span>
        <span class="overlay-hint">{{ footerHint }}</span>
      </footer>
    </section>
  </div>
</template>

<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { api } from '@/api/client'
import { useElectron } from '@/composables/useElectron'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseInput from '@/components/common/BaseInput.vue'
import EmptyState from '@/components/common/EmptyState.vue'
import Skeleton from '@/components/common/Skeleton.vue'
import '@/styles/clipboard-overlay.css'
import {
  OVERLAY_CLIPBOARD_FILTERS,
  buildClipboardQueryParams,
  enrichClipboardItems,
  getClipboardDisplayTitle,
  getClipboardSummaryText,
  getClipboardTypeIcon,
  getClipboardTypeLabel,
  isClipboardImageItem,
  revokeClipboardPreviewUrls
} from '@/utils/clipboardPresentation'

const props = defineProps({
  activation: {
    type: Object,
    default: () => ({})
  },
  pinned: {
    type: Boolean,
    default: false
  }
})

const emit = defineEmits(['close', 'pin'])

const { clipboard, onClipboardChanged, removeAllListeners } = useElectron()

const query = ref('')
const selectedFilter = ref('all')
const items = ref([])
const isLoading = ref(false)
const selectedIndex = ref(0)
const hasKeyboardSelection = ref(false)
const panelRef = ref(null)
const hoveredPreview = ref(null)
const hoveredPreviewStyle = ref({ top: '120px' })
const failedPreviewIds = ref({})
const settings = ref({
  clipboard_overlay_enter_behavior: 'copy_and_paste',
  clipboard_overlay_auto_focus_search: true,
  clipboard_overlay_close_on_click_copy: false
})

let searchTimer = null
const activeItem = computed(() => items.value[selectedIndex.value] || null)
const activeItemId = computed(() => activeItem.value ? `clipboard-item-${activeItem.value.id}` : undefined)
const footerHint = computed(() => {
  // 钉住时点击不再关闭浮层，提示语需要同步，避免误导（docs/todo.md 第 ② 条）
  if (props.pinned) {
    return '已钉住：点击只复制不关闭 | Enter 复制并回填 | Esc 关闭'
  }

  return settings.value.clipboard_overlay_enter_behavior === 'copy_only'
    ? 'Enter 复制 | Esc 关闭'
    : 'Enter 复制并回填 | Esc 关闭'
})
const previewUnavailable = computed(() => {
  if (!hoveredPreview.value) {
    return false
  }

  return failedPreviewIds.value[hoveredPreview.value.id] === true
})

function typeLabel(type) {
  return getClipboardTypeLabel(type)
}

function iconFor(type) {
  return getClipboardTypeIcon(type)
}

function displayTitle(item) {
  return getClipboardDisplayTitle(item)
}

function primaryText(item) {
  if (isClipboardImageItem(item)) {
    return displayTitle(item)
  }

  return getClipboardSummaryText(item) || '暂无摘要'
}

function relativeTime(isoString) {
  if (!isoString) {
    return '刚刚'
  }

  const diff = Date.now() - new Date(isoString).getTime()
  const minutes = Math.max(0, Math.floor(diff / 60000))

  if (minutes < 1) return '刚刚'
  if (minutes < 60) return `${minutes} 分钟前`

  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} 小时前`

  const days = Math.floor(hours / 24)
  return `${days} 天前`
}

function focusSearch() {
  nextTick(() => {
    // BaseInput 根节点即原生 input，这里直接用 DOM 查询，避免依赖组件实例 API
    const element = panelRef.value?.querySelector('.overlay-search input')
    element?.focus?.()
    element?.select?.()
  })
}

async function loadSettings() {
  const payload = await api.getSettings()
  settings.value = {
    clipboard_overlay_enter_behavior: payload.clipboard_overlay_enter_behavior || 'copy_and_paste',
    clipboard_overlay_auto_focus_search: payload.clipboard_overlay_auto_focus_search ?? true,
    clipboard_overlay_close_on_click_copy: payload.clipboard_overlay_close_on_click_copy ?? false
  }
}

async function loadItems() {
  isLoading.value = true

  try {
    revokeClipboardPreviewUrls(items.value)
    const result = await api.getClipboardItems(
      buildClipboardQueryParams(selectedFilter.value, query.value.trim(), 30)
    )
    items.value = await enrichClipboardItems(result)
    selectedIndex.value = Math.min(selectedIndex.value, Math.max(items.value.length - 1, 0))
    hasKeyboardSelection.value = false
    clearHoveredPreview()
  } catch (error) {
    console.error('[ClipboardPanel] Failed to load items:', error)
    items.value = []
    selectedIndex.value = 0
    clearHoveredPreview()
  } finally {
    isLoading.value = false
  }
}

async function refreshOverlay({ focusSearch: shouldFocusSearch = true } = {}) {
  await Promise.all([
    loadSettings(),
    loadItems()
  ])

  if (shouldFocusSearch && settings.value.clipboard_overlay_auto_focus_search) {
    focusSearch()
  }
}

async function copyItem(item) {
  if (!item) {
    return
  }

  if (item.item_type === 'image' && item.metadata?.image_path) {
    await clipboard.writeImage({
      imagePath: item.metadata.image_path,
      imageToken: item.content || ''
    })
    return
  }

  await clipboard.writeText(item.content || '')
}

async function applyItem(item) {
  if (!item) {
    return
  }

  try {
    await copyItem(item)

    if (settings.value.clipboard_overlay_enter_behavior === 'copy_and_paste') {
      emit('close')
      await new Promise((resolve) => setTimeout(resolve, 80))
      await clipboard.pasteActive()
      return
    }

    emit('close')
  } catch (error) {
    console.error('[ClipboardPanel] Failed to apply item:', error)
  }
}

async function handleItemClick(item) {
  hasKeyboardSelection.value = false

  try {
    await copyItem(item)

    // 钉住（pinned）时点击只复制、不关闭浮层，对应 docs/todo.md 第 ② 条
    if (settings.value.clipboard_overlay_close_on_click_copy && !props.pinned) {
      emit('close')
    }
  } catch (error) {
    console.error('[ClipboardPanel] Failed to copy item on click:', error)
  }
}

async function deleteActiveItem() {
  if (!activeItem.value) {
    return
  }

  try {
    await api.deleteClipboardItem(activeItem.value.id)
    await loadItems()
  } catch (error) {
    console.error('[ClipboardPanel] Failed to delete item:', error)
  }
}

async function handleFavoriteToggle(item) {
  if (!item?.id) {
    return
  }

  try {
    await api.updateClipboardItem(item.id, {
      is_favorite: !item.is_favorite
    })
    await loadItems()
  } catch (error) {
    console.error('[ClipboardPanel] Failed to toggle favorite item:', error)
  }
}

function handleItemMouseEnter(item, event) {
  if (!isClipboardImageItem(item) || !item.preview_url) {
    clearHoveredPreview()
    return
  }

  const panelRect = panelRef.value?.getBoundingClientRect?.()
  const itemRect = event.currentTarget?.getBoundingClientRect?.()
  if (!panelRect || !itemRect) {
    hoveredPreview.value = item
    hoveredPreviewStyle.value = { top: '120px' }
    return
  }

  const previewHeight = 242
  const minTop = 108
  const maxTop = Math.max(minTop, panelRect.height - previewHeight - 18)
  const nextTop = Math.min(
    Math.max(itemRect.top - panelRect.top - 12, minTop),
    maxTop
  )

  hoveredPreview.value = item
  hoveredPreviewStyle.value = {
    top: `${nextTop}px`
  }
}

function clearHoveredPreview() {
  hoveredPreview.value = null
}

function markPreviewFailed(item) {
  failedPreviewIds.value = {
    ...failedPreviewIds.value,
    [item.id]: true
  }
}

function handleKeydown(event) {
  if (event.key === 'Escape') {
    event.preventDefault()
    emit('close')
    return
  }

  if (event.key === 'ArrowDown') {
    event.preventDefault()
    hasKeyboardSelection.value = true
    selectedIndex.value = Math.min(selectedIndex.value + 1, Math.max(items.value.length - 1, 0))
    return
  }

  if (event.key === 'ArrowUp') {
    event.preventDefault()
    hasKeyboardSelection.value = true
    selectedIndex.value = Math.max(selectedIndex.value - 1, 0)
    return
  }

  if (event.key === 'Enter') {
    event.preventDefault()
    applyItem(activeItem.value)
    return
  }

  if (event.key === 'Delete') {
    event.preventDefault()
    deleteActiveItem()
  }
}

watch([query, selectedFilter], () => {
  if (searchTimer) {
    clearTimeout(searchTimer)
  }

  searchTimer = setTimeout(() => {
    loadItems()
  }, 120)
})

watch(() => props.activation.sequence, () => {
  if (props.activation.mode === 'clipboard') {
    refreshOverlay(props.activation)
  }
}, { immediate: true })

onMounted(() => {
  onClipboardChanged(() => {
    if (props.activation.mode === 'clipboard') {
      refreshOverlay({ focusSearch: false })
    }
  })

  window.addEventListener('keydown', handleKeydown, true)
})

onUnmounted(() => {
  if (searchTimer) {
    clearTimeout(searchTimer)
  }

  clearHoveredPreview()
  window.removeEventListener('keydown', handleKeydown, true)
  revokeClipboardPreviewUrls(items.value)
  removeAllListeners('clipboard:changed')
})
</script>

<style scoped>
/* 浮层列表内的骨架屏与空态：贴合浮层的紧凑留白 */
.overlay-loading {
  padding: 12px;
}

.overlay-list .overlay-empty-state {
  margin: 12px;
  border-color: var(--line-soft-strong);
  background: var(--line-soft);
}

/* 浮层里的图标按钮全部使用实心图标，需要覆盖原语的描边式 svg 规则 */
.overlay-panel :deep(.ui-btn__icon) svg {
  fill: currentColor;
  stroke: none;
}
</style>
