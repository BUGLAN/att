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
      <button class="audio-play-btn" :class="{ playing: isPlaying }" type="button" @click.stop="$emit('toggle-audio')">
        {{ isPlaying ? '⏸' : '▶' }}
      </button>
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
        <span v-if="record.status === 'transcribing'" class="record-status-badge transcribing">
          <span class="spinner-small"></span>
          转写中
        </span>
        <span v-else-if="record.status === 'optimizing'" class="record-status-badge optimizing">
          <span class="spinner-small"></span>
          优化中
        </span>
        <span v-else-if="record.status === 'failed'" class="record-status-badge failed">失败</span>
        <span v-else-if="record.is_modified" class="record-status-badge modified">已编辑</span>
        <span v-else-if="record.optimized_text" class="record-status-badge optimized">已优化</span>

        <button
          v-if="!selectMode && !record.optimized_text && (record.status === 'transcribed' || record.status === 'failed')"
          class="btn-optimize-inline"
          type="button"
          @click.stop="$emit('optimize')"
        >优化</button>

        <button
          v-if="!selectMode && (record.optimized_text || record.raw_text)"
          class="btn-copy-inline"
          type="button"
          @click.stop="$emit('copy')"
        >复制</button>
      </div>
    </div>
  </div>
</template>

<script setup>
import { computed } from 'vue'

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

const emit = defineEmits(['select', 'edit', 'copy', 'toggle-audio', 'optimize'])

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
