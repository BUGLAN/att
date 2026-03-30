<template>
  <div class="overlay-shell">
    <section class="overlay-panel">
      <header class="overlay-header">
        <div class="overlay-title-group">
          <span class="overlay-title-icon">O</span>
          <h1 class="overlay-title">最近记录</h1>
        </div>
        <div class="overlay-header-actions">
          <button
            class="overlay-icon-button"
            :class="{ active: pinned }"
            type="button"
            :title="pinned ? '取消钉住' : '钉住悬浮窗'"
            @click="emit('pin', !pinned)"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path
                d="M8.5 4.5h7l-1 5 3 3v1h-4v5l-1.5 1.5-1.5-1.5v-5h-4v-1l3-3-1-5Z"
                fill="currentColor"
              />
            </svg>
          </button>
          <button class="overlay-icon-button" type="button" title="关闭" @click="emit('close')">
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path
                d="M6.53 5.47a.75.75 0 0 1 1.06 0L12 9.88l4.41-4.41a.75.75 0 1 1 1.06 1.06L13.06 11l4.41 4.41a.75.75 0 0 1-1.06 1.06L12 12.06l-4.41 4.41a.75.75 0 1 1-1.06-1.06L10.94 11 6.53 6.59a.75.75 0 0 1 0-1.12Z"
                fill="currentColor"
              />
            </svg>
          </button>
        </div>
      </header>

      <div class="overlay-search">
        <input
          ref="searchInput"
          v-model="query"
          class="overlay-search-input"
          type="text"
          placeholder="查找剪贴板记录..."
        >
      </div>

      <main class="overlay-list" role="listbox" :aria-activedescendant="activeItemId">
        <div v-if="isLoading" class="overlay-empty">正在加载...</div>
        <div v-else-if="!items.length" class="overlay-empty">暂无可用记录</div>
        <div
          v-for="(item, index) in items"
          v-else
          :id="`clipboard-item-${item.id}`"
          :key="item.id"
          class="overlay-item"
          :class="{ active: hasKeyboardSelection && index === selectedIndex }"
          role="option"
          tabindex="-1"
          @click="handleItemClick(item)"
        >
          <span class="overlay-item-icon">{{ iconFor(item.item_type) }}</span>
          <span class="overlay-item-content">
            <span class="overlay-item-title">{{ item.title || '未命名记录' }}</span>
            <span class="overlay-item-meta">
              <span>{{ relativeTime(item.updated_at) }}</span>
              <span v-if="item.item_type">{{ typeLabel(item.item_type) }}</span>
            </span>
          </span>
        </div>
      </main>

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
import '@/styles/clipboard-overlay.css'

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
const items = ref([])
const isLoading = ref(false)
const selectedIndex = ref(0)
const hasKeyboardSelection = ref(false)
const searchInput = ref(null)
const settings = ref({
  clipboard_overlay_enter_behavior: 'copy_and_paste',
  clipboard_overlay_auto_focus_search: true,
  clipboard_overlay_close_on_click_copy: false
})

let searchTimer = null
const activeItem = computed(() => items.value[selectedIndex.value] || null)
const activeItemId = computed(() => activeItem.value ? `clipboard-item-${activeItem.value.id}` : undefined)
const footerHint = computed(() => (
  settings.value.clipboard_overlay_enter_behavior === 'copy_only'
    ? 'Enter 复制 | Esc 关闭'
    : 'Enter 复制并回填 | Esc 关闭'
))

function typeLabel(type) {
  const labels = {
    text: '文本',
    link: '链接',
    code: '代码',
    file: '文件',
    image: '图片'
  }
  return labels[type] || '记录'
}

function iconFor(type) {
  const icons = {
    text: 'T',
    link: 'L',
    code: '</>',
    file: 'F',
    image: 'I'
  }
  return icons[type] || '.'
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
    searchInput.value?.focus()
    searchInput.value?.select()
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
    items.value = await api.getClipboardItems({
      query: query.value.trim(),
      limit: 30
    })
    selectedIndex.value = Math.min(selectedIndex.value, Math.max(items.value.length - 1, 0))
    hasKeyboardSelection.value = false
  } catch (error) {
    console.error('[ClipboardPanel] Failed to load items:', error)
    items.value = []
    selectedIndex.value = 0
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

    if (settings.value.clipboard_overlay_close_on_click_copy) {
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

watch(query, () => {
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

  window.removeEventListener('keydown', handleKeydown, true)
  removeAllListeners('clipboard:changed')
})
</script>
