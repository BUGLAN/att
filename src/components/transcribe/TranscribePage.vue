<template>
  <section
    class="ui-page transcribe-page"
    @dragenter.prevent="onDragEnter"
    @dragover.prevent
    @dragleave="onDragLeave"
    @drop.prevent="onDrop"
  >
    <PageHeader crumb="首页 / 转文字" title="转文字" subtitle="把本地音视频转成文本，长文件自动进入后台任务。" />

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
              <div class="ui-row-title">松手，开始转写</div>
              <div class="ui-row-desc">一次转一个文件</div>
            </template>
            <template v-else-if="!filePath">
              <div class="ui-row-title">拖入音视频文件，或点击选择</div>
              <div class="ui-row-desc">mp4 / mov / mkv / webm / wav / mp3 / m4a / flac / ogg · 结果不进记录集 · 同一文件秒回缓存</div>
            </template>
            <template v-else>
              <div class="ui-row-title">{{ fileName }}</div>
              <div class="ui-row-desc ui-mono transcribe-path-text" :title="filePath">{{ filePath }}</div>
            </template>
          </div>
          <span v-if="isBusy" class="ui-meta">
            <span class="ui-dot blue pulse"></span>{{ statusText || '正在转写…' }}
          </span>
          <template v-else-if="filePath">
            <button class="ui-tbtn" type="button" @click.stop="chooseFile">换一个</button>
            <button class="ui-tbtn is-gold is-line" type="button" @click.stop="startTranscribe">
              {{ result && result.path === filePath ? '重新转写' : '开始转写' }}
            </button>
          </template>
        </div>

        <label class="ui-search transcribe-path">
          <span class="ui-search-glyph" aria-hidden="true">{{ G.file }}</span>
          <input
            v-model.trim="filePath"
            type="text"
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
      <div v-if="isBusy" class="ui-flow" style="--flow: var(--seal-converting-line)"></div>
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
              v-for="item in TRANSCRIBE_ENGINES"
              :key="item.value"
              :value="item.value"
              :disabled="item.value === 'doubao_asr' && !doubaoKeySet"
            >
              {{ item.label }} · {{ item.tag }}{{ item.value === 'doubao_asr' && !doubaoKeySet ? '（未配置）' : '' }}
            </option>
          </BaseSelect>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">识别语言</span>
            <span class="setting-desc">本次转写使用的语言</span>
          </div>
          <BaseSelect v-model="language" :disabled="isBusy" aria-label="识别语言">
            <option value="zh">中文</option>
            <option value="en">英语</option>
            <option value="ja">日语</option>
            <option value="ko">韩语</option>
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
            <span class="setting-desc">转写完成后按窗口纠错，不润色；失败的批次保留原文</span>
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
              <span class="setting-desc">每次送去校对的句数</span>
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
              <span class="setting-desc">携带前文让断句处更准，但更费 token</span>
            </div>
            <BaseSelect v-model.number="proofreadContextChars" :disabled="isBusy" aria-label="上下文重叠">
              <option :value="200">携带前文 200 字</option>
              <option :value="0">不携带（最省 token）</option>
            </BaseSelect>
          </div>
        </template>
      </div>
    </section>

    <!-- 结果：下划线标签切视图，复制在右侧 -->
    <section v-if="result" class="settings-section">
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

    <!-- 本次会话转过的文件 -->
    <section v-if="history.length" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">本次会话</h3>
        <button
          v-for="item in history"
          :key="item.path + item.text.length"
          type="button"
          class="ui-row is-clickable transcribe-history"
          :class="{ 'is-open': result && result.path === item.path }"
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
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import { useFileTranscribe } from '@/composables/useFileTranscribe'
import { useElectron } from '@/composables/useElectron'
import {
  TRANSCRIBE_ENGINES,
  buildProofreadDiff,
  formatDuration
} from '@/composables/transcribe/fileTranscribeModel.mjs'
import BaseSelect from '@/components/common/BaseSelect.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import { ACTION_GLYPHS as G, SEALS } from '@/components/common/glyphs.mjs'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { onFileDrop } = useElectron()
const {
  filePath,
  language,
  wantSegments,
  engine,
  proofreadOn,
  proofreadModelId,
  proofreadBatchSegments,
  proofreadContextChars,
  llmModels,
  loadLlmModels,
  isBusy,
  statusText,
  error,
  result,
  history,
  chooseFile,
  ingestDroppedFile,
  ingestDroppedPath,
  startTranscribe,
  copyResult,
  selectHistoryItem
} = useFileTranscribe()

const copied = ref(false)
const activeResultTab = ref('text')

// 豆包凭据态：设置页保存过的 Access Token（GET /api/settings 只回布尔）
const doubaoKeySet = computed(() => Boolean(settingsStore.settings?.doubao_asr_api_key_set))

// 引擎选择：豆包未配置凭据时卡片置灰不可选（后端也会 409 兜底）
function pickEngine(value) {
  if (value === 'doubao_asr' && !doubaoKeySet.value) {
    appStore.showToast('请先在设置页配置豆包 Access Token', { tone: 'info' })
    return
  }
  engine.value = value
}

const fileName = computed(() => filePath.value.split(/[\\/]/).pop() || filePath.value)
const engineDesc = computed(() => {
  if (!doubaoKeySet.value && engine.value !== 'doubao_asr') {
    return `${TRANSCRIBE_ENGINES.find((item) => item.value === engine.value)?.desc || ''} 豆包需先在设置页配置 Access Token。`
  }
  return TRANSCRIBE_ENGINES.find((item) => item.value === engine.value)?.desc || ''
})

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
  if (isBusy.value) {
    appStore.showToast('正在转写中，先等这一条跑完', { tone: 'info' })
    return
  }
  if (files.length > 1) {
    appStore.showToast('一次转一个：已取第一个文件', { tone: 'info' })
  }

  await ingestDroppedFile(files[0])
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
  if (isBusy.value) {
    appStore.showToast('正在转写中，先等这一条跑完', { tone: 'info' })
    return
  }
  if (paths.length > 1) {
    appStore.showToast('一次转一个：已取第一个文件', { tone: 'info' })
  }
  await ingestDroppedPath(paths[0])
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

.transcribe-tabs {
  align-items: center;
  margin-bottom: var(--s-4);
}

.transcribe-tabs-actions {
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
