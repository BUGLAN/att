<template>
  <div class="settings-stack">
    <!-- 引擎与录音模式放在一起选：流式只有豆包支持，切引擎时能立刻看到哪些模式可用 -->
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">识别引擎</h3>
        <div class="choice-grid" role="radiogroup" aria-label="识别引擎">
          <button
            v-for="engine in engines"
            :key="engine.value"
            type="button"
            role="radio"
            class="choice-card engine-card"
            :class="{ 'is-loading': engine.loading }"
            :aria-checked="form.asr_provider === engine.value ? 'true' : 'false'"
            :aria-busy="engine.loading ? 'true' : undefined"
            @click="selectEngine(engine.value)"
          >
            <span class="choice-title">{{ engine.label }}</span>
            <!-- 状态只用一个小圆点 + 灰字，不做成像按钮的徽章 -->
            <span class="engine-meta" :class="`is-${engine.tone}`">
              <span class="engine-dot" aria-hidden="true"></span>{{ engine.chip }}
            </span>
            <span class="choice-desc">{{ engine.desc }}</span>
            <!-- 加载中：卡片底边一道扫过的光带，详细状态见下方引擎配置区的「模型状态」 -->
            <span v-if="engine.loading" class="engine-card-progress" aria-hidden="true"></span>
          </button>
        </div>
      </div>

      <div class="settings-group">
        <h3 class="settings-group-title">录音模式</h3>
        <div class="choice-grid" role="radiogroup" aria-label="录音模式">
          <button
            v-for="mode in recordingModes"
            :key="mode.value"
            type="button"
            role="radio"
            class="choice-card"
            :class="{ 'is-muted': mode.disabled }"
            :aria-checked="form.recording_mode === mode.value ? 'true' : 'false'"
            :aria-disabled="mode.disabled ? 'true' : undefined"
            :title="mode.hint || undefined"
            @click="selectRecordingMode(mode)"
          >
            <span class="choice-title">{{ mode.label }}</span>
            <span class="choice-desc">{{ mode.disabled ? mode.hint : mode.desc }}</span>
          </button>
        </div>

        <div v-if="modeConflict" class="settings-note warn">
          <span>{{ modeConflict }}</span>
        </div>
        <div v-else-if="form.recording_mode === 'streaming' && streamingRuntimeSummary" class="settings-note warn">
          <span>{{ streamingRuntimeSummary }}</span>
        </div>

        <div v-if="form.recording_mode === 'multi'" class="setting-item settings-advanced">
          <div class="setting-info">
            <span class="setting-label">静音分段阈值</span>
            <span class="setting-desc">停顿超过该秒数即切段。</span>
          </div>
          <BaseInput v-model.number="form.silence_threshold" type="number" min="0.5" max="5" step="0.5" aria-label="静音分段阈值" />
        </div>
      </div>
    </section>

    <!-- 豆包语音（火山引擎 · 在线流式）：Access Token 只写不读，后端加密落盘 -->
    <section v-if="form.asr_provider === 'doubao_asr'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">豆包语音 · 接入配置</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">Access Token</span>
            <span class="setting-desc">
              {{ form.doubao_asr_api_key_set ? '已保存（留空表示不修改）' : '火山语音技术控制台的 Access Token（不是方舟 API Key）。' }}
            </span>
          </div>
          <BaseInput v-model.trim="form.doubao_asr_api_key" type="password" autocomplete="new-password" placeholder="请输入 Access Token" aria-label="Access Token" />
        </div>
        <div v-if="doubaoNote" class="settings-note" :class="doubaoNote.tone" role="status">
          <span>{{ doubaoNote.text }}</span>
        </div>
        <div v-if="form.doubao_asr_api_key_set" class="setting-item">
          <div class="setting-info">
            <span class="setting-label">清除已保存的 Token</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.doubao_asr_api_key_clear" type="checkbox" aria-label="清除已保存的 Token">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">Resource ID</span>
            <span class="setting-desc">与控制台开通的产品一致。</span>
          </div>
          <BaseSelect v-model="form.doubao_asr_resource_id" aria-label="Resource ID">
            <option value="volc.seedasr.sauc.duration">volc.seedasr.sauc.duration（ASR 2.0 · 时长）</option>
            <option value="volc.seedasr.sauc.concurrent">volc.seedasr.sauc.concurrent（ASR 2.0 · 并发）</option>
            <option value="volc.bigasr.sauc.duration">volc.bigasr.sauc.duration（ASR 1.0 · 时长）</option>
            <option value="volc.bigasr.sauc.concurrent">volc.bigasr.sauc.concurrent（ASR 1.0 · 并发）</option>
          </BaseSelect>
        </div>
        <details class="settings-advanced">
          <summary>高级：接入点、强制断句静音时长</summary>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">接入点</span>
              <span class="setting-desc">默认无需修改。</span>
            </div>
            <BaseInput v-model.trim="form.doubao_asr_endpoint" type="text" placeholder="wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async" aria-label="接入点" />
          </div>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">强制断句静音时长</span>
              <span class="setting-desc">毫秒；0 跟随服务端。</span>
            </div>
            <BaseInput v-model.number="form.doubao_asr_end_window_size" type="number" min="0" max="10000" step="100" aria-label="强制断句静音时长" />
          </div>
        </details>
      </div>
    </section>

    <!-- ChatGPT 听写（在线批量）：访问令牌只写不读，后端加密落盘 -->
    <section v-if="form.asr_provider === 'chatgpt_asr'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">ChatGPT 听写 · 接入配置</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">访问令牌</span>
            <span class="setting-desc">
              {{ form.chatgpt_asr_api_key_set ? '已保存（留空表示不修改）' : 'ChatGPT 网页登录的 access token（不是 API Key）。' }}
            </span>
          </div>
          <BaseInput v-model.trim="form.chatgpt_asr_api_key" type="password" autocomplete="new-password" placeholder="粘贴 ChatGPT 访问令牌（可带 Bearer 前缀）" aria-label="访问令牌" />
        </div>
        <div v-if="chatgptNote" class="settings-note" :class="chatgptNote.tone" role="status">
          <span>{{ chatgptNote.text }}</span>
        </div>
        <div v-if="form.chatgpt_asr_api_key_set" class="setting-item">
          <div class="setting-info">
            <span class="setting-label">清除已保存的令牌</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.chatgpt_asr_api_key_clear" type="checkbox" aria-label="清除已保存的令牌">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">请求超时</span>
            <span class="setting-desc">秒。</span>
          </div>
          <BaseInput v-model.number="form.chatgpt_asr_timeout_seconds" type="number" min="10" max="600" step="10" aria-label="请求超时" />
        </div>
        <details class="settings-advanced">
          <summary>高级：端点</summary>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">端点</span>
              <span class="setting-desc">默认无需修改。</span>
            </div>
            <BaseInput v-model.trim="form.chatgpt_asr_endpoint" type="text" placeholder="https://chatgpt.com/backend-api/transcribe" aria-label="端点" />
          </div>
        </details>
      </div>
    </section>

    <section v-if="form.asr_provider === 'whisper'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">faster-whisper · 本地模型</h3>
        <LocalModelStatus :view="localView" />
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">模型</span>
            <span class="setting-desc">越大越准，也越慢。</span>
          </div>
          <BaseSelect v-model="form.whisper_model" aria-label="Whisper 模型">
            <option value="tiny">tiny</option>
            <option value="base">base</option>
            <option value="small">small</option>
            <option value="medium">medium</option>
            <option value="large-v3">large-v3</option>
          </BaseSelect>
        </div>
      </div>
    </section>

    <section v-if="form.asr_provider === 'qwen_asr'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">Qwen3-ASR · 本地模型</h3>
        <LocalModelStatus :view="localView" />
        <!-- 模型目录自动查找（models/ 下的 Qwen3-ASR ONNX），推理固定走 CPU；
             旧版的设备 / dtype / 时间戳 / 强制对齐器选项在 Rust 引擎里没有作用，已从界面移除（字段保留兼容旧数据） -->
        <details class="settings-advanced">
          <summary>高级：输出长度、音频长度上限</summary>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">最大输出 token</span>
            </div>
            <BaseInput v-model.number="form.qwen_asr_max_new_tokens" type="number" min="32" max="2048" step="32" aria-label="最大输出 token" />
          </div>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">音频长度上限</span>
              <span class="setting-desc">秒。</span>
            </div>
            <BaseInput v-model.number="form.qwen_asr_max_audio_seconds" type="number" min="5" max="600" step="5" aria-label="音频长度上限" />
          </div>
        </details>
      </div>
    </section>

    <section class="settings-section">
      <div class="settings-group">
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">识别语言</span>
          </div>
          <BaseSelect v-model="form.asr_language" aria-label="识别语言">
            <option value="zh">中文</option>
            <option value="en">英文</option>
            <option value="ja">日文</option>
            <option value="ko">韩文</option>
          </BaseSelect>
        </div>
      </div>
    </section>

    <!-- 热词：词表与引擎无关地保存，只有部分引擎能用；状态圆点与引擎卡片同一套语义 -->
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">热词</h3>
        <BaseTextarea
          v-model="form.asr_hotwords_text"
          class="hotword-input"
          rows="6"
          spellcheck="false"
          :placeholder="'每行一个，例如：\nTauri\n落地页\nOption + V'"
          aria-label="热词"
        />
        <!-- 两路各一行：识别随引擎变化，LLM 纠错随模型配置变化 -->
        <ul class="hotword-status" aria-label="热词生效情况">
          <li v-for="row in hotwords.rows" :key="row.key" class="hotword-row">
            <span class="engine-meta hotword-row-label" :class="`is-${row.tone}`">
              <span class="engine-dot" aria-hidden="true"></span>{{ row.label }}
            </span>
            <span class="hotword-row-text" :class="{ 'is-warn': row.tone === 'warn' }">{{ row.text }}</span>
          </li>
        </ul>
      </div>
    </section>
  </div>
