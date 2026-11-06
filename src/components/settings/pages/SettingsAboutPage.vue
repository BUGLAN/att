<template>
  <div class="settings-stack">
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">关于</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">落墨 Luomo</span>
            <span class="setting-desc">
              {{ versionText }}
              <BaseBadge v-if="badge" class="about-badge" :tone="badge.tone">{{ badge.text }}</BaseBadge>
            </span>
          </div>
        </div>
      </div>
    </section>

    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">更新</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">检查更新</span>
            <span class="setting-desc">
              <span v-if="checkLine.error" class="about-error">{{ checkLine.error }}</span>
              <template v-if="checkLine.error && checkLine.meta"> · </template>{{ checkLine.meta }}
            </span>
          </div>
          <div class="setting-actions-inline about-actions">
            <BaseButton variant="outline" :loading="isChecking" :disabled="isChecking" @click="checkNow">
              {{ isChecking ? '正在检查…' : '检查更新' }}
            </BaseButton>
          </div>
        </div>

        <div v-if="loadError" class="settings-note about-note is-err" role="alert">
          <span>读取更新状态失败：{{ loadError }}</span>
        </div>

        <div v-if="checkView && check.status === 'available'" class="settings-note about-note" role="status">
          <div class="about-note__body">
            <strong class="about-note__title">{{ checkView.title }}</strong>
            <span v-if="checkView.detail" class="about-note__detail">{{ checkView.detail }}</span>
            <pre v-if="notes" class="about-note__notes">{{ notes }}</pre>
            <span v-if="downloadText" class="about-note__detail" :class="{ 'is-err-text': download.status === 'error' }">{{ downloadText }}</span>
            <div v-if="percent !== null && download.status === 'downloading'" class="about-progress" aria-hidden="true">
              <span class="about-progress__bar" :style="{ width: `${percent}%` }"></span>
            </div>
          </div>
          <div class="about-note__actions">
            <BaseButton v-if="canRestart" variant="solid" size="sm" :loading="isInstalling" @click="installNow">
              重启并更新
            </BaseButton>
            <BaseButton v-if="canDownload" variant="solid" size="sm" :loading="isStartingDownload" @click="downloadNow()">
              {{ download.status === 'error' ? '重试' : inApp ? '下载更新' : '下载安装包' }}
            </BaseButton>
            <BaseButton v-if="canFallback" variant="outline" size="sm" @click="downloadNow({ installer: true })">
              改为下载安装包
            </BaseButton>
            <BaseButton
              v-if="download.status === 'done' && download.mode !== 'in_app' && isDesktop"
              variant="outline"
              size="sm"
              @click="revealDownloaded"
            >
              在文件管理器中显示
            </BaseButton>
          </div>
        </div>

        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">自动下载更新</span>
            <span class="setting-desc">{{ autoDownloadDesc }}</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.auto_download_updates" type="checkbox" aria-label="自动下载更新">
            <span class="toggle-slider"></span>
          </label>
        </div>
      </div>
    </section>
  </div>
</template>

<script setup>
import { computed, inject, onMounted, onUnmounted, ref, watch } from 'vue'
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import { api } from '@/api/client'
import { useAppStore } from '@/stores/app'
import { useElectron } from '@/composables/useElectron'
import { SETTINGS_CTX } from '../settingsContext.js'
import {
  canFallbackToInstaller,
  canInstall,
  canStartDownload,
  describeCheck,
  describeCheckLine,
  describeDownload,
  downloadPercent,
  summarizeNotes,
  versionBadge
} from '../updateView.mjs'

// 自动下载开关走设置表单：与其他设置一样防抖保存到 settings.json（auto_download_updates）
const { form } = inject(SETTINGS_CTX)
const appStore = useAppStore()
const { isDesktop, getAppInfo, revealAppPath } = useElectron()

const appInfo = ref(null)
const status = ref(null)
const loadError = ref('')
const isChecking = ref(false)
const isStartingDownload = ref(false)
const isInstalling = ref(false)
let pollTimer = null

const currentVersion = computed(() => appInfo.value?.version || status.value?.current_version || '')
const versionText = computed(() => {
  const version = currentVersion.value ? `版本 ${currentVersion.value}` : '版本未知'
  return `${version} · ${isDesktop ? 'Tauri 桌面版' : '浏览器模式'}`
})
const check = computed(() => status.value?.check || null)
const download = computed(() => status.value?.download || { status: 'idle' })
// 桌面版有应用内更新：下载签名更新包，下好后「重启并更新」；否则下载完整安装包手动安装
const inApp = computed(() => status.value?.in_app === true)
const checkView = computed(() => describeCheck(check.value, { inApp: inApp.value }))
const notes = computed(() => (check.value?.status === 'available' ? summarizeNotes(check.value.notes) : ''))
const downloadText = computed(() => (check.value?.status === 'available' ? describeDownload(download.value) : ''))
const percent = computed(() => downloadPercent(download.value))
const canDownload = computed(() => canStartDownload(check.value, download.value, { inApp: inApp.value }))
const canRestart = computed(() => canInstall(download.value))
const canFallback = computed(() => canFallbackToInstaller(check.value, download.value))
const autoDownloadDesc = computed(() => (inApp.value
  ? '有新版本时在后台下载，下好后提示你重启更新。'
  : '有新版本时自动下载安装包。'))
