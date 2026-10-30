<template>
  <Modal :title="wizardTitle" modal-name="llmWizard">
    <!-- 步骤指示 -->
    <div class="wiz-steps">
      <span class="wiz-step" :class="{ on: step >= 1 }">1 选择</span>
      <span class="wiz-sep">—</span>
      <span class="wiz-step" :class="{ on: step >= 2 }">2 凭据</span>
      <span class="wiz-sep">—</span>
      <span class="wiz-step" :class="{ on: step >= 3 }">3 模型</span>
    </div>

    <!-- 步骤 1：选择供应商 -->
    <div v-if="step === 1">
      <p class="wiz-hint">
        内置 14 家服务商。未列出的 OpenAI 兼容端点（硅基流动、第三方中转等）选择「自定义 OpenAI 兼容」即可，无需等待应用更新。
      </p>
      <div class="wiz-search">
        <input v-model.trim="providerQuery" class="wiz-input" placeholder="搜索供应商，例如 deepseek、ollama、openrouter…">
      </div>
      <div class="wiz-grid">
        <button
          v-for="provider in filteredProviders"
          :key="provider.id"
          type="button"
          class="wiz-cell"
          :class="{ selected: selectedProvider?.id === provider.id }"
          @click="selectedProvider = provider"
        >
          <span class="wiz-cell-name">{{ provider.name }}</span>
          <span class="wiz-cell-prefix mono">{{ provider.litellm_prefix }}/…</span>
          <span class="wiz-cell-tag">{{ provider.requires_key ? '需密钥' : '无需密钥' }}</span>
        </button>
      </div>
      <p class="wiz-hint dim">目录随应用版本内置。</p>
    </div>

    <!-- 步骤 2：填写凭据（保存前验证） -->
    <div v-else-if="step === 2">
      <div class="wiz-field">
        <label class="wiz-label">档案名称</label>
        <input v-model.trim="form.name" class="wiz-input" placeholder="例如：DeepSeek 官方">
      </div>
      <div class="wiz-field">
        <label class="wiz-label">API Key</label>
        <input v-model="form.api_key" class="wiz-input" type="password" :placeholder="selectedProvider?.requires_key ? 'sk-…' : '本地服务可留空'">
        <p class="wiz-hint">仅保存在本机 data/（Fernet 加密），不会上传；保存前会先用它做一次真实验证，验证后即弃。</p>
      </div>
      <div class="wiz-field">
        <label class="wiz-label">API Base（可选，留空使用默认端点）</label>
        <input v-model.trim="form.api_base" class="wiz-input mono" :placeholder="basePlaceholder">
      </div>

      <div v-if="verifyState.status" class="wiz-banner" :class="verifyState.ok ? 'banner-ok' : 'banner-err'">
        <template v-if="verifyState.ok">
          ✓ 验证通过<span v-if="verifyState.latency">（{{ verifyState.latency }}）</span>，尚未保存；点「下一步」才会写入本机。
        </template>
        <template v-else>
          ✕ {{ verifyState.title }}：{{ verifyState.detail }}
        </template>
      </div>
    </div>

    <!-- 步骤 3：选择/输入模型 -->
    <div v-else>
      <div v-if="profileOptions.length > 1" class="wiz-field">
        <label class="wiz-label">添加到供应商档案</label>
        <select v-model="targetProfileId" class="wiz-input">
          <option v-for="profile in profileOptions" :key="profile.id" :value="profile.id">
            {{ profile.name }}
          </option>
        </select>
      </div>

      <div v-if="remoteLoading" class="wiz-banner banner-info">正在拉取上游模型列表…</div>
      <div v-else-if="remoteError" class="wiz-banner banner-warn">模型列表拉取失败（{{ remoteError }}），可直接手动输入模型名。</div>

      <div v-if="remoteModels.length" class="wiz-pick-list">
        <label v-for="name in remoteModels" :key="name" class="wiz-pick-item">
          <input
            v-model="checkedModels"
            type="checkbox"
            class="wiz-check"
            :value="name"
          >
          <span class="wiz-pick-name mono">{{ name }}</span>
        </label>
      </div>

      <div class="wiz-field" style="margin-top: 14px;">
        <label class="wiz-label">手动输入模型名（支持逗号 / 换行分隔多个）</label>
        <textarea v-model="manualInput" class="wiz-input mono" rows="2" placeholder="例如 deepseek-v4.2-preview（上游列表尚未收录时直接填这里）"></textarea>
        <p v-if="manualParsed.invalid.length" class="wiz-hint error-text">已忽略非法模型名：{{ manualParsed.invalid.join('、') }}</p>
      </div>

      <details class="wiz-collapse">
        <summary>模型参数（可稍后在编辑里修改）</summary>
        <div class="wiz-param-grid">
          <div class="wiz-field"><label class="wiz-label">Temperature</label><input v-model.number="form.params.temperature" type="number" min="0" max="2" step="0.1" class="wiz-input"></div>
          <div class="wiz-field"><label class="wiz-label">Max Tokens</label><input v-model.number="form.params.max_tokens" type="number" min="100" step="100" class="wiz-input"></div>
          <div class="wiz-field"><label class="wiz-label">请求超时（秒）</label><input v-model.number="form.params.timeout_s" type="number" min="5" max="600" class="wiz-input"></div>
          <div class="wiz-field"><label class="wiz-label">失败重试次数</label><input v-model.number="form.params.retries" type="number" min="0" max="5" class="wiz-input"></div>
          <div class="wiz-field">
            <label class="wiz-label">思考强度</label>
            <select v-model="form.params.reasoning_effort" class="wiz-input">
              <option v-for="item in REASONING_EFFORT_OPTIONS" :key="item.value" :value="item.value">{{ item.label }}</option>
            </select>
          </div>
        </div>
        <p class="wiz-hint">
          超时与重试由网关注入到每次请求，杜绝「网络黑洞挂 10 分钟」。
          语音润色建议把思考强度设为「关闭」：思考 token 计入 Max Tokens，占满后正文会为空。
        </p>
      </details>

      <label class="wiz-check-row">
        <input v-model="setAsDefault" type="checkbox" class="wiz-check">
        <span>保存后设为默认优化模型</span>
      </label>
    </div>

    <template #footer>
      <BaseButton v-if="step > 1" variant="ghost" @click="step -= 1">上一步</BaseButton>
      <span class="wiz-footer-spacer"></span>
      <BaseButton variant="outline" @click="close">取消</BaseButton>
      <BaseButton v-if="step === 1" variant="solid" :disabled="!selectedProvider" @click="goCredentials">下一步：填写凭据</BaseButton>
      <BaseButton v-else-if="step === 2" variant="solid" :disabled="!canSubmitCredentials" @click="verifyAndCreateProfile">
        下一步：验证并选择模型
      </BaseButton>
      <BaseButton v-else variant="solid" :disabled="!pendingModels.length" @click="saveModels">
        保存 {{ pendingModels.length }} 个模型
      </BaseButton>
    </template>
  </Modal>
