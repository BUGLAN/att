<template>
  <section
    class="tp"
    :class="{ 'tp--center': isIdleLayout }"
    @dragenter.prevent="onDragEnter"
    @dragover.prevent
    @dragleave="onDragLeave"
    @drop.prevent="onDrop"
  >
    <!-- ============ 页头：眉题 + 大标题，入场纵向错落 ============ -->
    <header class="tp-head tp-reveal" style="--d: 0">
      <p class="tp-eyebrow">
        <span class="tp-eyebrow-rule" aria-hidden="true"></span>
        LOCAL FILE · 本地文件转写台
        <span class="tp-eyebrow-rule" aria-hidden="true"></span>
      </p>
      <h2 class="tp-title">转文字</h2>
      <p class="tp-sub">
        把音视频文件拖进下面的控制台，直接换回纯文本。视频自动抽音轨；同一份内容转过的，秒回缓存；长文件自动进后台任务。
      </p>
    </header>

    <!-- ============ 控制台 = 投放区：波形 + 文件轨 + 参数 ============ -->
    <section
      class="tp-console tp-reveal"
      :class="{ 'is-dragover': isDragging }"
      style="--d: 1"
      aria-label="转写控制台，支持拖入音视频文件"
    >
      <!-- 拖拽悬停遮罩 -->
      <div class="tp-drop-overlay" aria-hidden="true">
        <span class="tp-drop-word">松手，开始转写</span>
      </div>

      <!-- 波形母题：空闲时是静采样，转写/拖拽时跳动，完成时归于一线 -->
      <div class="tp-wave" :class="waveMode" aria-hidden="true">
        <span
          v-for="(h, i) in waveBars"
          :key="i"
          class="tp-wave-bar"
          :style="{ '--h': h + '%', '--i': i }"
        ></span>
      </div>

      <div class="tp-rail">
        <BaseButton variant="solid" size="md" :disabled="isBusy" @click="chooseFile">
          选择文件
        </BaseButton>
        <div class="tp-path">
          <BaseInput
            v-model.trim="filePath"
            bare
            type="text"
            placeholder="或粘贴本地绝对路径 —— E:\tmp\dv\abc.mp4"
            :disabled="isBusy"
            @keyup.enter="startTranscribe"
          />
        </div>
        <BaseButton
          variant="outline"
          size="md"
          :disabled="isBusy || !filePath"
          @click="startTranscribe"
        >
          {{ isBusy ? '转写中' : '开始转写' }}
        </BaseButton>
      </div>

      <!-- 引擎三卡（UI 不暴露 auto：缺省即显式 whisper，设计 §7） -->
      <div class="tp-sec">
        <span class="tp-sec-key">ENGINE</span>
        <span class="tp-sec-name">转写引擎</span>
        <span class="tp-sec-note">缓存 key = 内容 sha256 + 引擎</span>
      </div>
      <div class="tp-engines">
        <button
          v-for="item in TRANSCRIBE_ENGINES"
          :key="item.value"
          type="button"
          class="tp-engine"
          :class="{
            'is-selected': engine === item.value,
            'is-disabled': item.value === 'doubao_asr' && !doubaoKeySet
          }"
          :disabled="isBusy"
          @click="pickEngine(item.value)"
        >
          <span v-if="item.badge" class="tp-engine-badge">{{ item.badge }}</span>
          <span class="tp-engine-name">
            <span class="tp-dot" :class="{ off: item.value === 'doubao_asr' && !doubaoKeySet }"></span>
            {{ item.label }}
          </span>
          <span class="tp-engine-tag">{{ item.tag }}</span>
          <p class="tp-engine-desc">{{ item.desc }}</p>
          <p v-if="item.value === 'doubao_asr' && !doubaoKeySet" class="tp-engine-warn">
            ⚠ 未配置 Access Token — <span class="tp-go-settings" @click.stop="goSettings">去设置</span>
          </p>
        </button>
      </div>

      <!-- LLM 校对面板：开关 + 窗口参数（默认关，设计 §10-1 倾向值） -->
      <div class="tp-proof">
        <div class="tp-proof-head">
          <button
            type="button"
            class="tp-switch"
            :class="{ on: proofreadOn }"
            :disabled="isBusy"
            :aria-pressed="proofreadOn"
            @click="toggleProofread"
          >
            <span class="tp-switch-knob"></span>
          </button>
          <span class="tp-proof-title">LLM 校对<span class="tp-proof-mono">SLIDING-WINDOW</span></span>
          <span class="tp-proof-hint">转写完成后自动执行 · 纠错不润色 · 失败批保留原文</span>
        </div>

        <div v-if="proofreadOn" class="tp-proof-body">
          <label class="tp-proof-field">
            <span class="tp-proof-key">LLM 模型</span>
            <select v-model="proofreadModelId" class="tp-tick-select" :disabled="isBusy">
              <option value="">默认模型</option>
              <option v-for="model in llmModels" :key="model.id" :value="model.id">
                {{ model.label }}
              </option>
            </select>
          </label>
          <label class="tp-proof-field">
            <span class="tp-proof-key">窗口批大小</span>
            <select v-model.number="proofreadBatchSegments" class="tp-tick-select" :disabled="isBusy">
              <option :value="20">20 段 / 批</option>
              <option :value="40">40 段 / 批</option>
              <option :value="60">60 段 / 批</option>
            </select>
          </label>
          <label class="tp-proof-field">
            <span class="tp-proof-key">上下文重叠</span>
            <select v-model.number="proofreadContextChars" class="tp-tick-select" :disabled="isBusy">
              <option :value="200">携带前文 200 字</option>
              <option :value="0">不携带（最省 token）</option>
            </select>
          </label>
        </div>
      </div>

      <div class="tp-ticks">
        <label class="tp-tick">
          <span class="tp-tick-key">LANG</span>
          <select v-model="language" class="tp-tick-select" :disabled="isBusy">
            <option value="zh">中文</option>
            <option value="en">英语</option>
            <option value="ja">日语</option>
            <option value="ko">韩语</option>
          </select>
        </label>

        <label class="tp-tick tp-tick--toggle">
          <span class="tp-tick-key">SEG</span>
          <input v-model="wantSegments" type="checkbox" class="tp-tick-box" :disabled="isBusy" />
          <span class="tp-tick-val">分段时间戳</span>
        </label>

        <span class="tp-tick-note">拖拽 / 点选 / 粘贴 · 纯文本 · 不入记录集 · 按内容缓存</span>
      </div>
    </section>

    <!-- 状态行：等宽 + 块状光标；错误走控制台告警 -->
    <p v-if="isBusy" class="tp-status tp-reveal" style="--d: 2" role="status">
      {{ statusText || '正在转写…' }}<span class="tp-caret" aria-hidden="true"></span>
    </p>
    <p v-else-if="error" class="tp-alert tp-reveal" style="--d: 2" role="alert">{{ error }}</p>

    <!-- ============ 转写结果：磁带日志 ============ -->
    <article v-if="result" class="tp-ledger tp-reveal" style="--d: 2">
      <header class="tp-ledger-head">
        <span class="tp-ledger-name" :title="result.path">{{ result.fileName || '未命名文件' }}</span>
        <span class="tp-chip tp-chip--time">{{ formatDuration(result.durationSeconds) }}</span>
        <span class="tp-chip">{{ result.engine || '本地引擎' }}</span>
        <span v-if="result.cached" class="tp-stamp">缓存命中</span>

        <span v-if="result.proofread && result.proofread.applied" class="tp-chip tp-chip--proof">
          已校对{{ result.proofread.fallbackBatches.length ? '·部分保留原文' : '' }}
        </span>

        <div v-if="result.rawText" class="tp-tabs">
          <button
            v-for="tab in resultTabs"
            :key="tab.key"
            type="button"
            class="tp-tab"
            :class="{ on: activeResultTab === tab.key }"
            @click="activeResultTab = tab.key"
          >
            {{ tab.label }}
          </button>
        </div>

        <BaseButton
          variant="ghost"
          size="sm"
          class="tp-copy"
          :disabled="!result.text"
          @click="handleCopy"
        >
          {{ copied ? '已复制 ✓' : '复制全文' }}
        </BaseButton>
      </header>

      <!-- 校对后（默认视图） -->
      <p v-if="activeResultTab !== 'raw'" class="tp-ledger-text">{{ result.text || '（没有识别到语音内容）' }}</p>
      <!-- ASR 原文 -->
      <p v-if="activeResultTab === 'raw'" class="tp-ledger-text tp-ledger-text--muted">
        {{ result.rawText || '（没有识别到语音内容）' }}
      </p>

      <!-- 对照视图：逐句 diff（句数不一致时整段一对，纯逻辑见 fileTranscribeModel.mjs） -->
      <div v-if="activeResultTab === 'diff'" class="tp-diff">
        <p class="tp-diff-meta">共 {{ proofreadDiff.pairs.length }} 句 · {{ proofreadDiff.changedCount }} 处修改</p>
        <div
          v-for="(pair, index) in proofreadDiff.pairs"
          :key="index"
          class="tp-diff-row"
          :class="{ changed: pair.changed }"
        >
          <template v-if="pair.changed">
            <p class="tp-diff-line tp-diff-line--raw">{{ pair.raw }}</p>
            <p class="tp-diff-line tp-diff-line--fixed">{{ pair.corrected }}</p>
          </template>
          <p v-else class="tp-diff-line tp-diff-line--same">{{ pair.raw }}</p>
        </div>
      </div>

      <details v-if="result.segments.length" class="tp-segments">
        <summary>时间轴 · {{ result.segments.length }} 段</summary>
        <ol class="tp-seg-list">
          <li v-for="(seg, index) in result.segments" :key="index" class="tp-seg">
            <span class="tp-seg-time">{{ seg.start.toFixed(1) }}</span>
            <span class="tp-seg-lead" aria-hidden="true"></span>
            <span class="tp-seg-text">{{ seg.text }}</span>
          </li>
        </ol>
      </details>
    </article>

    <!-- ============ 空态：控制台即空态，只补一行说明 ============ -->
    <p v-else-if="!isBusy && !error" class="tp-idle tp-reveal" style="--d: 2">
      <span class="tp-idle-mark" aria-hidden="true">REC</span>
      把文件拖进上面的控制台最省事；也支持 mp4 / flv / webm / mkv / wav / mp3 / m4a / flac / ogg
      等常见容器。结果只在这里，不会进入记录集。
    </p>

    <!-- ============ 历史架：本次会话 ============ -->
    <aside v-if="history.length" class="tp-rack tp-reveal" style="--d: 3">
      <h3 class="tp-rack-title">RECENT · 本次会话</h3>
      <button
        v-for="(item, index) in history"
        :key="item.path + item.text.length"
        type="button"
        class="tp-rack-row"
        :class="{ 'is-active': result && result.path === item.path }"
        @click="selectHistoryItem(item)"
      >
        <span class="tp-rack-idx">{{ String(history.length - index).padStart(2, '0') }}</span>
        <span class="tp-rack-name">{{ item.fileName }}</span>
        <span class="tp-rack-eng">{{ item.engineKey || 'whisper' }}</span>
        <span class="tp-rack-preview">{{ item.text.slice(0, 60) || '（空）' }}</span>
        <span class="tp-rack-dur">{{ formatDuration(item.durationSeconds) }}</span>
      </button>
    </aside>
  </section>
