<template>
  <div class="settings-stack">
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">服务状态</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">数据面服务</span>
            <span class="setting-desc mono">{{ appInfo ? `http://127.0.0.1:${appInfo.backend_port}/api` : '前端与本地服务的连接' }}</span>
          </div>
          <div class="setting-actions-inline diag-actions">
            <BaseBadge :tone="appStore.isConnected ? 'ok' : 'err'">{{ appStore.isConnected ? '已连接' : '未连接' }}</BaseBadge>
          </div>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">识别引擎</span>
            <span class="setting-desc">{{ runtime.model_service_error || `${runtime.asr_provider || '未知'} · ${runtime.runtime_environment || 'unknown'}` }}</span>
          </div>
          <div class="setting-actions-inline diag-actions">
            <BaseBadge :tone="runtime.model_service_ready ? 'ok' : 'warn'">{{ runtime.model_service_ready ? '就绪' : '不可用' }}</BaseBadge>
          </div>
        </div>
      </div>
    </section>

    <section v-if="appInfo" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">本地文件</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">数据目录</span>
            <span class="setting-desc mono">{{ appInfo.data_dir }}</span>
          </div>
          <div class="setting-actions-inline diag-actions">
            <BaseButton variant="outline" size="sm" @click="reveal('data')">在文件管理器中显示</BaseButton>
          </div>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">运行日志</span>
            <span class="setting-desc mono">{{ appInfo.log_file }}（每次启动覆盖）</span>
          </div>
          <div class="setting-actions-inline diag-actions">
            <BaseButton variant="outline" size="sm" @click="reveal('log')">在文件管理器中显示</BaseButton>
          </div>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">提示词目录</span>
            <span class="setting-desc mono">{{ appInfo.prompts_dir }}</span>
          </div>
        </div>
      </div>
    </section>
  </div>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import { useElectron } from '@/composables/useElectron'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { getAppInfo, revealAppPath } = useElectron()
const appInfo = ref(null)
const runtime = computed(() => settingsStore.runtimeStatus || {})

async function reveal(kind) {
  try {
    await revealAppPath(kind)
  } catch (error) {
    appStore.showToast(String(error?.message || error), { tone: 'error' })
  }
}

onMounted(async () => {
  settingsStore.loadRuntimeStatus(false).catch(() => {})
  try {
    appInfo.value = await getAppInfo()
  } catch (error) {
    console.warn('[Settings] 读取应用信息失败：', error)
  }
})
</script>

<style scoped>
.diag-actions {
  justify-content: flex-end;
}

.mono {
  font-family: var(--font-mono);
  word-break: break-all;
}
</style>
