<template>
  <section
    class="ui-page transcribe-page"
    @dragenter.prevent="onDragEnter"
    @dragover.prevent
    @dragleave="onDragLeave"
    @drop.prevent="onDrop"
  >
    <PageHeader crumb="首页 / 转文字" title="转文字" />

    <!-- 文件：投放区 + 路径输入；转写中印章换成「轉」，底边流动靛蓝细线 -->
    <section class="settings-section transcribe-file">
      <div class="settings-group">
        <h3 class="settings-group-title">文件</h3>
        <div
          class="ui-row transcribe-drop"
          :class="{ 'is-dragover': isDragging, 'is-empty': !filePath && !isBusy }"
          @click="!filePath && !isBusy && chooseFile()"
        >
          <span class="ui-seal transcribe-seal" :data-seal="isBusy ? 'converting' : 'idle'" aria-hidden="true">
            <Transition name="seal-stamp" mode="out-in">
              <span :key="isBusy ? 'busy' : 'idle'" class="ui-seal-ink">{{ isBusy ? SEALS.converting : G.import }}</span>
            </Transition>
          </span>
          <div class="ui-row-main">
            <template v-if="isDragging">
              <div class="ui-row-title">{{ isBusy ? '松手，排进队列' : '松手，开始转写' }}</div>
              <div class="ui-row-desc">可以一次拖入多个文件，按顺序逐个转写</div>
            </template>
            <template v-else-if="!filePath">
              <div class="ui-row-title">拖入音视频文件，或点击选择</div>
              <div class="ui-row-desc">mp4 / mov / mkv / webm / wav / mp3 / m4a / flac / ogg · 可多选排队 · 结果存入记录集 · 同一文件秒回缓存</div>
            </template>
            <template v-else>
              <div class="ui-row-title">{{ fileName }}</div>
              <div class="ui-row-desc ui-mono transcribe-path-text" :title="filePath">{{ filePath }}</div>
            </template>
          </div>
          <template v-if="isBusy">
            <span class="ui-meta transcribe-status" aria-live="polite">
              <span class="ui-dot blue pulse"></span>{{ statusText || '正在转写…' }}
            </span>
            <button class="ui-tbtn" type="button" title="选择更多文件排进队列" @click.stop="chooseFile">
              <span class="ui-tbtn-glyph">{{ G.import }}</span>加入
            </button>
            <button
              class="ui-tbtn is-danger is-line"
              type="button"
              :disabled="cancelRequested"
              @click.stop="cancelCurrent"
            >
              <span class="ui-tbtn-glyph">{{ G.cancel }}</span>{{ cancelRequested ? '取消中' : '取消' }}
            </button>
          </template>
          <template v-else-if="filePath">
            <button class="ui-tbtn" type="button" @click.stop="chooseFile">换一个</button>
            <button class="ui-tbtn is-gold is-line" type="button" @click.stop="startTranscribe">
              {{ result && result.path === filePath ? '重新转写' : '开始转写' }}
            </button>
          </template>
        </div>

        <label class="ui-search transcribe-path">
          <span class="ui-search-glyph" aria-hidden="true">{{ G.file }}</span>
          <!-- 关掉拼写检查与自动替换：macOS「智能破折号」会把手打的 -- 改成 —，路径就对不上了 -->
          <input
            v-model.trim="filePath"
            type="text"
            spellcheck="false"
            autocorrect="off"
            autocapitalize="off"
            autocomplete="off"
            placeholder="或粘贴本地绝对路径，回车开始转写"
            aria-label="本地文件路径"
            :disabled="isBusy"
            @keyup.enter="startTranscribe"
          >
        </label>

        <div v-if="error && !isBusy" class="settings-note warn" role="alert">
          <span>{{ error }}</span>
        </div>
      </div>
      <div
        v-if="isBusy && progressInfo.percent != null"
        class="transcribe-progress"
        role="progressbar"
        aria-label="转写进度"
        :aria-valuenow="progressInfo.percent"
        aria-valuemin="0"
        aria-valuemax="100"
      >
        <span class="transcribe-progress-bar" :style="{ width: progressInfo.percent + '%' }"></span>
      </div>
      <div v-else-if="isBusy" class="ui-flow" style="--flow: var(--seal-converting-line)"></div>
    </section>

    <!-- 排队：多选 / 一次拖入多个时按顺序逐个转写；只有一个文件时不显示 -->
    <section v-if="showQueue" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">
          队列
          <span class="ui-title-aside ui-meta">{{ queueMeta }}</span>
          <button
            v-if="queueStats.done + queueStats.failed + queueStats.cancelled > 0"
            class="ui-tbtn transcribe-queue-clear"
            type="button"
            @click="clearFinished"
          >
            清除已结束
          </button>
        </h3>
        <div
          v-for="item in queue"
          :key="item.id"
          class="ui-row transcribe-queue-item"
          :class="'is-' + item.status"
        >
          <span class="ui-glyph-tile">{{ G.file }}</span>
          <span class="ui-row-main">
            <span class="ui-row-title" :title="item.path">{{ item.fileName }}</span>
            <span v-if="item.status === 'failed' && item.error" class="ui-row-desc transcribe-queue-error">{{ item.error }}</span>
            <span v-else-if="item.status === 'running'" class="ui-row-desc">{{ statusText || '正在转写…' }}</span>
          </span>
          <span class="ui-meta">
            <span class="ui-dot" :class="queueDot(item.status)"></span>{{ QUEUE_STATUS_LABELS[item.status] }}
          </span>
          <button
            v-if="item.status === 'pending'"
            class="ui-tbtn is-danger"
            type="button"
            :aria-label="'移除 ' + item.fileName"
            @click="removeFromQueue(item.id)"
          >
            <span class="ui-tbtn-glyph">{{ G.delete }}</span>移除
          </button>
          <button
            v-else-if="item.status === 'running'"
            class="ui-tbtn is-danger"
            type="button"
            :disabled="cancelRequested"
            @click="cancelCurrent"
          >
            <span class="ui-tbtn-glyph">{{ G.cancel }}</span>{{ cancelRequested ? '取消中' : '取消' }}
          </button>
        </div>
      </div>
    </section>

    <!-- 转写选项：与设置页同款的行 -->
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">转写选项</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">识别引擎</span>
            <span class="setting-desc">{{ engineDesc }}</span>
          </div>
          <BaseSelect :model-value="engine" :disabled="isBusy" aria-label="识别引擎" @update:model-value="pickEngine">
            <option
              v-for="item in engineOptions"
              :key="item.value"
              :value="item.value"
              :disabled="item.disabled"
            >
              {{ item.label }} · {{ item.tag }}{{ item.badge ? `（${item.badge}）` : '' }}
            </option>
          </BaseSelect>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">识别语言</span>
            <span class="setting-desc">{{ languageHint }}</span>
          </div>
          <BaseSelect v-model="language" :disabled="isBusy" aria-label="识别语言">
            <option v-for="item in languageOptions" :key="item.value" :value="item.value">{{ item.label }}</option>
          </BaseSelect>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">分段时间戳</span>
            <span class="setting-desc">结果按句带上起止时间</span>
          </div>
          <label class="toggle-switch">
            <input v-model="wantSegments" type="checkbox" :disabled="isBusy" aria-label="分段时间戳">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">LLM 校对</span>
            <span class="setting-desc">转写后用 LLM 纠错，不改写</span>
          </div>
          <label class="toggle-switch">
            <input :checked="proofreadOn" type="checkbox" :disabled="isBusy" aria-label="LLM 校对" @change="toggleProofread">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <template v-if="proofreadOn">
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">校对模型</span>
              <span class="setting-desc">留空使用默认模型</span>
            </div>
            <BaseSelect v-model="proofreadModelId" :disabled="isBusy" aria-label="校对模型">
              <option value="">默认模型</option>
              <option v-for="model in llmModels" :key="model.id" :value="model.id">{{ model.label }}</option>
            </BaseSelect>
          </div>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">窗口批大小</span>
            </div>
            <BaseSelect v-model.number="proofreadBatchSegments" :disabled="isBusy" aria-label="窗口批大小">
              <option :value="20">20 段 / 批</option>
              <option :value="40">40 段 / 批</option>
              <option :value="60">60 段 / 批</option>
            </BaseSelect>
          </div>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">上下文重叠</span>
              <span class="setting-desc">更准，但更费 token</span>
            </div>
            <BaseSelect v-model.number="proofreadContextChars" :disabled="isBusy" aria-label="上下文重叠">
              <option :value="200">携带前文 200 字</option>
              <option :value="0">不携带（最省 token）</option>
            </BaseSelect>
          </div>
        </template>
      </div>
    </section>

    <!-- 结果：下划线标签切视图，复制在右侧；出结果时滚进视口 -->
    <section v-if="result" ref="resultSection" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">
          结果
          <span class="ui-title-aside ui-meta" :title="result.path">
            <span class="ui-dot ok"></span>{{ resultMeta }}
          </span>
        </h3>
        <div class="ui-tabs transcribe-tabs">
          <button
            v-for="tab in resultTabs"
            :key="tab.key"
            type="button"
            :class="{ 'is-on': activeResultTab === tab.key }"
            @click="activeResultTab = tab.key"
          >
            {{ tab.label }}
          </button>
          <span class="transcribe-tabs-actions">
            <button class="ui-tbtn" type="button" :disabled="!result.text" @click="handleCopy">
              <span class="ui-tbtn-glyph">{{ G.copy }}</span>{{ copied ? '已复制' : '复制全文' }}
            </button>
            <ExportFormatMenu :formats="exportFormats" :disabled="!result.text" @select="handleExport" />
          </span>
        </div>

        <p v-if="activeResultTab === 'text'" class="transcribe-text">{{ result.text || '（没有识别到语音内容）' }}</p>
        <p v-else-if="activeResultTab === 'raw'" class="transcribe-text is-muted">{{ result.rawText || '（没有识别到语音内容）' }}</p>
        <div v-else-if="activeResultTab === 'diff'" class="transcribe-diff">
          <p class="ui-meta">共 {{ proofreadDiff.pairs.length }} 句 · {{ proofreadDiff.changedCount }} 处修改</p>
          <div v-for="(pair, index) in proofreadDiff.pairs" :key="index" class="transcribe-diff-row" :class="{ changed: pair.changed }">
            <template v-if="pair.changed">
              <p class="transcribe-diff-raw">{{ pair.raw }}</p>
              <p class="transcribe-diff-fixed">{{ pair.corrected }}</p>
            </template>
            <p v-else class="transcribe-diff-same">{{ pair.raw }}</p>
          </div>
        </div>
        <ol v-else-if="activeResultTab === 'segments'" class="transcribe-segments">
          <li v-for="(seg, index) in result.segments" :key="index">
            <span class="ui-mono">{{ formatSegmentTime(seg.start) }}</span>
            <span>{{ seg.text }}</span>
          </li>
        </ol>
      </div>
    </section>

    <!-- 最近转写：从记录集读（带来源文件的记录），重启后仍在 -->
    <section v-if="history.length" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">
          最近转写
          <span class="ui-title-aside ui-meta">已存入记录集</span>
        </h3>
        <button
          v-for="item in history"
          :key="item.recordId || item.path"
          type="button"
          class="ui-row is-clickable transcribe-history"
          :class="{ 'is-open': isOpenHistory(item) }"
          @click="selectHistoryItem(item)"
        >
          <span class="ui-glyph-tile">{{ G.file }}</span>
          <span class="ui-row-main">
            <span class="ui-row-title">{{ item.fileName }}</span>
            <span class="ui-row-desc ui-clamp-2">{{ item.text.slice(0, 80) || '（空）' }}</span>
          </span>
          <span class="ui-meta ui-mono">{{ item.engineKey || 'whisper' }} · {{ formatDuration(item.durationSeconds) }}</span>
        </button>
      </div>
    </section>
  </section>