</template>

<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import { useFileTranscribe } from '@/composables/useFileTranscribe'
import { useElectron } from '@/composables/useElectron'
import {
  TRANSCRIBE_ENGINES,
  buildProofreadDiff,
  formatDuration
} from '@/composables/transcribe/fileTranscribeModel.mjs'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseInput from '@/components/common/BaseInput.vue'

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

function goSettings() {
  appStore.openSettings('asr')
}

async function toggleProofread() {
  proofreadOn.value = !proofreadOn.value
  if (proofreadOn.value) {
    const models = await loadLlmModels()
    if (!models.length) {
      appStore.showToast('尚未配置任何 LLM 模型，开启校对会失败；请先到设置页配置', { tone: 'info' })
    }
  }
}

// 结果三视图（有校对原文时才显示 tabs）
const resultTabs = computed(() => result.value?.rawText
  ? [
      { key: 'text', label: '校对后' },
      { key: 'raw', label: '原文' },
      { key: 'diff', label: '对照' }
    ]
  : [])

const proofreadDiff = computed(() => buildProofreadDiff(result.value?.rawText || '', result.value?.text || ''))

// 波形条：固定 32 根，种子化的伪随机高度，保证每次进来形状一致
const WAVE_SEED = [38, 62, 24, 80, 46, 30, 72, 55, 20, 88, 40, 64, 28, 76, 34, 58,
  18, 70, 44, 26, 82, 36, 60, 22, 90, 48, 32, 74, 42, 56, 30, 66]
