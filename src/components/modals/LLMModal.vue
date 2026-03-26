<template>
  <Modal title="LLM 配置管理" modal-name="llm">
    <LLMConfigManager
      :llm-configs="llmConfigs"
      :default-config-id="defaultConfigId"
      @create="openEditModal()"
      @edit="openEditModal"
      @delete="handleDelete"
    />
  </Modal>
</template>

<script setup>
import { computed, onMounted } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import Modal from '@/components/common/Modal.vue'
import LLMConfigManager from '@/components/settings/LLMConfigManager.vue'
import { useLLMConfig } from '@/composables/useSettings'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const {
  llmConfigs,
  loadLLMConfigs,
  deleteLLMConfig,
  defaultConfigId: storeDefaultConfigId
} = useLLMConfig()

const defaultConfigId = computed(() => storeDefaultConfigId.value)

onMounted(() => {
  loadLLMConfigs()
})

function openEditModal(config = null) {
  settingsStore.currentLLMConfig = config
  appStore.closeModal('llm')
  appStore.openModal('llmEdit')
}

async function handleDelete(id) {
  if (!confirm('确定要删除这个配置吗？')) {
    return
  }

  await deleteLLMConfig(id)
}
</script>
