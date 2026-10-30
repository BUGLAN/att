<template>
  <div class="settings-group llm-section">
    <div class="llm-head">
      <div>
        <h3 v-if="showTitle" class="settings-group-title">LLM 服务</h3>
        <p class="llm-head-desc">模型名原样透传给服务商 · 服务商发布新模型无需更新应用</p>
      </div>
      <div class="llm-head-actions">
        <BaseButton variant="outline" size="sm" @click="openWizard('provider')">添加供应商</BaseButton>
        <BaseButton variant="solid" size="sm" @click="openWizard('model')">添加模型</BaseButton>
      </div>
    </div>

    <div v-if="skippedEntries.length" class="llm-alert llm-alert-warn">
      有 {{ skippedEntries.length }} 条旧配置因数据损坏被跳过（{{ skippedEntries.join('、') }}），其余配置不受影响。
    </div>

    <!-- 供应商档案 -->
    <div class="llm-profile-list">
      <div v-for="profile in profiles" :key="profile.id" class="llm-profile-card">
        <div class="llm-profile-logo" :class="'logo-' + profile.provider">{{ providerName(profile.provider).charAt(0) || '?' }}</div>
        <div class="llm-profile-info">
          <div class="llm-profile-name">
            {{ profile.name }}
            <span class="llm-chip">{{ modelCount(profile.id) }} 个模型</span>
          </div>
          <div class="llm-profile-meta mono">
            {{ profile.provider }} · {{ profile.api_base || providerBase(profile.provider) || '默认端点' }} · {{ keyDisplay(profile.api_key_hint, providerRequiresKey(profile.provider)) }}
          </div>
        </div>
        <span class="llm-status" :class="'status-' + statusTone(profile.status?.state)">
          {{ statusText(profile) }}
        </span>
        <BaseButton variant="ghost" size="sm" @click="verifyProfile(profile)">重新验证</BaseButton>
        <BaseButton variant="ghost" size="sm" @click="editProfile(profile)">编辑</BaseButton>
      </div>
      <div v-if="!profiles.length" class="llm-empty">
        还没有供应商档案。点击「添加供应商」接入 DeepSeek、OpenAI、Ollama 或任何 OpenAI 兼容端点。
      </div>
    </div>

    <!-- 模型列表 -->
    <table v-if="models.length" class="llm-model-table">
      <thead>
        <tr>
          <th class="col-model">模型</th>
          <th class="col-string">模型串</th>
          <th class="col-params">参数</th>
          <th class="col-status">状态</th>
          <th class="col-actions"></th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="model in models" :key="model.id">
          <td>
            <div class="llm-model-name">
              {{ model.display_name || model.model }}
              <span v-if="model.id === defaultModelId" class="llm-chip llm-chip-gold">默认优化</span>
              <span v-else-if="model.id === fallbackModelId" class="llm-chip">备用</span>
            </div>
            <div v-if="model.status?.state === 'failed'" class="llm-model-error">
              {{ model.status.error || '验证失败' }}
            </div>
          </td>
          <td class="mono llm-model-string">{{ model.model }}</td>
          <td class="llm-params">{{ describeParams(model.params) }}</td>
          <td>
            <span class="llm-status" :class="'status-' + statusTone(model.status?.state)">
              {{ statusLabel(model) }}
            </span>
          </td>
          <td class="llm-actions">
            <BaseButton variant="ghost" size="sm" @click="verifyModel(model)">测试</BaseButton>
            <BaseButton v-if="model.id !== defaultModelId" variant="ghost" size="sm" @click="activate(model)">设为默认</BaseButton>
            <BaseButton variant="ghost" size="sm" @click="editModel(model)">编辑</BaseButton>
            <BaseButton variant="danger" size="sm" @click="removeModel(model)">删除</BaseButton>
          </td>
        </tr>
      </tbody>
    </table>
    <div v-else class="llm-empty">
      还没有模型。点击「添加模型」从上游拉取列表或直接手输模型名。
    </div>

    <p class="llm-foot-hint">
      默认优化模型：<b>{{ defaultModel?.display_name || '未设置' }}</b>
      <template v-if="fallbackModelLabel">
        　·　备用模型：<b>{{ fallbackModelLabel }}</b>（默认模型失败时自动切换）
      </template>
      <template v-else>
        　·　备用模型：未设置（默认不自动切换，避免意外费用）
      </template>
    </p>

    <ConfirmDialog
      v-model="isDeleteModelVisible"
      title="删除这个模型？"
      description="删除后使用该模型的优化记录不会被影响，但无法再用它发起新的优化。"
      confirm-text="删除"
      @confirm="confirmDeleteModel"
    />
  </div>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import BaseButton from '@/components/common/BaseButton.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import {
  describeParams,
  errorCopy,
  formatLatency,
  keyDisplay,
  statusTone,
  STATUS_LABELS
} from './llmLogic.mjs'
import { useLLMService } from '@/composables/useSettings'