const waveBars = WAVE_SEED

const waveMode = computed(() => {
  if (isBusy.value || isDragging.value) return 'is-live'
  if (result.value) return 'is-done'
  return 'is-idle'
})

// 空态（无结果/未忙/无报错/无历史）时页面整体垂直居中，避免「一条浮在顶部」
const isIdleLayout = computed(() =>
  !result.value && !isBusy.value && !error.value && history.value.length === 0
)

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
/* ============================================================
   转写控制台 —— 金色暗色「磁带日志」方向
   色彩全部引用 src/styles/tokens.css（金色只有一套；文字三级取 text-1/2/3）
   ============================================================ */

.tp {
  --tp-pad: 28px;
  display: flex;
  flex-direction: column;
  gap: 20px;
  width: 100%;
  max-width: 920px;
  margin: 0 auto;
  padding: var(--tp-pad) var(--tp-pad) 48px;
}

/* 空态时占满一屏并垂直居中，页面不再「一条浮在顶部」 */
.tp--center {
  min-height: 100%;
  justify-content: center;
  padding-bottom: 64px;
}

/* ---------- 入场：纵向错落揭示 ---------- */
.tp-reveal {
  animation: tp-rise var(--t-slow) var(--ease) both;
  animation-delay: calc(var(--d, 0) * 90ms);
}