</template>

<script setup>
import { computed, onMounted, ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import Modal from '@/components/common/Modal.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import { errorCopy, formatLatency, parseManualModelInput, REASONING_EFFORT_OPTIONS } from '@/components/settings/llm/llmLogic.mjs'

const appStore = useAppStore()
const settingsStore = useSettingsStore()

const step = ref(1)
const providerQuery = ref('')
const selectedProvider = ref(null)
const targetProfileId = ref('')
const remoteModels = ref([])
const checkedModels = ref([])
const remoteLoading = ref(false)
const remoteError = ref('')
const manualInput = ref('')
const setAsDefault = ref(true)
const verifyState = ref({ status: false, ok: false, title: '', detail: '', latency: '' })
const form = ref(defaultForm())

function defaultForm() {
  return {
    name: '',
    api_key: '',
    api_base: '',
    params: { temperature: 0.3, max_tokens: 4096, timeout_s: 60, retries: 2, reasoning_effort: '' }
  }
}

const wizardMode = computed(() => settingsStore.llmWizardMode || 'provider')
const wizardTitle = computed(() => {
  if (step.value === 1) return '添加供应商'
  if (step.value === 2) return `${selectedProvider.value?.name || ''} · 供应商档案`
  return '添加模型'
})

const filteredProviders = computed(() => {
  const query = providerQuery.value.toLowerCase()
  if (!query) {
    return settingsStore.llmProviders
  }
  return settingsStore.llmProviders.filter((provider) =>
    provider.name.toLowerCase().includes(query) ||
    provider.id.toLowerCase().includes(query) ||
    (provider.litellm_prefix || '').toLowerCase().includes(query)
  )
})

const basePlaceholder = computed(() => {
  if (selectedProvider.value?.id === 'custom') {
    return 'https://api.siliconflow.cn/v1（必填）'
  }
  return selectedProvider.value?.default_base_url || '留空使用默认端点'
})

const profileOptions = computed(() => settingsStore.llmProfiles)
const targetProfile = computed(() =>
  profileOptions.value.find((profile) => profile.id === targetProfileId.value) || null
)

const manualParsed = computed(() => parseManualModelInput(manualInput.value))

const pendingModels = computed(() => {
  const items = [...checkedModels.value, ...manualParsed.value.valid]
  return [...new Set(items)]
})

const canSubmitCredentials = computed(() => {
  if (!form.value.name) {
    return false
  }
  if (selectedProvider.value?.requires_key && !form.value.api_key) {
    return false
  }
  if (selectedProvider.value?.id === 'custom' && !form.value.api_base) {
    return false
  }
  return true
})

onMounted(async () => {
  await settingsStore.loadLLMProviders().catch(() => {})
  if (wizardMode.value === 'model' && settingsStore.currentLLMProfile) {
    // 从「添加模型」入口进来：跳过建档案，直接选模型
    targetProfileId.value = settingsStore.currentLLMProfile.id
    step.value = 3
  }
})

watch(targetProfileId, () => {
  checkedModels.value = []
  remoteModels.value = []
  remoteError.value = ''
  loadRemoteModels()
})

watch(step, (value) => {
  if (value === 3 && targetProfileId.value) {
    loadRemoteModels()
  }
})

async function loadRemoteModels() {
  const profile = targetProfile.value
  if (!profile) {
    return
  }
  remoteLoading.value = true
  remoteError.value = ''
  try {
    const result = await settingsStore.getLLMRemoteModels(profile.id)
    remoteModels.value = result.models || []
  } catch (error) {
    // 409 = 上游不支持列取；其余错误也统一降级为手输路径，不阻断向导
    remoteModels.value = []
    remoteError.value = error?.detail?.reason || error?.message || '未知错误'
  } finally {
    remoteLoading.value = false
  }
}

function goCredentials() {
  form.value = defaultForm()
  form.value.name = selectedProvider.value?.name || ''
  verifyState.value = { status: false, ok: false, title: '', detail: '', latency: '' }
  step.value = 2
}

async function verifyAndCreateProfile() {
  const provider = selectedProvider.value
  const draft = {
    provider: provider.id,
    api_key: form.value.api_key,
    api_base: form.value.api_base || null
  }
  verifyState.value = { status: true, ok: false, title: '验证中', detail: '正在连接上游…', latency: '' }
  try {
    const outcome = await settingsStore.verifyLLMDraft(draft)
    if (!outcome.ok) {
      const copy = errorCopy(outcome.error_code, outcome.message)
      verifyState.value = { status: true, ok: false, title: copy.title, detail: copy.detail, latency: '' }
      return
    }
    verifyState.value = {
      status: true,
      ok: true,
      title: '',
      detail: '',
      latency: formatLatency(outcome.latency_ms)
    }
    // 验证通过才落盘
    const profile = await settingsStore.createLLMProfile({
      provider: provider.id,
      name: form.value.name,
      api_key: form.value.api_key,
      api_base: form.value.api_base || null
    })
    targetProfileId.value = profile.id
    settingsStore.currentLLMProfile = profile
    step.value = 3
  } catch (error) {
    verifyState.value = { status: true, ok: false, title: '验证请求失败', detail: error?.message || '', latency: '' }
  }
}

async function saveModels() {
  const profileId = targetProfileId.value
  if (!profileId || !pendingModels.value.length) {
    return
  }
  try {
    const result = await settingsStore.createLLMModels(pendingModels.value.map((name) => ({
      profile_id: profileId,
      model: name,
      display_name: name.includes('/') ? name.split('/').pop() : name,
      params: { ...form.value.params },
      set_default: setAsDefault.value
    })))
    if (result.skipped?.length) {
      appStore.showToast(`已保存 ${result.created.length} 个模型，跳过 ${result.skipped.length} 个（${result.skipped[0].reason}）`, { tone: 'error', duration: 6000 })
    } else {
      appStore.showToast(`已保存 ${result.created.length} 个模型`)
    }
    close()
  } catch (error) {
    appStore.showToast(error?.message || '保存模型失败', { tone: 'error' })
  }
}

function close() {
  appStore.closeModal('llmWizard')
}
</script>

<style scoped>
.wiz-steps { display: flex; align-items: center; gap: 8px; margin-bottom: 14px; font-size: 11px; color: var(--text-3); }
.wiz-step.on { color: var(--accent-hi); font-weight: 600; }
.wiz-sep { opacity: 0.5; }

.wiz-hint { margin: 0 0 12px; font-size: 12px; color: var(--text-3); line-height: 1.7; }
.wiz-hint.dim { margin: 10px 0 0; }
.wiz-hint.error-text { color: var(--err); margin-top: 6px; }

.wiz-search { margin-bottom: 10px; }
.wiz-input {
  width: 100%; background: var(--bg-raise); border: 1px solid var(--line-strong);
  border-radius: var(--r-md); color: var(--text-1); font: inherit; font-size: 13px;
  padding: 8px 12px; outline: none; box-sizing: border-box;
}
.wiz-input:focus { border-color: var(--accent-line-strong); box-shadow: 0 0 0 2px var(--accent); }
.mono { font-family: var(--font-mono, Consolas, monospace); font-size: 12px; }

.wiz-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; }
.wiz-cell {
  display: flex; flex-direction: column; gap: 4px; align-items: flex-start;
  border: 1px solid var(--line-strong); border-radius: var(--r-md); background: var(--bg-surface);
  padding: 10px 12px; cursor: pointer; color: var(--text-1); font: inherit; text-align: left;
}
.wiz-cell:hover { border-color: var(--accent-line-strong); background: var(--bg-raise); }
.wiz-cell.selected { border-color: var(--accent); background: var(--accent-dim); }
.wiz-cell-name { font-size: 13px; font-weight: 600; }
.wiz-cell-prefix { font-size: 11px; color: var(--text-3); }
.wiz-cell-tag { font-size: 10px; color: var(--text-3); border: 1px solid var(--line-strong); border-radius: var(--r-full); padding: 0 6px; }