</template>

<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import { useFileTranscribe } from '@/composables/useFileTranscribe'
import { useElectron } from '@/composables/useElectron'
import {
  QUEUE_STATUS_LABELS,
  buildProofreadDiff,
  formatDuration,
  queueSummary,
  transcribeEngineOptions
} from '@/composables/transcribe/fileTranscribeModel.mjs'
import {
  languageHint as buildLanguageHint,
  languageOptions as buildLanguageOptions,
  normalizeLanguageForProvider
} from '@/components/settings/asrLanguages.mjs'
import BaseSelect from '@/components/common/BaseSelect.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import ExportFormatMenu from '@/components/records/ExportFormatMenu.vue'
import {
  SUBTITLE_NEEDS_SEGMENTS,
  TRANSCRIBE_EXPORT_FORMATS,
  canExportSubtitles
} from '@/composables/transcribe/subtitleExport.mjs'
import { ACTION_GLYPHS as G, SEALS } from '@/components/common/glyphs.mjs'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { onFileDrop } = useElectron()
const {
  filePath,
  language,
  wantSegments,
  engine,
  selectEngine,
  proofreadOn,
  proofreadModelId,
  proofreadBatchSegments,
  proofreadContextChars,
  llmModels,
  loadLlmModels,
  isBusy,
  statusText,
  progressInfo,
  cancelRequested,
  queue,
  error,
  result,
  history,
  chooseFile,
  ingestDroppedFiles,
  ingestDroppedPaths,
  startTranscribe,
  cancelCurrent,
  removeFromQueue,
  clearFinished,
  copyResult,
  exportResult,
  selectHistoryItem
} = useFileTranscribe()

