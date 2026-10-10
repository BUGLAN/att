<template>
  <article
    class="clipboard-card"
    :class="[{ pinned: item.is_pinned, 'image-card': isImageItem }]"
    @mouseenter="handlePreviewHover"
  >
    <div class="clipboard-card-action-bar">
      <BaseButton
        variant="ghost"
        icon
        class="clipboard-icon-btn"
        style="--ui-icon-size: 34px; --ui-btn-radius: var(--r-full)"
        :active="item.is_pinned"
        :disabled="itemActionState.pin"
        :title="item.is_pinned ? '取消置顶' : '置顶'"
        @click.stop="emit('toggle-pin', item)"
      >
        <template #icon>
          <svg viewBox="0 0 24 24" fill="none">
            <path d="M8.5 4.5h7l-1 5 3 3v1h-4v5l-1.5 1.5-1.5-1.5v-5h-4v-1l3-3-1-5Z" />
          </svg>
        </template>
      </BaseButton>
      <BaseButton
        variant="ghost"
        icon
        class="clipboard-icon-btn"
        style="--ui-icon-size: 34px; --ui-btn-radius: var(--r-full)"
        :active="item.is_favorite"
        :disabled="itemActionState.favorite"
        :title="item.is_favorite ? '取消收藏' : '收藏'"
        @click.stop="emit('toggle-favorite', item)"
      >
        <template #icon>
          <svg viewBox="0 0 24 24" fill="none">
            <path d="m12 3.75 2.55 5.17 5.7.83-4.13 4.03.98 5.67L12 16.78 6.9 19.45l.97-5.67-4.12-4.03 5.7-.83L12 3.75Z" />
          </svg>
        </template>
      </BaseButton>
    </div>

    <div class="clipboard-card-top">
      <div class="clipboard-card-meta">
        <BaseBadge tone="accent" uppercase>{{ typeLabel }}</BaseBadge>
        <span class="clipboard-time">{{ relativeTime }}</span>
      </div>
    </div>

    <div class="clipboard-card-body">
      <h3 class="clipboard-card-title">{{ displayTitle }}</h3>
      <p class="clipboard-card-summary">{{ summaryText || '暂无内容' }}</p>

      <div v-if="isImageItem" class="clipboard-card-image-state">
        <span class="clipboard-card-image-tip">悬停查看缩略图</span>
        <span class="clipboard-card-image-meta">{{ imageMetaText }}</span>
      </div>
    </div>

    <div v-if="isImageItem" class="clipboard-card-image-hover">
      <div class="clipboard-card-image-hover-title">{{ displayTitle }}</div>
      <div v-if="previewLoading" class="clipboard-card-image-empty">图片加载中...</div>
      <div v-else-if="!hasPreviewUrl || previewFailed" class="clipboard-card-image-empty">图片不可用</div>
      <img
        v-else
        class="clipboard-card-image"
        :src="resolvedPreviewUrl"
        :alt="displayTitle"
        @error="handlePreviewError"
      >
    </div>

    <footer class="clipboard-card-footer">
      <span class="clipboard-size">{{ sizeLabel }}</span>
      <div class="clipboard-card-actions">
        <BaseButton variant="ghost" size="sm" @click.stop="emit('copy', item)">复制</BaseButton>
        <BaseButton variant="danger" size="sm" @click.stop="emit('delete', item)">删除</BaseButton>
      </div>
    </footer>
  </article>
</template>

<script setup>
import { computed, ref, watch } from 'vue'
import { api } from '@/api/client'
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import {
  getClipboardDisplayTitle,
  getClipboardSummaryText,
  getClipboardTypeLabel,
  isClipboardImageItem
} from '@/utils/clipboardPresentation'

const props = defineProps({
  item: {
    type: Object,
    required: true
  },
  itemActionState: {
    type: Object,
    default: () => ({
      favorite: false,
      pin: false
    })
  }
})

const emit = defineEmits(['copy', 'delete', 'toggle-favorite', 'toggle-pin'])
const previewFailed = ref(false)
const previewLoading = ref(false)
const previewUrl = ref('')
const previewRequested = ref(false)

watch(() => props.item.id, () => {
  previewFailed.value = false
  previewLoading.value = false
  previewRequested.value = false
  previewUrl.value = ''
})

const isImageItem = computed(() => isClipboardImageItem(props.item))
const resolvedPreviewUrl = computed(() => String(previewUrl.value || props.item?.preview_url || '').trim())
const hasPreviewUrl = computed(() => resolvedPreviewUrl.value.length > 0)
const typeLabel = computed(() => getClipboardTypeLabel(props.item.item_type))
const displayTitle = computed(() => getClipboardDisplayTitle(props.item))
const summaryText = computed(() => getClipboardSummaryText(props.item))
const imageMetaText = computed(() => {
  const metadata = props.item?.metadata || {}
  const width = Number(metadata.width || 0)
  const height = Number(metadata.height || 0)
  if (width > 0 && height > 0) {
    return `${width} × ${height}`
  }

  return '图片条目'
})

const sizeLabel = computed(() => {
  const size = Number(props.item.content_size || 0)
  if (size < 1024) {
    return `${size} 字符`
  }
  return `${(size / 1024).toFixed(1)} KB`
})

const relativeTime = computed(() => {
  const value = props.item.updated_at || props.item.created_at
  if (!value) {
    return '刚刚'
  }

  const date = new Date(value)
  const diffMs = Date.now() - date.getTime()
  const diffMinutes = Math.floor(diffMs / 60000)

  if (diffMinutes < 1) {
    return '刚刚'
  }
  if (diffMinutes < 60) {
    return `${diffMinutes} 分钟前`
  }

  const diffHours = Math.floor(diffMinutes / 60)
  if (diffHours < 24) {
    return `${diffHours} 小时前`
  }

  const now = new Date()
  const yesterday = new Date(now)
  yesterday.setDate(now.getDate() - 1)
  if (date.toDateString() === yesterday.toDateString()) {
    return `昨天 ${date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}`
  }

  return date.toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit'
  })
})

