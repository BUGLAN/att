<template>
  <section class="wordbook-page">
    <PageHeader
      variant="hero"
      title="生词本"
      kicker="AI SELECTION CENTER"
      subtitle="自动沉淀所有查过的外语单词、短语和句子，让零摩擦划词变成可检索、可积累的长期资产。"
      :stats="[
        { label: '今日新增', value: stats.today },
        { label: '累计词条', value: stats.total },
        { label: '收藏词条', value: stats.favorites }
      ]"
    />

    <section class="wordbook-toolbar">
      <div class="wordbook-search">
        <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <circle cx="11" cy="11" r="6" />
          <path d="m16 16 4 4" />
        </svg>
        <BaseInput v-model.trim="query" bare type="search" placeholder="搜索单词、短语、释义或上下文..." />
      </div>

      <div class="wordbook-filters">
        <BaseButton
          v-for="filter in filters"
          :key="filter.value"
          size="sm"
          pill
          :active="selectedType === filter.value"
          @click="selectedType = filter.value"
        >
          {{ filter.label }}
        </BaseButton>
      </div>

      <div class="wordbook-actions">
        <BaseButton variant="outline" size="sm" pill :disabled="isLoading" @click="refreshCurrentView">
          刷新
        </BaseButton>
        <BaseButton size="sm" pill :active="favoriteOnly" @click="favoriteOnly = !favoriteOnly">
          {{ favoriteOnly ? '仅看收藏中' : '筛选收藏' }}
        </BaseButton>
      </div>
    </section>

    <section class="wordbook-summary-row">
      <BaseBadge tone="accent" size="md" pill>总查询 {{ stats.queried_total || 0 }} 次</BaseBadge>
      <BaseBadge tone="neutral" size="md" pill>最近持续扩充中</BaseBadge>
    </section>

    <section v-if="isLoading" class="wordbook-loading">
      <div v-for="index in 6" :key="index" class="wordbook-loading-card">
        <Skeleton :lines="4" inline />
      </div>
    </section>

    <EmptyState
      v-else-if="!items.length"
      size="lg"
      title="还没有生词条目"
      description="等划词动作中心接入后，这里会自动沉淀你查过的单词、短语和句子。"
    />

    <section v-else class="wordbook-grid">
      <article v-for="item in items" :key="item.id" class="wordbook-card">
        <header class="wordbook-card-header">
          <div>
            <BaseBadge tone="accent" pill>{{ typeLabel(item.entry_type) }}</BaseBadge>
            <h3>{{ item.normalized_text }}</h3>
          </div>
          <BaseButton
            size="sm"
            pill
            :active="item.is_favorite"
            :aria-pressed="item.is_favorite ? 'true' : 'false'"
            @click="toggleFavorite(item)"
          >
            {{ item.is_favorite ? '已收藏' : '收藏' }}
          </BaseButton>
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
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseInput from '@/components/common/BaseInput.vue'
import EmptyState from '@/components/common/EmptyState.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import Skeleton from '@/components/common/Skeleton.vue'

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
    appStore.showToast('加载生词本失败', { tone: 'error' })
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
    appStore.showToast(item.is_favorite ? '已取消收藏' : '已加入收藏', { tone: 'success' })
  } catch (error) {
    console.error('[Wordbook] Failed to toggle favorite:', error)
    appStore.showToast('更新收藏状态失败', { tone: 'error' })
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
/* 页面级 padding 归零：外层 .container 已提供统一留白 */
.wordbook-page {
  width: 100%;
  max-width: 100%;
}

.wordbook-toolbar {
  display: grid;
  grid-template-columns: minmax(0, 1.1fr) auto auto;
  gap: var(--s-4);
  align-items: center;
  margin-bottom: var(--s-4);
}

.wordbook-search {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  height: 44px;
  padding: 0 var(--s-4);
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  background: var(--bg-raise);
  transition: border-color var(--t-fast) var(--ease), box-shadow var(--t-fast) var(--ease);
}

/* 统一焦点环：不再把 outline 直接删掉，作用在包裹层上 */
.wordbook-search:focus-within {
  border-color: var(--accent);
  box-shadow: var(--focus-halo);
}

.wordbook-search svg {
  width: 18px;
  height: 18px;
  flex-shrink: 0;
  stroke: var(--text-3);
  stroke-width: 1.7;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.wordbook-search :deep(.ui-input) {
  font-size: var(--fs-base);
}

.wordbook-filters,
.wordbook-actions {
  display: flex;
  gap: var(--s-2);
  flex-wrap: wrap;
}

.wordbook-summary-row {
  display: flex;
  gap: var(--s-3);
  flex-wrap: wrap;
  margin-bottom: var(--s-6);
}

.wordbook-loading,
.wordbook-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
  gap: var(--s-4);
}

.wordbook-loading-card {
  padding: var(--s-4);
  border: 1px solid var(--line);
  border-radius: var(--r-lg);
  background: var(--bg-card);
}

.wordbook-card {
  padding: var(--s-4);
  border: 1px solid var(--line);
  border-radius: var(--r-lg);
  background: var(--bg-card);
  box-shadow: var(--edge-hi);
}

.wordbook-card-header {
  display: flex;
  justify-content: space-between;
  gap: var(--s-3);
  align-items: flex-start;
}

.wordbook-card-header h3 {
  margin-top: var(--s-1);
  font-size: var(--fs-lg);
}

.wordbook-translation {
  margin-top: var(--s-4);
  font-size: var(--fs-md);
  color: var(--text-1);
}

.wordbook-explanations {
  margin-top: var(--s-3);
  padding-left: var(--s-4);
  color: var(--text-2);
  line-height: 1.7;
}

.wordbook-context {
  margin-top: var(--s-3);
  padding: var(--s-3);
  border-radius: var(--r-md);
  background: var(--bg-raise);
  color: var(--text-2);
  line-height: 1.7;
}

.wordbook-card-footer {
  display: flex;
  gap: var(--s-3);
  flex-wrap: wrap;
  margin-top: var(--s-4);
  color: var(--text-3);
  font-size: var(--fs-sm);
}

@media (max-width: 960px) {
  .wordbook-toolbar {
    grid-template-columns: 1fr;
  }
}
</style>
