<template>
  <section class="wordbook-page">
    <header class="wordbook-hero">
      <div class="wordbook-hero-copy">
        <span class="wordbook-kicker">AI SELECTION CENTER</span>
        <h1 class="wordbook-title">生词本</h1>
        <p class="wordbook-subtitle">自动沉淀所有查过的外语单词、短语和句子，让零摩擦划词变成可检索、可积累的长期资产。</p>
      </div>

      <div class="wordbook-hero-stats">
        <div class="hero-stat">
          <span class="hero-stat-label">今日新增</span>
          <strong>{{ stats.today }}</strong>
        </div>
        <div class="hero-stat">
          <span class="hero-stat-label">累计词条</span>
          <strong>{{ stats.total }}</strong>
        </div>
        <div class="hero-stat">
          <span class="hero-stat-label">收藏词条</span>
          <strong>{{ stats.favorites }}</strong>
        </div>
      </div>
    </header>

    <section class="wordbook-toolbar">
      <div class="wordbook-search">
        <svg viewBox="0 0 24 24" fill="none">
          <circle cx="11" cy="11" r="6" />
          <path d="m16 16 4 4" />
        </svg>
        <input v-model.trim="query" type="search" placeholder="搜索单词、短语、释义或上下文...">
      </div>

      <div class="wordbook-filters">
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

      <div class="wordbook-actions">
        <button class="ghost-button" type="button" :disabled="isLoading" @click="refreshCurrentView">刷新</button>
        <button class="ghost-button" :class="{ active: favoriteOnly }" type="button" @click="favoriteOnly = !favoriteOnly">
          {{ favoriteOnly ? '仅看收藏中' : '筛选收藏' }}
        </button>
      </div>
    </section>

    <section class="wordbook-summary-row">
      <div class="summary-pill">总查询 {{ stats.queried_total || 0 }} 次</div>
      <div class="summary-pill muted">最近持续扩充中</div>
    </section>

    <section v-if="isLoading" class="wordbook-empty">
      正在加载生词本...
    </section>

    <section v-else-if="!items.length" class="wordbook-empty">
      <strong>还没有生词条目</strong>
      <p>等划词动作中心接入后，这里会自动沉淀你查过的单词、短语和句子。</p>
    </section>

    <section v-else class="wordbook-grid">
      <article v-for="item in items" :key="item.id" class="wordbook-card">
        <header class="wordbook-card-header">
          <div>
            <span class="wordbook-entry-type">{{ typeLabel(item.entry_type) }}</span>
            <h3>{{ item.normalized_text }}</h3>
          </div>
          <button
            class="favorite-button"
            :class="{ active: item.is_favorite }"
            type="button"
            @click="toggleFavorite(item)"
          >
            {{ item.is_favorite ? '已收藏' : '收藏' }}
          </button>
        </header>

        <p class="wordbook-translation">{{ item.translation }}</p>
        <ul v-if="item.explanations?.length" class="wordbook-explanations">
          <li v-for="(explanation, index) in item.explanations" :key="`${item.id}-${index}`">
            {{ explanation }}
          </li>
        </ul>
        <p v-if="item.source_context" class="wordbook-context">{{ item.source_context }}</p>

        <footer class="wordbook-card-footer">
          <span>{{ item.language || 'unknown' }}</span>
          <span>{{ item.query_count }} 次查询</span>
          <span v-if="item.source_app">{{ item.source_app }}</span>
        </footer>
      </article>
    </section>
  </section>
</template>

<script setup>
import { onMounted, ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useElectron } from '@/composables/useElectron'

const appStore = useAppStore()
const { api } = useElectron()

const items = ref([])
const stats = ref({
  total: 0,
  today: 0,
  favorites: 0,
  queried_total: 0
})
const isLoading = ref(false)
const query = ref('')
const selectedType = ref('all')
const favoriteOnly = ref(false)

const filters = [
  { value: 'all', label: '全部内容' },
  { value: 'word', label: '单词' },
  { value: 'phrase', label: '短语' },
  { value: 'sentence', label: '句子' }
]

function debounce(fn, delay) {
  let timer = null
  return function (...args) {
    if (timer) {
      clearTimeout(timer)
    }
    timer = setTimeout(() => fn.apply(this, args), delay)
  }
}

function typeLabel(type) {
  const labels = {
    word: '单词',
    phrase: '短语',
    sentence: '句子'
  }
  return labels[type] || '条目'
}

function buildFilters() {
  return {
    query: query.value,
    entry_type: selectedType.value === 'all' ? undefined : selectedType.value,
    favorite_only: favoriteOnly.value,
    limit: 120
  }
}

async function refreshCurrentView() {
  isLoading.value = true

  try {
    const [entries, nextStats] = await Promise.all([
      api.getWordbookEntries(buildFilters()),
      api.getWordbookStats()
    ])
    items.value = entries
    stats.value = nextStats
  } catch (error) {
    console.error('[Wordbook] Failed to load entries:', error)
    appStore.showToast('加载生词本失败')
  } finally {
    isLoading.value = false
  }
}

async function toggleFavorite(item) {
  try {
    await api.updateWordbookEntry(item.id, {
      is_favorite: !item.is_favorite
    })
    await refreshCurrentView()
    appStore.showToast(item.is_favorite ? '已取消收藏' : '已加入收藏')
  } catch (error) {
    console.error('[Wordbook] Failed to toggle favorite:', error)
    appStore.showToast('更新收藏状态失败')
  }
}

const debouncedRefresh = debounce(refreshCurrentView, 250)

