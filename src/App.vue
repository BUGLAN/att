<template>
  <div class="app-container">
    <Sidebar
      :collapsed="sidebarCollapsed"
      @toggle="toggleSidebar"
    />

    <main
      class="main-content"
      :class="{
        'sidebar-collapsed': sidebarCollapsed,
        'has-title-bar': isElectron
      }"
    >
      <TitleBar v-if="isElectron" :sidebar-collapsed="sidebarCollapsed" />

      <div v-show="currentPage === 'home'" class="container page-view">
        <RecorderSection />
        <RecordsSection />
      </div>

      <div v-show="currentPage === 'records'" class="container page-view">
        <RecordsLibraryPage />
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
import RecordsLibraryPage from '@/components/records/RecordsLibraryPage.vue'
import SettingsPage from '@/components/settings/SettingsPage.vue'

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
const { connect } = useGlobalWebSocket()
const { isElectron, onPasteText, onNavigateSettings, removeAllListeners, syncDesktopSettings } = useElectron()

const currentPage = computed(() => appStore.currentPage)
const sidebarCollapsed = computed(() => appStore.sidebarCollapsed)
const modals = computed(() => appStore.modals)

function toggleSidebar() {
  appStore.toggleSidebar()
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

  connect()
})

onUnmounted(() => {
  removeAllListeners('paste-text')
  removeAllListeners('navigate:settings')
})
</script>

<style scoped>
.app-container {
  display: flex;
  min-height: 100vh;
}

.page-view {
  width: 100%;
  min-height: 100%;
}
</style>
