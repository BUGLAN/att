<template>
  <article class="settings-record-card" @click="$emit('open')">
    <div class="settings-record-card-head">
      <div class="settings-record-meta">
        <span class="settings-record-time">{{ formattedTime }}</span>
        <BaseBadge :tone="statusTone" pill>{{ statusLabel }}</BaseBadge>
      </div>

      <BaseButton
        variant="ghost"
        size="sm"
        icon
        class="settings-record-favorite"
        :class="{ favorited: record.is_favorite }"
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

    <p class="settings-record-text">{{ displayText }}</p>

    <div class="settings-record-foot">
      <span class="settings-record-kind">{{ record.optimized_text ? '优化文本' : '原始文本' }}</span>
      <BaseButton
        variant="outline"
        size="sm"
        class="settings-record-copy"
        @click.stop="$emit('copy')"
      >
        复制
      </BaseButton>
    </div>
  </article>
</template>

<script setup>
import { computed } from 'vue'
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'

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

// 同语义状态统一到同一套语义色（accent/ok/warn/err/info）
const statusTone = computed(() => {
  if (props.record.is_modified) {
    return 'info'
  }

  const toneMap = {
    pending: 'neutral',
    transcribing: 'warn',
    transcribed: 'ok',
    optimizing: 'warn',
    optimized: 'ok',
    failed: 'err'
  }

  return toneMap[props.record.status] || 'neutral'
})
</script>

<style scoped>
/* 实心星需要覆盖原语的描边式 svg 规则 */
.settings-record-card-head .settings-record-favorite.is-active :deep(svg) {
  fill: currentColor;
  stroke: none;
}
</style>
