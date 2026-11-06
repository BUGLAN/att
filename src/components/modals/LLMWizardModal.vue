<template>
  <Modal :title="wizardTitle" modal-name="llmWizard">
    <!-- 步骤指示：编号方块 + 连接线，已完成的步骤换成对勾 -->
    <ol class="wiz-steps" aria-label="添加步骤">
      <li
        v-for="(label, index) in STEP_LABELS"
        :key="label"
        class="wiz-step"
        :class="{ 'is-current': step === index + 1, 'is-done': step > index + 1 }"
        :aria-current="step === index + 1 ? 'step' : undefined"
      >
        <span class="wiz-step-num">
          <svg v-if="step > index + 1" viewBox="0 0 24 24" aria-hidden="true"><path d="m5 12.5 4.5 4.5L19 7.5" /></svg>
          <template v-else>{{ index + 1 }}</template>
        </span>
        <span class="wiz-step-label">{{ label }}</span>
      </li>
    </ol>

    <!-- 步骤 1：选择供应商 -->
    <div v-if="step === 1">
      <div class="wiz-search">
        <svg class="wiz-search-icon" viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="6.5" /><path d="m20 20-4.2-4.2" /></svg>
        <input v-model.trim="providerQuery" class="wiz-input" placeholder="搜索供应商，例如 deepseek、ollama、openrouter…" aria-label="搜索供应商">
      </div>
      <div v-if="builtinProviders.length" class="wiz-grid" role="radiogroup" aria-label="内置服务商">
        <button
          v-for="provider in builtinProviders"
          :key="provider.id"
          type="button"
          role="radio"
          class="wiz-cell"
          :class="{ selected: selectedProvider?.id === provider.id }"
          :aria-checked="selectedProvider?.id === provider.id ? 'true' : 'false'"
          :title="provider.name"
          @click="selectedProvider = provider"
        >
          <span class="wiz-cell-logo" aria-hidden="true">{{ provider.name.charAt(0).toUpperCase() }}</span>
          <span class="wiz-cell-text">
            <span class="wiz-cell-name">{{ provider.name }}</span>
            <span class="wiz-cell-meta">
              <span class="mono">{{ provider.litellm_prefix }}/</span>
              <!-- 绝大多数都要密钥，只把例外标出来 -->
              <span v-if="!provider.requires_key" class="wiz-cell-tag">免密钥</span>
            </span>
          </span>
          <svg class="wiz-cell-check" viewBox="0 0 24 24" aria-hidden="true"><path d="m5 12.5 4.5 4.5L19 7.5" /></svg>
        </button>
      </div>
      <!-- 自定义端点是兜底入口，单独一行放在最后 -->
      <button
        v-if="customProvider"
        type="button"
        class="wiz-cell wiz-cell-custom"
        :class="{ selected: selectedProvider?.id === customProvider.id }"
        :aria-pressed="selectedProvider?.id === customProvider.id ? 'true' : 'false'"
        @click="selectedProvider = customProvider"
      >
        <span class="wiz-cell-logo" aria-hidden="true">+</span>
        <span class="wiz-cell-text">
          <span class="wiz-cell-name">{{ customProvider.name }}</span>
          <span class="wiz-cell-meta">未列出的端点（硅基流动、第三方中转等）选这里，无需等待应用更新</span>
        </span>
        <svg class="wiz-cell-check" viewBox="0 0 24 24" aria-hidden="true"><path d="m5 12.5 4.5 4.5L19 7.5" /></svg>
      </button>
      <p v-if="!builtinProviders.length && !customProvider" class="wiz-empty">没有匹配「{{ providerQuery }}」的服务商，可以试试「自定义 OpenAI 兼容」。</p>
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
        <p class="wiz-hint">只加密保存在本机，保存前会先验证一次。</p>
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
          语音润色建议把思考强度设为「关闭」：更快，也不会因思考占满输出长度而返回空文本。
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

