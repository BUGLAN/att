<template>
  <article class="settings-record-card" @click="$emit('open')">
    <div class="settings-record-card-head">
      <div class="settings-record-meta">
        <span class="settings-record-time">{{ formattedTime }}</span>
        <span class="settings-record-status" :class="record.status">{{ statusLabel }}</span>
      </div>

      <button
        class="settings-record-favorite"
        :class="{ favorited: record.is_favorite }"
        type="button"
        :title="record.is_favorite ? '取消收藏' : '收藏记录'"
        @click.stop="$emit('toggle-favorite')"
      >
        {{ record.is_favorite ? '★' : '☆' }}
      </button>
    </div>

    <p class="settings-record-text">{{ displayText }}</p>

    <div class="settings-record-foot">
      <span class="settings-record-kind">{{ record.optimized_text ? '优化文本' : '原始文本' }}</span>
      <button class="btn btn-secondary settings-record-copy" type="button" @click.stop="$emit('copy')">
        复制
      </button>
    </div>
  </article>
</template>

<script setup>
import { computed } from 'vue'

const props = defineProps({
  record: {
    type: Object,
    required: true
  }
})

defineEmits(['toggle-favorite', 'copy', 'open'])

const displayText = computed(() => {
  return props.record.optimized_text || props.record.raw_text || '暂无文本内容'
})

const formattedTime = computed(() => {
  if (!props.record.created_at) {
    return '未知时间'
  }

  const date = new Date(props.record.created_at)
  return date.toLocaleString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit'
  })
})

const statusLabel = computed(() => {
  const statusMap = {
    pending: '待处理',
    transcribing: '转写中',
    transcribed: '已转写',
    optimizing: '优化中',
    optimized: '已优化',
    failed: '失败'
  }

  if (props.record.is_modified) {
    return '已编辑'
  }

  return statusMap[props.record.status] || '未知状态'
})
</script>