.wiz-field { margin-bottom: 14px; }
.wiz-label { display: block; font-size: 12px; color: var(--text-2); margin-bottom: 6px; }

.wiz-banner { border-radius: var(--r-md); padding: 10px 14px; font-size: 12px; line-height: 1.7; margin-top: 4px; }
.banner-ok { background: var(--ok-soft); border: 1px solid var(--ok-line); color: var(--ok); }
.banner-err { background: var(--err-soft); border: 1px solid var(--err-line); color: var(--err); }
.banner-warn { background: var(--warn-soft); border: 1px solid var(--warn-line); color: var(--warn); }
.banner-info { background: var(--info-soft); border: 1px solid var(--info-line); color: var(--info); }

.wiz-pick-list { border: 1px solid var(--line-strong); border-radius: var(--r-md); overflow: hidden; max-height: 220px; overflow-y: auto; }
.wiz-pick-item { display: flex; align-items: center; gap: 10px; padding: 8px 14px; border-bottom: 1px solid var(--line); background: var(--bg-surface); cursor: pointer; }
.wiz-pick-item:last-child { border-bottom: none; }
.wiz-pick-item:hover { background: var(--bg-raise); }
.wiz-pick-name { font-size: 12px; }
.wiz-check { accent-color: var(--accent); }

.wiz-collapse { border: 1px solid var(--line); border-radius: var(--r-md); background: var(--bg-surface); margin: 4px 0 12px; }
.wiz-collapse summary { cursor: pointer; padding: 9px 14px; font-size: 12px; color: var(--text-2); }
.wiz-param-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 0 14px; padding: 4px 14px 0; }

.wiz-check-row { display: flex; align-items: center; gap: 8px; font-size: 13px; color: var(--text-2); cursor: pointer; }
.wiz-footer-spacer { flex: 1; }
</style>
