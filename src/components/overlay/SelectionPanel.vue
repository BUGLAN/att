<template>
  <div class="selection-shell">
    <section class="selection-panel">
      <header class="selection-toolbar">
        <nav class="selection-tabs" aria-label="划词动作">
          <button
            v-for="tab in tabs"
            :key="tab.key"
            class="selection-tab"
            :class="{ active: tab.key === currentTab }"
            type="button"
            @click="handleTabClick(tab.key)"
          >
            <svg
              v-if="tab.key === 'selected'"
              class="selection-tab-icon"
              viewBox="0 0 24 24"
              aria-hidden="true"
            >
              <path
                d="M5 5.75A2.75 2.75 0 0 1 7.75 3h8.5A2.75 2.75 0 0 1 19 5.75v12.5A2.75 2.75 0 0 1 16.25 21h-8.5A2.75 2.75 0 0 1 5 18.25V5.75Zm3 1.25a.75.75 0 0 0-.75.75v8.5c0 .414.336.75.75.75h8.5a.75.75 0 0 0 .75-.75v-8.5a.75.75 0 0 0-.75-.75H8Z"
                fill="currentColor"
              />
            </svg>
            <span>{{ tab.label }}</span>
          </button>
        </nav>

        <div class="selection-toolbar-actions">
          <BaseButton
            variant="ghost"
            icon
            class="selection-icon-btn"
            :active="pinned"
            :title="pinned ? '取消固定' : '固定窗口'"
            @click="emit('pin', !pinned)"
          >
            <template #icon>
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path
                  d="M8.5 4.5h7l-1 5 3 3v1h-4v5l-1.5 1.5-1.5-1.5v-5h-4v-1l3-3-1-5Z"
                  fill="currentColor"
                />
              </svg>
            </template>
          </BaseButton>
          <BaseButton
            variant="ghost"
            icon
            class="selection-icon-btn"
            title="关闭"
            @click="emit('close')"
          >
            <template #icon>
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path
                  d="M6.53 5.47a.75.75 0 0 1 1.06 0L12 9.88l4.41-4.41a.75.75 0 1 1 1.06 1.06L13.06 11l4.41 4.41a.75.75 0 0 1-1.06 1.06L12 12.06l-4.41 4.41a.75.75 0 1 1-1.06-1.06L10.94 11 6.53 6.59a.75.75 0 0 1 0-1.12Z"
                  fill="currentColor"
                />
              </svg>
            </template>
          </BaseButton>
        </div>
      </header>

      <div class="selection-body">
        <Transition :name="transitionName" mode="out-in">
          <section :key="currentTab" class="selection-view">
            <template v-if="currentTab === 'selected'">
              <div class="selection-text-card">
                <span class="selection-text-label">当前选中</span>
                <div class="selection-text-content">
                  {{ selectedText || '等待选区...' }}
                </div>
              </div>
            </template>

            <template v-else-if="currentActionState.loading">
              <div class="selection-loading-card">
                <span class="loading-spinner" aria-hidden="true" />
                <div class="loading-copy">
                  <strong>{{ loadingTitle }}</strong>
                  <p>{{ loadingDescription }}</p>
                </div>
              </div>
            </template>

            <template v-else-if="currentActionState.error">
              <div class="selection-error-card">
                <strong>{{ currentActionLabel }}失败</strong>
                <p>{{ currentActionState.error }}</p>
                <BaseButton variant="outline" size="sm" class="selection-retry-btn" @click="retryCurrentTab">
                  重试
                </BaseButton>
              </div>
            </template>

            <template v-else-if="currentActionState.result">
              <SelectionResultCard
                :result="currentActionState.result"
                :show-examples="showExamples"
              />
            </template>

            <template v-else>
              <EmptyState
                class="selection-empty-state"
                :title="currentActionLabel"
                description="正在准备内容，请稍候。"
              />
            </template>
          </section>
        </Transition>
      </div>
    </section>
  </div>
</template>

<script setup>
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useElectron } from '@/composables/useElectron'
import SelectionResultCard from '@/components/selection/SelectionResultCard.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import EmptyState from '@/components/common/EmptyState.vue'

const tabs = [
  { key: 'selected', label: '选中' },
  { key: 'translate', label: '翻译' }
]

const ACTION_TAB_KEYS = ['translate']
const TAB_LABELS = Object.fromEntries(tabs.map((tab) => [tab.key, tab.label]))