</template>

<script setup>
import { computed, inject, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import BaseInput from '@/components/common/BaseInput.vue'
import BaseSelect from '@/components/common/BaseSelect.vue'
import BaseTextarea from '@/components/common/BaseTextarea.vue'
import { useAppStore } from '@/stores/app'
import LocalModelStatus from '../LocalModelStatus.vue'
import { useSettingsStore } from '@/stores/settings'
import { SETTINGS_CTX } from '../settingsContext.js'
import {
  localEngineView,
  recordingModeConflict,
  recordingModeOptions
} from '../asrEngineStatus.mjs'
import { hotwordView, parseHotwords } from '../asrHotwords.mjs'
import { credentialNote, isOnlineProvider, onlineEngineChip } from '../onlineEngineStatus.mjs'
import { useAsrCredentialCheck } from '@/composables/useAsrCredentialCheck'

const { form, streamingRuntimeSummary } = inject(SETTINGS_CTX)
const appStore = useAppStore()
const settingsStore = useSettingsStore()
const credential = useAsrCredentialCheck()

const LOCAL_STATUS_POLL_MS = 1000

const recordingModes = computed(() => recordingModeOptions(form.value.asr_provider))
const modeConflict = computed(() => recordingModeConflict(form.value.asr_provider, form.value.recording_mode))
// LLM 模型列表没加载过时按「有模型」处理，免得误报（同输出页）；挂载时补拉一次
const llmLoaded = ref(false)
const hotwords = computed(() => hotwordView(form.value.asr_provider, parseHotwords(form.value.asr_hotwords_text), {
  hasLlmModel: !llmLoaded.value || settingsStore.llmModels.length > 0,
  autoOptimize: Boolean(form.value.auto_optimize)
}))

onMounted(() => {
  settingsStore.loadLLMState().catch(() => {}).finally(() => {
    llmLoaded.value = true
  })
})
// 「提前加载 / 重试」请求进行中：后端可能还没来得及置位 loading，前端先按加载中展示
const isLoadingModel = ref(false)
const localView = computed(() => {
  const provider = form.value.asr_provider
  const status = isLoadingModel.value ? { provider, loading: true } : settingsStore.runtimeStatus.local_engine
  return localEngineView(provider, status)
})

// 点引擎卡片：切换引擎；再点已选中的本地引擎 = 提前加载（未加载时）或重试（加载失败时），界面上不另放按钮
function selectEngine(value) {
  if (form.value.asr_provider === value && ['idle', 'failed'].includes(localView.value.state)) {
    loadLocalModel()
    return
  }
  // 再点已选中、已填凭据的在线引擎 = 重新验证（与本地引擎「再点一下重试」同一个手势）
  if (form.value.asr_provider === value && isOnlineProvider(value) && onlineKeySet(value)) {
    credential.verify(value)
    return
  }
  form.value.asr_provider = value
}

function onlineKeySet(provider) {
  return provider === 'doubao_asr' ? Boolean(form.value.doubao_asr_api_key_set) : Boolean(form.value.chatgpt_asr_api_key_set)
}

// 在线引擎：卡片状态与接入配置区的提示都跟随凭据验证结论
function onlineChip(provider) {
  return onlineEngineChip({ keySet: onlineKeySet(provider), check: credential.checks[provider] || null })
}

const doubaoNote = computed(() => (form.value.doubao_asr_api_key_set ? credentialNote(credential.checks.doubao_asr) : null))
const chatgptNote = computed(() => (form.value.chatgpt_asr_api_key_set ? credentialNote(credential.checks.chatgpt_asr) : null))

// 进入本页时：当前在线引擎没有「可用」结论就补验一次（上次没验成、验证失败后在控制台改过开通等）
onMounted(async () => {
  const provider = form.value.asr_provider
  if (!isOnlineProvider(provider) || !onlineKeySet(provider)) {
    return
  }
  const check = await credential.load(provider)
  if (check?.status !== 'ok' && check?.status !== 'checking') {
    credential.verify(provider)
  }
})

function selectRecordingMode(mode) {
  if (mode.disabled) {
    appStore.showToast(`${mode.hint}，请先切换到豆包语音`)
    return
  }
  form.value.recording_mode = mode.value
}

// 切到本地引擎后后端在后台预加载：加载中每秒刷新一次状态，结束（就绪/失败/缺模型）即停
let pollTimer = null

function stopPolling() {
  if (pollTimer) {
    window.clearInterval(pollTimer)
    pollTimer = null
  }
}

watch(() => localView.value.state === 'loading', (loading) => {
  if (!loading) {
    stopPolling()
    return
  }
  if (pollTimer) {
    return
  }
  pollTimer = window.setInterval(() => {
    settingsStore.loadRuntimeStatus(false).catch(() => {
      // 单次探测失败不打断轮询：下一拍再试
    })
  }, LOCAL_STATUS_POLL_MS)
}, { immediate: true })

onBeforeUnmount(stopPolling)

async function loadLocalModel() {
  if (isLoadingModel.value) {
    return
  }
  isLoadingModel.value = true
  try {
    await settingsStore.startStreamingService(true)
    await settingsStore.loadRuntimeStatus(false)
  } catch (error) {
    appStore.showToast(error.message || '模型加载失败')
    await settingsStore.loadRuntimeStatus(false).catch(() => {})
  } finally {
    isLoadingModel.value = false
  }
}

// 本地引擎的卡片徽章跟随加载状态（只描述当前选中的那个；其余显示「本地」）
function localChip(value) {
  if (form.value.asr_provider !== value || localView.value.state === 'none') {
    return { chip: '本地', tone: 'neutral', loading: false }
  }
  return {
    chip: `本地 · ${localView.value.chip}`,
    // 加载中用强调金，与卡片底边的光带同色
    tone: localView.value.state === 'loading' ? 'accent' : localView.value.tone,
    loading: localView.value.state === 'loading'
  }
}

// 本地引擎（faster-whisper / Qwen3-ASR）在 Tauri 版已接入：进程内推理，模型目录见设置文档
const engines = computed(() => [
  {
    value: 'doubao_asr',
    label: '豆包语音',
    ...onlineChip('doubao_asr'),
    desc: '单段 / 多段 / 流式'
  },
  {
    value: 'chatgpt_asr',
    label: 'ChatGPT 听写',
    ...onlineChip('chatgpt_asr'),
    desc: '单段 / 多段'
  },
  {
    value: 'whisper',
    label: 'faster-whisper',
    ...localChip('whisper'),
    desc: '离线，需安装模型'
  },
  {
    value: 'qwen_asr',
    label: 'Qwen3-ASR',
    ...localChip('qwen_asr'),
    desc: '离线，需安装模型'
  }
])
</script>

<style scoped>
/* 选择器带上父级：与 BaseTextarea 自身的 scoped 样式同权重时谁后加载谁赢，这里要稳定压过它 */
.settings-group .hotword-input {
  font-size: var(--fs-sm);
  line-height: 1.7;
}

/* 生效情况：左列是「通道 + 状态圆点」，右列是说明；窄屏时说明换到下一行 */
.hotword-status {
  display: grid;
  gap: var(--s-2);
  margin: var(--s-3) 0 0;
  padding: var(--s-3);
  list-style: none;
  border: 1px solid var(--line);
  border-radius: var(--r-sm);
  background: var(--bg-raise);
}

.hotword-row {
  display: grid;
  grid-template-columns: minmax(132px, max-content) 1fr;
  align-items: baseline;
  gap: var(--s-1) var(--s-3);
}

.hotword-row-label {
  color: var(--text-2);
  white-space: nowrap;
}

.hotword-row-text {
  font-size: var(--fs-cap);
  color: var(--text-3);
  line-height: 1.6;
}

.hotword-row-text.is-warn {
  color: var(--text-2);
}

@media (max-width: 560px) {
  .hotword-row {
    grid-template-columns: 1fr;
  }
}

.engine-card {
  position: relative;
  overflow: hidden;
}

/* 选中的本地引擎加载中：底边 2px 金色光带来回扫过 */
.engine-card-progress {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 2px;
  background: var(--accent-faint);
  overflow: hidden;
}

.engine-card-progress::after {
  content: '';
  position: absolute;
  top: 0;
  bottom: 0;
  width: 40%;
  background: linear-gradient(90deg, transparent, var(--accent), transparent);
  animation: engine-sweep 1.4s var(--ease) infinite;
}

.engine-meta {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--fs-cap);
  color: var(--text-3);
  --dot: var(--text-3);
}

.engine-meta.is-ok { --dot: var(--ok); }
.engine-meta.is-warn { --dot: var(--warn); }
.engine-meta.is-err { --dot: var(--err); }
.engine-meta.is-accent { --dot: var(--accent); }

.engine-dot {
  flex: none;
  width: 6px;
  height: 6px;
  border-radius: var(--r-xs);
  background: var(--dot);
}

.engine-meta.is-accent .engine-dot {
  animation: engine-pulse 1.4s var(--ease) infinite;
}

@keyframes engine-pulse {
  50% { opacity: 0.35; }
}

@keyframes engine-sweep {
  from { left: -40%; }
  to { left: 100%; }
}

@media (prefers-reduced-motion: reduce) {
  .engine-meta.is-accent .engine-dot { animation: none; }
  .engine-card-progress::after { animation: none; left: 0; width: 100%; opacity: 0.5; }
}
</style>
