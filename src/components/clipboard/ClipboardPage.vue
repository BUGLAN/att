<template>
  <section class="clipboard-page">
    <header class="clipboard-hero">
      <div class="clipboard-hero-copy">
        <span class="clipboard-kicker">DIGITAL OBSIDIAN</span>
        <h1 class="clipboard-title">剪切板历史</h1>
        <div class="clipboard-today-pill">今日记录 {{ stats.today }} 条</div>
        <p class="clipboard-subtitle">琥珀黑曜石风格的剪切板记录中心，支持文本、图片、文件与链接的历史整理与悬停预览。</p>
      </div>

      <div class="clipboard-hero-stats">
        <div class="hero-stat">
          <span class="hero-stat-label">今日记录</span>
          <strong>{{ stats.today }}</strong>
        </div>
        <div class="hero-stat">
          <span class="hero-stat-label">总条目</span>
          <strong>{{ stats.total }}</strong>
        </div>
      </div>
    </header>

    <section class="clipboard-toolbar">
      <div class="clipboard-search">
        <svg viewBox="0 0 24 24" fill="none">
          <circle cx="11" cy="11" r="6" />
          <path d="m16 16 4 4" />
        </svg>
        <input
          v-model.trim="query"
          type="search"
          placeholder="搜索剪切板记录..."
        >
      </div>

      <div class="clipboard-filters">
        <button
          v-for="filter in filters"
          :key="filter.value"
          class="filter-button"
          :class="{ active: selectedType === filter.value }"
          type="button"
          @click="selectedType = filter.value"
        >
          {{ filter.label }}
        </button>
      </div>

      <div class="clipboard-actions">
        <button class="ghost-button" type="button" :disabled="isLoading" @click="refreshCurrentView">
          刷新
        </button>
      </div>
    </section>

    <section class="clipboard-summary-row">
      <div class="summary-pill">今天 {{ stats.today }} 条记录</div>
      <div class="summary-pill muted">收藏 {{ stats.favorites }}</div>
      <div class="summary-pill muted">置顶 {{ stats.pinned }}</div>
    </section>

    <section v-if="isLoading" class="clipboard-empty">
      正在加载剪切板记录...
    </section>

    <section v-else-if="!items.length" class="clipboard-empty">
      <strong>还没有剪切板记录</strong>
      <p>当前还没有可展示的剪切板历史记录，等待系统监听产生新内容。</p>
    </section>

    <section v-else class="clipboard-grid">
      <ClipboardCard
        v-for="item in items"
        :key="item.id"
        :item="item"
        @copy="handleCopy"
        @delete="handleDelete"
        @toggle-favorite="handleToggleFavorite"
        @toggle-pin="handleTogglePin"
      />
    </section>
  </section>
</template>

<script setup>
import { onMounted, onUnmounted, ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useClipboard } from '@/composables/useClipboard'
import { useElectron } from '@/composables/useElectron'
import ClipboardCard from '@/components/clipboard/ClipboardCard.vue'
import { PAGE_CLIPBOARD_FILTERS } from '@/utils/clipboardPresentation'

const appStore = useAppStore()
const { onClipboardChanged, removeAllListeners } = useElectron()
const {
  items,
  stats,
  isLoading,
  refresh,
  toggleFavorite,
  togglePin,
  removeItem,
  copyItemContent
} = useClipboard()

const query = ref('')
const selectedType = ref('all')
const filters = PAGE_CLIPBOARD_FILTERS

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
    item_type: selectedType.value === 'all' ? undefined : selectedType.value,
    limit: 120
  }
}

async function refreshCurrentView() {
  await refresh(buildFilters())
}

async function handleCopy(item) {
  await copyItemContent(item)
}

async function handleDelete(item) {
  try {
    await removeItem(item.id)
    appStore.showToast('已删除剪切板记录')
    await refreshCurrentView()
  } catch (error) {
    console.error('[Clipboard] Failed to delete item:', error)
    appStore.showToast('删除剪切板记录失败')
  }
}

async function handleToggleFavorite(item) {
  try {
    await toggleFavorite(item)
    await refreshCurrentView()
    appStore.showToast(item.is_favorite ? '已取消收藏' : '已加入收藏')
  } catch (error) {
    console.error('[Clipboard] Failed to toggle favorite:', error)
    appStore.showToast('更新收藏状态失败')
  }
}

async function handleTogglePin(item) {
  try {
    await togglePin(item)
    await refreshCurrentView()
    appStore.showToast(item.is_pinned ? '已取消置顶' : '已置顶该记录')
  } catch (error) {
    console.error('[Clipboard] Failed to toggle pin:', error)
    appStore.showToast('更新置顶状态失败')
  }
}

const debouncedRefresh = debounce(refreshCurrentView, 250)