// 导出：字幕要分段时间戳，没有时置灰并说明
const exportFormats = computed(() => {
  const subtitles = canExportSubtitles(result.value)
  const hint = result.value?.fromRecord
    ? '从记录集回看的结果不含分段时间戳：重新转写这个文件（开启「分段时间戳」）后可导出字幕'
    : SUBTITLE_NEEDS_SEGMENTS
  return TRANSCRIBE_EXPORT_FORMATS.map((option) => (
    option.value === 'txt' || subtitles ? option : { ...option, disabled: true, hint }
  ))
})

function handleExport(format) {
  const outcome = exportResult(format)
  if (outcome.error) {
    appStore.showToast(outcome.error, { tone: 'info' })
    return
  }
  appStore.showToast(`已导出 ${outcome.filename}`, { tone: 'success' })
}

function isOpenHistory(item) {
  if (!result.value) return false
  return item.recordId ? result.value.recordId === item.recordId : result.value.path === item.path
}

// 队列：不止一个文件，或唯一那个已经结束且失败 / 取消（留着看原因）时才显示
const queueStats = computed(() => queueSummary(queue.value))
const showQueue = computed(() => queueStats.value.total > 1)
const queueMeta = computed(() => {
  const stats = queueStats.value
  const parts = [`${stats.done}/${stats.total} 完成`]
  if (stats.failed) parts.push(`${stats.failed} 失败`)
  if (stats.cancelled) parts.push(`${stats.cancelled} 已取消`)
  return parts.join(' · ')
})

