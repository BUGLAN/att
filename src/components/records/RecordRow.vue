<template>
  <!--
    录音记录行（首页「最近」与记录集共用）：时刻 · 正文 · 时长；
    操作按钮是单字（播 / 複 / 藏），悬停才出现；点开整行看原始转写和更多操作。
  -->
  <div
    class="ui-row is-clickable record-row"
    :class="{ 'is-open': expanded, 'is-selected': selected }"
    @click="handleClick"
    @contextmenu.prevent="!selectMode && $emit('edit')"
  >
    <input
      v-if="selectMode"
      class="record-row-check"
      type="checkbox"
      :checked="selected"
      :aria-label="`选择 ${clock} 的记录`"
      @click.stop
      @change="$emit('select')"
    >
    <span class="record-row-time ui-mono">{{ clock }}</span>

    <div class="ui-row-main">
      <p class="record-row-text" :class="{ 'ui-clamp-2': !expanded, 'is-pending': !hasText }">
        {{ displayText }}<span v-if="showTypingCursor" class="typing-cursor" aria-hidden="true"></span>
      </p>
      <span v-if="statusMeta" class="ui-meta record-row-status">
        <span class="ui-dot" :class="statusMeta.dot"></span>{{ statusMeta.text }}
      </span>

      <div v-if="expanded" class="record-row-detail" @click.stop>
        <div>
          <h4>原始转写</h4>
          <p>{{ record.raw_text || '（没有原始转写）' }}</p>
        </div>
        <div>
          <h4>识别信息</h4>
          <p class="ui-mono">时长 {{ duration }}<template v-if="record.processing_time"> · 处理 {{ Number(record.processing_time).toFixed(1) }}s</template></p>
          <div class="record-row-actions">
            <button class="ui-tbtn is-line" type="button" :disabled="!hasText" @click="$emit('copy')">
              <span class="ui-tbtn-glyph">{{ G.copy }}</span>复制
            </button>
            <button v-if="isStreamingOptimization" class="ui-tbtn" type="button" @click="$emit('cancel-optimize')">
              <span class="ui-tbtn-glyph">{{ G.cancel }}</span>取消优化
            </button>
            <button v-else-if="canOptimize" class="ui-tbtn" type="button" @click="$emit('optimize')">
              <span class="ui-tbtn-glyph">{{ G.optimize }}</span>{{ record.optimized_text ? '重新优化' : '优化' }}
            </button>
            <button class="ui-tbtn" type="button" @click="$emit('edit')">
              <span class="ui-tbtn-glyph">{{ G.edit }}</span>编辑
            </button>
            <button class="ui-tbtn" type="button" :disabled="!hasText" @click="$emit('export', 'txt')">
              <span class="ui-tbtn-glyph">{{ G.export }}</span>TXT
            </button>
            <button class="ui-tbtn" type="button" :disabled="!hasText" @click="$emit('export', 'json')">
              <span class="ui-tbtn-glyph">{{ G.export }}</span>JSON
            </button>
            <button class="ui-tbtn is-danger" type="button" @click="$emit('delete')">
              <span class="ui-tbtn-glyph">{{ G.delete }}</span>删除
            </button>
          </div>
        </div>
      </div>
    </div>

    <div class="record-row-side">
      <span v-if="record.is_favorite" class="record-row-fav" title="已收藏" aria-hidden="true">{{ G.favorite }}</span>
      <div v-if="!selectMode" class="ui-hover-actions" :class="{ 'is-pinned': isPlaying }" @click.stop>
        <button
          class="ui-gbtn"
          :class="{ 'is-on': isPlaying }"
          type="button"
          :title="isPlaying ? '暂停试听' : '播放录音'"
          :aria-label="isPlaying ? '暂停试听' : '播放录音'"
          @click="$emit('toggle-audio')"
        >
          {{ isPlaying ? G.pause : G.play }}
        </button>
        <button class="ui-gbtn" type="button" title="复制" aria-label="复制" :disabled="!hasText" @click="$emit('copy')">{{ G.copy }}</button>
        <button
          class="ui-gbtn"
          :class="{ 'is-on': record.is_favorite }"
          type="button"
          :title="record.is_favorite ? '取消收藏' : '收藏'"
          :aria-label="record.is_favorite ? '取消收藏' : '收藏'"
          @click="$emit('toggle-favorite')"
        >
          {{ G.favorite }}
        </button>
      </div>
      <span class="record-row-duration ui-mono">{{ duration }}</span>
    </div>
  </div>
