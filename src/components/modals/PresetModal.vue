<template>
  <Modal title="预设管理" modal-name="preset">
    <PresetManager
      :presets="builtinPresets"
      :default-preset-id="defaultPresetId"
      @view="openViewModal"
      @select="handleSelect"
    />

    <template #footer>
      <BaseButton variant="outline" @click="close">关闭</BaseButton>
    </template>
  </Modal>
</template>

<script setup>
import { computed, onMounted } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import Modal from '@/components/common/Modal.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import PresetManager from '@/components/settings/PresetManager.vue'
import { usePresets } from '@/composables/useSettings'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { presets, defaultPresetId: storeDefaultPresetId, loadPresets, setDefaultPreset } = usePresets()

const defaultPresetId = computed(() => storeDefaultPresetId.value)
const builtinPresets = computed(() => presets.value.filter((preset) => preset.is_builtin))

onMounted(() => {
  loadPresets()
})

function close() {
  appStore.closeModal('preset')
}

function openViewModal(preset) {
  settingsStore.currentPreset = preset
  appStore.closeModal('preset')
  appStore.openModal('presetEdit')
}

async function handleSelect(id) {
  await setDefaultPreset(id)
}
</script>
