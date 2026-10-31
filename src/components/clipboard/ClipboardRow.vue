<template>
  <!--
    剪贴板条目行：类型单字（文 / 長 / 圖 / 鏈 / 檔）· 内容 · 类型 / 尺寸 / 时间；
    图片在列表里同样只占一枚「圖」，悬停时才浮出缩略图；操作按钮（複 / 頂 / 藏 / 刪）悬停才出现。
  -->
  <div class="ui-row clipboard-row" :class="{ 'is-pinned': item.is_pinned }" @mouseenter="handlePreviewHover">
    <span class="clipboard-row-type">
      <span class="ui-glyph-tile" :class="{ 'is-accent': item.is_pinned }" :title="typeLabel">{{ typeGlyph }}</span>
      <span v-if="isImageItem" class="clipboard-peek" aria-hidden="true">
        <span v-if="previewLoading" class="ui-meta"><span class="ui-dot blue pulse"></span>加载中…</span>
        <span v-else-if="!hasPreviewUrl || previewFailed" class="ui-meta">图片不可用</span>
        <img v-else :src="resolvedPreviewUrl" :alt="displayTitle" @error="handlePreviewError">
        <span v-if="imageSize" class="ui-meta ui-mono">{{ imageSize }}</span>
      </span>
    </span>

    <div class="ui-row-main">
      <p class="clipboard-row-text ui-clamp-2" :class="{ 'ui-mono': isMonoText, 'is-link': item.item_type === 'link' }">{{ primaryText }}</p>
      <span class="ui-row-desc">{{ metaText }}</span>
    </div>

    <span v-if="item.is_favorite" class="clipboard-row-fav" title="已收藏" aria-hidden="true">{{ G.favorite }}</span>
    <div class="ui-hover-actions">
      <button class="ui-gbtn" type="button" title="复制" aria-label="复制" @click.stop="emit('copy', item)">{{ G.copy }}</button>
      <button
        class="ui-gbtn"
        :class="{ 'is-on': item.is_pinned }"
        type="button"
        :title="item.is_pinned ? '取消置顶' : '置顶'"
        :aria-label="item.is_pinned ? '取消置顶' : '置顶'"
        :disabled="itemActionState.pin"
        @click.stop="emit('toggle-pin', item)"
      >
        {{ G.pin }}
      </button>
      <button
        class="ui-gbtn"
        :class="{ 'is-on': item.is_favorite }"
        type="button"
        :title="item.is_favorite ? '取消收藏' : '收藏'"
        :aria-label="item.is_favorite ? '取消收藏' : '收藏'"
        :disabled="itemActionState.favorite"
        @click.stop="emit('toggle-favorite', item)"
      >
        {{ G.favorite }}
      </button>
      <button class="ui-gbtn is-danger" type="button" title="删除" aria-label="删除" @click.stop="emit('delete', item)">{{ G.delete }}</button>
    </div>
  </div>
</template>

<script setup>
import { computed, ref, watch } from 'vue'
import { api } from '@/api/client'
import { ACTION_GLYPHS as G, clipTypeGlyph } from '@/components/common/glyphs.mjs'
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
const typeGlyph = computed(() => clipTypeGlyph(props.item.item_type))
const displayTitle = computed(() => getClipboardDisplayTitle(props.item))
// 文本类直接显示内容；图片 / 文件显示名字
const primaryText = computed(() => {
  if (isImageItem.value || props.item.item_type === 'file') {
    return displayTitle.value
  }
  return getClipboardSummaryText(props.item) || displayTitle.value
})
// 命令、链接、路径这类短文本用等宽字，长段落保持正文字体
const isMonoText = computed(() => ['text', 'link', 'file'].includes(props.item.item_type) && primaryText.value.length <= 160)

const imageSize = computed(() => {
  const metadata = props.item?.metadata || {}
  const width = Number(metadata.width || 0)
  const height = Number(metadata.height || 0)
  return width > 0 && height > 0 ? `${width} × ${height}` : ''
})

const sizeLabel = computed(() => {
  if (isImageItem.value) {
    return imageSize.value
  }
  const size = Number(props.item.content_size || 0)
  if (!size) {
    return ''
  }
  return size < 1024 ? `${size} 字符` : `${(size / 1024).toFixed(1)} KB`
})

const relativeTime = computed(() => {
  const value = props.item.updated_at || props.item.created_at
  if (!value) {
    return '刚刚'
  }

  const date = new Date(value)
  const diffMinutes = Math.floor((Date.now() - date.getTime()) / 60000)
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

  const yesterday = new Date()
  yesterday.setDate(yesterday.getDate() - 1)
  if (date.toDateString() === yesterday.toDateString()) {
    return `昨天 ${date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}`
  }
  return date.toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
})

const metaText = computed(() => [
  typeLabel.value,
  sizeLabel.value,
  relativeTime.value,
  props.item.is_pinned ? '已置顶' : ''
].filter(Boolean).join(' · '))

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
    console.error('[ClipboardRow] Failed to resolve image preview URL:', error)
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
.clipboard-row-type {
  position: relative;
  display: inline-flex;
  flex: none;
}

.clipboard-row:hover .ui-glyph-tile {
  color: var(--accent);
}

.clipboard-row-text {
  margin: 0;
  font-size: var(--fs-sm);
  line-height: 1.6;
  color: var(--text-1);
  word-break: break-word;
}

.clipboard-row-text.is-link {
  color: var(--accent);
}

.clipboard-row-fav {
  font-family: var(--font-seal);
  font-weight: 900;
  font-size: 12px;
  line-height: 1;
  color: var(--accent);
}

.clipboard-row:hover .clipboard-row-fav {
  display: none;
}

/* 图片缩略图：悬停整行时浮在「圖」上方 */
.clipboard-peek {
  position: absolute;
  left: 0;
  bottom: calc(100% + 8px);
  z-index: 20;
  display: grid;
  gap: 6px;
  width: 240px;
  padding: 8px;
  background: var(--bg-raise);
  border: 1px solid var(--line-strong);
  border-radius: var(--r-md);
  box-shadow: var(--e-4);
  opacity: 0;
  transform: translateY(4px);
  pointer-events: none;
  transition: opacity var(--t-fast) var(--ease), transform var(--t-fast) var(--ease);
}

.clipboard-peek img {
  display: block;
  width: 100%;
  max-height: 280px;
  object-fit: contain;
  border-radius: var(--r-sm);
  background: var(--bg-base);
}

.clipboard-row:hover .clipboard-peek {
  opacity: 1;
  transform: none;
}
</style>
