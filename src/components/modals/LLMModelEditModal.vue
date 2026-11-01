<template>
  <Modal :title="isEdit ? '编辑模型' : '添加模型'" modal-name="llmModelEdit">
    <template v-if="model">
      <div class="med-status">
        <span class="med-status-label">当前状态：</span>
        <span :class="model.status?.state === 'ok' ? 'text-ok' : 'text-err'">
          {{ model.status?.state === 'ok' ? `可用 · ${formatLatency(model.status?.latency_ms)}` : (model.status?.error || '未验证或失败') }}
        </span>
      </div>

      <div class="med-field">
        <label class="med-label">显示名称</label>
        <input v-model.trim="form.display_name" class="med-input" placeholder="例如：DeepSeek Flash">
      </div>

      <div class="med-field">
        <label class="med-label">模型名（自动按档案前缀组装；自定义端点统一经 openai/ 调用，含斜杠的模型 ID 也没问题）</label>
        <input v-model.trim="form.model" class="med-input mono" placeholder="deepseek-flash">
        <p class="med-hint mono">当前完整模型串：{{ model.model }}</p>
      </div>

      <div class="med-grid">
        <div class="med-field"><label class="med-label">Temperature</label><input v-model.number="form.params.temperature" type="number" min="0" max="2" step="0.1" class="med-input"></div>
        <div class="med-field"><label class="med-label">Max Tokens</label><input v-model.number="form.params.max_tokens" type="number" min="100" step="100" class="med-input"></div>
        <div class="med-field"><label class="med-label">请求超时（秒）</label><input v-model.number="form.params.timeout_s" type="number" min="5" max="600" class="med-input"></div>
        <div class="med-field"><label class="med-label">失败重试次数</label><input v-model.number="form.params.retries" type="number" min="0" max="5" class="med-input"></div>
        <div class="med-field">
          <label class="med-label">思考强度</label>
          <select v-model="form.params.reasoning_effort" class="med-input">
            <option v-for="item in REASONING_EFFORT_OPTIONS" :key="item.value" :value="item.value">{{ item.label }}</option>
          </select>
        </div>
      </div>
      <p class="med-hint">
        语音转写后的短文本润色不需要思考链。DeepSeek 现行模型默认强度是 high，每次都要先写一段推理才吐正文
        （实测首字 1.6s → 0.55s）。思考 token 还计入 Max Tokens，占满后正文会为空。
      </p>

      <div v-if="verifyState.status" class="med-banner" :class="verifyState.ok ? 'banner-ok' : 'banner-err'">
        {{ verifyState.ok ? `✓ 验证通过（${verifyState.latency}）` : `✕ ${verifyState.title}：${verifyState.detail}` }}
      </div>
    </template>

    <template #footer>
      <BaseButton variant="danger" @click="removeModel">删除</BaseButton>
      <span class="med-spacer"></span>
      <BaseButton variant="outline" @click="verify">测试</BaseButton>
      <BaseButton variant="outline" @click="makeDefault">设为默认</BaseButton>
      <BaseButton variant="solid" @click="save">保存</BaseButton>
    </template>
  </Modal>

  <ConfirmDialog
    v-model="isDeleteVisible"
    title="删除这个模型？"
    description="删除后无法再用它发起新的优化；已有的优化记录不受影响。"
    confirm-text="删除"
    @confirm="confirmDelete"
  />
</template>