@keyframes tp-rise {
  from {
    opacity: 0;
    transform: translateY(10px);
  }
  to {
    opacity: 1;
    transform: none;
  }
}

/* ---------- 页头 ---------- */
.tp-head {
  text-align: center;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
  padding-top: 8px;
}

.tp-eyebrow {
  display: inline-flex;
  align-items: center;
  gap: 12px;
  margin: 0;
  font-family: var(--font-mono);
  font-size: var(--fs-cap);
  letter-spacing: 0.28em;
  color: var(--accent);
}

.tp-eyebrow-rule {
  width: 36px;
  height: 1px;
  background: var(--accent-line);
}

.tp-title {
  margin: 0;
  font-size: 44px;
  font-weight: 650;
  letter-spacing: 0.06em;
  line-height: 1.1;
  color: var(--text-1);
}

.tp-sub {
  margin: 0;
  max-width: 620px;
  font-size: var(--fs-sm);
  line-height: 1.8;
  color: var(--text-3);
}

/* ---------- 控制台卡（= 投放区） ---------- */
.tp-console {
  position: relative;
  display: flex;
  flex-direction: column;
  padding: 20px 22px 18px;
  background: var(--grad-card);
  border: 1px solid var(--line);
  border-radius: var(--r-lg);
  box-shadow: var(--e-2), var(--edge-hi);
  transition: border-color var(--t-fast) var(--ease), box-shadow var(--t-fast) var(--ease);
}

/* 四角定位标记：像磁带仓的装配角 */
.tp-console::before,
.tp-console::after {
  content: '';
  position: absolute;
  width: 14px;
  height: 14px;
  border: 1px solid var(--accent-line);
  pointer-events: none;
  z-index: 3;
}

.tp-console::before {
  top: -1px;
  left: -1px;
  border-right: 0;
  border-bottom: 0;
  border-top-left-radius: var(--r-lg);
}

.tp-console::after {
  right: -1px;
  bottom: -1px;
  border-left: 0;
  border-top: 0;
  border-bottom-right-radius: var(--r-lg);
}

/* ---------- 拖拽悬停态 ---------- */
.tp-console.is-dragover {
  border-color: var(--accent-line-strong);
  box-shadow: 0 0 0 1px var(--accent-line-strong), var(--accent-glow);
}

.tp-drop-overlay {
  position: absolute;
  inset: 0;
  z-index: 2;
  display: none;
  align-items: center;
  justify-content: center;
  background: var(--accent-veil);
  border-radius: inherit;
  pointer-events: none;
}

.tp-drop-word {
  padding: 10px 22px;
  font-family: var(--font-mono);
  font-size: var(--fs-md);
  letter-spacing: 0.34em;
  color: var(--accent-hi);
  background: var(--bg-card);
  border: 1px dashed var(--accent-line-strong);
  border-radius: var(--r-md);
}

.tp-console.is-dragover .tp-drop-overlay {
  display: flex;
}