function queueDot(status) {
  return {
    pending: '',
    running: 'blue pulse',
    done: 'ok',
    failed: 'err',
    cancelled: 'warn'
  }[status] || ''
}

const copied = ref(false)
const activeResultTab = ref('text')

// 引擎可用性：豆包看设置页是否保存过 Access Token（GET /api/settings 只回布尔）；
// 本地引擎只知道设置里选中的那个（/api/health 的 local_engine）。不可用的置灰并给出原因，后端 409 兜底
const engineOptions = computed(() => transcribeEngineOptions({
  doubaoKeySet: Boolean(settingsStore.settings?.doubao_asr_api_key_set),
  deepseekKeySet: Boolean(settingsStore.settings?.deepseek_asr_api_key_set),
  localEngine: settingsStore.runtimeStatus?.local_engine || null
}))
const currentEngine = computed(() => engineOptions.value.find((item) => item.value === engine.value))
const languageOptions = computed(() => buildLanguageOptions(engine.value))
const languageHint = computed(() => buildLanguageHint(engine.value))

function pickEngine(value) {
  const option = engineOptions.value.find((item) => item.value === value)
  if (option?.disabled) {
    appStore.showToast(option.hint, { tone: 'info' })
    return
  }
  const adjusted = normalizeLanguageForProvider(value, language.value)
  selectEngine(value)
  if (adjusted.changed) {
    language.value = adjusted.language
  }
  if (adjusted.message) {
    appStore.showToast(adjusted.message, { tone: 'info' })
  }
}

