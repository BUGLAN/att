<template>
  <Modal :title="isEdit ? '编辑 LLM 配置' : '新建 LLM 配置'" modal-name="llmEdit">
    <div class="form-group">
      <label>服务商</label>
      <BaseSelect v-model="form.provider" @change="handleProviderChange">
        <option v-for="provider in providers" :key="provider.id" :value="provider.id">
          {{ provider.name }}
        </option>
      </BaseSelect>
    </div>

    <div class="form-group">
      <label>配置名称</label>
      <BaseInput v-model="form.name" type="text" placeholder="例如：我的 DeepSeek" />
    </div>

    <div class="form-group">
      <label>API Key</label>
      <BaseInput v-model="form.api_key" type="password" :placeholder="isEdit ? '留空保持现有密钥' : 'sk-...'" />
      <span class="form-hint">编辑时留空会保留现有 API Key。</span>
    </div>

    <div class="form-group">
      <label>API Base</label>
      <BaseInput v-model="form.api_base" type="text" placeholder="留空使用默认端点" />
    </div>

    <div class="form-group">
      <label>模型</label>
      <BaseSelect v-model="form.model">
        <option v-for="model in availableModels" :key="model" :value="model">
          {{ model }}
        </option>
      </BaseSelect>
    </div>

    <div class="form-row">
      <div class="form-group half">
        <label>Temperature</label>
        <BaseInput v-model.number="form.temperature" type="number" min="0" max="2" step="0.1" />
      </div>
      <div class="form-group half">
        <label>Max Tokens</label>
        <BaseInput v-model.number="form.max_tokens" type="number" min="100" max="32000" step="100" />
      </div>
    </div>

    <template #footer>
      <BaseButton variant="outline" @click="close">取消</BaseButton>
      <BaseButton variant="solid" @click="save">保存</BaseButton>
    </template>
  </Modal>
</template>

<script setup>
import { computed, onMounted, ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import Modal from '@/components/common/Modal.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseInput from '@/components/common/BaseInput.vue'
import BaseSelect from '@/components/common/BaseSelect.vue'
import { useLLMConfig } from '@/composables/useSettings'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { saveLLMConfig, loadLLMProviders, llmProviders } = useLLMConfig()

const isEdit = computed(() => !!settingsStore.currentLLMConfig?.id)
const providers = computed(() => llmProviders.value)
const availableModels = ref([])

const form = ref({
  name: '',
  provider: 'deepseek',
  api_key: '',
  api_base: '',
  model: '',
  temperature: 0.7,
  max_tokens: 4096
})

function applyProviderDefaults(providerId) {
  const provider = providers.value.find((item) => item.id === providerId)
  const models = provider?.models || []
  availableModels.value = models.length ? models : ['']

  if (!form.value.model || !models.includes(form.value.model)) {
    form.value.model = provider?.default_model || models[0] || ''
  }

  if (!form.value.api_base) {
    form.value.api_base = provider?.default_base_url || ''
  }
}

function handleProviderChange() {
  form.value.api_base = ''
  form.value.model = ''
  applyProviderDefaults(form.value.provider)
}

watch(() => settingsStore.currentLLMConfig, (config) => {
  if (!config) {
    form.value = {
      name: '',
      provider: providers.value[0]?.id || 'deepseek',
      api_key: '',
      api_base: '',
      model: '',
      temperature: 0.7,
      max_tokens: 4096
    }
    applyProviderDefaults(form.value.provider)
    return
  }

  form.value = {
    name: config.name || '',
    provider: config.provider || 'deepseek',
    api_key: '',
    api_base: config.api_base || '',
    model: config.model || '',
    temperature: config.temperature ?? 0.7,
    max_tokens: config.max_tokens ?? 4096
  }
  applyProviderDefaults(form.value.provider)
}, { immediate: true })

async function ensureProvidersLoaded() {
  await loadLLMProviders()
  if (!form.value.provider) {
    form.value.provider = providers.value[0]?.id || 'deepseek'
  }
  applyProviderDefaults(form.value.provider)
}

function close() {
  appStore.closeModal('llmEdit')
  appStore.openModal('llm')
}

function buildPayload() {
  return {
    id: settingsStore.currentLLMConfig?.id,
    name: form.value.name,
    provider: form.value.provider,
    model: form.value.model,
    api_key: form.value.api_key || undefined,
    api_base: form.value.api_base || null,
    temperature: Number(form.value.temperature) || 0.7,
    max_tokens: Number(form.value.max_tokens) || 4096
  }
}

async function save() {
  if (!form.value.name.trim()) {
    appStore.showToast('请输入配置名称', { tone: 'error' })
    return
  }

  if (!form.value.api_key && !isEdit.value) {
    appStore.showToast('请输入 API Key', { tone: 'error' })
    return
  }

  try {
    await saveLLMConfig(buildPayload())
    close()
  } catch (error) {
    appStore.showToast('保存配置失败', { tone: 'error' })
  }
}

onMounted(() => {
  ensureProvidersLoaded()
})
</script>