const checkLine = computed(() => describeCheckLine(check.value))
const badge = computed(() => versionBadge(check.value))

function applyStatus(next) {
  status.value = next
  loadError.value = ''
  // 安装失败时状态回到 error：按钮恢复可点
  if (next?.download?.status !== 'installing') {
    isInstalling.value = false
  }
  // 下载 / 安装中：轮询进度；结束后停止（安装成功时应用直接重启，轮询随之中断）
  if (['downloading', 'installing'].includes(next?.download?.status)) {
    schedulePoll()
  }
}

function schedulePoll() {
  if (pollTimer) {
    return
  }
  pollTimer = setTimeout(async () => {
    pollTimer = null
    await refreshStatus()
  }, 1000)
}

async function refreshStatus() {
  try {
    applyStatus(await api.getUpdateStatus())
  } catch (error) {
    loadError.value = String(error?.message || error)
  }
}

async function checkNow() {
  isChecking.value = true
  try {
    applyStatus(await api.checkForUpdates())
  } catch (error) {
    // 接口本身不可达（数据面断开等）：同样把原因显示出来
    status.value = {
      ...(status.value || {}),
      check: {
        status: 'error',
        current_version: currentVersion.value,
        error: String(error?.message || error),
        checked_at: new Date().toISOString()
      }
    }
  } finally {
    isChecking.value = false
  }
}

async function downloadNow(options = {}) {
  isStartingDownload.value = true
  try {
    applyStatus(await api.downloadUpdate(options))
  } catch (error) {
    appStore.showToast(String(error?.message || error), { tone: 'error' })
  } finally {
    isStartingDownload.value = false
  }
}

async function installNow() {
  isInstalling.value = true
  try {
    applyStatus(await api.installUpdate())
  } catch (error) {
    isInstalling.value = false
    appStore.showToast(String(error?.message || error), { tone: 'error' })
  }
}

async function revealDownloaded() {
  try {
    await revealAppPath('update')
  } catch (error) {
    appStore.showToast(String(error?.message || error), { tone: 'error' })
  }
}

// 打开开关时如果已经检查到新版本，立即补一次下载，免得要再点一次「检查更新」
watch(() => form.value.auto_download_updates, (enabled, previous) => {
  if (enabled && previous === false && canDownload.value) {
    downloadNow()
  }
})

onMounted(async () => {
  refreshStatus()
  try {
    appInfo.value = await getAppInfo()
  } catch (error) {
    console.warn('[Settings] 读取应用信息失败：', error)
  }
})

onUnmounted(() => {
  if (pollTimer) {
    clearTimeout(pollTimer)
    pollTimer = null
  }
})
</script>

<style scoped>
.about-actions {
  justify-content: flex-end;
}

/* 版本号旁的小标记：样式全用 BaseBadge（tone ok / info），这里只管与版本文字的间距 */
.about-badge {
  margin-left: var(--s-2);
  vertical-align: 1px;
}

/* 「检查更新」行内的失败原因 */
.about-error {
  color: var(--err);
  word-break: break-all;
}

/* 发现新版本的提示条：沿用 .settings-note（读取状态失败也用它，带 is-err） */
.about-note {
  align-items: flex-start;
  margin-bottom: var(--s-3);
}

.about-note.is-err {
  background: var(--err-soft);
  border-color: var(--err-line);
}

.about-note__body {
  display: flex;
  flex-direction: column;
  gap: var(--s-1);
  min-width: 0;
  flex: 1;
}

.about-note__title {
  color: var(--text-1);
  font-weight: 600;
}

.about-note__detail {
  word-break: break-all;
}

.is-err-text {
  color: var(--err);
}

.about-note__notes {
  margin: var(--s-1) 0 0;
  font-family: inherit;
  white-space: pre-wrap;
  color: var(--text-3);
}

.about-note__actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--s-2);
  flex-shrink: 0;
}

.about-progress {
  height: 4px;
  border-radius: 2px;
  background: var(--line);
  overflow: hidden;
}

.about-progress__bar {
  display: block;
  height: 100%;
  background: var(--info);
  transition: width 0.3s ease;
}
</style>
