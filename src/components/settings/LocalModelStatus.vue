<template>
  <!-- 本地引擎的模型状态行 + 可安装模型列表：状态点 / 结论 / 失败原因折叠；模型行带下载、进度、取消与删除 -->
  <!-- 已就绪时额外提供手动释放内存。 -->
  <div class="model-status" :class="`is-${view.state}`">
    <div class="model-status-head">
      <span class="model-status-dot" aria-hidden="true"></span>
      <div class="model-status-body" aria-live="polite">
        <span class="model-status-title">{{ view.title }}</span>
        <span class="model-status-desc" :class="{ 'is-mono': view.state === 'ready' }">{{ view.message }}</span>
      </div>
      <BaseButton
        v-if="view.state === 'ready'"
        class="model-status-release"
        variant="outline"
        size="sm"
        :loading="releasing"
        @click="emit('release')"
      >
        释放内存
      </BaseButton>
    </div>

    <!-- 加载中：底边一道扫过的金色光带（与选中卡片同款） -->
    <div v-if="view.state === 'loading'" class="model-status-track" aria-hidden="true">
      <span class="model-status-bar"></span>
    </div>

    <div v-if="blocks.length || view.tip" class="model-status-extra">
      <details v-if="blocks.length" class="model-status-more">
        <summary>{{ view.detailLabel || '详情' }}</summary>
        <template v-for="(block, index) in blocks" :key="index">
          <pre v-if="block.type === 'code'" class="model-status-code"><code>{{ block.text }}</code></pre>
          <p v-else class="model-status-note">{{ block.text }}</p>
        </template>
      </details>
      <p v-if="view.tip" class="model-status-note">{{ view.tip }}</p>
    </div>

    <div v-if="models.length" class="model-manager">
      <div v-for="model in models" :key="model.id" class="model-row" :class="`is-${model.status}`">
        <div class="model-row-head">
          <span class="model-row-name">{{ model.title }}</span>
          <span class="model-row-meta">
            <span v-if="model.official_only" class="model-row-badge">仅官方源</span>
            <span class="model-row-state">{{ modelStatusText(model) }}</span>
          </span>
        </div>
        <p class="model-row-desc">{{ model.description }}</p>
        <p v-if="jobFor(model)" class="model-row-note">{{ jobFor(model).label }}</p>
        <p v-else-if="lastProblem(model)" class="model-row-note is-error">{{ lastProblem(model) }}</p>
        <div v-if="jobFor(model)" class="model-job">
          <div
            class="model-progress"
            role="progressbar"
            :aria-label="`${model.title}下载进度`"
            :aria-valuenow="Math.round(jobFor(model).percent)"
            :aria-valuetext="`${Math.round(jobFor(model).percent)}%，${jobFor(model).label}`"
            aria-valuemin="0"
            aria-valuemax="100"
          >
            <span :style="{ width: `${jobFor(model).percent}%` }"></span>
          </div>
          <div class="model-job-meta">
            <span v-if="jobFor(model).file">当前文件：{{ jobFor(model).file }}</span>
            <span v-if="jobFor(model).speed">{{ jobFor(model).speed }}</span>
          </div>
        </div>
        <div class="model-row-actions">
          <template v-if="jobFor(model)">
            <BaseButton v-if="jobFor(model).canCancel" variant="outline" size="sm" @click="$emit('cancel', job.id)">
              取消下载
            </BaseButton>
          </template>
          <template v-else>
            <BaseButton
              v-if="model.status !== 'missing'"
              variant="ghost"
              size="sm"
              :disabled="Boolean(activeJob) || startingModelId === model.id"
              @click="$emit('delete', model)"
            >
              删除
            </BaseButton>
            <BaseButton
              v-if="model.status !== 'installed'"
              variant="solid"
              size="sm"
              :disabled="Boolean(activeJob) || startingModelId === model.id"
              @click="$emit('download', model)"
            >
              {{ startingModelId === model.id ? '正在启动…' : downloadLabel(model) }}
            </BaseButton>
          </template>
        </div>
      </div>
    </div>

    <p v-if="activeJob && !models.some((model) => model.id === activeJob.model_id)" class="model-status-note">
      另一个本地模型正在下载：{{ activeJobView.label }}
    </p>
    <p v-if="vadWarning" class="model-status-note is-warning">{{ vadWarning }}</p>

    <div v-if="modelsDir" class="model-dir-row">
      <span class="model-dir-label">模型目录</span>
      <code class="model-dir-path">{{ modelsDir }}</code>
      <BaseButton v-if="isDesktop" variant="outline" size="sm" @click="$emit('reveal')">
        在文件管理器中显示
      </BaseButton>
    </div>
  </div>