/* ---------- 波形母题 ---------- */
.tp-wave {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 3px;
  height: 44px;
  margin-bottom: 18px;
  padding-bottom: 10px;
  border-bottom: 1px dashed var(--line-strong);
}

.tp-wave-bar {
  width: 3px;
  height: var(--h, 30%);
  border-radius: 2px;
  background: var(--accent-line);
  transition: background var(--t-base) var(--ease), height var(--t-slow) var(--ease);
}

/* 空闲：整体静默，只留中间几根微亮 */
.tp-wave.is-idle .tp-wave-bar {
  opacity: 0.5;
}

.tp-wave.is-idle .tp-wave-bar:nth-child(8n + 3) {
  background: var(--accent);
  opacity: 0.85;
}

/* 转写中 / 拖拽悬停：逐根错拍跳动 */
.tp-wave.is-live .tp-wave-bar {
  background: var(--accent-hi);
  animation: tp-dance 900ms var(--ease) infinite alternate;
  animation-delay: calc(var(--i) * -73ms);
}

@keyframes tp-dance {
  from {
    height: 12%;
  }
  to {
    height: 92%;
  }
}

/* 完成：归于一线低幅，金实色 */
.tp-wave.is-done .tp-wave-bar {
  height: 14%;
  background: var(--accent);
}

.tp-wave.is-done .tp-wave-bar:nth-child(-n + 4),
.tp-wave.is-done .tp-wave-bar:nth-last-child(-n + 4) {
  height: 8%;
  opacity: 0.6;
}

/* ---------- 文件轨 ---------- */
.tp-rail {
  display: flex;
  align-items: stretch;
  gap: 10px;
}

.tp-rail > :first-child {
  flex-shrink: 0;
}

.tp-path {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  padding: 0 12px;
  background: var(--bg-raise);
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  transition: border-color var(--t-fast) var(--ease);
}

.tp-path:focus-within {
  border-color: var(--accent-line-strong);
  box-shadow: var(--focus-halo);
}

.tp-path :deep(input) {
  width: 100%;
  font-family: var(--font-mono);
  font-size: var(--fs-sm);
  color: var(--text-1);
}

/* bare 形态压掉了 :focus 边框，但文本框聚焦恒命中 :focus-visible，
   会再画一圈 2px 金色 outline 叠在容器边框内侧——聚焦指示统一由
   上方 .tp-path:focus-within 的 halo 承担，这里去掉内层的那圈。 */
.tp-path :deep(input:focus-visible) {
  outline: none;
}

.tp-path :deep(input)::placeholder {
  color: var(--text-3);
}

/* ---------- 引擎三卡 ---------- */
.tp-sec {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 16px;
}

.tp-sec-key {
  font-family: var(--font-mono);
  font-size: 11px;
  letter-spacing: 0.18em;
  color: var(--accent);
  padding: 1px 8px;
  border: 1px solid var(--accent-line);
  border-radius: var(--r-sm);
  background: var(--accent-faint);
}

.tp-sec-name {
  font-size: var(--fs-sm);
  font-weight: 600;
}

.tp-sec-note {
  margin-left: auto;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--text-3);
}

.tp-engines {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 10px;
  margin-top: 10px;
}

.tp-engine {
  position: relative;
  text-align: left;
  padding: 12px 14px;
  cursor: pointer;
  background: var(--bg-raise);
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  color: inherit;
  font-family: inherit;
  transition: border-color var(--t-fast) var(--ease), background var(--t-fast) var(--ease);
}

.tp-engine:hover:not(.is-disabled) {
  border-color: var(--accent-line);
}

.tp-engine.is-selected {
  border-color: var(--accent-line-strong);
  background: var(--accent-veil);
  box-shadow: 0 0 0 1px var(--accent-line-strong);
}

.tp-engine.is-disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.tp-engine-badge {
  position: absolute;
  top: 10px;
  right: 12px;
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.1em;
  color: var(--accent);
}

.tp-engine-name {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: var(--fs-sm);
  font-weight: 600;
}

.tp-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--ok);
  flex-shrink: 0;
}

.tp-dot.off {
  background: var(--err);
}

.tp-engine-tag {
  font-family: var(--font-mono);
  font-size: 10px;
  padding: 1px 7px;
  margin-left: 6px;
  border-radius: var(--r-full);
  border: 1px solid var(--line-strong);
  color: var(--text-3);
}

