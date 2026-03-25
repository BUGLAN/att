<template>
  <Modal title="预设管理" modal-name="preset">
    <PresetManager
      :presets="presets"
      @create="openEditModal"
      @edit="openEditModal"
      @delete="deletePreset"
      @select="setDefaultPreset"
    />

    <template #footer>
      <button class="btn btn-secondary" @click="close">关闭</button>
    </template>
  </Modal>
</template>

<script setup>
import { computed, onMounted } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import Modal from '@/components/common/Modal.vue'
import PresetManager from '@/components/settings/PresetManager.vue'
import { usePresets } from '@/composables/useSettings'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { presets, loadPresets, deletePreset, setDefaultPreset } = usePresets()

onMounted(() => {
  loadPresets()
})

function close() {
  appStore.closeModal('preset')
}

function openEditModal(preset = null) {
  settingsStore.currentPreset = preset
  appStore.closeModal('preset')
  appStore.openModal('presetEdit')
}

async function handleDelete(id) {
  if (confirm('确定要删除这个预设吗？')) {
    await deletePreset(id)
  }
}
</script>