watch([query, selectedType, favoriteOnly], () => {
  debouncedRefresh()
})

onMounted(async () => {
  await refreshCurrentView()
})
</script>

<style scoped>
.wordbook-page {
  width: 100%;
  max-width: 1280px;
  padding: 36px 32px 48px;
}

.wordbook-hero {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 24px;
  align-items: end;
  padding: 28px 30px;
  margin-bottom: 24px;
  border-radius: 24px;
  background:
    radial-gradient(circle at top right, rgba(112, 197, 141, 0.16), transparent 34%),
    linear-gradient(180deg, rgba(28, 27, 27, 0.9), rgba(19, 19, 19, 0.98));
}

.wordbook-kicker {
  display: inline-flex;
  margin-bottom: 10px;
  color: #8ed4a2;
  font-family: 'Space Mono', var(--font-mono);
  font-size: 0.74rem;
  letter-spacing: 0.18em;
}

.wordbook-title {
  font-family: 'Instrument Sans', var(--font-display);
  font-size: clamp(2rem, 3.2vw, 3rem);
  line-height: 1.05;
  margin-bottom: 10px;
}

.wordbook-subtitle {
  max-width: 760px;
  color: var(--text-secondary);
  font-size: 0.98rem;
  line-height: 1.8;
}

.wordbook-hero-stats {
  display: flex;
  gap: 14px;
  flex-wrap: wrap;
  justify-content: flex-end;
}

.hero-stat {
  min-width: 110px;
  padding: 16px 18px;
  border-radius: 18px;
  background: rgba(255, 255, 255, 0.06);
  border: 1px solid rgba(255, 255, 255, 0.08);
}

.hero-stat-label {
  display: block;
  color: var(--text-muted);
  font-size: 0.76rem;
  letter-spacing: 0.08em;
  margin-bottom: 8px;
}

.hero-stat strong {
  font-size: 1.4rem;
}

.wordbook-toolbar {
  display: grid;
  grid-template-columns: minmax(0, 1.1fr) auto auto;
  gap: 16px;
  align-items: center;
  margin-bottom: 20px;
}

.wordbook-search {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 14px;
  height: 52px;
  border-radius: 18px;
  background: rgba(255, 255, 255, 0.05);
  border: 1px solid rgba(255, 255, 255, 0.08);
}

.wordbook-search input {
  flex: 1;
  background: transparent;
  border: none;
  color: inherit;
  font: inherit;
}

.wordbook-search input:focus {
  outline: none;
}

.wordbook-search svg {
  width: 18px;
  height: 18px;
  stroke: var(--text-secondary);
  stroke-width: 1.7;
}

.wordbook-filters,
.wordbook-actions {
  display: flex;
  gap: 10px;
  flex-wrap: wrap;
}

.filter-button,
.ghost-button,
.favorite-button {
  padding: 10px 14px;
  border-radius: 999px;
  border: 1px solid rgba(255, 255, 255, 0.1);
  background: rgba(255, 255, 255, 0.04);
  color: inherit;
  cursor: pointer;
}

.filter-button.active,
.ghost-button.active,
.favorite-button.active {
  background: rgba(142, 212, 162, 0.16);
  border-color: rgba(142, 212, 162, 0.34);
}

.wordbook-summary-row {
  display: flex;
  gap: 12px;
  flex-wrap: wrap;
  margin-bottom: 24px;
}

.summary-pill {
  padding: 10px 14px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.06);
}

.summary-pill.muted {
  color: var(--text-secondary);
}

.wordbook-empty {
  padding: 56px 24px;
  border-radius: 22px;
  background: rgba(255, 255, 255, 0.04);
  border: 1px solid rgba(255, 255, 255, 0.08);
  text-align: center;
  color: var(--text-secondary);
}

.wordbook-empty strong {
  display: block;
  margin-bottom: 10px;
  color: var(--text-primary);
}

.wordbook-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
  gap: 18px;
}

.wordbook-card {
  padding: 18px;
  border-radius: 22px;
  background:
    linear-gradient(180deg, rgba(255, 255, 255, 0.06), rgba(255, 255, 255, 0.03));
  border: 1px solid rgba(255, 255, 255, 0.08);
}

.wordbook-card-header {
  display: flex;
  justify-content: space-between;
  gap: 14px;
  align-items: start;
}

.wordbook-card-header h3 {
  margin-top: 6px;
  font-size: 1.3rem;
}

.wordbook-entry-type {
  display: inline-flex;
  padding: 4px 10px;
  border-radius: 999px;
  background: rgba(142, 212, 162, 0.16);
  color: #b8ecc6;
  font-size: 0.76rem;
}

.wordbook-translation {
  margin-top: 16px;
  font-size: 1.06rem;
  color: var(--text-primary);
}

.wordbook-explanations {
  margin-top: 14px;
  padding-left: 18px;
  color: var(--text-secondary);
  line-height: 1.7;
}

.wordbook-context {
  margin-top: 14px;
  padding: 12px 14px;
  border-radius: 16px;
  background: rgba(255, 255, 255, 0.04);
  color: var(--text-secondary);
  line-height: 1.7;
}

.wordbook-card-footer {
  display: flex;
  gap: 12px;
  flex-wrap: wrap;
  margin-top: 16px;
  color: var(--text-muted);
  font-size: 0.82rem;
}

@media (max-width: 960px) {
  .wordbook-toolbar {
    grid-template-columns: 1fr;
  }

  .wordbook-hero {
    grid-template-columns: 1fr;
  }

  .wordbook-hero-stats {
    justify-content: flex-start;
  }
}
</style>
