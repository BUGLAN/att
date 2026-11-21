<template>
  <div class="settings-stack">
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">服务状态</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">本地服务</span>
            <span class="setting-desc mono">{{ appInfo ? `http://127.0.0.1:${appInfo.backend_port}/api` : '前端与本地服务的连接' }}</span>
          </div>
          <div class="setting-actions-inline diag-actions">
            <BaseBadge :tone="appStore.isConnected ? 'ok' : 'err'">{{ appStore.isConnected ? '已连接' : '未连接' }}</BaseBadge>
          </div>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">识别引擎</span>
            <span class="setting-desc">{{ runtime.model_service_error || providerLabel(runtime.asr_provider) }}</span>
          </div>
          <div class="setting-actions-inline diag-actions">
            <BaseBadge :tone="runtime.model_service_ready ? 'ok' : 'warn'">{{ runtime.model_service_ready ? '就绪' : '不可用' }}</BaseBadge>
          </div>
        </div>
      </div>
    </section>

    <!-- 存储：各部分占用（缓存 1 分钟）、录音音频保留天数、清理孤儿音频；浏览器模式同样可用（走数据面） -->
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">
          存储
          <span class="ui-title-aside ui-meta">
            <template v-if="storage">合计 {{ totalSize }}</template>
            <template v-else-if="storageLoading">正在统计…</template>
          </span>
        </h3>
        <div v-for="row in storageItems" :key="row.key" class="setting-item">
          <div class="setting-info">
            <span class="setting-label">{{ row.label }}</span>
            <span class="setting-desc mono" :title="row.fullPath">{{ row.path }}</span>
          </div>
          <div class="setting-actions-inline diag-actions storage-figures">
            <span class="ui-mono storage-size">{{ row.size }}</span>
            <span class="ui-meta">{{ row.files }}</span>
            <BaseButton
              v-if="row.key === 'orphans'"
              variant="outline"
              size="sm"
              :disabled="!canClearOrphans || clearingOrphans"
              :loading="clearingOrphans"
              @click="isOrphanConfirmVisible = true"
            >
              清理
            </BaseButton>
          </div>
        </div>
        <div v-if="storageError" class="settings-note warn">
          <span>{{ storageError }}</span>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">录音音频保留天数</span>
            <span class="setting-desc">超过天数的录音只删音频文件，记录和文字保留；收藏的记录不删。每天检查一次。</span>
          </div>
          <BaseSelect
            :key="`audio-retention-${retentionSelectKey}`"
            :model-value="form.audio_retention_days"
            aria-label="录音音频保留天数"
            @update:model-value="requestAudioRetention"
          >
            <option v-for="option in retentionChoices" :key="option.value" :value="option.value">{{ option.label }}</option>
          </BaseSelect>
        </div>
        <div class="settings-note">
          <span>{{ storage ? `统计于 ${measuredAt}，1 分钟内复用结果。` : '统计各目录占用需要遍历文件，稍等片刻。' }}</span>
          <BaseButton variant="ghost" size="sm" :loading="storageLoading" @click="loadStorage(true)">重新统计</BaseButton>
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
            <span class="setting-desc mono">{{ appInfo.log_file }}（上一次启动的日志另存为 att-app.prev.log）</span>
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

    <ConfirmDialog
      v-model="isOrphanConfirmVisible"
      title="清理孤儿音频？"
      description="这些是升级迁移时没有记录认领、被隔离到 audio/orphans/ 的旧录音，删除后无法恢复。"
      confirm-text="清理"
      @confirm="clearOrphans"
    />

    <ConfirmDialog
      :model-value="Boolean(pendingRetention)"
      :title="pendingRetentionCopy.title"
      :description="pendingRetentionCopy.description"
      confirm-text="确定"
      @update:model-value="(open) => { if (!open) closeRetentionConfirm() }"
      @confirm="applyPendingRetention"
    />
  </div>
</template>

<script setup>
import { computed, inject, onMounted, ref } from 'vue'
import { providerLabel } from '../asrEngineStatus.mjs'
import { SETTINGS_CTX } from '../settingsContext.js'
import {
  audioRetentionConfirm,
  audioRetentionOptions,
  formatBytes,
  hasOrphans,
  isTighter,
  normalizeLimit,
  storageRows
} from '../storageUsage.mjs'
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseSelect from '@/components/common/BaseSelect.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import { useElectron } from '@/composables/useElectron'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { form } = inject(SETTINGS_CTX)
const { api, getAppInfo, revealAppPath } = useElectron()
const appInfo = ref(null)
const runtime = computed(() => settingsStore.runtimeStatus || {})

// ---- 存储 ----
const storage = ref(null)
const storageLoading = ref(false)
const storageError = ref('')
const clearingOrphans = ref(false)
const isOrphanConfirmVisible = ref(false)
const storageItems = computed(() => storageRows(storage.value))
const totalSize = computed(() => formatBytes(storage.value?.total_bytes))
const canClearOrphans = computed(() => hasOrphans(storage.value))
const measuredAt = computed(() => {
  const date = new Date(storage.value?.measured_at || '')
  return Number.isNaN(date.getTime()) ? '刚刚' : date.toLocaleTimeString('zh-CN', { hour12: false })
})

async function loadStorage(refresh = false) {
  storageLoading.value = true
  storageError.value = ''
  try {
    storage.value = await api.getStorageUsage(refresh)
  } catch (error) {
    storageError.value = `统计数据占用失败：${error?.message || error}`
  } finally {
    storageLoading.value = false
  }
}

async function clearOrphans() {
  clearingOrphans.value = true
  try {
    const result = await api.clearOrphanAudio()
    appStore.showToast(`已清理 ${result?.deleted ?? 0} 个孤儿音频（${formatBytes(result?.bytes)}）`, { tone: 'success' })
  } catch (error) {
    appStore.showToast(String(error?.message || error), { tone: 'error' })
  } finally {
    clearingOrphans.value = false
  }
  await loadStorage()
}

// 录音音频保留天数：改紧了会立即删过期音频，先确认再写进表单（表单变化即自动保存）
const retentionChoices = computed(() => audioRetentionOptions(form.value.audio_retention_days))
const pendingRetention = ref(null)
const pendingRetentionCopy = computed(() => (
  pendingRetention.value === null ? { title: '', description: '' } : audioRetentionConfirm(pendingRetention.value)
))
// 下拉框在确认前已经显示了新选项：取消时表单值没变，Vue 不会回写，重建一次让它按表单值显示
const retentionSelectKey = ref(0)

function requestAudioRetention(raw) {
  const value = normalizeLimit(raw)
  if (isTighter(form.value.audio_retention_days, value)) {
    pendingRetention.value = value
    return
  }
  form.value.audio_retention_days = value
}

function applyPendingRetention() {
  const value = pendingRetention.value
  pendingRetention.value = null
  if (value !== null) {
    form.value.audio_retention_days = value
    // 后端保存后立即按新天数清理：稍后重新统计，让占用数字跟上
    setTimeout(() => loadStorage(true), 1500)
  }
}

function closeRetentionConfirm() {
  pendingRetention.value = null
  retentionSelectKey.value += 1
}

async function reveal(kind) {
  try {
    await revealAppPath(kind)
  } catch (error) {
    appStore.showToast(String(error?.message || error), { tone: 'error' })
  }
}

onMounted(async () => {
  settingsStore.loadRuntimeStatus(false).catch(() => {})
  void loadStorage()
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

.storage-figures {
  flex: none;
  gap: var(--s-3);
}

.storage-size {
  min-width: 72px;
  text-align: right;
  color: var(--text-1);
}
</style>
