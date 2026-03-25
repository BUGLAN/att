<template>
  <div
    class="record-item"
    :class="{ 'select-mode': selectMode, modified: record.is_modified }"
    @click="handleClick"
  >
    <!-- 复选框 (选择模式) -->
    <input
      v-if="selectMode"
      type="checkbox"
      class="record-checkbox"
      :checked="selected"
      @change="$emit('select')"
      @click.stop
    />

    <!-- 音频播放按钮 -->
    <div class="record-audio">
      <button class="audio-play-btn" @click.stop="$emit('play')">
        <span class="play-icon">▶</span>
      </button>
    </div>

    <!-- 内容 -->
    <div class="record-content">
      <p class="record-text">
        {{ record.optimized_text || record.raw_text || '转写中...' }}
      </p>
    </div>

    <!-- 元信息和操作 -->
    <div class="record-meta">
      <div class="record-times">
        <span class="record-duration">{{ formatDuration(record.duration) }}</span>
        <span class="record-time">{{ formatTime(record.created_at) }}</span>
        <span v-if="record.processing_time" class="record-processing">
          {{ record.processing_time.toFixed(1) }}s
        </span>
      </div>

      <!-- 状态标签 -->
      <span
        v-if="record.status === 'transcribing'"
        class="record-status-badge transcribing"
      >
        <span class="spinner-small"></span>
        转写中
      </span>
      <span v-else-if="record.status === 'failed'" class="record-status-badge failed">
        失败
      </span>
      <span v-else-if="record.is_modified" class="record-status-badge modified">
        已编辑
      </span>
      <span v-else-if="record.optimized_text" class="record-status-badge optimized">
        已优化
      </span>

      <!-- 操作按钮 -->
      <div v-if="!selectMode" class="record-actions">
        <button
          v-if="record.status === 'transcribed' && !record.optimized_text"
          class="btn-record-action btn-optimize"
          @click.stop="$emit('optimize')"
        >
          优化
        </button>
        <button
          class="btn-record-action btn-copy"
          @click.stop="$emit('copy')"
        >
          复制
        </button>
        <button
          class="btn-record-action"
          @click.stop="$emit('edit')"
        >
          编辑
        </button>
        <button
          class="btn-record-action"
          @click.stop="$emit('delete')"
        >
          删除
        </button>
      </div>
    </div>
  </div>
</template>

<script setup>
defineProps({
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
  }
})

defineEmits(['select', 'edit', 'delete', 'optimize', 'copy', 'play'])

function handleClick() {
  // 非选择模式下，点击进入编辑
  // 选择模式下，切换选中状态
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
  const diff = now - date

  // 今天
  if (diff < 86400000 && date.getDate() === now.getDate()) {
    return date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
  }

  // 昨天
  const yesterday = new Date(now)
  yesterday.setDate(yesterday.getDate() - 1)
  if (date.getDate() === yesterday.getDate()) {
    return '昨天'
  }

  // 其他
  return date.toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' })
}
</script>
