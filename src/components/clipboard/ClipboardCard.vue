<template>
  <article class="clipboard-card" :class="[{ pinned: item.is_pinned }]">
    <div class="clipboard-card-top">
      <div class="clipboard-card-meta">
        <span class="clipboard-chip">{{ typeLabel }}</span>
        <span class="clipboard-time">{{ relativeTime }}</span>
      </div>

      <div class="clipboard-card-flags">
        <button
          class="icon-button"
          :class="{ active: item.is_pinned }"
          type="button"
          :title="item.is_pinned ? '取消置顶' : '置顶'"
          @click="$emit('toggle-pin', item)"
        >
          <svg viewBox="0 0 24 24" fill="none">
            <path d="M8.5 4.5h7l-1 5 3 3v1h-4v5l-1.5 1.5-1.5-1.5v-5h-4v-1l3-3-1-5Z" />
          </svg>
        </button>
        <button
          class="icon-button"
          :class="{ active: item.is_favorite }"
          type="button"
          :title="item.is_favorite ? '取消收藏' : '收藏'"
          @click="$emit('toggle-favorite', item)"
        >
          <svg viewBox="0 0 24 24" fill="none">
            <path d="m12 3.75 2.55 5.17 5.7.83-4.13 4.03.98 5.67L12 16.78 6.9 19.45l.97-5.67-4.12-4.03 5.7-.83L12 3.75Z" />
          </svg>
        </button>
      </div>
    </div>

    <div class="clipboard-card-body">
      <h3 class="clipboard-card-title">{{ item.title || '未命名记录' }}</h3>
      <p class="clipboard-card-summary">{{ item.summary || item.content || '暂无内容' }}</p>
    </div>

    <footer class="clipboard-card-footer">
      <span class="clipboard-size">{{ sizeLabel }}</span>
      <div class="clipboard-card-actions">
        <button class="text-button" type="button" @click="$emit('copy', item)">复制</button>
        <button class="text-button danger" type="button" @click="$emit('delete', item)">删除</button>
      </div>
    </footer>
  </article>
</template>

<script setup>
import { computed } from 'vue'

const props = defineProps({
  item: {
    type: Object,
    required: true
  }
})

defineEmits(['copy', 'delete', 'toggle-favorite', 'toggle-pin'])

const typeLabels = {
  text: '文本记录',
  link: '链接',
  code: '长文本',
  file: '文件记录',
  image: '图像'
}

const typeLabel = computed(() => typeLabels[props.item.item_type] || '记录')

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
</script>

<style scoped>
.clipboard-card {
  display: flex;
  flex-direction: column;
  gap: 18px;
  min-height: 240px;
  padding: 22px;
  background:
    linear-gradient(180deg, rgba(53, 53, 52, 0.28), rgba(28, 27, 27, 0.94)),
    rgba(42, 42, 42, 0.82);
  border-radius: var(--radius-lg);
  transition: transform var(--transition-base), background var(--transition-base), box-shadow var(--transition-base);
  position: relative;
  overflow: hidden;
}

.clipboard-card::after {
  content: '';
  position: absolute;
  inset: 0;
  background: linear-gradient(135deg, rgba(255, 197, 87, 0.06), transparent 45%);
  opacity: 0;
  transition: opacity var(--transition-base);
  pointer-events: none;
}

.clipboard-card:hover {
  transform: translateY(-3px);
  box-shadow: 0 18px 40px rgba(0, 0, 0, 0.36);
  background:
    linear-gradient(180deg, rgba(64, 64, 63, 0.38), rgba(28, 27, 27, 0.96)),
    rgba(42, 42, 42, 0.92);
}

.clipboard-card:hover::after {
  opacity: 1;
}

.clipboard-card.pinned {
  box-shadow: inset 2px 0 0 rgba(255, 197, 87, 0.92);
}

.clipboard-card-top,
.clipboard-card-footer {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 12px;
}

.clipboard-card-meta,
.clipboard-card-flags,
.clipboard-card-actions {
  display: flex;
  align-items: center;
  gap: 10px;
}

.clipboard-chip {
  display: inline-flex;
  align-items: center;
  padding: 4px 10px;
  border-radius: 999px;
  background: rgba(255, 197, 87, 0.12);
  color: var(--accent);
  font-size: 0.72rem;
  font-weight: 700;
  letter-spacing: 0.08em;
}

.clipboard-time,
.clipboard-size {
  color: var(--text-muted);
  font-size: 0.75rem;
}

.clipboard-card-body {
  display: flex;
  flex-direction: column;
  gap: 12px;
  flex: 1;
}

.clipboard-card-title {
  font-size: 1.02rem;
  line-height: 1.4;
  color: var(--text-primary);
  overflow-wrap: anywhere;
}

.clipboard-card-summary {
  color: var(--text-secondary);
  font-size: 0.9rem;
  line-height: 1.7;
  display: -webkit-box;
  -webkit-line-clamp: 6;
  -webkit-box-orient: vertical;
  overflow: hidden;
  white-space: pre-wrap;
  word-break: break-word;
}

.icon-button,
.text-button {
  -webkit-appearance: none;
  appearance: none;
  border: none;
  cursor: pointer;
  transition: all var(--transition-fast);
}

.icon-button {
  width: 34px;
  height: 34px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.04);
  color: var(--text-muted);
}

.icon-button:hover,
.icon-button.active {
  background: rgba(255, 197, 87, 0.12);
  color: var(--accent);
}

.icon-button svg {
  width: 16px;
  height: 16px;
  stroke: currentColor;
  stroke-width: 1.8;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.text-button {
  padding: 8px 12px;
  border-radius: var(--radius-md);
  background: rgba(255, 255, 255, 0.04);
  color: var(--text-secondary);
  font-size: 0.78rem;
}

.text-button:hover {
  background: rgba(255, 197, 87, 0.12);
  color: var(--accent);
}

.text-button.danger:hover {
  background: rgba(248, 113, 113, 0.16);
  color: var(--danger);
}
</style>