<script setup>
import { computed, ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import Modal from '@/components/common/Modal.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import { errorCopy, formatLatency, REASONING_EFFORT_OPTIONS } from '@/components/settings/llm/llmLogic.mjs'
import { useLLMService } from '@/composables/useSettings'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { verifyModel, deleteModel, activateModel } = useLLMService()

const model = computed(() => settingsStore.currentLLMModel)
const isEdit = computed(() => !!model.value?.id)

const form = ref(defaultForm())
const verifyState = ref({ status: false, ok: false, title: '', detail: '', latency: '' })
const isDeleteVisible = ref(false)

function defaultForm() {
  return {
    display_name: '',
    model: '',
    params: { temperature: 0.3, max_tokens: 4096, timeout_s: 60, retries: 2, reasoning_effort: '' }
  }
}

watch(model, (value) => {
  if (!value) {
    return
  }
  form.value = {
    display_name: value.display_name || '',
    model: value.model || '',
    params: { reasoning_effort: '', ...value.params }
  }
  verifyState.value = { status: false, ok: false, title: '', detail: '', latency: '' }
}, { immediate: true })

async function verify() {
  if (!model.value?.id) {
    return
  }
  verifyState.value = { status: true, ok: false, title: '验证中', detail: '', latency: '' }
  try {
    // 编辑过的参数先落盘再验证，保证「测试」反映的就是用户看到的内容
    await save({ silent: true })
    const outcome = await verifyModel(model.value.id)
    if (outcome.ok) {
      verifyState.value = { status: true, ok: true, title: '', detail: '', latency: formatLatency(outcome.latency_ms) }
    } else {
      const copy = errorCopy(outcome.error_code, outcome.message)
      verifyState.value = { status: true, ok: false, title: copy.title, detail: copy.detail, latency: '' }
    }
  } catch (error) {
    verifyState.value = { status: true, ok: false, title: '验证请求失败', detail: error?.message || '', latency: '' }
  }
}

async function makeDefault() {
  try {
    await save({ silent: true })
    await activateModel(model.value.id)
    appStore.showToast('已设为默认优化模型')
    close()
  } catch (error) {
    appStore.showToast('设置默认模型失败', { tone: 'error' })
  }
}

async function save(options = {}) {
  const { silent = false } = options
  if (!form.value.model) {
    appStore.showToast('模型名不能为空', { tone: 'error' })
    throw new Error('empty model')
  }
  try {
    await settingsStore.updateLLMModel(model.value.id, {
      display_name: form.value.display_name,
      model: form.value.model,
      params: form.value.params
    })
    if (!silent) {
      appStore.showToast('模型已保存')
      close()
    }
  } catch (error) {
    if (!silent) {
      appStore.showToast(error?.message || '保存失败', { tone: 'error' })
    }
    throw error
  }
}

function removeModel() {
  isDeleteVisible.value = true
}

async function confirmDelete() {
  if (!model.value?.id) {
    return
  }
  try {
    await deleteModel(model.value.id)
    close()
  } catch (error) {
    // 409（默认模型不可删）等错误文案由 useLLMService 统一 toast
  }
}

function close() {
  appStore.closeModal('llmModelEdit')
}
</script>

<style scoped>
.med-status { font-size: 12px; color: var(--text-3); margin-bottom: 14px; }
.text-ok { color: var(--ok); }
.text-err { color: var(--err); }
.med-field { margin-bottom: 14px; }
.med-label { display: block; font-size: 12px; color: var(--text-2); margin-bottom: 6px; }
.med-input {
  width: 100%; background: var(--bg-raise); border: 1px solid var(--line-strong);
  border-radius: var(--r-sm); color: var(--text-1); font: inherit; font-size: 13px;
  padding: 8px 12px; outline: none; box-sizing: border-box;
}
.med-input:focus { border-color: var(--accent-line-strong); box-shadow: 0 0 0 2px var(--accent); }
.med-hint { margin: 5px 0 0; font-size: 11px; color: var(--text-3); }
.mono { font-family: var(--font-mono, Consolas, monospace); font-size: 12px; }
.med-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 0 14px; }
.med-banner { border-radius: var(--r-md); padding: 10px 14px; font-size: 12px; line-height: 1.7; }
.banner-ok { background: var(--ok-soft); border: 1px solid var(--ok-line); color: var(--ok); }
.banner-err { background: var(--err-soft); border: 1px solid var(--err-line); color: var(--err); }
.med-spacer { flex: 1; }
</style>
