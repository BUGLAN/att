<template>
  <section class="usage-stats" :class="{ 'is-open': open }">
    <!-- 收起时一行：今天 / 近 7 天；点开看按天的柱状图 -->
    <button class="usage-strip" type="button" :aria-expanded="open" aria-controls="usage-stats-panel" @click="toggle">
      <span class="usage-cell">
        <span class="usage-cap">今天</span>
        <span class="usage-val">{{ stripText(weekStats?.today) }}</span>
      </span>
      <span class="usage-cell">
        <span class="usage-cap">近 7 天</span>
        <span class="usage-val">{{ stripText(weekStats?.total) }}</span>
      </span>
      <span class="usage-toggle ui-meta">{{ open ? '收起' : '按天查看' }}<span class="usage-chevron" aria-hidden="true">›</span></span>
    </button>

    <div v-if="open" id="usage-stats-panel" class="usage-panel">
      <div class="usage-controls">
        <div class="ui-tabs usage-tabs" role="group" aria-label="统计范围">
          <button
            v-for="option in USAGE_RANGES"
            :key="option.value"
            type="button"
            :class="{ 'is-on': range === option.value }"
            :aria-pressed="range === option.value"
            @click="range = option.value"
          >{{ option.label }}</button>
        </div>
        <div class="ui-tabs usage-tabs" role="group" aria-label="柱状图指标">
          <button
            v-for="option in USAGE_METRICS"
            :key="option.key"
            type="button"
            :class="{ 'is-on': metric === option.key }"
            :aria-pressed="metric === option.key"
            @click="metric = option.key"
          >{{ option.label }}</button>
        </div>
      </div>

      <p class="usage-readout ui-meta" aria-live="polite">{{ readout }}</p>

      <div v-if="!bars.length" class="usage-empty ui-meta">{{ failed ? '统计暂不可用' : (loading ? '正在统计…' : '暂无记录') }}</div>
      <template v-else>
        <div class="usage-chart" role="img" :aria-label="chartLabel" @mouseleave="hovered = null">
          <div
            v-for="bar in bars"
            :key="bar.key"
            class="usage-col"
            :class="{ 'is-hover': hovered === bar.key }"
            :title="bar.title"
            @mouseenter="hovered = bar.key"
          >
            <div class="usage-bar" :class="{ 'is-zero': !bar.value }" :style="{ height: `${Math.max(bar.ratio * 100, 0)}%` }"></div>
          </div>
        </div>
        <div class="usage-axis" aria-hidden="true">
          <span v-for="(bar, index) in bars" :key="bar.key" class="usage-tick">{{ showTick(index, bars.length) ? bar.label : '' }}</span>
        </div>
      </template>
    </div>
  </section>
</template>

<script setup>
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useElectron } from '@/composables/useElectron'
import { useRecordsStore } from '@/stores/records'
import { createLatestRequestGuard, runLatest } from '@/utils/latestRequest.mjs'
import {
  USAGE_METRICS,
  USAGE_RANGES,
  showTick,
  usageBars,
  usageSummary
} from './usageStatsModel.mjs'

const { api } = useElectron()
const recordsStore = useRecordsStore()

const open = ref(true)
const range = ref('7d')
const metric = ref('count')
const hovered = ref(null)
// 顶部那行固定用近 7 天的数据；展开后选了别的范围另取一份
const weekStats = ref(null)
const rangeStats = ref(null)
const loading = ref(false)
const failed = ref(false)
const weekRequest = createLatestRequestGuard()
const rangeRequest = createLatestRequestGuard()

const panelStats = computed(() => (range.value === '7d' ? weekStats.value : rangeStats.value))
const bars = computed(() => usageBars(panelStats.value, metric.value))
const rangeLabel = computed(() => USAGE_RANGES.find((option) => option.value === range.value)?.label || '')
const metricLabel = computed(() => USAGE_METRICS.find((option) => option.key === metric.value)?.label || '')
const chartLabel = computed(() => `${rangeLabel.value}每天的${metricLabel.value}`)
const readout = computed(() => {
  const bar = bars.value.find((item) => item.key === hovered.value)
  if (bar) return bar.title
  return `${rangeLabel.value}合计：${usageSummary(panelStats.value?.total)}`
})

function stripText(totals) {
  if (failed.value && !weekStats.value) return '—'
  if (!totals) return '…'
  return totals.count ? usageSummary(totals) : '暂无记录'
}