const props = defineProps({
  activation: {
    type: Object,
    default: () => ({})
  },
  pinned: {
    type: Boolean,
    default: false
  }
})

const emit = defineEmits(['close', 'pin'])

const { api } = useElectron()

const selectedText = ref('')
const currentTab = ref('selected')
const transitionName = ref('slide-left')
const showExamples = ref(true)
const autoAddToWordbook = ref(true)
const activationVersion = ref(0)
const actionState = reactive({
  translate: { loading: false, error: '', result: null }
})

const LOADING_TITLES = {
  translate: '正在翻译'
}

const LOADING_DESCRIPTIONS = {
  translate: '正在生成更准确的翻译结果，请稍候。'
}

const currentActionState = computed(() => (
  ACTION_TAB_KEYS.includes(currentTab.value)
    ? actionState[currentTab.value]
    : { loading: false, error: '', result: null }
))
const currentActionLabel = computed(() => TAB_LABELS[currentTab.value] || '动作')
const loadingTitle = computed(() => LOADING_TITLES[currentTab.value] || '正在处理')
const loadingDescription = computed(() => LOADING_DESCRIPTIONS[currentTab.value] || '请稍候。')

async function loadSettings() {
  try {
    const settings = await api.getSettings()
    showExamples.value = settings.selection_toolbar_show_examples !== false
    autoAddToWordbook.value = settings.selection_toolbar_auto_add_to_wordbook !== false
  } catch (error) {
    console.error('[SelectionPanel] Failed to load settings:', error)
  }
}

function resetActionState() {
  for (const actionKey of ACTION_TAB_KEYS) {
    actionState[actionKey].loading = false
    actionState[actionKey].error = ''
    actionState[actionKey].result = null
  }
}

function resetFromActivation(payload = {}) {
  activationVersion.value += 1
  selectedText.value = payload.text || ''
  currentTab.value = 'selected'
  transitionName.value = 'slide-left'
  resetActionState()
}

function getTabIndex(tabKey) {
  return tabs.findIndex((tab) => tab.key === tabKey)
}

function getActionRequest(actionKey) {
  const payload = {
    source_text: selectedText.value,
    auto_add_to_wordbook: autoAddToWordbook.value
  }

  return api.runAITranslate(payload)
}

async function ensureActionResult(actionKey, { force = false } = {}) {
  if (!ACTION_TAB_KEYS.includes(actionKey) || !selectedText.value) {
    return
  }

  const state = actionState[actionKey]
  if (state.loading) {
    return
  }
  if (!force && state.result) {
    return
  }

  const versionAtStart = activationVersion.value
  state.loading = true
  state.error = ''

  try {
    const result = await getActionRequest(actionKey)
    if (activationVersion.value !== versionAtStart) {
      return
    }
    state.result = result
  } catch (error) {
    if (activationVersion.value !== versionAtStart) {
      return
    }
    state.error = error.message || `${TAB_LABELS[actionKey]}失败`
    state.result = null
  } finally {
    if (activationVersion.value === versionAtStart) {
      state.loading = false
    }
  }
}

function handleTabClick(tabKey) {
  if (tabKey === currentTab.value) {
    return
  }

  const currentIndex = getTabIndex(currentTab.value)
  const nextIndex = getTabIndex(tabKey)
  transitionName.value = nextIndex > currentIndex ? 'slide-left' : 'slide-right'
  currentTab.value = tabKey

  if (ACTION_TAB_KEYS.includes(tabKey)) {
    void ensureActionResult(tabKey)
  }
}

function retryCurrentTab() {
  if (!ACTION_TAB_KEYS.includes(currentTab.value)) {
    return
  }

  void ensureActionResult(currentTab.value, { force: true })
}

watch(() => props.activation.sequence, () => {
  resetFromActivation(props.activation)
}, { immediate: true })

onMounted(async () => {
  await loadSettings()
})
</script>

<style scoped>
.selection-shell {
  width: 100%;
  height: 100vh;
  overflow: hidden;
}

.selection-panel {
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  padding: 16px;
  border-radius: var(--r-lg);
  /* 原为暖棕渐变 + 亮金，与同窗口的 ClipboardPanel（冷灰）是两套语言：现统一到共享令牌 */
  background: linear-gradient(180deg, var(--bg-card), var(--bg-base));
  color: var(--text-1);
  box-shadow: var(--e-4);
  overflow: hidden;
  -webkit-app-region: drag;
}