async function handlePreviewHover() {
  if (!isImageItem.value || previewRequested.value || resolvedPreviewUrl.value) {
    return
  }

  previewRequested.value = true
  previewLoading.value = true
  previewFailed.value = false

  try {
    const sourceUrl = await api.getClipboardItemImageUrl(
      props.item.id,
      props.item.updated_at || props.item.created_at || Date.now()
    )
    const response = await fetch(sourceUrl)
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`)
    }

    const blob = await response.blob()
    previewUrl.value = await blobToDataUrl(blob)
  } catch (error) {
    console.error('[ClipboardCard] Failed to resolve image preview URL:', error)
    previewFailed.value = true
    previewRequested.value = false
  } finally {
    previewLoading.value = false
  }
}

function handlePreviewError() {
  previewFailed.value = true
  previewRequested.value = false
  previewUrl.value = ''
}

function blobToDataUrl(blob) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(reader.result)
    reader.onerror = () => reject(reader.error || new Error('Failed to read image blob'))
    reader.readAsDataURL(blob)
  })
}
</script>

<style scoped>
.clipboard-card {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
  min-height: 240px;
  padding: var(--s-6);
  /* 原为暖灰 rgba(28,27,27)/rgba(42,42,42) 叠加，改取共享冷灰色阶 */
  background: var(--grad-raise);
  border: 1px solid var(--line);
  border-radius: var(--r-lg);
  transition:
    transform var(--t-base) var(--ease),
    background var(--t-base) var(--ease),
    border-color var(--t-base) var(--ease),
    box-shadow var(--t-base) var(--ease);
  position: relative;
  overflow: hidden;
}

.clipboard-card.image-card {
  min-height: 280px;
}

.clipboard-card::after {
  content: '';
  position: absolute;
  inset: 0;
  /* 明暗档位取自唯一金色（--accent）的同一色相 */
  background: var(--grad-sheen);
  opacity: 0;
  transition: opacity var(--t-base) var(--ease);
  pointer-events: none;
}

.clipboard-card-action-bar {
  position: absolute;
  top: var(--s-4);
  right: var(--s-4);
  display: flex;
  align-items: center;
  gap: var(--s-2);
  z-index: 30;
}

.clipboard-card:hover {
  transform: translateY(-3px);
  border-color: var(--accent-line);
  box-shadow: var(--e-hover);
  background: var(--grad-raise-hi);
}

.clipboard-card:hover::after {
  opacity: 1;
}

.clipboard-card.pinned {
  border-color: var(--accent-line);
  box-shadow: inset 2px 0 0 var(--accent);
}

.clipboard-card-top,
.clipboard-card-footer {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: var(--s-3);
  position: relative;
  z-index: 5;
}

.clipboard-card-top {
  padding-right: 92px;
}

.clipboard-card-meta,
.clipboard-card-actions {
  display: flex;
  align-items: center;
  gap: var(--s-2);
}

.clipboard-time,
.clipboard-size {
  color: var(--text-3);
  font-size: var(--fs-cap);
}

.clipboard-card-body {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  flex: 1;
  position: relative;
  z-index: 4;
}

.clipboard-card-title {
  font-size: var(--fs-lg);
  line-height: 1.4;
  color: var(--text-1);
  overflow-wrap: anywhere;
}

.clipboard-card-summary {
  color: var(--text-2);
  font-size: var(--fs-base);
  line-height: 1.7;
  display: -webkit-box;
  -webkit-line-clamp: 6;
  -webkit-box-orient: vertical;
  overflow: hidden;
  white-space: pre-wrap;
  word-break: break-word;
}

.clipboard-card-image-state {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-2);
  margin-top: auto;
  padding: var(--s-2) var(--s-3);
  border-radius: var(--r-md);
  background: var(--line-soft);
  font-size: var(--fs-cap);
}

.clipboard-card-image-tip {
  color: var(--accent-hi);
}

.clipboard-card-image-meta {
  color: var(--text-3);
}

.clipboard-card-image-hover {
  position: absolute;
  top: 64px;
  right: var(--s-4);
  bottom: var(--s-4);
  left: var(--s-4);
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
  padding: var(--s-3);
  border-radius: var(--r-lg);
  background: var(--bg-base);
  border: 1px solid var(--accent-line);
  box-shadow: var(--e-4);
  opacity: 0;
  pointer-events: none;
  transform: translateY(8px);
  transition: opacity var(--t-fast) var(--ease), transform var(--t-fast) var(--ease);
  z-index: 12;
  overflow: hidden;
}

.clipboard-card:hover .clipboard-card-image-hover {
  opacity: 1;
  transform: translateY(0);
}

.clipboard-card-image-hover-title {
  font-size: var(--fs-sm);
  color: var(--text-1);
  line-height: 1.5;
}

.clipboard-card-image-empty {
  display: grid;
  place-items: center;
  flex: 1;
  border-radius: var(--r-md);
  background: var(--line-soft);
  color: var(--text-3);
  font-size: var(--fs-sm);
}

.clipboard-card-image {
  flex: 1 1 auto;
  width: 100%;
  min-height: 0;
  max-height: 100%;
  object-fit: contain;
  border-radius: var(--r-md);
  background: var(--line-soft);
}

/* 卡片上的图标按钮：34px 圆形，尺寸通过原语的 CSS 变量注入 */
.clipboard-card-action-bar .clipboard-icon-btn {
  background: var(--line-soft);
  color: var(--text-3);
}
</style>