const fileName = computed(() => filePath.value.split(/[\\/]/).pop() || filePath.value)
// 选中的引擎不可用时直接说原因，否则一句介绍
const engineDesc = computed(() => currentEngine.value?.hint || currentEngine.value?.desc || '')

async function toggleProofread() {
  proofreadOn.value = !proofreadOn.value
  if (proofreadOn.value) {
    const models = await loadLlmModels()
    if (!models.length) {
      appStore.showToast('尚未配置任何 LLM 模型，开启校对会失败；请先到设置页配置', { tone: 'info' })
    }
  }
}

// 结果视图：文本（有校对时叫「校对后」）/ 原文 / 对照 / 分段
const resultTabs = computed(() => {
  if (!result.value) return []
  const tabs = [{ key: 'text', label: result.value.rawText ? '校对后' : '文本' }]
  if (result.value.rawText) {
    tabs.push({ key: 'raw', label: '原文' }, { key: 'diff', label: '对照' })
  }
  if (result.value.segments?.length) {
    tabs.push({ key: 'segments', label: `分段 ${result.value.segments.length}` })
  }
  return tabs
})

// 换了结果（新转写 / 点了历史）而当前视图不存在时回到文本
watch(resultTabs, (tabs) => {
  if (!tabs.some((tab) => tab.key === activeResultTab.value)) {
    activeResultTab.value = 'text'
  }
})

const resultMeta = computed(() => {
  const value = result.value
  if (!value) return ''
  const parts = [formatDuration(value.durationSeconds), value.engine || '本地引擎']
  if (value.cached) parts.push('缓存命中')
  if (value.proofread?.applied) {
    parts.push(value.proofread.fallbackBatches.length ? '已校对 · 部分保留原文' : '已校对')
  }
  return parts.join(' · ')
})

const proofreadDiff = computed(() => buildProofreadDiff(result.value?.rawText || '', result.value?.text || ''))

// 出结果（新转写完成 / 点了历史）后把结果区滚进视口：它排在转写选项下面，完成时常在视口外。
// nearest：已在视口内就不动，结果比视口高时对齐顶部
const resultSection = ref(null)
watch(result, async (value) => {
  if (!value) return
  await nextTick()
  const reduceMotion = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
  resultSection.value?.scrollIntoView({ behavior: reduceMotion ? 'auto' : 'smooth', block: 'nearest' })
})

function formatSegmentTime(seconds) {
  const total = Math.max(0, Number(seconds) || 0)
  const minutes = Math.floor(total / 60)
  return `${String(minutes).padStart(2, '0')}:${(total - minutes * 60).toFixed(1).padStart(4, '0')}`
}

// ---------- 拖拽投放 ----------
const dragDepth = ref(0)
const isDragging = computed(() => dragDepth.value > 0)

function onDragEnter() {
  dragDepth.value += 1
}

function onDragLeave() {
  dragDepth.value = Math.max(0, dragDepth.value - 1)
}

async function onDrop(event) {
  dragDepth.value = 0
  const files = Array.from(event.dataTransfer?.files || [])

  if (!files.length) {
    return // 拖进来的不是文件（比如一段文本），安静忽略
  }
  await ingestDroppedFiles(files)
}

// 桌面端：WebView 拿不到拖入文件的路径，改订阅原生拖放事件（页面常驻挂载，只在本页可见时响应）
let unlistenFileDrop = null

async function onNativeFileDrop({ type, paths }) {
  if (appStore.currentPage !== 'transcribe') {
    return
  }
  if (type === 'enter' || type === 'over') {
    dragDepth.value = 1
    return
  }
  dragDepth.value = 0
  if (type !== 'drop' || !paths.length) {
    return
  }
  // 转写中再拖入：排在队列后面
  await ingestDroppedPaths(paths)
}