const STEP_LABELS = ['选择服务商', '填写凭据', '选择模型']

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
const builtinProviders = computed(() => filteredProviders.value.filter((provider) => provider.id !== 'custom'))
// 自定义端点不参与搜索过滤：搜不到时它正是要找的出路
const customProvider = computed(() => settingsStore.llmProviders.find((provider) => provider.id === 'custom') || null)

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
/* ---------- 步骤指示 ---------- */
.wiz-steps {
  display: flex; align-items: center; gap: var(--s-2);
  list-style: none; margin: 0 0 var(--s-4); padding: 0;
}
.wiz-step { display: flex; align-items: center; gap: var(--s-2); font-size: var(--fs-cap); color: var(--text-3); white-space: nowrap; }
/* 步骤之间的连接线 */
.wiz-step + .wiz-step::before {
  content: ''; width: 24px; height: 1px; background: var(--line-strong); margin-right: var(--s-1);
}
.wiz-step.is-done + .wiz-step::before { background: var(--accent-line-strong); }
.wiz-step-num {
  width: 20px; height: 20px; border-radius: var(--r-sm); flex: none;
  display: inline-flex; align-items: center; justify-content: center;
  border: 1px solid var(--line-strong); font-size: 11px; font-weight: 600; font-variant-numeric: tabular-nums;
}
.wiz-step-num svg { width: 12px; height: 12px; fill: none; stroke: currentColor; stroke-width: 2.5; stroke-linecap: round; stroke-linejoin: round; }
.wiz-step.is-current { color: var(--text-1); font-weight: 600; }
.wiz-step.is-current .wiz-step-num { background: var(--accent); border-color: var(--accent); color: var(--accent-ink); }
.wiz-step.is-done { color: var(--text-2); }
.wiz-step.is-done .wiz-step-num { background: var(--accent-dim); border-color: var(--accent-line); color: var(--accent-hi); }

.wiz-hint { margin: 0 0 12px; font-size: 12px; color: var(--text-3); line-height: 1.7; }
.wiz-field .wiz-hint { margin: 6px 0 0; }
.wiz-hint.error-text { color: var(--err); margin-top: 6px; }

.wiz-input {
  width: 100%; background: var(--bg-raise); border: 1px solid var(--line-strong);
  border-radius: var(--r-sm); color: var(--text-1); font: inherit; font-size: 13px;
  padding: 8px 12px; outline: none; box-sizing: border-box;
}
.wiz-input:focus { border-color: var(--accent-line-strong); box-shadow: 0 0 0 2px var(--accent); }
.mono { font-family: var(--font-mono, Consolas, monospace); font-size: 12px; }

/* ---------- 步骤 1：搜索 + 服务商网格 ---------- */
.wiz-search { position: relative; margin-bottom: var(--s-3); }
.wiz-search .wiz-input { height: 36px; padding-left: 34px; }
.wiz-search-icon {
  position: absolute; left: 11px; top: 50%; transform: translateY(-50%);
  width: 15px; height: 15px; fill: none; stroke: var(--text-3); stroke-width: 1.75; stroke-linecap: round;
  pointer-events: none;
}

.wiz-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--s-2); }
.wiz-cell {
  position: relative;
  display: flex; align-items: center; gap: 10px; min-width: 0; height: 52px;
  border: 1px solid var(--line); border-radius: var(--r-md); background: var(--bg-card);
  padding: 0 32px 0 10px; cursor: pointer; color: var(--text-1); font: inherit; text-align: left;
  transition: border-color var(--t-fast) var(--ease), background-color var(--t-fast) var(--ease);
}
.wiz-cell:hover { border-color: var(--line-strong); background: var(--bg-raise); }
.wiz-cell:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.wiz-cell.selected { border-color: var(--accent-line-strong); background: var(--accent-dim); }
.wiz-cell-logo {
  width: 30px; height: 30px; border-radius: var(--r-sm); flex: none;
  display: inline-flex; align-items: center; justify-content: center;
  background: var(--grad-raise); border: 1px solid var(--line-strong);
  font-size: 13px; font-weight: 700; color: var(--text-2);
}
.wiz-cell.selected .wiz-cell-logo { background: var(--accent); border-color: var(--accent); color: var(--accent-ink); }
.wiz-cell-text { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.wiz-cell-name { font-size: 13px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.wiz-cell-meta {
  display: flex; align-items: center; gap: 6px; min-width: 0;
  font-size: 11px; color: var(--text-3); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.wiz-cell-meta .mono { font-size: 11px; }
.wiz-cell-tag {
  flex: none; height: 16px; line-height: 16px; padding: 0 5px; border-radius: var(--r-xs);
  background: var(--ok-soft); color: var(--ok); font-size: 10px;
}
.wiz-cell-check {
  position: absolute; right: 10px; top: 50%; transform: translateY(-50%);
  width: 15px; height: 15px; fill: none; stroke: var(--accent-hi); stroke-width: 2.25;
  stroke-linecap: round; stroke-linejoin: round;
  opacity: 0; transition: opacity var(--t-fast) var(--ease);
}
.wiz-cell.selected .wiz-cell-check { opacity: 1; }

.wiz-cell-custom { width: 100%; margin-top: var(--s-2); border-style: dashed; background: transparent; }
.wiz-cell-custom.selected { border-style: solid; }
.wiz-cell-custom .wiz-cell-logo { background: transparent; border-style: dashed; font-weight: 400; font-size: 16px; }
.wiz-cell-custom .wiz-cell-meta { display: block; }

.wiz-empty { margin: var(--s-2) 0 0; font-size: var(--fs-cap); color: var(--text-3); text-align: center; }

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