.selection-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding-bottom: 14px;
  border-bottom: 1px solid var(--line-soft);
}

.selection-tabs,
.selection-toolbar-actions {
  display: flex;
  align-items: center;
}

.selection-tabs {
  gap: 8px;
  min-width: 0;
  flex-wrap: nowrap;
  -webkit-app-region: no-drag;
}

.selection-toolbar-actions {
  gap: 8px;
  flex: 0 0 auto;
  -webkit-app-region: no-drag;
}

.selection-tab {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 34px;
  padding: 0 12px;
  border: none;
  border-radius: var(--r-full);
  background: transparent;
  color: var(--text-3);
  cursor: pointer;
  transition: background-color var(--t-fast) var(--ease), color var(--t-fast) var(--ease), transform var(--t-fast) var(--ease);
  -webkit-app-region: no-drag;
}

.selection-tab:hover {
  color: var(--text-1);
  background: var(--line-soft);
}

.selection-tab:active {
  transform: translateY(1px);
  filter: brightness(0.94);
}

.selection-tab:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

.selection-tab.active {
  background: var(--accent-dim);
  color: var(--accent-hi);
}

.selection-tab-icon {
  width: 14px;
  height: 14px;
  display: block;
}

.selection-toolbar-actions .selection-icon-btn {
  width: 32px;
  height: 32px;
  border-radius: var(--r-full);
  background: var(--line-soft);
  color: inherit;
  -webkit-app-region: no-drag;
}

/* 实心图标需要覆盖原语的描边式 svg 规则 */
.selection-toolbar-actions .selection-icon-btn :deep(svg) {
  fill: currentColor;
  stroke: none;
}

.selection-body {
  position: relative;
  flex: 1;
  min-height: 0;
  margin-top: 16px;
  overflow: hidden;
  -webkit-app-region: no-drag;
}

.selection-view {
  height: 100%;
  min-height: 0;
}

.selection-text-card,
.selection-loading-card,
.selection-error-card {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  padding: 16px;
  border-radius: var(--r-lg);
  background: var(--line-soft);
  -webkit-app-region: no-drag;
}

/* 空态改用原语，仅补齐"占满浮层高度 + 居中"的容器约束 */
.selection-view .selection-empty-state {
  height: 100%;
  justify-content: center;
  gap: 10px;
  border: 1px solid var(--line-soft-strong);
  border-radius: var(--r-lg);
  background: var(--line-soft);
  -webkit-app-region: no-drag;
}

.selection-text-label {
  display: inline-flex;
  margin-bottom: 10px;
  color: var(--text-3);
  font-size: var(--fs-sm);
}

.selection-text-content {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  line-height: 1.75;
  color: var(--text-1);
  scrollbar-width: thin;
  scrollbar-color: var(--scroll-thumb) transparent;
}

.selection-loading-card {
  align-items: center;
  justify-content: center;
  gap: 14px;
  text-align: center;
}

.selection-error-card,
.selection-loading-card {
  justify-content: center;
  gap: 10px;
}

.selection-error-card {
  background: var(--err-soft);
  color: var(--err);
}

/* 重试按钮：在原语基线上保留胶囊外观 */
.selection-error-card .selection-retry-btn {
  width: fit-content;
  border-radius: var(--r-full);
  background: var(--line-soft);
  border-color: var(--line-soft-strong);
  color: inherit;
  -webkit-app-region: no-drag;
}

.loading-spinner {
  width: 18px;
  height: 18px;
  border-radius: var(--r-full);
  border: 2px solid var(--accent-tint);
  border-top-color: var(--accent);
  animation: selection-spin 0.8s linear infinite;
}

.loading-copy strong {
  display: block;
  color: var(--text-1);
}

.loading-copy p {
  margin-top: 4px;
  color: var(--text-3);
  line-height: 1.5;
}

.slide-left-enter-active,
.slide-left-leave-active,
.slide-right-enter-active,
.slide-right-leave-active {
  transition: transform 200ms ease, opacity 200ms ease;
}

.slide-left-enter-from {
  opacity: 0;
  transform: translateX(24px);
}

.slide-left-leave-to {
  opacity: 0;
  transform: translateX(-24px);
}

.slide-right-enter-from {
  opacity: 0;
  transform: translateX(-24px);
}

.slide-right-leave-to {
  opacity: 0;
  transform: translateX(24px);
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
