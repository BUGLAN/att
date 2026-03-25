<template>
  <Modal title="LLM 配置管理" modal-name="llm">
    <LLMConfigManager
      :llm-configs="llmConfigs"
      @create="openEditModal"
      @edit="openEditModal"
      @delete="deleteConfig"
      @test="testConfig"
      @set-default="setDefaultConfig"
    />

    <template #footer>
      <button class="btn btn-secondary" @click="close">关闭</button>
    </template>
  </Modal>
</template>

<script setup>
import { onMounted } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import Modal from '@/components/common/Modal.vue'
import LLMConfigManager from '@/components/settings/LLMConfigManager.vue'
import { useLLMConfig } from '@/composables/useSettings'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { llmConfigs, loadLLMConfigs, deleteLLMConfig, testLLMConfig, setDefaultLLMConfig } = useLLMConfig()

onMounted(() => {
  loadLLMConfigs()
})

function close() {
  appStore.closeModal('llm')
}

function openEditModal(config = null) {
  settingsStore.currentLLMConfig = config
  appStore.closeModal('llm')
  appStore.openModal('llmEdit')
}

async function deleteConfig(id) {
  if (confirm('确定要删除这个配置吗？')) {
    await deleteLLMConfig(id)
  }
}
</script>