.tp-engine.is-selected .tp-engine-tag {
  color: var(--accent);
  border-color: var(--accent-line);
}

.tp-engine-desc {
  margin: 6px 0 0;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-3);
}

.tp-engine-warn {
  margin: 8px 0 0;
  font-size: 11px;
  color: var(--err);
}

.tp-go-settings {
  text-decoration: underline;
  cursor: pointer;
}

/* ---------- LLM 校对面板 ---------- */
.tp-proof {
  margin-top: 14px;
  padding-top: 14px;
  border-top: 1px solid var(--line-soft);
}

.tp-proof-head {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}

.tp-switch {
  position: relative;
  width: 38px;
  height: 20px;
  border-radius: var(--r-full);
  background: var(--gray-700);
  border: 1px solid var(--line-strong);
  cursor: pointer;
  flex-shrink: 0;
  transition: background var(--t-base) var(--ease);
  padding: 0;
}

.tp-switch.on {
  background: var(--accent);
  border-color: var(--accent);
}

.tp-switch-knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--text-2);
  transition: all var(--t-base) var(--ease);
}

.tp-switch.on .tp-switch-knob {
  left: 20px;
  background: var(--accent-ink);
}

.tp-proof-title {
  font-size: var(--fs-sm);
  font-weight: 600;
}

.tp-proof-mono {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.16em;
  color: var(--accent);
  margin-left: 8px;
}

.tp-proof-hint {
  margin-left: auto;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--text-3);
}

.tp-proof-body {
  display: flex;
  gap: 12px;
  flex-wrap: wrap;
  margin-top: 12px;
  padding-top: 12px;
  border-top: 1px dashed var(--line-strong);
}

.tp-proof-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.tp-proof-key {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.18em;
  color: var(--text-3);
}

/* ---------- 结果 tabs 与对照视图 ---------- */
.tp-tabs {
  display: flex;
  gap: 4px;
  background: var(--bg-raise);
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  padding: 3px;
}

.tp-tab {
  border: 0;
  background: transparent;
  color: var(--text-2);
  font-size: 12px;
  font-family: inherit;
  padding: 4px 12px;
  border-radius: 7px;
  cursor: pointer;
}

.tp-tab.on {
  background: var(--accent);
  color: var(--accent-ink);
  font-weight: 600;
}

.tp-chip--proof {
  color: var(--ok);
  border-color: var(--ok-line);
  background: var(--ok-soft);
}

.tp-ledger-text--muted {
  color: var(--text-2);
}

.tp-diff {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.tp-diff-meta {
  margin: 0;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--text-3);
}

.tp-diff-row {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 4px 10px;
  border-radius: var(--r-sm);
}

.tp-diff-row.changed {
  background: var(--white-veil);
}

.tp-diff-line {
  margin: 0;
  font-size: var(--fs-sm);
  line-height: 1.7;
  word-break: break-word;
}

.tp-diff-line--raw {
  color: var(--err);
  text-decoration: line-through;
  text-decoration-color: rgba(248, 113, 113, 0.5);
}

.tp-diff-line--fixed {
  color: var(--ok);
}

.tp-diff-line--same {
  color: var(--text-3);
}

/* ---------- 参数刻度行 ---------- */
.tp-ticks {
  display: flex;
  align-items: center;
  gap: 18px;
  flex-wrap: wrap;
  margin-top: 14px;
  padding-top: 12px;
  border-top: 1px solid var(--line-soft);
}

.tp-tick {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  cursor: pointer;
}

.tp-tick-key {
  font-family: var(--font-mono);
  font-size: 11px;
  letter-spacing: 0.18em;
  color: var(--accent);
  padding: 1px 6px;
  border: 1px solid var(--accent-line);
  border-radius: var(--r-sm);
  background: var(--accent-faint);
}

/* 下拉框：脱离原生渲染（原生白底会砸穿深色主题），自绘金色箭头 */
.tp-tick-select {
  appearance: none;
  -webkit-appearance: none;
  padding: 4px 26px 4px 10px;
  font-size: var(--fs-sm);
  font-family: var(--font-sans);
  color: var(--text-1);
  background-color: var(--bg-raise);
  border: 1px solid var(--line);
  border-radius: var(--r-sm);
  background-image: url("data:image/svg+xml;charset=utf-8,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='6' viewBox='0 0 10 6'%3E%3Cpath d='M1 1l4 4 4-4' fill='none' stroke='%23c9a227' stroke-width='1.5' stroke-linecap='round'/%3E%3C/svg%3E");
  background-repeat: no-repeat;
  background-position: right 9px center;
  cursor: pointer;
  transition: border-color var(--t-fast) var(--ease);
}

