<template>
  <section class="tp">
    <!-- ============ 页头：眉题 + 大标题，入场纵向错落 ============ -->
    <header class="tp-head tp-reveal" style="--d: 0">
      <p class="tp-eyebrow">
        <span class="tp-eyebrow-rule" aria-hidden="true"></span>
        LOCAL FILE · 本地文件转写台
        <span class="tp-eyebrow-rule" aria-hidden="true"></span>
      </p>
      <h2 class="tp-title">转文字</h2>
      <p class="tp-sub">
        丢进一个音视频文件，直接换回纯文本。视频自动抽音轨；同一份内容转过的，秒回缓存；长文件自动进后台任务。
      </p>
    </header>

    <!-- ============ 控制台：波形 + 文件轨 + 参数 ============ -->
    <section class="tp-console tp-reveal" style="--d: 1" aria-label="转写控制台">
      <!-- 波形母题：空闲时是静采样，转写时跳动，完成时归于一线 -->
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

        <span class="tp-tick-note">输出：纯文本 · 不入记录集 · 按内容缓存</span>
      </div>

      <!-- 状态行：等宽 + 块状光标；错误走控制台告警 -->
      <p v-if="isBusy" class="tp-status" role="status">
        {{ statusText || '正在转写…' }}<span class="tp-caret" aria-hidden="true"></span>
      </p>
      <p v-else-if="error" class="tp-alert" role="alert">{{ error }}</p>
    </section>

    <!-- ============ 转写结果：磁带日志 ============ -->
    <article v-if="result" class="tp-ledger tp-reveal" style="--d: 2">
      <header class="tp-ledger-head">
        <span class="tp-ledger-name" :title="result.path">{{ result.fileName || '未命名文件' }}</span>
        <span class="tp-chip tp-chip--time">{{ formatDuration(result.durationSeconds) }}</span>
        <span class="tp-chip">{{ result.engine || '本地引擎' }}</span>
        <span v-if="result.cached" class="tp-stamp">缓存命中</span>

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

      <p class="tp-ledger-text">{{ result.text || '（没有识别到语音内容）' }}</p>

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
      支持 mp4 / flv / webm / mkv / wav / mp3 / m4a / flac / ogg 等常见容器；结果只在这里，不会进入记录集。
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
        <span class="tp-rack-preview">{{ item.text.slice(0, 60) || '（空）' }}</span>
        <span class="tp-rack-dur">{{ formatDuration(item.durationSeconds) }}</span>
      </button>
    </aside>
  </section>
</template>

<script setup>
import { computed, ref } from 'vue'
import { useAppStore } from '@/stores/app'
import { useFileTranscribe } from '@/composables/useFileTranscribe'
import { formatDuration } from '@/composables/transcribe/fileTranscribeModel.mjs'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseInput from '@/components/common/BaseInput.vue'

const appStore = useAppStore()
const {
  filePath,
  language,
  wantSegments,
  isBusy,
  statusText,
  error,
  result,
  history,
  chooseFile,
  startTranscribe,
  copyResult,
  selectHistoryItem
} = useFileTranscribe()

const copied = ref(false)

// 波形条：固定 32 根，种子化的伪随机高度，保证每次进来形状一致
const WAVE_SEED = [38, 62, 24, 80, 46, 30, 72, 55, 20, 88, 40, 64, 28, 76, 34, 58,
  18, 70, 44, 26, 82, 36, 60, 22, 90, 48, 32, 74, 42, 56, 30, 66]
const waveBars = WAVE_SEED

const waveMode = computed(() => {
  if (isBusy.value) return 'is-live'
  if (result.value) return 'is-done'
  return 'is-idle'
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
  max-width: 880px;
  padding: var(--tp-pad) var(--tp-pad) 48px;
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
  max-width: 560px;
  font-size: var(--fs-sm);
  line-height: 1.8;
  color: var(--text-3);
}

/* ---------- 控制台卡 ---------- */
.tp-console {
  position: relative;
  display: flex;
  flex-direction: column;
  padding: 20px 22px 18px;
  background: var(--grad-card);
  border: 1px solid var(--line);
  border-radius: var(--r-lg);
  box-shadow: var(--e-2), var(--edge-hi);
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

/* 卡面顶部一层极淡的斜向金光 */
.tp-console > .tp-wave {
  margin-top: -2px;
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

/* 转写中：逐根错拍跳动 */
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

.tp-path :deep(input)::placeholder {
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

.tp-tick-select {
  padding: 3px 8px;
  font-size: var(--fs-sm);
  color: var(--text-1);
  background: var(--bg-raise);
  border: 1px solid var(--line);
  border-radius: var(--r-sm);
  cursor: pointer;
}

.tp-tick-select:focus-visible {
  outline: none;
  box-shadow: var(--focus-ring);
}

.tp-tick--toggle {
  user-select: none;
}

.tp-tick-box {
  appearance: none;
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
  margin: 14px 0 0;
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
  margin: 14px 0 0;
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

/* ---------- 窄面板兜底 ---------- */
@media (max-width: 720px) {
  .tp {
    --tp-pad: 18px;
  }

  .tp-rail {
    flex-direction: column;
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