watch([query, selectedType], () => {
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
.clipboard-page {
  width: 100%;
  max-width: 1280px;
  padding: 36px 32px 48px;
}

.clipboard-hero {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 24px;
  align-items: end;
  padding: 28px 30px;
  margin-bottom: 24px;
  border-radius: 24px;
  background:
    radial-gradient(circle at top right, rgba(255, 197, 87, 0.14), transparent 30%),
    linear-gradient(180deg, rgba(28, 27, 27, 0.9), rgba(19, 19, 19, 0.98));
}

.clipboard-kicker {
  display: inline-flex;
  margin-bottom: 10px;
  color: var(--accent);
  font-family: 'Space Mono', var(--font-mono);
  font-size: 0.74rem;
  letter-spacing: 0.18em;
}

.clipboard-title {
  font-family: 'Instrument Sans', var(--font-display);
  font-size: clamp(2rem, 3.2vw, 3rem);
  line-height: 1.05;
  margin-bottom: 10px;
}

.clipboard-subtitle {
  max-width: 720px;
  color: var(--text-secondary);
  font-size: 0.98rem;
  line-height: 1.8;
}

.clipboard-today-pill {
  display: inline-flex;
  align-items: center;
  min-height: 32px;
  margin-bottom: 14px;
  padding: 0 14px;
  border-radius: 999px;
  background: rgba(255, 197, 87, 0.14);
  color: var(--accent);
  font-size: 0.8rem;
  font-weight: 700;
}

.clipboard-hero-stats {
  display: flex;
  gap: 14px;
  flex-wrap: wrap;
  justify-content: flex-end;
}

.hero-stat {
  min-width: 124px;
  padding: 18px 18px 16px;
  border-radius: 18px;
  background: rgba(42, 42, 42, 0.72);
  text-align: left;
}

.hero-stat-label {
  display: block;
  margin-bottom: 8px;
  color: var(--text-muted);
  font-size: 0.74rem;
  letter-spacing: 0.08em;
}

.hero-stat strong {
  font-size: 1.6rem;
  color: var(--text-primary);
}

.clipboard-toolbar {
  display: grid;
  grid-template-columns: minmax(240px, 1.1fr) minmax(280px, 1fr) auto;
  gap: 16px;
  align-items: center;
  margin-bottom: 18px;
}

.clipboard-search {
  position: relative;
}

.clipboard-search svg {
  position: absolute;
  left: 16px;
  top: 50%;
  width: 18px;
  height: 18px;
  stroke: var(--text-muted);
  stroke-width: 1.8;
  stroke-linecap: round;
  stroke-linejoin: round;
  transform: translateY(-50%);
}

.clipboard-search input {
  width: 100%;
  padding: 14px 18px 14px 48px;
  border: none;
  border-radius: 18px;
  background: rgba(14, 14, 14, 0.94);
  color: var(--text-primary);
  font-size: 0.92rem;
}

.clipboard-search input:focus {
  outline: 1px solid rgba(255, 197, 87, 0.35);
}

.clipboard-actions,
.clipboard-summary-row {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}

.clipboard-filters {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: nowrap;
}

.filter-button,
.ghost-button,
.primary-button {
  -webkit-appearance: none;
  appearance: none;
  border: none;
  cursor: pointer;
  transition: all var(--transition-fast);
  white-space: nowrap;
}

.filter-button,
.ghost-button {
  padding: 11px 14px;
  border-radius: 14px;
  background: rgba(42, 42, 42, 0.8);
  color: var(--text-secondary);
  font-size: 0.82rem;
}

.filter-button:hover,
.ghost-button:hover,
.filter-button.active,
.ghost-button.active {
  background: rgba(255, 197, 87, 0.12);
  color: var(--accent);
}

.primary-button {
  padding: 12px 18px;
  border-radius: 14px;
  background: linear-gradient(135deg, #ffc557, #e8a70f);
  color: #1b1300;
  font-weight: 700;
  font-size: 0.84rem;
}

.primary-button:hover:not(:disabled) {
  transform: translateY(-1px);
}

.primary-button:disabled,
.ghost-button:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.clipboard-summary-row {
  margin-bottom: 22px;
}

.summary-pill {
  display: inline-flex;
  align-items: center;
  min-height: 34px;
  padding: 0 14px;
  border-radius: 999px;
  background: rgba(255, 197, 87, 0.12);
  color: var(--accent);
  font-size: 0.78rem;
  font-weight: 600;
}

.summary-pill.muted {
  background: rgba(255, 255, 255, 0.04);
  color: var(--text-secondary);
}

.clipboard-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
  gap: 18px;
}

.clipboard-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  min-height: 300px;
  padding: 24px;
  border-radius: 24px;
  background: rgba(28, 27, 27, 0.84);
  color: var(--text-secondary);
  text-align: center;
}

.clipboard-empty strong {
  color: var(--text-primary);
  font-size: 1rem;
}

.clipboard-empty p {
  max-width: 520px;
  line-height: 1.8;
}

@media (max-width: 1080px) {
  .clipboard-toolbar {
    grid-template-columns: 1fr;
  }

  .clipboard-actions {
    justify-content: flex-start;
  }
}

@media (max-width: 768px) {
  .clipboard-page {
    padding: 24px 18px 36px;
  }

  .clipboard-hero {
    grid-template-columns: 1fr;
  }

  .clipboard-hero-stats {
    justify-content: flex-start;
  }
}
</style>