</template>

<script setup>
import { computed } from 'vue'
import { useOptimizationStreamStore } from '@/stores/optimizationStream'
import { ACTION_GLYPHS as G } from '@/components/common/glyphs.mjs'
import { formatClock, formatRecordDuration, recordStatusMeta } from './recordListModel.mjs'

const props = defineProps({
  record: {
    type: Object,
    required: true
  },
  expanded: {
    type: Boolean,
    default: false
  },
  isPlaying: {
    type: Boolean,
    default: false
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

const emit = defineEmits([
  'toggle',
  'select',
  'edit',
  'copy',
  'export',
  'delete',
  'toggle-audio',
  'optimize',
  'cancel-optimize',
  'toggle-favorite'
])

const optimizationStream = useOptimizationStreamStore()
const isStreamingOptimization = computed(() => (
  optimizationStream.recordId === props.record.id && optimizationStream.isActive
))
const showTypingCursor = computed(() => (
  isStreamingOptimization.value && optimizationStream.phase === 'streaming'
))

const hasText = computed(() => Boolean(props.record.optimized_text || props.record.raw_text))
const displayText = computed(() => {
  if (isStreamingOptimization.value) {
    return optimizationStream.text || '正在优化…'
  }
  return props.record.optimized_text || props.record.raw_text || '处理中…'
})
const canOptimize = computed(() => (
  Boolean(props.record.raw_text) && ['transcribed', 'optimized', 'failed'].includes(props.record.status)
))

const clock = computed(() => formatClock(props.record.created_at))
const duration = computed(() => formatRecordDuration(props.record.duration))
const statusMeta = computed(() => recordStatusMeta(props.record, { optimizing: isStreamingOptimization.value }))

function handleClick() {
  if (props.selectMode) {
    emit('select')
    return
  }
  emit('toggle')
}
</script>

<style scoped>
.record-row {
  align-items: flex-start;
}

.record-row.is-selected {
  border-color: var(--accent-line);
  background: var(--accent-veil);
}

.record-row-check {
  margin: 4px 0 0;
  accent-color: var(--accent);
}

.record-row-time {
  width: 46px;
  flex: none;
  padding-top: 2px;
  font-size: var(--fs-cap);
  color: var(--text-3);
}

.record-row-text {
  margin: 0;
  font-size: var(--fs-base);
  line-height: 1.65;
  color: var(--text-1);
  white-space: pre-wrap;
  word-break: break-word;
}

.record-row-text.ui-clamp-2 {
  white-space: normal;
}

.record-row-text.is-pending {
  color: var(--text-3);
}

.record-row-status {
  margin-top: 4px;
}

.record-row-detail {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  gap: var(--s-4);
  margin-top: var(--s-3);
  cursor: default;
}

.record-row-detail h4 {
  margin: 0 0 4px;
  font-size: var(--fs-cap);
  font-weight: 500;
  color: var(--text-3);
}

.record-row-detail p {
  margin: 0;
  font-size: var(--fs-sm);
  line-height: 1.7;
  color: var(--text-2);
  white-space: pre-wrap;
  word-break: break-word;
}

.record-row-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  margin-top: 10px;
}

.record-row-side {
  display: flex;
  align-items: center;
  gap: 10px;
  flex: none;
  margin-top: -4px;
}

.record-row-fav {
  font-family: var(--font-seal);
  font-weight: 900;
  font-size: 12px;
  line-height: 1;
  color: var(--accent);
}

/* 悬停时操作按钮里已有「藏」，常驻的小标记让位 */
.record-row:hover .record-row-fav {
  display: none;
}

.record-row-duration {
  width: 38px;
  text-align: right;
  font-size: var(--fs-cap);
  color: var(--text-3);
}

@media (max-width: 760px) {
  .record-row-detail {
    grid-template-columns: minmax(0, 1fr);
  }
}
</style>