.tp-tick-select:hover:not(:disabled) {
  border-color: var(--accent-line);
}

.tp-tick-select:focus-visible {
  outline: none;
  border-color: var(--accent-line-strong);
  box-shadow: var(--focus-halo);
}

.tp-tick-select:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.tp-tick-select option {
  background: var(--bg-card);
  color: var(--text-1);
}

.tp-tick--toggle {
  user-select: none;
}

.tp-tick-box {
  appearance: none;
  -webkit-appearance: none;
  width: 14px;
  height: 14px;
  border: 1px solid var(--line-strong);
  border-radius: 4px;
  background: var(--bg-raise);
  cursor: pointer;
  display: grid;
  place-content: center;
}

.tp-tick-box:checked {
  background: var(--accent);
  border-color: var(--accent);
}

.tp-tick-box:checked::before {
  content: '';
  width: 8px;
  height: 8px;
  background: var(--accent-ink);
  clip-path: polygon(14% 44%, 0 65%, 50% 100%, 100% 16%, 80% 0%, 43% 62%);
}

.tp-tick-val {
  font-size: var(--fs-sm);
  color: var(--text-2);
}

.tp-tick-note {
  margin-left: auto;
  font-family: var(--font-mono);
  font-size: 11px;
  letter-spacing: 0.08em;
  color: var(--text-3);
}

/* ---------- 状态行 / 告警 ---------- */
.tp-status {
  display: flex;
  align-items: center;
  margin: 0;
  padding: 0 4px;
  font-family: var(--font-mono);
  font-size: var(--fs-sm);
  color: var(--info);
}

.tp-caret {
  display: inline-block;
  width: 8px;
  height: 15px;
  margin-left: 6px;
  background: var(--info);
  animation: tp-blink 1s steps(1) infinite;
}

@keyframes tp-blink {
  50% {
    opacity: 0;
  }
}

.tp-alert {
  display: flex;
  align-items: center;
  margin: 0;
  padding: 10px 14px;
  font-size: var(--fs-sm);
  color: var(--err);
  background: var(--err-soft);
  border: 1px solid var(--err-line);
  border-radius: var(--r-md);
}

/* ---------- 转写日志 ---------- */
.tp-ledger {
  display: flex;
  flex-direction: column;
  gap: 14px;
  padding: 18px 20px;
  background: var(--bg-card);
  border: 1px solid var(--accent-line);
  border-radius: var(--r-lg);
  box-shadow: var(--e-1);
}

.tp-ledger-head {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}

.tp-ledger-name {
  max-width: 300px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-family: var(--font-mono);
  font-size: var(--fs-sm);
  color: var(--text-1);
}

.tp-chip {
  padding: 2px 9px;
  font-family: var(--font-mono);
  font-size: 11px;
  letter-spacing: 0.06em;
  color: var(--text-2);
  border: 1px solid var(--line-strong);
  border-radius: var(--r-full);
}

.tp-chip--time {
  color: var(--accent-hi);
  border-color: var(--accent-line);
}

/* 缓存命中：斜盖的金色印章 */
.tp-stamp {
  padding: 2px 10px;
  font-family: var(--font-mono);
  font-size: 11px;
  letter-spacing: 0.22em;
  color: var(--accent);
  border: 1px solid var(--accent-line-strong);
  border-radius: 3px;
  transform: rotate(-2.4deg);
  box-shadow: var(--accent-halo);
}

.tp-copy {
  margin-left: auto;
}

.tp-ledger-text {
  margin: 0;
  padding: 4px 0 4px 18px;
  max-height: 420px;
  overflow-y: auto;
  font-size: var(--fs-md);
  line-height: 1.95;
  color: var(--text-1);
  white-space: pre-wrap;
  word-break: break-word;
  border-left: 2px solid var(--accent-line);
}

/* ---------- 时间轴 ---------- */
.tp-segments {
  font-size: var(--fs-sm);
}

