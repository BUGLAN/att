<template>
  <Modal :title="isEdit ? `供应商档案 · ${profile?.name || ''}` : '供应商档案'" modal-name="llmProviderEdit">
    <template v-if="profile">
      <div class="pep-meta mono">{{ profile.provider }} · {{ providerPrefix }}</div>

      <div class="pep-field">
        <label class="pep-label">档案名称</label>
        <input v-model.trim="form.name" class="pep-input" placeholder="例如：DeepSeek 官方">
      </div>

      <div class="pep-field">
        <label class="pep-label">API Key</label>
        <input v-model="form.api_key" class="pep-input" type="password" :placeholder="profile.api_key_hint ? '留空保持现有密钥' : 'sk-…'">
        <p class="pep-hint">
          {{ profile.api_key_hint ? `当前：${profile.api_key_hint} · ` : '' }}仅保存在本机 data/（Fernet 加密）；编辑时留空表示保持现有密钥。
        </p>
      </div>

      <div class="pep-field">
        <label class="pep-label">API Base（留空使用默认端点）</label>
        <input v-model.trim="form.api_base" class="pep-input mono" :placeholder="providerBase || 'https://'">
      </div>

      <div v-if="verifyState.status" class="pep-banner" :class="verifyState.ok ? 'banner-ok' : 'banner-err'">
        <template v-if="verifyState.ok">✓ 验证通过<span v-if="verifyState.latency">（{{ verifyState.latency }}）</span></template>
        <template v-else>✕ {{ verifyState.title }}：{{ verifyState.detail }}</template>
      </div>
    </template>

    <template #footer>
      <BaseButton variant="danger" @click="removeProfile">删除档案</BaseButton>
      <span class="pep-spacer"></span>
      <BaseButton variant="outline" :disabled="verifying" @click="verify">重新验证</BaseButton>
      <BaseButton variant="solid" @click="save">保存</BaseButton>
    </template>
  </Modal>

  <ConfirmDialog
    v-model="isDeleteVisible"
    title="删除这个供应商档案？"
    description="其下的所有模型条目会一并删除；使用它们的优化记录不受影响。"
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
import { errorCopy, formatLatency } from '@/components/settings/llm/llmLogic.mjs'
import { useLLMService } from '@/composables/useSettings'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { deleteProfile } = useLLMService()

const profile = computed(() => settingsStore.currentLLMProfile)
const isEdit = computed(() => !!profile.value?.id)

const providerPrefix = computed(() =>
  settingsStore.llmProviders.find((item) => item.id === profile.value?.provider)?.litellm_prefix || profile.value?.provider || ''
)
const providerBase = computed(() =>
  settingsStore.llmProviders.find((item) => item.id === profile.value?.provider)?.default_base_url || ''
)

const form = ref({ name: '', api_key: '', api_base: '' })
const verifyState = ref({ status: false, ok: false, title: '', detail: '', latency: '' })
const verifying = ref(false)
const isDeleteVisible = ref(false)

watch(profile, (value) => {
  if (!value) {
    return
  }
  form.value = {
    name: value.name || '',
    api_key: '',
    api_base: value.api_base || ''
  }
  verifyState.value = { status: false, ok: false, title: '', detail: '', latency: '' }
}, { immediate: true })

async function verify() {
  if (!profile.value?.id) {
    return
  }
  verifying.value = true
  verifyState.value = { status: true, ok: false, title: '验证中', detail: '', latency: '' }
  try {
    // 编辑过的内容先落盘再验证（与模型编辑弹窗同一语义：测试所见即所测）
    await save({ silent: true })
    const outcome = await settingsStore.verifyLLMProfile(profile.value.id)
    if (outcome.ok) {
      const models = outcome.remote_models?.length ? `，列取到 ${outcome.remote_models.length} 个模型` : ''
      verifyState.value = { status: true, ok: true, title: '', detail: '', latency: formatLatency(outcome.latency_ms) + models }
    } else {
      const copy = errorCopy(outcome.error_code, outcome.message)
      verifyState.value = { status: true, ok: false, title: copy.title, detail: copy.detail, latency: '' }
    }
  } catch (error) {
    verifyState.value = { status: true, ok: false, title: '验证请求失败', detail: error?.message || '', latency: '' }
  } finally {
    verifying.value = false
  }
}

async function save(options = {}) {
  const { silent = false } = options
  if (!form.value.name) {
    if (!silent) {
      appStore.showToast('档案名称不能为空', { tone: 'error' })
    }
    throw new Error('empty name')
  }
  const payload = { name: form.value.name, api_base: form.value.api_base }
  if (form.value.api_key) {
    payload.api_key = form.value.api_key
  }
  try {
    await settingsStore.updateLLMProfile(profile.value.id, payload)
    if (!silent) {
      appStore.showToast('档案已保存')
      close()
    }
  } catch (error) {
    if (!silent) {
      appStore.showToast(error?.message || '保存失败', { tone: 'error' })
    }
    throw error
  }
}

function removeProfile() {
  isDeleteVisible.value = true
}

async function confirmDelete() {
  if (!profile.value?.id) {
    return
  }
  try {
    await deleteProfile(profile.value.id)
    close()
  } catch (error) {
    // 409（含默认模型）等文案由 useLLMService 统一 toast
  }
}

function close() {
  appStore.closeModal('llmProviderEdit')
}
</script>

<style scoped>
.pep-meta { font-size: 11px; color: var(--text-3); margin-bottom: 14px; }
.pep-field { margin-bottom: 14px; }
.pep-label { display: block; font-size: 12px; color: var(--text-2); margin-bottom: 6px; }
.pep-input {
  width: 100%; background: var(--bg-raise); border: 1px solid var(--line-strong);
  border-radius: var(--r-sm); color: var(--text-1); font: inherit; font-size: 13px;
  padding: 8px 12px; outline: none; box-sizing: border-box;
}
.pep-input:focus { border-color: var(--accent-line-strong); box-shadow: 0 0 0 2px var(--accent); }
.pep-hint { margin: 5px 0 0; font-size: 11px; color: var(--text-3); line-height: 1.6; }
.mono { font-family: var(--font-mono, Consolas, monospace); font-size: 12px; }
.pep-banner { border-radius: var(--r-md); padding: 10px 14px; font-size: 12px; line-height: 1.7; }
.banner-ok { background: var(--ok-soft); border: 1px solid var(--ok-line); color: var(--ok); }
.banner-err { background: var(--err-soft); border: 1px solid var(--err-line); color: var(--err); }
.pep-spacer { flex: 1; }
</style>