defineProps({
  // 设置页分页头已给出「LLM 服务」标题时传 false，避免重复
  showTitle: {
    type: Boolean,
    default: true
  }
})

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const {
  profiles,
  models,
  providers,
  defaultModel,
  defaultModelId,
  skippedEntries,
  loadLLMState,
  loadLLMProviders,
  activateModel,
  verifyModel: verifyModelApi,
  deleteModel,
  deleteProfile
} = useLLMService()

const fallbackModelId = computed(() => settingsStore.llmFallbackModelId)
const fallbackModelLabel = computed(() => {
  const fallback = models.value.find((model) => model.id === fallbackModelId.value)
  return fallback ? (fallback.display_name || fallback.model) : ''
})

const pendingDeleteModelId = ref(null)
const isDeleteModelVisible = ref(false)

onMounted(() => {
  // 各自失败互不影响：供应商目录与状态来自不同端点
  loadLLMState().catch(() => appStore.showToast('LLM 配置加载失败', { tone: 'error' }))
  loadLLMProviders().catch(() => {})
})

function providerName(providerId) {
  return providers.value.find((item) => item.id === providerId)?.name || providerId
}

function providerBase(providerId) {
  return providers.value.find((item) => item.id === providerId)?.default_base_url || ''
}

function providerRequiresKey(providerId) {
  return providers.value.find((item) => item.id === providerId)?.requires_key ?? true
}

function modelCount(profileId) {
  return models.value.filter((model) => model.profile_id === profileId).length
}

function statusText(profile) {
  const state = profile.status?.state
  if (state === 'ok') {
    const latency = formatLatency(profile.status?.latency_ms)
    return latency ? `已验证 · ${latency}` : '已验证'
  }
  if (state === 'failed' || state === 'decrypt_failed') {
    return state === 'decrypt_failed' ? '密钥无法解密' : '验证失败'
  }
  return '未验证'
}

function statusLabel(model) {
  const state = model.status?.state
  const latency = formatLatency(model.status?.latency_ms)
  const label = STATUS_LABELS[state] || '未验证'
  return state === 'ok' && latency ? `${label} · ${latency}` : label
}

function openWizard(mode) {
  if (mode === 'model' && !profiles.value.length) {
    appStore.showToast('请先添加一个供应商档案', { tone: 'error' })
  }
  // 'provider' = 从第 1 步开始；'model' = 已有档案时直接进第 3 步
  settingsStore.currentLLMProfile = mode === 'model' && profiles.value.length
    ? (settingsStore.llmDefaultModel ? profiles.value.find((p) => p.id === defaultModelId.value) || profiles.value[0] : profiles.value[0])
    : null
  settingsStore.llmWizardMode = mode
  appStore.openModal('llmWizard')
}

function editProfile(profile) {
  settingsStore.currentLLMProfile = profile
  appStore.openModal('llmProviderEdit')
}

function editModel(model) {
  settingsStore.currentLLMModel = model
  appStore.openModal('llmModelEdit')
}

async function verifyProfile(profile) {
  try {
    const outcome = await settingsStore.verifyLLMProfile(profile.id)
    if (outcome.ok) {
      appStore.showToast('验证通过' + (outcome.remote_models?.length ? `，拉取到 ${outcome.remote_models.length} 个模型` : ''))
    } else {
      const copy = errorCopy(outcome.error_code, outcome.message)
      appStore.showToast(`${copy.title}：${copy.detail}`, { tone: 'error', duration: 6000 })
    }
  } catch (error) {
    appStore.showToast('验证请求失败', { tone: 'error' })
  }
}

async function verifyModel(model) {
  try {
    const outcome = await verifyModelApi(model.id)
    if (outcome.ok) {
      appStore.showToast(`验证通过 · ${formatLatency(outcome.latency_ms)}`)
    } else {
      const copy = errorCopy(outcome.error_code, outcome.message)
      appStore.showToast(`${copy.title}：${copy.detail}`, { tone: 'error', duration: 6000 })
    }
  } catch (error) {
    appStore.showToast('验证请求失败', { tone: 'error' })
  }
}

