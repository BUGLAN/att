<template>
  <div class="settings-stack">
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">系统权限</h3>
        <p v-if="!isDesktop" class="setting-desc">浏览器模式下由浏览器管理权限。</p>
        <div v-for="item in items" :key="item.id" class="setting-item">
          <div class="setting-info">
            <span class="setting-label">{{ item.label }}</span>
            <span class="setting-desc">{{ item.desc }}</span>
          </div>
          <div class="setting-actions-inline permission-actions">
            <BaseBadge :tone="STATE_TONE[item.state] || 'neutral'">{{ STATE_LABEL[item.state] || '未知' }}</BaseBadge>
            <BaseButton
              v-if="item.state !== 'granted' && item.state !== 'not_required'"
              variant="outline"
              size="sm"
              @click="openSettings(item.id)"
            >
              打开系统设置
            </BaseButton>
          </div>
        </div>
        <div v-if="isDesktop" class="settings-note">
          <span>{{ isDevBuild && platform === 'darwin' ? '开发态权限归属启动 tauri dev 的终端；授权后自动刷新。' : '授权后自动刷新。' }}</span>
          <BaseButton variant="ghost" size="sm" :loading="loading" @click="refresh">重新检测</BaseButton>
        </div>
      </div>
    </section>
  </div>
</template>

<script setup>
import { onMounted, onUnmounted, ref } from 'vue'
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import { useAppStore } from '@/stores/app'
import { useElectron } from '@/composables/useElectron'

const STATE_LABEL = { granted: '已授权', denied: '未授权', unknown: '未确认', not_required: '无需授权' }
const STATE_TONE = { granted: 'ok', denied: 'warn', unknown: 'neutral', not_required: 'neutral' }

const appStore = useAppStore()
const { isDesktop, platform, getPermissionStatus, openPermissionSettings } = useElectron()
// 开发态（tauri dev）权限归属启动它的终端，提示只在开发态出现
const isDevBuild = import.meta.env.DEV
const items = ref([])
const loading = ref(false)

async function refresh() {
  loading.value = true
  try {
    items.value = (await getPermissionStatus())?.items || []
  } catch (error) {
    console.warn('[Settings] 读取权限状态失败：', error)
  } finally {
    loading.value = false
  }
}

async function openSettings(id) {
  try {
    await openPermissionSettings(id)
  } catch (error) {
    appStore.showToast(String(error?.message || error), { tone: 'error' })
  }
}

// 用户去系统设置授权后切回窗口时自动刷新
function handleFocus() {
  void refresh()
}

onMounted(() => {
  void refresh()
  window.addEventListener('focus', handleFocus)
})

onUnmounted(() => {
  window.removeEventListener('focus', handleFocus)
})
</script>

<style scoped>
.permission-actions {
  justify-content: flex-end;
}
</style>
