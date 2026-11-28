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

    <!-- 外部 API：给 SourceFlow 等外部进程一个稳定的回环令牌；最小权限到转写任务，默认全关 -->
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">外部 API</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">持久访问令牌</span>
            <span class="setting-desc">开启后，外部进程可用固定的 X-ATT-Token 提交转写、查询或取消任务（仅回环），应用重启后令牌不变。</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.api_token_enabled" type="checkbox" aria-label="开启持久访问令牌">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">令牌</span>
            <span class="setting-desc mono">{{ tokenDisplay }}</span>
          </div>
          <div class="setting-actions-inline diag-actions">
            <BaseButton
              variant="outline"
              size="sm"
              :disabled="!form.api_token_enabled || !hasApiToken"
              :loading="revealingToken"
              @click="toggleToken"
            >
              {{ tokenRevealed ? '隐藏' : '显示' }}
            </BaseButton>
            <BaseButton
              variant="outline"
              size="sm"
              :disabled="!form.api_token_enabled || !hasApiToken"
              :loading="revealingToken"
              @click="copyToken"
            >
              复制
            </BaseButton>
            <BaseButton
              variant="outline"
              size="sm"
              :disabled="!form.api_token_enabled || !hasApiToken"
              :loading="regeneratingToken"
              @click="isTokenRegenerateVisible = true"
            >
              重新生成
            </BaseButton>
          </div>
        </div>
        <div class="settings-note">
          <span>
            POST http://127.0.0.1:{{ appInfo?.backend_port || 17890 }}/api/transcribe，请求头 X-ATT-Token
            用上面的令牌，path 可以是任意可读的本地音视频文件（不再需要目录授权），请求体如
            {"path":"D:/audio/a.mp3","want_segments":true,"async_job":true}；返回任务号后用 GET
            /api/transcribe/jobs/{id} 轮询。
          </span>
        </div>
        <div class="settings-note">
          <span>令牌只存本机加密数据；界面默认脱敏，仅在点「显示 / 复制」时读取明文，且不写进日志。持久令牌不能访问记录、设置或 LLM 配置，仅接受回环地址。</span>
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
        <div v-if="isDesktop" class="setting-item">
          <div class="setting-info">
            <span class="setting-label">导出备份</span>
            <span class="setting-desc">把设置、记录、录音、剪贴板、提示词预设和 LLM 配置打包成一个 zip；日志与临时文件不会进入备份。</span>
          </div>
          <div class="setting-actions-inline diag-actions">
            <BaseButton
              ref="exportButton"
              variant="outline"
              size="sm"
              :loading="exporting"
              :disabled="exporting || importing || restartRequired"
              @click="exportDialogOpen = true"
            >
              导出备份
            </BaseButton>
          </div>
        </div>
        <div v-if="isDesktop" class="setting-item">
          <div class="setting-info">
            <span class="setting-label">从备份恢复</span>
            <span class="setting-desc">会用备份覆盖本机数据；导入前自动把当前数据复制到 data/.pre-import-*，失败时回滚。</span>
          </div>
          <div class="setting-actions-inline diag-actions">
            <BaseButton
              ref="importButton"
              variant="outline"
              size="sm"
              :loading="importing"
              :disabled="exporting || importing || restartRequired"
              @click="importConfirmOpen = true"
            >
              从备份恢复
            </BaseButton>
          </div>
        </div>
        <div v-if="restartRequired" class="settings-note warn diag-restart" role="status">
          <div class="diag-restart__body">
            <strong class="diag-restart__title">恢复完成，请重启应用后生效</strong>
            <span>{{ importSummary }}</span>
            <span v-if="importCredentialsReset" class="diag-restart__detail">
              备份不含加密密钥，{{ importCredentialsReset }} 项识别 / LLM 凭据或外部 API 令牌已按「未设置」处理，重启后请重新填写。
            </span>
            <span v-if="importBackupPath" class="diag-restart__detail mono">导入前数据：{{ importBackupPath }}</span>
          </div>
        </div>
      </div>
    </section>

    <BackupExportDialog v-model="exportDialogOpen" @confirm="startExport" />

    <ConfirmDialog
      v-model="importConfirmOpen"
      title="从备份恢复"
      description="备份中的设置、记录、录音、剪贴板、提示词预设和 LLM 配置将覆盖本机数据。导入前会自动保存一份当前数据快照，导入失败时回滚。完成后需要重启应用。"
      confirm-text="选择备份并导入"
      cancel-text="取消"
      tone="default"
      :loading="importing"
      @confirm="startImport"
    />

    <ConfirmDialog
      v-model="isTokenRegenerateVisible"
      title="重新生成持久令牌？"
      description="旧令牌会立即失效。请更新所有仍在使用旧令牌的外部工具，新令牌将在生成后显示。"
      confirm-text="重新生成"
      cancel-text="取消"
      tone="default"
      :loading="regeneratingToken"
      @confirm="regenerateToken"
    />

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

    <Teleport to="body">
      <div
        v-if="exporting || importing"
        class="backup-busy-gate"
        @keydown.tab.prevent="focusBusyPanel"
        @keydown.esc.prevent
      >
        <div
          ref="busyPanel"
          class="backup-busy-gate__panel"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="backupBusyTitle"
          aria-describedby="backupBusyDesc"
          tabindex="-1"
        >
          <span class="backup-busy-gate__spinner" aria-hidden="true"></span>
          <div>
            <strong id="backupBusyTitle">{{ importing ? '正在恢复数据' : '正在导出备份' }}</strong>
            <p id="backupBusyDesc">
              {{ importing ? '处理期间会暂停录音与剪贴板写入，请勿关闭应用。' : '正在打包本地数据，请稍候。' }}
            </p>
          </div>
        </div>
      </div>
    </Teleport>

    <Teleport to="body">
      <div
        v-if="restartRequired"
        class="backup-restart-gate"
        @keydown.tab.prevent="focusRestartButton"
        @keydown.esc.prevent
      >
        <div
          ref="restartPanel"
          class="backup-restart-gate__panel"
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="backupRestartTitle"
          aria-describedby="backupRestartDesc"
        >
          <h3 id="backupRestartTitle" class="backup-restart-gate__title">恢复完成，需要重启应用</h3>
          <div id="backupRestartDesc" class="backup-restart-gate__body">
            <p>{{ importSummary }}旧进程仍持有恢复前的设置与目录索引，继续使用可能把旧数据写回备份。</p>
            <p v-if="importCredentialsReset">
              备份不含加密密钥，{{ importCredentialsReset }} 项识别 / LLM 凭据或外部 API 令牌已按「未设置」处理，重启后请重新填写。
            </p>
            <p v-if="importBackupPath" class="mono">导入前数据已保存在：{{ importBackupPath }}</p>
          </div>
          <BaseButton variant="solid" :loading="restarting" @click="restartNow">立即重启</BaseButton>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<script setup>