</template>

<script setup>
import { computed } from 'vue'
import BaseButton from '@/components/common/BaseButton.vue'
import { hintBlocks } from './asrEngineStatus.mjs'
import { downloadLabel, isJobActive, jobView, modelStatusText } from './modelManager.mjs'

defineOptions({ name: 'LocalModelStatus' })

const props = defineProps({
  // asrEngineStatus.localEngineView() 的返回值
  view: {
    type: Object,
    required: true
  },
  // 当前引擎对应的 /api/models 清单项（Whisper 含 VAD）
  models: {
    type: Array,
    default: () => []
  },
  // 最近一次下载任务（可能已完成 / 失败）
  job: {
    type: Object,
    default: null
  },
  modelsDir: {
    type: String,
    default: ''
  },
  isDesktop: {
    type: Boolean,
    default: false
  },
  startingModelId: {
    type: String,
    default: ''
  },
  vadWarning: {
    type: String,
    default: ''
  },
  releasing: {
    type: Boolean,
    default: false
  }
})

const emit = defineEmits(['download', 'cancel', 'delete', 'reveal', 'release'])

// 详情按原顺序排成「说明 / 命令块」，命令单独成块便于复制
const blocks = computed(() => hintBlocks(props.view.detail))
const activeJob = computed(() => (isJobActive(props.job) ? props.job : null))
const activeJobView = computed(() => jobView(props.job))

// 只在这个模型自己的任务进行中时把进度挂在它下面；Whisper 顺带装的 VAD 不重复刷进度
function jobFor(model) {
  return activeJob.value && props.job.model_id === model.id ? activeJobView.value : null
}

function lastProblem(model) {
  if (!props.job || props.job.model_id !== model.id || isJobActive(props.job)) {
    return ''
  }
  if (props.job.status === 'error' && props.job.error) {
    return `上次下载失败：${props.job.error}`
  }
  if (props.job.status === 'cancelled') {
    return '上次下载已取消；重新下载会从已下载部分续传。'
  }
  return ''
}
</script>

<style scoped>
.model-status {
  --status-color: var(--text-3);
  position: relative;
  margin-bottom: var(--s-3);
  padding: var(--s-3) var(--s-4);
  background: var(--bg-card);
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  overflow: hidden;
}

.model-status.is-loading { --status-color: var(--accent); }
.model-status.is-ready { --status-color: var(--ok); }
.model-status.is-missing { --status-color: var(--warn); }
.model-status.is-failed { --status-color: var(--err); }

.model-status-head {
  display: flex;
  align-items: center;
  gap: var(--s-3);
}

/* 状态点与标题首行对齐 */
.model-status-dot {
  flex: none;
  width: 6px;
  height: 6px;
  margin-top: 8px;
  border-radius: var(--r-xs);
  background: var(--status-color);
}

.model-status.is-loading .model-status-dot {
  animation: model-status-pulse 1.4s var(--ease) infinite;
}

