<template>
  <Modal :title="isEdit ? '编辑 LLM 配置' : '添加 LLM 配置'" modal-name="llmEdit">
    <div class="form-group">
      <label>名称</label>
      <input
        type="text"
        class="form-input"
        v-model="form.name"
        placeholder="配置名称"
      >
    </div>

    <div class="form-row">
      <div class="form-group half">
        <label>提供商</label>
        <select class="form-select" v-model="form.provider" @change="onProviderChange">
          <option v-for="provider in providers" :key="provider.id" :value="provider.id">
            {{ provider.name }}
          </option>
        </select>
      </div>

      <div class="form-group half">
        <label>模型</label>
        <select class="form-select" v-model="form.model">
          <option v-for="model in models" :key="model" :value="model">
            {{ model }}
          </option>
        </select>
      </div>
    </div>

    <div class="form-group">
      <label>API Key</label>
      <input
        type="password"
        class="form-input"
        v-model="form.api_key"
        placeholder="API Key"
      >
      <span class="form-hint">API 密钥将安全存储</span>
    </div>

    <div class="form-group">
      <label>API Base URL</label>
      <input
        type="text"
        class="form-input"
        v-model="form.base_url"
        placeholder="可选，用于自定义端点"
      >
    </div>

    <template #footer>
      <button class="btn btn-secondary" @click="close">取消</button>
      <button class="btn" @click="test" :disabled="testing">
        {{ testing ? '测试中...' : '测试连接' }}
      </button>
      <button class="btn btn-primary" @click="save">保存</button>
    </template>
  </Modal>
</template>

<script setup>
import { ref, computed, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import Modal from '@/components/common/Modal.vue'
import { useLLMConfig } from '@/composables/useSettings'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { saveLLMConfig, testLLMConfig, loadLLMProviders, llmProviders } = useLLMConfig()

const isEdit = computed(() => !!settingsStore.currentLLMConfig?.id)
const testing = ref(false)

const providers = computed(() => llmProviders.value)
const models = ref([])

const form = ref({
  name: '',
  provider: 'deepseek',
  model: 'deepseek-chat',
  api_key: '',
  base_url: ''
})

// 监听当前配置
watch(() => settingsStore.currentLLMConfig, (config) => {
  if (config) {
    form.value = {
      name: config.name || '',
      provider: config.provider || 'deepseek',
      model: config.model || 'deepseek-chat',
      api_key: '',
      base_url: config.base_url || ''
    }
    loadModels(form.value.provider)
  } else {
    form.value = {
      name: '',
      provider: 'deepseek',
      model: 'deepseek-chat',
      api_key: '',
      base_url: ''
    }
    loadModels('deepseek')
  }
}, { immediate: true })

// 加载提供商
loadLLMProviders()

function close() {
  appStore.closeModal('llmEdit')
  appStore.openModal('llm')
}

async function loadModels(provider) {
  // 根据提供商设置可用模型
  const providerModels = {
    deepseek: ['deepseek-chat', 'deepseek-coder'],
    openai: ['gpt-4o', 'gpt-4o-mini', 'gpt-4-turbo', 'gpt-3.5-turbo'],
    anthropic: ['claude-sonnet-4-20250514', 'claude-3-5-haiku-20241022', 'claude-3-opus-20240229'],
    custom: ['custom-model']
  }

  models.value = providerModels[provider] || []
  if (models.value.length > 0 && !models.value.includes(form.value.model)) {
    form.value.model = models.value[0]
  }
}

function onProviderChange() {
  loadModels(form.value.provider)
}

async function test() {
  if (!form.value.api_key) {
    appStore.showToast('请输入 API Key')
    return
  }

  testing.value = true
  try {
    // 先保存配置，然后测试
    const config = {
      ...form.value,
      id: settingsStore.currentLLMConfig?.id
    }
    const saved = await saveLLMConfig(config)
    await testLLMConfig(saved.id)
  } finally {
    testing.value = false
  }
}

async function save() {
  if (!form.value.name.trim()) {
    appStore.showToast('请输入配置名称')
    return
  }
  if (!form.value.api_key && !isEdit.value) {
    appStore.showToast('请输入 API Key')
    return
  }

  try {
    await saveLLMConfig({
      id: settingsStore.currentLLMConfig?.id,
      ...form.value
    })
    close()
  } catch (error) {
    appStore.showToast('保存失败')
  }
}
</script>
