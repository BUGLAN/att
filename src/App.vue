<template>
  <div
    class="app-container"
    :class="{
      'has-title-bar': isElectron,
      'is-sidebar-resizing': isSidebarResizing
    }"
    :style="shellStyles"
  >
    <TitleBar v-if="isElectron" />

    <Sidebar
      :collapsed="sidebarDisplayCollapsed"
      :width="currentSidebarWidth"
    />

    <main
      class="main-content"
      :class="{
        'has-title-bar': isElectron
      }"
    >
      <div v-show="currentPage === 'home'" class="container page-view">
        <div class="ui-page">
          <RecorderSection />
          <RecordsSection />
        </div>
      </div>

      <div v-show="currentPage === 'transcribe'" class="container page-view">
        <TranscribePage />
      </div>

      <div v-show="currentPage === 'records'" class="container page-view">
        <RecordsLibraryPage />
      </div>

      <div v-show="currentPage === 'clipboard'" class="container page-view">
        <ClipboardPage />
      </div>

      <div v-show="currentPage === 'settings'" class="container page-view">
        <SettingsPage />
      </div>
    </main>

    <RecordEditModal v-if="modals.recordEdit" />
    <PresetModal v-if="modals.preset" />
    <PresetEditModal v-if="modals.presetEdit" />
    <LLMWizardModal v-if="modals.llmWizard" />
    <LLMModelEditModal v-if="modals.llmModelEdit" />
    <LLMProviderEditModal v-if="modals.llmProviderEdit" />

    <Toast />
  </div>
</template>

<script setup>
import { computed, onMounted, onUnmounted } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import { useGlobalWebSocket } from '@/composables/useWebSocket'

import Sidebar from '@/components/layout/Sidebar.vue'
import TitleBar from '@/components/layout/TitleBar.vue'
import RecorderSection from '@/components/home/RecorderSection.vue'
import RecordsSection from '@/components/home/RecordsSection.vue'
import ClipboardPage from '@/components/clipboard/ClipboardPage.vue'
import RecordsLibraryPage from '@/components/records/RecordsLibraryPage.vue'
import TranscribePage from '@/components/transcribe/TranscribePage.vue'
import SettingsPage from '@/components/settings/SettingsPage.vue'

import RecordEditModal from '@/components/modals/RecordEditModal.vue'
import PresetModal from '@/components/modals/PresetModal.vue'
import PresetEditModal from '@/components/modals/PresetEditModal.vue'
import LLMWizardModal from '@/components/modals/LLMWizardModal.vue'
import LLMModelEditModal from '@/components/modals/LLMModelEditModal.vue'
import LLMProviderEditModal from '@/components/modals/LLMProviderEditModal.vue'
import Toast from '@/components/common/Toast.vue'

import { useElectron } from '@/composables/useElectron'
import { useTrayIcon } from '@/composables/useTrayIcon'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { connect } = useGlobalWebSocket()
const {
  isElectron,
  onPasteText,
  onNavigateSettings,
  onDesktopPermissionRequired,
  onDesktopAgentError,
  removeAllListeners,
  syncDesktopSettings,
  getHotkeyStatus,
  debugLog
} = useElectron()

// 托盘图标：录音 / 转写由 useRecorder 直接切，这里补上"后端断连"那条边
useTrayIcon()

const currentPage = computed(() => appStore.currentPage)
const sidebarDisplayCollapsed = computed(() => appStore.sidebarDisplayCollapsed)
const currentSidebarWidth = computed(() => appStore.currentSidebarWidth)
const isSidebarResizing = computed(() => appStore.isSidebarResizing)
const modals = computed(() => appStore.modals)
const shellStyles = computed(() => ({
  '--sidebar-width': `${currentSidebarWidth.value}px`
}))

onMounted(async () => {
  onPasteText(() => {
    appStore.showToast('已复制到剪贴板')
  })

  onNavigateSettings(() => {
    appStore.openSettings()
  })

  onDesktopPermissionRequired((payload) => {
    debugLog('renderer.app', 'Desktop permission required', payload)
    appStore.showToast(payload?.message || '桌面权限不足，请在系统设置中授权')
  })

  onDesktopAgentError((payload) => {
    debugLog('renderer.app', 'Desktop agent error', payload)
    appStore.showToast(payload?.error || '本地桌面代理发生错误')
  })

  // 全局快捷键在 Rust 启动期就已注册：权限缺失（macOS「输入监控」）的原因在这里补拉并提示，
  // 不能只依赖事件（启动期事件发出时前端还没订阅）
  getHotkeyStatus()
    .then((status) => {
      if (status?.error) {
        appStore.showToast(status.error, { tone: 'error', duration: 8000 })
      }
    })
    .catch(() => {})

  await settingsStore.loadSettings()

  try {
    await syncDesktopSettings({
      pause_audio_on_recording: settingsStore.settings.pause_audio_on_recording
    })
  } catch (error) {
    console.error('[Desktop] Failed to sync settings on startup:', error)
  }

  connect()
})

onUnmounted(() => {
  removeAllListeners('paste-text')
  removeAllListeners('navigate:settings')
  removeAllListeners('desktop:permission-required')
  removeAllListeners('desktop:agent-error')
})
</script>

<style scoped>
.app-container {
  display: flex;
  min-height: 100vh;
  width: 100%;
}

.app-container.is-sidebar-resizing,
.app-container.is-sidebar-resizing * {
  cursor: col-resize !important;
  user-select: none;
}

.page-view {
  flex: 1;
  width: 100%;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
}
</style>