async function loadWeek() {
  await runLatest(weekRequest, () => api.getUsageStats('7d'), {
    onResult: (result) => {
      weekStats.value = result
      failed.value = false
    },
    onError: (error) => {
      // 统计只是附加信息：失败不弹提示，只在这一行显示占位
      console.warn('Failed to load usage stats:', error)
      failed.value = true
    }
  })
}

async function loadRange() {
  if (range.value === '7d') return
  await runLatest(rangeRequest, () => api.getUsageStats(range.value), {
    onStart: () => {
      loading.value = true
    },
    onResult: (result) => {
      rangeStats.value = result
      failed.value = false
    },
    onError: (error) => {
      console.warn('Failed to load usage stats:', error)
      failed.value = true
    },
    onFinally: () => {
      loading.value = false
    }
  })
}

function refresh() {
  void loadWeek()
  if (open.value) void loadRange()
}

function toggle() {
  open.value = !open.value
  if (open.value) void loadRange()
}

watch(range, () => {
  // 换范围时先清空旧图，避免短暂显示上一个范围的柱子；后台刷新时保留旧图
  hovered.value = null
  rangeStats.value = null
  void loadRange()
})

// 录音 / 优化 / 删除都会改最近记录：稍等合并多次变化再重新统计（流式优化时变化很密）
let refreshTimer = null
watch(() => recordsStore.records, () => {
  clearTimeout(refreshTimer)
  refreshTimer = setTimeout(refresh, 1200)
}, { deep: true })

onMounted(() => {
  void loadWeek()
})

onBeforeUnmount(() => {
  clearTimeout(refreshTimer)
})

defineExpose({ refresh })
</script>

<style scoped>
.usage-stats {
  margin-bottom: var(--s-3);
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  background: var(--bg-card);
  overflow: hidden;
}

.usage-strip {
  display: flex;
  align-items: center;
  gap: var(--s-6);
  width: 100%;
  padding: var(--s-2) var(--s-4);
  border: 0;
  background: none;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
}

.usage-strip:hover { background: var(--white-veil); }
.usage-strip:focus-visible { outline: none; box-shadow: inset var(--focus-ring); }

.usage-cell {
  display: inline-flex;
  align-items: baseline;
  gap: var(--s-2);
  min-width: 0;
}

.usage-cap {
  flex: none;
  font-size: var(--fs-cap);
  color: var(--text-3);
}

.usage-val {
  overflow: hidden;
  font-size: var(--fs-sm);
  color: var(--text-1);
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.usage-toggle { margin-left: auto; }

.usage-chevron {
  display: inline-block;
  margin-left: 2px;
  transition: transform 0.18s var(--ease, ease);
}

.is-open .usage-chevron { transform: rotate(90deg); }
.usage-strip:hover .usage-toggle { color: var(--accent); }

.usage-panel {
  padding: var(--s-2) var(--s-4) var(--s-4);
  border-top: 1px solid var(--line);
}

.usage-controls {
  display: flex;
  flex-wrap: wrap;
  justify-content: space-between;
  gap: var(--s-2) var(--s-6);
}

.usage-tabs { border-bottom: 0; }

.usage-readout {
  display: block;
  margin: var(--s-2) 0 var(--s-3);
  overflow: hidden;
  text-overflow: ellipsis;
  font-variant-numeric: tabular-nums;
}

.usage-empty {
  display: flex;
  justify-content: center;
  padding: var(--s-6) 0;
}

.usage-chart {
  display: flex;
  align-items: flex-end;
  gap: 3px;
  height: 120px;
  padding-bottom: 1px;
  border-bottom: 1px solid var(--line-strong);
}

.usage-col {
  display: flex;
  flex: 1 1 0;
  align-items: flex-end;
  justify-content: center;
  min-width: 2px;
  height: 100%;
  cursor: default;
}

.usage-bar {
  width: 100%;
  max-width: 28px;
  min-height: 2px;
  border-radius: var(--r-xs) var(--r-xs) 0 0;
  background: var(--accent-line-strong);
  transition: background 0.12s ease;
}

.usage-bar.is-zero { background: var(--line-strong); }
.usage-col.is-hover .usage-bar { background: var(--accent); }
.usage-col.is-hover .usage-bar.is-zero { background: var(--text-3); }

.usage-axis {
  display: flex;
  gap: 3px;
  margin-top: var(--s-1);
}

.usage-tick {
  flex: 1 1 0;
  min-width: 2px;
  overflow: visible;
  font-size: 11px;
  color: var(--text-3);
  text-align: center;
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}
</style>