onMounted(async () => {
  unlistenFileDrop = await onFileDrop(onNativeFileDrop)
})

onUnmounted(() => {
  unlistenFileDrop?.()
  unlistenFileDrop = null
})

async function handleCopy() {
  try {
    await copyResult()
    copied.value = true
    appStore.showToast('转写全文已复制到剪贴板', { tone: 'success' })
    setTimeout(() => {
      copied.value = false
    }, 2000)
  } catch (err) {
    appStore.showToast(err?.message || '复制失败', { tone: 'error' })
  }
}
</script>

<style scoped>
.transcribe-file {
  position: relative;
}

.transcribe-drop {
  min-height: 76px;
  border-style: dashed;
  border-color: var(--line-strong);
}

.transcribe-drop.is-empty {
  cursor: pointer;
}

.transcribe-drop.is-empty:hover,
.transcribe-drop.is-dragover {
  border-color: var(--accent-line-strong);
}

.transcribe-drop.is-dragover {
  background: var(--accent-veil);
}

.transcribe-seal {
  width: 40px;
  height: 40px;
  font-size: 22px;
}

.transcribe-path-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.transcribe-path {
  margin-top: var(--s-2);
}

.transcribe-status {
  max-width: 260px;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 确定进度：底边细条（替代流动线），宽度随识别 / 校对进度推进 */
.transcribe-progress {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 2px;
  overflow: hidden;
  background: var(--accent-faint);
}

.transcribe-progress-bar {
  display: block;
  height: 100%;
  background: var(--seal-converting-line);
  transition: width 0.6s var(--ease);
}

.transcribe-queue-clear {
  height: 24px;
  font-weight: 400;
  letter-spacing: 0;
  text-transform: none;
}

.transcribe-queue-item {
  margin-top: var(--s-2);
}

.transcribe-queue-item .ui-row-main {
  display: grid;
  gap: 2px;
}

.transcribe-queue-item.is-done .ui-row-title,
.transcribe-queue-item.is-cancelled .ui-row-title {
  color: var(--text-3);
}

.transcribe-queue-error {
  color: var(--err);
}

@media (prefers-reduced-motion: reduce) {
  .transcribe-progress-bar {
    transition: none;
  }
}

.transcribe-tabs {
  align-items: center;
  margin-bottom: var(--s-4);
}

.transcribe-tabs-actions {
  display: inline-flex;
  gap: 4px;
  margin-left: auto;
  padding-bottom: 4px;
}

.transcribe-text {
  margin: 0;
  max-height: 520px;
  overflow-y: auto;
  font-size: var(--fs-md);
  line-height: 1.85;
  color: var(--text-1);
  white-space: pre-wrap;
  word-break: break-word;
}

.transcribe-text.is-muted {
  color: var(--text-3);
}

.transcribe-diff {
  display: grid;
  gap: var(--s-2);
  max-height: 520px;
  overflow-y: auto;
}

.transcribe-diff-row p {
  margin: 0;
  font-size: var(--fs-sm);
  line-height: 1.7;
}

.transcribe-diff-same {
  color: var(--text-2);
}

.transcribe-diff-raw {
  color: var(--text-3);
  text-decoration: line-through;
  text-decoration-color: var(--err-line, var(--err));
}

.transcribe-diff-fixed {
  color: var(--text-1);
  padding-left: var(--s-3);
  border-left: 2px solid var(--accent-line);
}

.transcribe-segments {
  display: grid;
  gap: 10px;
  max-height: 520px;
  margin: 0;
  padding: 0;
  overflow-y: auto;
  list-style: none;
}

.transcribe-segments li {
  display: grid;
  grid-template-columns: 72px minmax(0, 1fr);
  gap: var(--s-3);
  font-size: var(--fs-sm);
  line-height: 1.7;
  color: var(--text-1);
}

.transcribe-segments .ui-mono {
  color: var(--text-3);
}

.transcribe-history {
  width: 100%;
  font: inherit;
  text-align: left;
  color: inherit;
}

.transcribe-history .ui-row-main {
  display: grid;
  gap: 2px;
}
</style>
