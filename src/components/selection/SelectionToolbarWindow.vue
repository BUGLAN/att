<template>
  <div class="selection-shell">
    <section class="selection-panel">
      <header class="selection-header">
        <div>
          <span class="selection-kicker">AI ACTION CENTER</span>
          <h1 class="selection-title">划词动作中心</h1>
        </div>
        <div class="selection-header-actions">
          <button
            class="selection-icon-button"
            :class="{ active: isPinned }"
            type="button"
            :title="isPinned ? '取消固定' : '固定窗口'"
            @click="togglePin"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path
                d="M8.5 4.5h7l-1 5 3 3v1h-4v5l-1.5 1.5-1.5-1.5v-5h-4v-1l3-3-1-5Z"
                fill="currentColor"
              />
            </svg>
          </button>
          <button class="selection-icon-button" type="button" title="关闭" @click="hideToolbar">
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path
                d="M6.53 5.47a.75.75 0 0 1 1.06 0L12 9.88l4.41-4.41a.75.75 0 1 1 1.06 1.06L13.06 11l4.41 4.41a.75.75 0 0 1-1.06 1.06L12 12.06l-4.41 4.41a.75.75 0 1 1-1.06-1.06L10.94 11 6.53 6.59a.75.75 0 0 1 0-1.12Z"
                fill="currentColor"
              />
            </svg>
          </button>
        </div>
      </header>

      <div class="selection-source">
        <span class="selection-source-label">当前选中</span>
        <p>{{ selectedText || '等待选区...' }}</p>
      </div>

      <div class="selection-actions">
        <button
          v-for="action in actions"
          :key="action.value"
          class="action-button"
          :class="{ active: action.value === activeAction, loading: isLoading && action.value === activeAction }"
          type="button"
          :disabled="!selectedText || isLoading"
          @click="runAction(action.value)"
        >
          {{ action.label }}
        </button>
      </div>

      <div v-if="errorMessage" class="selection-error">
        {{ errorMessage }}
      </div>

      <div v-if="isLoading" class="selection-loading">
        <span class="loading-spinner" aria-hidden="true" />
        <div class="loading-copy">
          <strong>{{ loadingTitle }}</strong>
          <p>{{ loadingDescription }}</p>
        </div>
      </div>

      <div v-if="result" class="selection-result-region">
        <SelectionResultCard
          :result="result"
          :show-examples="showExamples"
        />
      </div>
    </section>
  </div>
</template>

<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useElectron } from '@/composables/useElectron'
import SelectionResultCard from '@/components/selection/SelectionResultCard.vue'

const { api, removeAllListeners } = useElectron()
const COMPACT_WINDOW_HEIGHT = 320
const EXPANDED_WINDOW_HEIGHT = 520

const selectedText = ref('')
const activeAction = ref('translate')
const isLoading = ref(false)
const errorMessage = ref('')
const result = ref(null)
const showExamples = ref(true)
const autoAddToWordbook = ref(true)
const isPinned = ref(false)

const actions = [
  { value: 'translate', label: '翻译' },
  { value: 'explain', label: '解释' },
  { value: 'summarize', label: '总结' },
  { value: 'rewrite', label: '改写' }
]

function hideToolbar() {
  window.electronAPI?.hideSelectionToolbar?.()
}

async function togglePin() {
  try {
    const response = await window.electronAPI?.toggleSelectionToolbarPin?.({
      pinned: !isPinned.value
    })
    if (response?.success) {
      isPinned.value = response.pinned === true
    }
  } catch (error) {
    console.error('[SelectionToolbar] Failed to toggle pin:', error)
  }
}

function syncWindowHeight(height) {
  window.electronAPI?.resizeSelectionToolbar?.({
    height
  })
}

async function loadSettings() {
  try {
    const settings = await api.getSettings()
    showExamples.value = settings.selection_toolbar_show_examples !== false
    autoAddToWordbook.value = settings.selection_toolbar_auto_add_to_wordbook !== false
  } catch (error) {
    console.error('[SelectionToolbar] Failed to load settings:', error)
  }
}

async function runAction(actionType) {
  if (!selectedText.value || isLoading.value) {
    return
  }

  isLoading.value = true
  activeAction.value = actionType
  errorMessage.value = ''
  await nextTick()
  await new Promise((resolve) => {
    if (typeof requestAnimationFrame === 'function') {
      requestAnimationFrame(() => resolve())
      return
    }
    setTimeout(resolve, 0)
  })

  try {
    const payload = {
      source_text: selectedText.value,
      auto_add_to_wordbook: autoAddToWordbook.value
    }
    if (actionType === 'translate') {
      result.value = await api.runAITranslate(payload)
    } else if (actionType === 'explain') {
      result.value = await api.runAIExplain(payload)
    } else if (actionType === 'summarize') {
      result.value = await api.runAISummarize(payload)
    } else if (actionType === 'rewrite') {
      result.value = await api.runAIRewrite(payload)
    }
  } catch (error) {
    console.error('[SelectionToolbar] Failed to run action:', error)
    errorMessage.value = error.message || 'AI 动作执行失败'
  } finally {
    isLoading.value = false
  }
}

onMounted(async () => {
  await loadSettings()

  window.electronAPI?.onSelectionToolbarActivate?.((payload) => {
    selectedText.value = payload?.text || ''
    activeAction.value = 'translate'
    errorMessage.value = ''
    result.value = null
    isLoading.value = false
    syncWindowHeight(COMPACT_WINDOW_HEIGHT)
  })

  window.electronAPI?.onSelectionToolbarPinnedChanged?.((payload) => {
    isPinned.value = payload?.pinned === true
  })

  await nextTick()
  syncWindowHeight(COMPACT_WINDOW_HEIGHT)
})

