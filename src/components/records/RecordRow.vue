<template>
  <!--
    录音记录行（首页「最近」与记录集共用）：时刻 · 正文 · 时长；
    操作按钮是单字（播 / 複 / 藏），悬停才出现；点开整行看原始转写和更多操作。
    有录音的记录可以「重新识别」（展开后选引擎；识别失败的在折叠行上直接给按钮）。
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
      <div v-if="statusMeta || showQuickRetry" class="record-row-status">
        <span v-if="statusMeta" class="ui-meta" :title="statusMeta.title || failureReason || null">
          <span class="ui-dot" :class="statusMeta.dot"></span>{{ statusMeta.text }}
        </span>
        <button
          v-if="showQuickRetry"
          class="ui-tbtn is-line record-row-retry"
          type="button"
          title="用当前设置的识别引擎重新识别这段录音"
          @click.stop="$emit('retranscribe', '')"
        >
          <span class="ui-tbtn-glyph">{{ G.refresh }}</span>重新识别
        </button>
      </div>

      <div v-if="expanded" class="record-row-detail" @click.stop>
        <div>
          <h4>{{ isTextRecord ? '选中原文' : '原始转写' }}</h4>
          <p>{{ record.raw_text || '（没有原始转写）' }}</p>
        </div>
        <div>
          <h4>{{ isTextRecord ? '记录信息' : '识别信息' }}</h4>
          <p class="ui-mono"><template v-if="isTextRecord">来自优化选中文本</template><template v-else>时长 {{ duration }}<template v-if="record.processing_time"> · 处理 {{ Number(record.processing_time).toFixed(1) }}s</template><template v-if="asrSummary"> · {{ asrSummary }}</template></template></p>
          <p v-if="failureReason" class="record-row-error">{{ failureReason }}</p>
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
          <div v-if="hasAudio" class="record-row-retranscribe">
            <BaseSelect
              v-model="selectedEngine"
              size="sm"
              class="record-row-engine"
              aria-label="重新识别用的识别引擎"
              :disabled="!engineOptions.length || transcribing"
            >
              <option v-if="!engineOptions.length" value="">{{ asrEngines.loaded ? '没有可用的识别引擎' : '正在读取引擎…' }}</option>
              <option v-for="option in engineOptions" :key="option.value" :value="option.value">{{ option.label }}</option>
            </BaseSelect>
            <button
              class="ui-tbtn"
              type="button"
              :disabled="!selectedEngine || transcribing"
              :title="engineOptions.length ? '用选中的引擎重新识别这段录音（覆盖现在的文字）' : '先在「设置 → 识别引擎」配好一个引擎'"
              @click="$emit('retranscribe', selectedEngine)"
            >
              <span class="ui-tbtn-glyph">{{ G.refresh }}</span>{{ transcribing ? '识别中…' : '重新识别' }}
            </button>
          </div>
        </div>
      </div>
    </div>

    <div class="record-row-side">
      <span v-if="record.is_favorite" class="record-row-fav" title="已收藏" aria-hidden="true">{{ G.favorite }}</span>
      <div v-if="!selectMode" class="ui-hover-actions" :class="{ 'is-pinned': isPlaying }" @click.stop>
        <button
          v-if="hasAudio"
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
      <span class="record-row-duration ui-mono">{{ isTextRecord ? '文本' : duration }}</span>
    </div>
  </div>
</template>

<script setup>
import { computed, ref, watch } from 'vue'
import { useOptimizationStreamStore } from '@/stores/optimizationStream'
import BaseSelect from '@/components/common/BaseSelect.vue'
import { ACTION_GLYPHS as G } from '@/components/common/glyphs.mjs'
import { formatClock, formatRecordDuration, recordStatusMeta } from './recordListModel.mjs'
import {
  defaultRetranscribeEngine,
  isTranscribing,
  recordAsrSummary,
  retranscribeEngineOptions,
  showQuickRetranscribe
} from './retranscribeModel.mjs'

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
  },
  // 「重新识别」可选引擎（useRecords().asrEngines）：{ current, engines, loaded }
  asrEngines: {
    type: Object,
    default: () => ({ current: '', engines: [], loaded: false })
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
  'toggle-favorite',
  'retranscribe'
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
  const fallback = props.record.status === 'failed' ? '识别失败，没有文字' : '处理中…'
  return props.record.optimized_text || props.record.raw_text || fallback
})
const canOptimize = computed(() => (
  Boolean(props.record.raw_text) && ['transcribed', 'optimized', 'failed'].includes(props.record.status)
))

const clock = computed(() => formatClock(props.record.created_at))
const duration = computed(() => formatRecordDuration(props.record.duration))
// 优化选中文本建的记录：没有音频也没有时长（录音落盘失败的记录仍有时长，照常显示）
const isTextRecord = computed(() => !props.record.audio_path && !Number(props.record.duration))
const statusMeta = computed(() => recordStatusMeta(props.record, { optimizing: isStreamingOptimization.value }))

// ---- 重新识别 ----
const hasAudio = computed(() => Boolean(props.record.audio_path))
const transcribing = computed(() => isTranscribing(props.record))
const showQuickRetry = computed(() => !props.selectMode && !props.expanded && showQuickRetranscribe(props.record))
const failureReason = computed(() => (props.record.status === 'failed' && props.record.error) || '')
const asrSummary = computed(() => recordAsrSummary(props.record))
const engineOptions = computed(() => (
  retranscribeEngineOptions(props.asrEngines?.engines, props.asrEngines?.current)
))
const selectedEngine = ref('')
// 引擎列表变了（首次拉到、设置里换了引擎）：选中项不在列表里时回到默认
watch(engineOptions, (options) => {
  if (!options.some((option) => option.value === selectedEngine.value)) {
    selectedEngine.value = defaultRetranscribeEngine(options, props.asrEngines?.current)
  }
}, { immediate: true })

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
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--s-2);
  margin-top: 4px;
}

.record-row-retry {
  height: 24px;
  padding: 0 8px;
}

.record-row-error {
  margin-top: 4px !important;
  color: var(--err) !important;
}

.record-row-retranscribe {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px;
  margin-top: 6px;
}

.record-row-engine {
  flex: none;
  width: auto;
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
