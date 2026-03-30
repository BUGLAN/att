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
        <RecorderSection />
        <RecordsSection />
      </div>

      <div v-show="currentPage === 'records'" class="container page-view">
        <RecordsLibraryPage />
      </div>

      <div v-show="currentPage === 'clipboard'" class="container page-view">
        <ClipboardPage />
      </div>

      <div v-show="currentPage === 'wordbook'" class="container page-view">
        <WordbookPage />
      </div>

      <div v-show="currentPage === 'settings'" class="container page-view">
        <SettingsPage />
      </div>
    </main>

    <RecordEditModal v-if="modals.recordEdit" />
    <PresetModal v-if="modals.preset" />
    <PresetEditModal v-if="modals.presetEdit" />
    <LLMModal v-if="modals.llm" />
    <LLMEditModal v-if="modals.llmEdit" />

    <Toast />
  </div>
</template>

<script setup>
import { computed, onMounted, onUnmounted } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import { useRecorder } from '@/composables/useRecorder'
import { useGlobalWebSocket } from '@/composables/useWebSocket'

import Sidebar from '@/components/layout/Sidebar.vue'
import TitleBar from '@/components/layout/TitleBar.vue'
import RecorderSection from '@/components/home/RecorderSection.vue'
import RecordsSection from '@/components/home/RecordsSection.vue'
import ClipboardPage from '@/components/clipboard/ClipboardPage.vue'
import RecordsLibraryPage from '@/components/records/RecordsLibraryPage.vue'
import SettingsPage from '@/components/settings/SettingsPage.vue'
import WordbookPage from '@/components/wordbook/WordbookPage.vue'

import RecordEditModal from '@/components/modals/RecordEditModal.vue'
import PresetModal from '@/components/modals/PresetModal.vue'
import PresetEditModal from '@/components/modals/PresetEditModal.vue'
import LLMModal from '@/components/modals/LLMModal.vue'
import LLMEditModal from '@/components/modals/LLMEditModal.vue'
import Toast from '@/components/common/Toast.vue'

import { useElectron } from '@/composables/useElectron'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
useRecorder()
const { connect, addMessageHandler, removeMessageHandler } = useGlobalWebSocket()
const {
  isElectron,
  clipboardOverlay,
  onPasteText,
  onNavigateSettings,
  removeAllListeners,
  syncDesktopSettings,
  debugLog
} = useElectron()

const currentPage = computed(() => appStore.currentPage)
const sidebarDisplayCollapsed = computed(() => appStore.sidebarDisplayCollapsed)
const currentSidebarWidth = computed(() => appStore.currentSidebarWidth)
const isSidebarResizing = computed(() => appStore.isSidebarResizing)
const modals = computed(() => appStore.modals)
const shellStyles = computed(() => ({
  '--sidebar-width': `${currentSidebarWidth.value}px`
}))

function handleGlobalMessage(message) {
  if (message.type !== 'clipboard_overlay_command' || message.action !== 'trigger') {
    return
  }

  if (isElectron) {
    debugLog('renderer.app', 'Ignored websocket clipboard overlay trigger in Electron runtime', {
      message
    })
    return
  }

  debugLog('renderer.app', 'Received clipboard overlay trigger message', {
    message,
    settings: {
      clipboard_overlay_retrigger_action: settingsStore.settings.clipboard_overlay_retrigger_action,
      clipboard_overlay_auto_focus_search: settingsStore.settings.clipboard_overlay_auto_focus_search
    }
  })

  const payload = {
    focusSearch: settingsStore.settings.clipboard_overlay_auto_focus_search ?? true
  }

  if (settingsStore.settings.clipboard_overlay_retrigger_action === 'show_only') {
    debugLog('renderer.app', 'Using clipboard overlay show', payload)
    clipboardOverlay.show(payload)
    return
  }

  debugLog('renderer.app', 'Using clipboard overlay toggle', payload)
  clipboardOverlay.toggle(payload)
}

onMounted(async () => {
  onPasteText(() => {
    appStore.showToast('已复制到剪贴板')
  })

  onNavigateSettings(() => {
    appStore.setCurrentPage('settings')
  })

  await settingsStore.loadSettings()

  try {
    await syncDesktopSettings({
      pause_audio_on_recording: settingsStore.settings.pause_audio_on_recording
    })
  } catch (error) {
    console.error('[Desktop] Failed to sync settings on startup:', error)
  }

  addMessageHandler(handleGlobalMessage)
  connect()
})

onUnmounted(() => {
  removeMessageHandler(handleGlobalMessage)
  removeAllListeners('paste-text')
  removeAllListeners('navigate:settings')
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