import { computed, inject, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { providerLabel } from '../asrEngineStatus.mjs'
import { SETTINGS_CTX } from '../settingsContext.js'
import { apiTokenDisplay } from '../apiAccess.mjs'
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
import BackupExportDialog from '../BackupExportDialog.vue'
import { api } from '@/api/client'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import { useElectron } from '@/composables/useElectron'
import { createLatestRequestGuard, runLatest } from '@/utils/latestRequest.mjs'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { form } = inject(SETTINGS_CTX)
const {
  isDesktop,
  getAppInfo,
  revealAppPath,
  chooseBackupExportPath,
  chooseBackupImportPath,
  clipboard,
  recording,
  restartApp
} = useElectron()
const appInfo = ref(null)
const runtime = computed(() => settingsStore.runtimeStatus || {})
const exportDialogOpen = ref(false)
const importConfirmOpen = ref(false)
const busyPanel = ref(null)
const restartPanel = ref(null)
const exportButton = ref(null)
const importButton = ref(null)
const exporting = ref(false)
const importing = ref(false)
const restarting = ref(false)
const restartRequired = ref(false)
const importBackupPath = ref('')
const importCredentialsReset = ref(0)
const importRecordCount = ref(0)
const importSummary = computed(() => {
  const records = importRecordCount.value
  return records > 0 ? `已恢复 ${records} 条记录。` : '已恢复备份内容。'
})

// ---- 外部 API ----
const revealedToken = ref('')
const tokenRevealed = ref(false)
const revealingToken = ref(false)
const regeneratingToken = ref(false)
const isTokenRegenerateVisible = ref(false)
const hasApiToken = computed(() => Boolean(form.value.api_token_set))
const tokenDisplay = computed(() => apiTokenDisplay({
  enabled: form.value.api_token_enabled === true,
  hasToken: hasApiToken.value,
  revealedToken: tokenRevealed.value ? revealedToken.value : ''
}))

watch(() => form.value.api_token_enabled, (enabled) => {
  if (!enabled) {
    tokenRevealed.value = false
    revealedToken.value = ''
  }
})

async function loadRevealedToken() {
  revealingToken.value = true
  try {
    const result = await api.revealApiToken()
    if (!result?.enabled) {
      throw new Error('持久访问令牌未开启')
    }
    if (!result.token) {
      throw new Error('令牌尚未生成，请稍等设置保存后重试')
    }
    revealedToken.value = result.token
    return result.token
  } finally {
    revealingToken.value = false
  }
}

async function toggleToken() {
  if (tokenRevealed.value) {
    tokenRevealed.value = false
    return
  }
  try {
    await loadRevealedToken()
    tokenRevealed.value = true
  } catch (error) {
    appStore.showToast(String(error?.message || error), { tone: 'error' })
  }
}

async function copyToken() {
  try {
    const token = revealedToken.value || await loadRevealedToken()
    await clipboard.writeText(token)
    appStore.showToast('持久访问令牌已复制', { tone: 'success' })
  } catch (error) {
    appStore.showToast(String(error?.message || error), { tone: 'error' })
  }
}

async function regenerateToken() {
  if (!form.value.api_token_enabled || !hasApiToken.value) {
    return
  }
  regeneratingToken.value = true
  tokenRevealed.value = false
  revealedToken.value = ''
  try {
    const result = await api.saveSettings({ api_token_regenerate: true })
    form.value.api_token_enabled = result.api_token_enabled === true
    form.value.api_token_set = result.api_token_set === true
    await settingsStore.loadSettings()
    await loadRevealedToken()
    tokenRevealed.value = true
    appStore.showToast('持久访问令牌已重新生成，旧令牌已失效', { tone: 'success' })
  } catch (error) {
    appStore.showToast(String(error?.message || error), { tone: 'error' })
  } finally {
    regeneratingToken.value = false
  }
}

// ---- 存储 ----
const storage = ref(null)
const storageLoading = ref(false)
const storageError = ref('')
const storageRequest = createLatestRequestGuard()
let retentionRefreshTimer = null
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
  await runLatest(storageRequest, () => api.getStorageUsage(refresh), {
    onStart: () => {
      storageLoading.value = true
      storageError.value = ''
    },
    onResult: (result) => {
      storage.value = result
    },
    onError: (error) => {
      storageError.value = `统计数据占用失败：${error?.message || error}`
    },
    onFinally: () => {
      storageLoading.value = false
    }
  })
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
    clearTimeout(retentionRefreshTimer)
    retentionRefreshTimer = setTimeout(() => loadStorage(true), 1500)
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

function formatSize(bytes) {
  const value = Number(bytes) || 0
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  if (value < 1024 * 1024 * 1024) return `${(value / 1024 / 1024).toFixed(1)} MB`
  return `${(value / 1024 / 1024 / 1024).toFixed(2)} GB`
}

async function startExport(options) {
  exporting.value = true
  try {
    await nextTick()
    focusBusyPanel()
    await recording.cancel()
    const path = await chooseBackupExportPath()
    if (!path) return
    await recording.cancel()
    const result = await api.exportBackup({
      path,
      include_key: options.include_key,
      include_audio: options.include_audio
    })
    appStore.showToast(`备份已导出（${formatSize(result.bytes)}）：${result.path}`, { tone: 'success', duration: 8000 })
  } catch (error) {
    appStore.showToast(`导出备份失败：${error?.message || error}`, { tone: 'error', duration: 10000 })
  } finally {
    exporting.value = false
    await restoreActionFocus(exportButton)
  }
}

async function startImport() {
  importing.value = true
  try {
    await nextTick()
    focusBusyPanel()
    await recording.cancel()
    const path = await chooseBackupImportPath()
    if (!path) return
    await recording.cancel()
    const result = await api.importBackup({ path })
    importBackupPath.value = result.pre_import_backup || ''
    importCredentialsReset.value = Number(result.credentials_reset) || 0
    importRecordCount.value = Number(result.manifest?.sections?.records?.files) || 0
    restartRequired.value = result.restart_required !== false
    if (restartRequired.value) {
      await nextTick()
      focusRestartButton()
      await restartNow()
    }
    appStore.showToast(
      restartRequired.value ? '数据已恢复，请重启应用后生效' : '数据已恢复',
      { tone: 'success', duration: 10000 }
    )
  } catch (error) {
    appStore.showToast(`恢复备份失败：${error?.message || error}`, { tone: 'error', duration: 10000 })
  } finally {
    importing.value = false
    if (!restartRequired.value) {
      await restoreActionFocus(importButton)
    }
  }
}

function focusBusyPanel() {
  busyPanel.value?.focus()
}

async function restoreActionFocus(target) {
  await nextTick()
  const element = target.value?.$el || target.value
  element?.focus?.()
}

function focusRestartButton() {
  restartPanel.value?.querySelector('button')?.focus()
}

async function restartNow() {
  if (restarting.value) return
  restarting.value = true
  try {
    await restartApp()
  } catch (error) {
    restarting.value = false
    appStore.showToast(`重启应用失败：${error?.message || error}`, { tone: 'error', duration: 10000 })
  }
}

onMounted(async () => {
  settingsStore.loadRuntimeStatus().catch(() => {})
  void loadStorage()
  try {
    appInfo.value = await getAppInfo()
  } catch (error) {
    console.warn('[Settings] 读取应用信息失败：', error)
  }
})

onBeforeUnmount(() => {
  clearTimeout(retentionRefreshTimer)
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

.diag-restart {
  align-items: flex-start;
}

.diag-restart__body {
  display: grid;
  gap: var(--s-1);
  min-width: 0;
}

.diag-restart__title {
  color: var(--text-1);
  font-weight: 600;
}

.diag-restart__detail {
  color: var(--text-2);
  font-size: var(--fs-sm);
}

.backup-restart-gate {
  position: fixed;
  inset: 0;
  z-index: calc(var(--z-modal) + 1);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: var(--s-6);
  background: var(--scrim-soft);
  backdrop-filter: blur(8px);
}

.backup-busy-gate {
  position: fixed;
  inset: 0;
  z-index: var(--z-modal);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: var(--s-6);
  background: var(--scrim-soft);
  backdrop-filter: blur(6px);
}

.backup-busy-gate__panel {
  display: flex;
  align-items: flex-start;
  gap: var(--s-4);
  width: min(430px, 100%);
  padding: var(--s-4);
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  background: var(--bg-surface);
  box-shadow: var(--e-3), var(--edge-hi);
}

.backup-busy-gate__panel strong {
  color: var(--text-1);
  font-size: var(--fs-sm);
}

.backup-busy-gate__panel p {
  margin: var(--s-1) 0 0;
  color: var(--text-2);
  font-size: var(--fs-sm);
  line-height: 1.6;
}

.backup-busy-gate__spinner {
  flex: none;
  width: 18px;
  height: 18px;
  margin-top: 1px;
  border: 2px solid var(--accent);
  border-top-color: transparent;
  border-radius: var(--r-full);
  animation: backup-spin 0.8s linear infinite;
}

.backup-restart-gate__panel {
  width: min(520px, 100%);
  max-height: calc(100dvh - 2 * var(--s-6));
  overflow-y: auto;
  padding: var(--s-6);
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  background: var(--bg-surface);
  box-shadow: var(--e-3), var(--edge-hi);
}

.backup-restart-gate__title {
  margin: 0 0 var(--s-3);
  color: var(--text-1);
  font-size: var(--fs-md);
  font-weight: 600;
}

.backup-restart-gate__body {
  display: grid;
  gap: var(--s-3);
  margin-bottom: var(--s-6);
  color: var(--text-2);
  font-size: var(--fs-sm);
  line-height: 1.7;
}

.backup-restart-gate__body p {
  margin: 0;
}

@keyframes backup-spin {
  to {
    transform: rotate(360deg);
  }
}

@media (prefers-reduced-motion: reduce) {
  .backup-busy-gate__spinner {
    animation-duration: 1.6s;
  }
}
</style>