.model-status-body {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.model-status-release {
  flex: none;
  align-self: center;
}

.model-status-title {
  font-size: var(--fs-base);
  font-weight: 500;
  color: var(--text-1);
}

.model-status-desc {
  font-size: var(--fs-cap);
  line-height: 1.55;
  color: var(--text-3);
  overflow-wrap: anywhere;
}

.model-status-desc.is-mono,
.model-status-code {
  font-family: var(--font-mono, ui-monospace, SFMono-Regular, Menlo, monospace);
}

.model-status-track {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 2px;
  background: var(--accent-faint);
  overflow: hidden;
}

.model-status-bar {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 35%;
  background: linear-gradient(90deg, transparent, var(--accent), transparent);
  animation: model-status-sweep 1.4s var(--ease) infinite;
}

/* 说明与命令和标题左对齐（让出状态点 6px + 间距） */
.model-status-extra {
  margin-left: calc(6px + var(--s-3));
}

.model-status-code {
  margin: var(--s-2) 0 0;
  padding: var(--s-2) var(--s-3);
  overflow-x: auto;
  font-size: var(--fs-cap);
  line-height: 1.7;
  color: var(--text-2);
  white-space: pre;
  background: var(--bg-base);
  border-radius: var(--r-sm);
  user-select: text;
}

.model-status-note {
  margin: var(--s-1) 0 0;
  font-size: var(--fs-cap);
  color: var(--text-3);
  overflow-wrap: anywhere;
  user-select: text;
}

.model-status-note.is-warning {
  color: var(--warn);
}

/* 折叠的详情：开关做成一行小字，展开后说明与命令按后端给的顺序排列 */
.model-status-more {
  margin-top: var(--s-1);
}

.model-status-more > summary {
  width: fit-content;
  font-size: var(--fs-cap);
  color: var(--text-2);
  cursor: pointer;
}

.model-status-more > summary:hover {
  color: var(--text-1);
}

.model-status-more[open] > summary {
  margin-bottom: var(--s-1);
}

/* 模型列表：每条一行，状态、体积、下载/删除与进度都在行内 */
.model-manager {
  margin-top: var(--s-3);
  padding-top: var(--s-3);
  border-top: 1px solid var(--line);
  display: grid;
  gap: var(--s-2);
}

.model-row {
  padding: var(--s-2) var(--s-3);
  background: var(--bg-base);
  border: 1px solid transparent;
  border-radius: var(--r-sm);
}

.model-row.is-installed {
  border-color: color-mix(in srgb, var(--ok) 28%, transparent);
}

.model-row-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: var(--s-2);
}

.model-row-name {
  font-size: var(--fs-cap);
  font-weight: 500;
  color: var(--text-1);
}

.model-row-state {
  flex: none;
  font-size: var(--fs-cap);
  color: var(--text-3);
}

.model-row-meta {
  display: inline-flex;
  align-items: baseline;
  gap: var(--s-2);
  flex: none;
}

.model-row-badge {
  padding: 0 6px;
  font-size: var(--fs-cap);
  color: var(--text-3);
  border: 1px solid var(--line);
  border-radius: var(--r-xs);
}

.model-row.is-installed .model-row-state {
  color: var(--ok);
}

.model-row-desc {
  margin: 2px 0 0;
  font-size: var(--fs-cap);
  line-height: 1.55;
  color: var(--text-3);
}

.model-row-note {
  margin: var(--s-1) 0 0;
  font-size: var(--fs-cap);
  color: var(--text-2);
  overflow-wrap: anywhere;
}

.model-row-note.is-error {
  color: var(--err);
}

.model-job {
  margin-top: var(--s-2);
}

.model-progress {
  height: 4px;
  overflow: hidden;
  background: var(--accent-faint);
  border-radius: var(--r-xs);
}

.model-progress > span {
  display: block;
  height: 100%;
  background: var(--accent);
  transition: width 0.3s var(--ease);
}

.model-job-meta {
  display: flex;
  justify-content: space-between;
  gap: var(--s-2);
  margin-top: 4px;
  font-size: var(--fs-cap);
  color: var(--text-3);
}

.model-row-actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--s-2);
  margin-top: var(--s-2);
}

.model-dir-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--s-2);
  margin-top: var(--s-3);
  padding-top: var(--s-3);
  border-top: 1px solid var(--line);
}

.model-dir-label {
  flex: none;
  font-size: var(--fs-cap);
  color: var(--text-3);
}

.model-dir-path {
  flex: 1;
  min-width: 0;
  font-family: var(--font-mono, ui-monospace, SFMono-Regular, Menlo, monospace);
  font-size: var(--fs-cap);
  color: var(--text-2);
  overflow-wrap: anywhere;
  user-select: text;
}

@keyframes model-status-sweep {
  from { left: -35%; }
  to { left: 100%; }
}

@keyframes model-status-pulse {
  50% { opacity: 0.35; }
}

@media (prefers-reduced-motion: reduce) {
  .model-status.is-loading .model-status-dot { animation: none; }
  .model-status-bar { animation: none; left: 0; width: 100%; opacity: 0.5; }
}

@media (max-width: 560px) {
  .model-status-head {
    align-items: flex-start;
    flex-wrap: wrap;
  }

  .model-status-release {
    margin-left: calc(6px + var(--s-3));
  }
}
</style>