async function activate(model) {
  await activateModel(model.id).catch(() => appStore.showToast('设置默认模型失败', { tone: 'error' }))
}

function removeModel(model) {
  pendingDeleteModelId.value = model.id
  isDeleteModelVisible.value = true
}

async function confirmDeleteModel() {
  const id = pendingDeleteModelId.value
  pendingDeleteModelId.value = null
  if (!id) {
    return
  }
  await deleteModel(id)
}

// deleteProfile 供「供应商编辑弹窗」使用（该弹窗自己确认）；这里暴露只是保持依赖面最小
defineExpose({ deleteProfile })
</script>

<style scoped>
.llm-section { display: flex; flex-direction: column; gap: 14px; }
.llm-head { display: flex; align-items: flex-start; gap: 12px; }
.llm-head-desc { margin: 4px 0 0; font-size: 12px; color: var(--text-3); }
.llm-head-actions { margin-left: auto; display: flex; gap: 8px; flex: none; }

.llm-alert { border-radius: var(--r-md); padding: 10px 14px; font-size: 12px; line-height: 1.7; }
.llm-alert-warn { background: var(--warn-soft); border: 1px solid var(--warn-line); color: var(--warn); }

.llm-profile-list { display: flex; flex-direction: column; gap: 8px; }
.llm-profile-card {
  display: flex; align-items: center; gap: 12px;
  border: 1px solid var(--line-strong); border-radius: var(--r-md);
  background: var(--bg-surface); padding: 11px 14px;
}
.llm-profile-logo {
  width: 30px; height: 30px; border-radius: var(--r-md); flex: none;
  display: flex; align-items: center; justify-content: center;
  background: var(--bg-hover); color: var(--text-1); font-weight: 700; font-size: 14px;
}
.llm-profile-info { min-width: 0; flex: 1; }
.llm-profile-name { font-size: 14px; font-weight: 600; display: flex; align-items: center; gap: 8px; }
.llm-profile-meta { margin-top: 2px; font-size: 11px; color: var(--text-3); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

.llm-chip {
  font-size: 10px; font-weight: 500; color: var(--text-3);
  border: 1px solid var(--line-strong); border-radius: var(--r-full); padding: 1px 8px; white-space: nowrap;
}
.llm-chip-gold { color: var(--accent-hi); background: var(--accent-dim); border-color: var(--accent-line); }

.llm-status { font-size: 11px; white-space: nowrap; }
.status-ok { color: var(--ok); }
.status-err { color: var(--err); }
.status-neutral { color: var(--text-3); }

.llm-empty {
  border: 1px dashed var(--line-strong); border-radius: var(--r-md);
  padding: 18px; text-align: center; font-size: 13px; color: var(--text-3);
}

.llm-model-table { width: 100%; border-collapse: collapse; font-size: 13px; }
.llm-model-table th {
  font-size: 11px; font-weight: 500; color: var(--text-3); text-align: left;
  padding: 8px 10px; border-bottom: 1px solid var(--line-strong); letter-spacing: 0.4px;
}
.llm-model-table td { padding: 11px 10px; border-bottom: 1px solid var(--line); vertical-align: top; }
.llm-model-table tr:last-child td { border-bottom: none; }
.llm-model-table tr:hover td { background: var(--white-tint, rgba(255, 255, 255, 0.03)); }
.col-model { width: 26%; } .col-string { width: 28%; } .col-params { width: 22%; } .col-status { width: 12%; }

.llm-model-name { font-weight: 500; display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.llm-model-error { margin-top: 3px; font-size: 12px; color: var(--err); max-width: 380px; line-height: 1.6; }
.llm-model-string { color: var(--text-2); font-size: 12px; }
.llm-params { font-size: 12px; color: var(--text-3); }
.llm-actions { text-align: right; white-space: nowrap; }
.llm-actions .BaseButton + .BaseButton { margin-left: 2px; }

.llm-foot-hint { margin: 0; font-size: 12px; color: var(--text-3); line-height: 1.7; }
.llm-foot-hint b { color: var(--text-2); font-weight: 600; }
.mono { font-family: var(--font-mono, Consolas, monospace); }
</style>
