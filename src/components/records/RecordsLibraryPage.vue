<template>
  <div class="records-library-page">
    <PageHeader title="记录集" subtitle="搜索、筛选并整理你的录音记录" />

    <section class="settings-section">
      <div class="settings-group settings-records-group">
        <div class="settings-records-header">
          <div>
            <h3 class="settings-group-title">记录列表</h3>
            <p class="settings-records-subtitle">支持按文本范围搜索，并快速收藏、复制和查看内容。</p>
          </div>
          <BaseBadge tone="neutral" mono>{{ total }} 条</BaseBadge>
        </div>

        <div class="settings-records-toolbar">
          <BaseInput
            v-model.trim="recordSearch"
            class="settings-records-search"
            type="search"
            placeholder="搜索转写或优化文本"
          />

          <BaseSelect v-model="recordSearchType" class="settings-records-select">
            <option value="all">全部文本</option>
            <option value="raw">仅原始文本</option>
            <option value="optimized">仅优化文本</option>
          </BaseSelect>

          <BaseButton
            :variant="favoriteOnly ? 'solid' : 'outline'"
            @click="favoriteOnly = !favoriteOnly"
          >
            {{ favoriteOnly ? '仅看收藏中' : '筛选收藏' }}
          </BaseButton>
        </div>

        <div v-if="isLoading" class="settings-records-list">
          <div v-for="index in 6" :key="index" class="settings-records-skeleton">
            <Skeleton :lines="4" inline />
          </div>
        </div>
        <EmptyState
          v-else-if="!records.length"
          size="sm"
          title="没有匹配的记录"
          description="换个关键词或清空筛选条件再试一次。"
        />
        <div v-else class="settings-records-list">
          <SettingsRecordCard
            v-for="record in records"
            :key="record.id"
            :record="record"
            @copy="handleRecordCopy(record)"
            @open="openRecord(record)"
            @toggle-favorite="handleToggleFavorite(record)"
          />
        </div>

        <div v-if="totalPages > 1" class="records-pagination">
          <BaseButton
            variant="outline"
            size="sm"
            :disabled="currentPage <= 1 || isLoading"
            @click="goToPage(currentPage - 1)"
          >
            上一页
          </BaseButton>
          <span class="records-pagination-status">第 {{ currentPage }} / {{ totalPages }} 页</span>
          <BaseButton
            variant="outline"
            size="sm"
            :disabled="currentPage >= totalPages || isLoading"
            @click="goToPage(currentPage + 1)"
          >
            下一页
          </BaseButton>
        </div>
      </div>
    </section>
  </div>
</template>

<script setup>
import { onMounted, ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useRecordsStore } from '@/stores/records'
import { useElectron } from '@/composables/useElectron'
import SettingsRecordCard from '@/components/settings/SettingsRecordCard.vue'
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseInput from '@/components/common/BaseInput.vue'
import BaseSelect from '@/components/common/BaseSelect.vue'
import EmptyState from '@/components/common/EmptyState.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import Skeleton from '@/components/common/Skeleton.vue'

const appStore = useAppStore()
const recordsStore = useRecordsStore()
const { api, clipboard } = useElectron()

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

async function handleRecordCopy(record) {
  const text = record.optimized_text || record.raw_text
  if (!text) {
    return
  }

  await clipboard.writeText(text)
  appStore.showToast('文本已复制到剪贴板', { tone: 'success' })
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
