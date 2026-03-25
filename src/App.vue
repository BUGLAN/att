<template>
  <div class="app-container">
    <!-- 侧边栏 -->
    <Sidebar
      :collapsed="sidebarCollapsed"
      @toggle="toggleSidebar"
    />

    <!-- 主内容区 -->
    <main class="main-content" :class="{ 'sidebar-collapsed': sidebarCollapsed }">
      <!-- 首页 -->
      <div v-if="currentPage === 'home'" class="container">
        <Header />
        <RecorderSection />
        <RecordsSection />
      </div>

      <!-- 设置页 -->
      <SettingsPage v-else-if="currentPage === 'settings'" />
    </main>

    <!-- 模态框 -->
    <RecordEditModal v-if="modals.recordEdit" />
    <PresetModal v-if="modals.preset" />
    <PresetEditModal v-if="modals.presetEdit" />
    <LLMModal v-if="modals.llm" />
    <LLMEditModal v-if="modals.llmEdit" />

    <!-- Toast -->
    <Toast />
  </div>
</template>

<script setup>
import { computed, onMounted, onUnmounted } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'

import Sidebar from '@/components/layout/Sidebar.vue'
import Header from '@/components/layout/Header.vue'
import RecorderSection from '@/components/home/RecorderSection.vue'
import RecordsSection from '@/components/home/RecordsSection.vue'
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
const { onHotkeyStart, onHotkeyStop, onPasteText, removeAllListeners } = useElectron()

const currentPage = computed(() => appStore.currentPage)
const sidebarCollapsed = computed(() => appStore.sidebarCollapsed)
const modals = computed(() => appStore.modals)

const toggleSidebar = () => {
  appStore.toggleSidebar()
}

// 热键监听
onMounted(async () => {
  await settingsStore.loadSettings()

  onHotkeyStart(() => {
    appStore.startRecording()
  })

  onHotkeyStop(() => {
    appStore.stopRecording()
  })

  onPasteText(() => {
    // 模拟粘贴操作（通过 Electron 的 clipboard 已复制，这里通知用户）
    appStore.showToast('已复制到剪贴板')
  })
})

onUnmounted(() => {
  removeAllListeners('hotkey:start')
  removeAllListeners('hotkey:stop')
  removeAllListeners('paste-text')
})
</script>

<style scoped>
.app-container {
  display: flex;
  min-height: 100vh;
}
</style>
