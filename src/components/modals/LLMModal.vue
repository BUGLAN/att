<template>
  <Modal title="LLM 配置管理" modal-name="llm">
    <LLMConfigManager
      :llm-configs="llmConfigs"
      :default-config-id="defaultConfigId"
      @create="openEditModal()"
      @edit="openEditModal"
      @delete="requestDelete"
    />
  </Modal>

  <!-- 危险操作统一走二次确认，替换原来的原生 confirm() -->
  <ConfirmDialog
    v-model="isDeleteConfirmVisible"
    title="删除这个 LLM 配置？"
    description="删除后需要重新填写 API Key 才能再次使用该服务商。"
    confirm-text="删除"
    @confirm="confirmDelete"
  />
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import Modal from '@/components/common/Modal.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
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
const pendingDeleteId = ref(null)
const isDeleteConfirmVisible = ref(false)

onMounted(() => {
  loadLLMConfigs()
})

function openEditModal(config = null) {
  settingsStore.currentLLMConfig = config
  appStore.closeModal('llm')
  appStore.openModal('llmEdit')
}

function requestDelete(id) {
  pendingDeleteId.value = id
  isDeleteConfirmVisible.value = true
}

async function confirmDelete() {
  const id = pendingDeleteId.value
  pendingDeleteId.value = null

  if (!id) {
    return
  }

  await deleteLLMConfig(id)
}
</script>
