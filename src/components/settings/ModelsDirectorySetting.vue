<template>
  <BaseButton
    variant="outline"
    size="sm"
    :disabled="!view.canChange"
    :loading="isPicking"
    @click="pickDirectory"
  >
    更改…
  </BaseButton>
  <BaseButton
    v-if="view.canReset"
    variant="ghost"
    size="sm"
    @click="resetDirectory"
  >
    恢复默认
  </BaseButton>
  <div
    v-if="view.message"
    class="settings-note model-directory-note"
    :class="{ warn: view.restartPending }"
    role="status"
  >
    <span>{{ view.message }}</span>
    <BaseButton
      v-if="view.restartPending"
      variant="outline"
      size="sm"
      :disabled="!view.canRestart"
      :loading="isRestarting"
      @click="restartApplication"
    >
      立即重启
    </BaseButton>
  </div>
</template>

<script setup>
import { computed, inject, ref } from 'vue'
import BaseButton from '@/components/common/BaseButton.vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import { useElectron } from '@/composables/useElectron'
import { SETTINGS_CTX } from './settingsContext.js'
import { modelsDirectoryView } from './modelsDirectory.mjs'

defineOptions({ name: 'ModelsDirectorySetting' })

const { form } = inject(SETTINGS_CTX)
const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { isDesktop, openModelsDirectory, restartApp } = useElectron()
const isPicking = ref(false)
const isRestarting = ref(false)

const view = computed(() => modelsDirectoryView({
  savedDir: settingsStore.settings.models_dir,
  draftDir: form.value.models_dir,
  effectiveDir: settingsStore.settings.models_dir_effective,
  nextDir: settingsStore.settings.models_dir_next,
  envOverride: Boolean(settingsStore.settings.models_dir_env_override),
  desktop: isDesktop,
  picking: isPicking.value,
  restarting: isRestarting.value
}))

async function pickDirectory() {
  if (!view.value.canChange || isPicking.value) {
    return
  }
  isPicking.value = true
  try {
    const picked = await openModelsDirectory()
    if (picked) {
      form.value.models_dir = picked
    }
  } catch (error) {
    appStore.showToast(String(error?.message || error || '无法选择模型目录'), { tone: 'error' })
  } finally {
    isPicking.value = false
  }
}

function resetDirectory() {
  if (view.value.canReset) {
    form.value.models_dir = ''
  }
}

async function restartApplication() {
  if (!view.value.canRestart || isRestarting.value) {
    return
  }
  isRestarting.value = true
  try {
    await restartApp()
  } catch (error) {
    isRestarting.value = false
    appStore.showToast(String(error?.message || error || '重启应用失败'), { tone: 'error' })
  }
}
</script>

<style scoped>
.model-directory-note {
  flex-basis: 100%;
  width: 100%;
  margin: var(--s-2) 0 0;
}
</style>