.tp-segments summary {
  cursor: pointer;
  font-family: var(--font-mono);
  font-size: var(--fs-cap);
  letter-spacing: 0.14em;
  color: var(--text-3);
  user-select: none;
}

.tp-segments summary:hover {
  color: var(--accent);
}

.tp-seg-list {
  margin: 10px 0 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  max-height: 280px;
  overflow-y: auto;
}

.tp-seg {
  display: flex;
  align-items: baseline;
  gap: 10px;
  padding: 6px 0;
}

.tp-seg + .tp-seg {
  border-top: 1px solid var(--line-soft);
}

.tp-seg-time {
  flex-shrink: 0;
  min-width: 64px;
  text-align: right;
  font-family: var(--font-mono);
  font-size: var(--fs-cap);
  color: var(--accent);
  font-variant-numeric: tabular-nums;
}

/* 点线引导：时间码 → 文本 */
.tp-seg-lead {
  flex: 0 1 48px;
  border-bottom: 1px dotted var(--line-strong);
  transform: translateY(-3px);
}

.tp-seg-text {
  flex: 1;
  color: var(--text-2);
  word-break: break-word;
}

/* ---------- 空态提示 ---------- */
.tp-idle {
  display: flex;
  align-items: center;
  gap: 12px;
  margin: 0;
  padding: 0 6px;
  font-size: var(--fs-sm);
  line-height: 1.8;
  color: var(--text-3);
}

.tp-idle-mark {
  flex-shrink: 0;
  padding: 2px 8px;
  font-family: var(--font-mono);
  font-size: 11px;
  letter-spacing: 0.24em;
  color: var(--err);
  border: 1px solid var(--err-line);
  border-radius: 3px;
  animation: tp-rec 2.4s var(--ease) infinite;
}

@keyframes tp-rec {
  0%, 100% {
    opacity: 1;
  }
  50% {
    opacity: 0.45;
  }
}

/* ---------- 历史架 ---------- */
.tp-rack {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.tp-rack-title {
  margin: 0 0 2px;
  font-family: var(--font-mono);
  font-size: var(--fs-cap);
  font-weight: 400;
  letter-spacing: 0.22em;
  color: var(--text-3);
}

.tp-rack-row {
  display: flex;
  align-items: baseline;
  gap: 12px;
  padding: 9px 14px;
  text-align: left;
  background: transparent;
  border: 1px solid transparent;
  border-left: 2px solid var(--line);
  border-radius: var(--r-sm);
  cursor: pointer;
  transition: background var(--t-fast) var(--ease), border-color var(--t-fast) var(--ease);
}

.tp-rack-row:hover {
  background: var(--white-veil);
  border-left-color: var(--accent);
}

.tp-rack-row.is-active {
  background: var(--accent-veil);
  border-color: var(--accent-line);
  border-left: 2px solid var(--accent);
}

.tp-rack-idx {
  flex-shrink: 0;
  font-family: var(--font-mono);
  font-size: var(--fs-cap);
  color: var(--accent);
  font-variant-numeric: tabular-nums;
}

.tp-rack-name {
  flex-shrink: 0;
  max-width: 200px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--fs-sm);
  color: var(--text-1);
}

.tp-rack-eng {
  flex-shrink: 0;
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--text-3);
  border: 1px solid var(--line-strong);
  padding: 0 6px;
  border-radius: var(--r-full);
}

.tp-rack-preview {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--fs-cap);
  color: var(--text-3);
}

.tp-rack-dur {
  flex-shrink: 0;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--text-3);
  font-variant-numeric: tabular-nums;
}

/* ---------- 宽屏：放开列宽，波形加高，标题加大 ---------- */
@media (min-width: 1500px) {
  .tp {
    max-width: 1080px;
  }

  .tp-title {
    font-size: 52px;
  }

  .tp-wave {
    height: 56px;
  }

  .tp-wave-bar {
    width: 4px;
  }
}

/* ---------- 窄面板兜底 ---------- */
@media (max-width: 720px) {
  .tp {
    --tp-pad: 18px;
  }

  .tp-rail {
    flex-direction: column;
  }

  .tp-engines {
    grid-template-columns: 1fr;
  }

  .tp-path {
    padding: 8px 12px;
  }

  .tp-tick-note {
    margin-left: 0;
  }

  .tp-title {
    font-size: 34px;
  }
}
</style>