onUnmounted(() => {
  removeAllListeners('selection-toolbar:activate')
  removeAllListeners('selection-toolbar:pinned-changed')
})

const LOADING_TITLES = {
  translate: '正在翻译',
  explain: '正在解释',
  summarize: '正在总结',
  rewrite: '正在改写'
}

const LOADING_DESCRIPTIONS = {
  translate: '正在生成更准确的翻译结果，请稍候。',
  explain: '正在结合上下文解释含义，请稍候。',
  summarize: '正在压缩关键信息，请稍候。',
  rewrite: '正在调整表达方式，请稍候。'
}

const loadingTitle = computed(() => LOADING_TITLES[activeAction.value] || '正在处理')
const loadingDescription = computed(() => LOADING_DESCRIPTIONS[activeAction.value] || '请稍候。')
const shouldUseExpandedWindow = computed(() => isLoading.value || !!errorMessage.value || !!result.value)

watch(shouldUseExpandedWindow, (expanded) => {
  syncWindowHeight(expanded ? EXPANDED_WINDOW_HEIGHT : COMPACT_WINDOW_HEIGHT)
}, { immediate: false })
</script>

<style scoped>
.selection-shell {
  height: 100vh;
  padding: 10px;
  overflow: hidden;
}

.selection-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
  padding: 16px;
  border-radius: 24px;
  background:
    linear-gradient(180deg, rgba(26, 21, 18, 0.95), rgba(16, 14, 12, 0.98));
  color: #f7ecdf;
  box-shadow: 0 28px 64px rgba(0, 0, 0, 0.34);
  overflow: hidden;
}

.selection-header {
  display: flex;
  justify-content: space-between;
  gap: 14px;
  align-items: start;
  -webkit-app-region: drag;
}

.selection-header-actions {
  display: flex;
  gap: 8px;
  align-items: center;
}

.selection-kicker {
  display: inline-flex;
  color: #f3b460;
  font-size: 0.72rem;
  letter-spacing: 0.16em;
}

.selection-title {
  margin-top: 8px;
  font-size: 1.16rem;
}

.selection-icon-button {
  width: 32px;
  height: 32px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: none;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.08);
  color: inherit;
  cursor: pointer;
  -webkit-app-region: no-drag;
}

.selection-icon-button svg {
  width: 16px;
  height: 16px;
}

.selection-icon-button.active {
  background: rgba(243, 180, 96, 0.22);
  color: #ffd9a5;
}

.selection-source {
  margin-top: 16px;
  padding: 14px;
  border-radius: 18px;
  background: rgba(255, 255, 255, 0.05);
}

.selection-source-label {
  display: inline-flex;
  color: rgba(247, 236, 223, 0.6);
  font-size: 0.76rem;
  margin-bottom: 8px;
}

.selection-source p {
  line-height: 1.7;
  color: #fff6ee;
  max-height: 88px;
  overflow-y: auto;
  scrollbar-width: thin;
  scrollbar-color: rgba(243, 180, 96, 0.28) transparent;
}

.selection-actions {
  display: flex;
  gap: 10px;
  flex-wrap: wrap;
  margin-top: 16px;
  margin-bottom: 16px;
}

.action-button {
  padding: 9px 14px;
  border-radius: 999px;
  border: 1px solid rgba(255, 255, 255, 0.1);
  background: rgba(255, 255, 255, 0.06);
  color: inherit;
  cursor: pointer;
  -webkit-app-region: no-drag;
}

.action-button.active {
  background: rgba(243, 180, 96, 0.18);
  border-color: rgba(243, 180, 96, 0.36);
  color: #ffd9a5;
}

.action-button.loading {
  opacity: 0.72;
}

.selection-error {
  margin-bottom: 14px;
  padding: 12px 14px;
  border-radius: 14px;
  background: rgba(212, 93, 93, 0.14);
  color: #ffc6c6;
}

.selection-loading {
  display: flex;
  gap: 12px;
  align-items: center;
  margin-bottom: 14px;
  padding: 14px;
  border-radius: 16px;
  background: rgba(255, 255, 255, 0.07);
  border: 1px solid rgba(243, 180, 96, 0.14);
}

.selection-result-region {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}

.selection-source p::-webkit-scrollbar {
  width: 6px;
}

.selection-source p::-webkit-scrollbar-track {
  background: transparent;
}

.selection-source p::-webkit-scrollbar-thumb {
  border-radius: 999px;
  background: rgba(243, 180, 96, 0.22);
}

.selection-source p::-webkit-scrollbar-thumb:hover {
  background: rgba(243, 180, 96, 0.32);
}

.loading-spinner {
  width: 18px;
  height: 18px;
  border-radius: 999px;
  border: 2px solid rgba(243, 180, 96, 0.18);
  border-top-color: #f3b460;
  animation: selection-spin 0.8s linear infinite;
  flex: 0 0 auto;
}

.loading-copy strong {
  display: block;
  color: #fff4e6;
}

.loading-copy p {
  margin-top: 4px;
  color: rgba(247, 236, 223, 0.72);
  line-height: 1.5;
}

@keyframes selection-spin {
  from {
    transform: rotate(0deg);
  }

  to {
    transform: rotate(360deg);
  }
}
</style>
