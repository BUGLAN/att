<template>
  <div
    class="record-item"
    :class="{ 'select-mode': selectMode, modified: record.is_modified }"
    @click="handleClick"
    @contextmenu.prevent="handleRightClick"
  >
    <input
      v-if="selectMode"
      :checked="selected"
      class="record-checkbox"
      type="checkbox"
      @change="$emit('select')"
      @click.stop
    >

    <div class="record-audio">
      <BaseButton
        variant="ghost"
        icon
        class="record-play-btn"
        style="--ui-icon-size: 40px; --ui-btn-radius: var(--r-full)"
        :class="{ playing: isPlaying }"
        :title="isPlaying ? '暂停试听' : '播放录音'"
        @click.stop="$emit('toggle-audio')"
      >
        <template #icon>
          <svg v-if="isPlaying" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <rect x="7" y="5" width="4" height="14" rx="1" />
            <rect x="13" y="5" width="4" height="14" rx="1" />
          </svg>
          <svg v-else viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <path d="M8 5.5v13l11-6.5-11-6.5Z" />
          </svg>
        </template>
      </BaseButton>
    </div>

    <div class="record-content">
      <p class="record-text">{{ displayText }}</p>
    </div>

    <div class="record-meta">
      <div class="record-times">
        <span class="record-duration">{{ formatDuration(record.duration) }}</span>
        <span class="record-time">{{ formatTime(record.created_at) }}</span>
        <span v-if="record.processing_time" class="record-processing">{{ record.processing_time.toFixed(1) }}s</span>
      </div>

      <div class="record-status-group">
        <BaseBadge v-if="record.status === 'transcribing'" tone="accent">
          <span class="spinner-small"></span>
          转写中
        </BaseBadge>
        <BaseBadge v-else-if="record.status === 'optimizing'" tone="warn">
          <span class="spinner-small"></span>
          优化中
        </BaseBadge>
        <BaseBadge v-else-if="record.status === 'failed'" tone="err">失败</BaseBadge>
        <BaseBadge v-else-if="record.is_modified" tone="info">已编辑</BaseBadge>
        <BaseBadge v-else-if="record.optimized_text" tone="warn">已优化</BaseBadge>

        <BaseButton
          v-if="!selectMode && !record.optimized_text && (record.status === 'transcribed' || record.status === 'failed')"
          variant="outline"
          size="sm"
          @click.stop="$emit('optimize')"
        >
          优化
        </BaseButton>

        <BaseButton
          v-if="!selectMode && (record.optimized_text || record.raw_text)"
          variant="ghost"
          size="sm"
          @click.stop="$emit('copy')"
        >
          复制
        </BaseButton>

        <BaseButton
          v-if="!selectMode && (record.optimized_text || record.raw_text)"
          variant="ghost"
          size="sm"
          icon
          title="导出 TXT"
          @click.stop="$emit('export', 'txt')"
        >
          <template #icon>
            <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
              <path d="M12 4v10" />
              <path d="m8 10.5 4 4 4-4" />
              <path d="M5 19h14" />
            </svg>
          </template>
        </BaseButton>

        <BaseButton
          v-if="!selectMode && (record.optimized_text || record.raw_text)"
          variant="ghost"
          size="sm"
          icon
          title="导出 JSON"
          @click.stop="$emit('export', 'json')"
        >
          <template #icon>
            <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
              <path d="M9 5c-2 0-3 1-3 3v2c0 1.2-.7 2-2 2 1.3 0 2 .8 2 2v2c0 2 1 3 3 3" />
              <path d="M15 5c2 0 3 1 3 3v2c0 1.2.7 2 2 2-1.3 0-2 .8-2 2v2c0 2-1 3-3 3" />
            </svg>
          </template>
        </BaseButton>

        <BaseButton
          v-if="!selectMode"
          variant="ghost"
          size="sm"
          icon
          class="record-favorite-btn"
          :active="record.is_favorite"
          :title="record.is_favorite ? '取消收藏' : '收藏记录'"
          @click.stop="$emit('toggle-favorite')"
        >
          <template #icon>
            <svg v-if="record.is_favorite" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path d="m12 3.75 2.55 5.17 5.7.83-4.13 4.03.98 5.67L12 16.78 6.9 19.45l.97-5.67-4.12-4.03 5.7-.83L12 3.75Z" />
            </svg>
            <svg v-else viewBox="0 0 24 24" fill="none" aria-hidden="true">
              <path d="m12 3.75 2.55 5.17 5.7.83-4.13 4.03.98 5.67L12 16.78 6.9 19.45l.97-5.67-4.12-4.03 5.7-.83L12 3.75Z" />
            </svg>
          </template>
        </BaseButton>
      </div>
    </div>
  </div>
</template>

<script setup>
import { computed } from 'vue'
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'

const props = defineProps({
  record: {
    type: Object,
    required: true
  },
  selectMode: {
    type: Boolean,
    default: false
  },
  selected: {
    type: Boolean,
    default: false
  },
  isPlaying: {
    type: Boolean,
    default: false
  }
})

const emit = defineEmits(['select', 'edit', 'copy', 'export', 'toggle-audio', 'optimize', 'toggle-favorite'])

const displayText = computed(() => {
  return props.record.optimized_text || props.record.raw_text || '处理中...'
})

function handleClick() {
  if (props.selectMode) {
    emit('select')
    return
  }

  emit('copy')
}

function handleRightClick() {
  if (props.selectMode) {
    return
  }

  emit('edit')
}

function formatDuration(seconds) {
  if (!seconds) return '0:00'
  const mins = Math.floor(seconds / 60)
  const secs = Math.floor(seconds % 60)
  return `${mins}:${secs.toString().padStart(2, '0')}`
}

function formatTime(timestamp) {
  if (!timestamp) return ''

  const date = new Date(timestamp)
  const now = new Date()
  const isToday = date.toDateString() === now.toDateString()

  if (isToday) {
    return date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
  }

  const yesterday = new Date(now)
  yesterday.setDate(yesterday.getDate() - 1)
  if (date.toDateString() === yesterday.toDateString()) {
    return '昨天'
  }

  return date.toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' })
}
</script>

<style scoped>
/* 试听按钮：在幽灵按钮基线上保留圆形描边外观（尺寸由 --ui-icon-size 注入） */
.record-audio .record-play-btn {
  border: 1px solid var(--line);
  background: var(--bg-raise);
  color: var(--text-2);
}

.record-audio .record-play-btn:hover:not(:disabled) {
  background: var(--accent-dim);
  border-color: var(--accent);
  color: var(--accent);
}

.record-audio .record-play-btn.playing {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--accent-ink);
}

/* 实心图标（播放/暂停、实心星）需要覆盖原语的描边式 svg 规则 */
.record-audio .record-play-btn :deep(svg),
.record-status-group .record-favorite-btn.is-active :deep(svg) {
  fill: currentColor;
  stroke: none;
}
</style>
