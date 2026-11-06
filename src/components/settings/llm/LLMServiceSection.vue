<template>
  <div class="settings-group llm-section">
    <div class="llm-head">
      <div>
        <h3 v-if="showTitle" class="settings-group-title">LLM 服务</h3>
      </div>
      <div class="llm-head-actions">
        <BaseButton variant="outline" size="sm" @click="openWizard('provider')">添加供应商</BaseButton>
        <BaseButton variant="solid" size="sm" @click="openWizard('model')">添加模型</BaseButton>
      </div>
    </div>

    <div v-if="skippedEntries.length" class="llm-alert llm-alert-warn">
      有 {{ skippedEntries.length }} 条旧配置因数据损坏被跳过（{{ skippedEntries.join('、') }}），其余配置不受影响。
    </div>

    <div v-if="!profiles.length && !models.length" class="llm-empty">
      还没有供应商档案。点击「添加供应商」接入 DeepSeek、OpenAI、Ollama 或任何 OpenAI 兼容端点。
    </div>

    <!-- 供应商档案：每张卡片 = 档案头 + 该档案下的模型行，归属关系一眼可见 -->
    <div v-else class="llm-profile-list">
      <article
        v-for="group in groups"
        :key="group.profile?.id ?? 'orphan'"
        class="llm-profile-card"
      >
        <header v-if="group.profile" class="llm-profile-head">
          <div class="llm-profile-logo" :title="providerName(group.profile.provider)">{{ providerName(group.profile.provider).charAt(0).toUpperCase() || '?' }}</div>
          <div class="llm-profile-info">
            <div class="llm-profile-name">
              <span class="llm-ellipsis">{{ group.profile.name }}</span>
              <span class="llm-count">{{ group.models.length }} 个模型</span>
            </div>
            <!-- 端点优先完整显示；密钥只给尾号，完整说明放 title -->
            <div class="llm-profile-meta">
              <span class="mono llm-ellipsis" :title="profileBase(group.profile)">{{ profileBase(group.profile) }}</span>
              <span class="llm-meta-sep" aria-hidden="true">·</span>
              <span class="mono" :title="keyDisplay(group.profile.api_key_hint, providerRequiresKey(group.profile.provider))">{{ group.profile.api_key_hint || keyDisplay('', providerRequiresKey(group.profile.provider)) }}</span>
            </div>
          </div>
          <span class="llm-status" :class="'status-' + statusTone(group.profile.status?.state)">
            <span class="llm-dot" aria-hidden="true"></span>{{ statusText(group.profile) }}
          </span>
          <div class="llm-row-actions">
            <BaseButton variant="ghost" size="sm" icon :loading="busyIds.has(group.profile.id)" title="重新验证" aria-label="重新验证" @click="verifyProfile(group.profile)">
              <template #icon><svg viewBox="0 0 24 24"><path d="M20 11a8 8 0 1 0-2.3 5.7" /><path d="M20 4v7h-7" /></svg></template>
            </BaseButton>
            <BaseButton variant="ghost" size="sm" icon title="编辑供应商" aria-label="编辑供应商" @click="editProfile(group.profile)">
              <template #icon><svg viewBox="0 0 24 24"><path d="M4 20h4L18.5 9.5a2.8 2.8 0 0 0-4-4L4 16z" /><path d="m13.5 6.5 4 4" /></svg></template>
            </BaseButton>
          </div>
        </header>
        <header v-else class="llm-profile-head">
          <div class="llm-profile-info">
            <div class="llm-profile-name">未关联供应商</div>
            <div class="llm-profile-meta">所属档案已不存在，这些模型无法发起请求，建议删除后重新添加</div>
          </div>
        </header>

        <ul v-if="group.models.length" class="llm-model-list">
          <li
            v-for="model in group.models"
            :key="model.id"
            class="llm-model-row"
            :class="{ 'is-default': model.id === defaultModelId }"
          >
            <div class="llm-model-info">
              <div class="llm-model-name">
                <span class="llm-ellipsis">{{ model.display_name || model.model }}</span>
                <span v-if="model.id === defaultModelId" class="llm-chip llm-chip-gold">默认优化</span>
                <span v-else-if="model.id === fallbackModelId" class="llm-chip">备用</span>
              </div>
              <div class="llm-model-meta">
                <span class="mono llm-model-string llm-ellipsis" :title="model.model">{{ model.model }}</span>
                <span v-for="item in paramItems(model.params)" :key="item.label" class="llm-param">
                  <span class="llm-param-label">{{ item.label }}</span>{{ item.value }}
                </span>
              </div>
              <div v-if="model.status?.state === 'failed'" class="llm-model-error">
                {{ model.status.error || '验证失败' }}
              </div>
            </div>
            <span class="llm-status" :class="'status-' + statusTone(model.status?.state)">
              <span class="llm-dot" aria-hidden="true"></span>{{ statusLabel(model) }}
            </span>
            <div class="llm-row-actions">
              <BaseButton variant="ghost" size="sm" icon :loading="busyIds.has(model.id)" title="测试连通" aria-label="测试连通" @click="verifyModel(model)">
                <template #icon><svg viewBox="0 0 24 24"><path d="M13 3 5 13.5h6L10 21l8-10.5h-6z" /></svg></template>
              </BaseButton>
              <!-- 默认模型的星标位换成实心金星：图标列上下对齐，也直接标出谁是默认 -->
              <span v-if="model.id === defaultModelId" class="llm-star-current" title="当前默认优化模型">
                <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m12 3.5 2.6 5.3 5.9.9-4.3 4.1 1 5.8-5.2-2.7-5.2 2.7 1-5.8-4.3-4.1 5.9-.9z" /></svg>
              </span>
              <BaseButton v-else variant="ghost" size="sm" icon title="设为默认优化模型" aria-label="设为默认优化模型" @click="activate(model)">
                <template #icon><svg viewBox="0 0 24 24"><path d="m12 3.5 2.6 5.3 5.9.9-4.3 4.1 1 5.8-5.2-2.7-5.2 2.7 1-5.8-4.3-4.1 5.9-.9z" /></svg></template>
              </BaseButton>
              <BaseButton variant="ghost" size="sm" icon title="编辑模型" aria-label="编辑模型" @click="editModel(model)">
                <template #icon><svg viewBox="0 0 24 24"><path d="M4 20h4L18.5 9.5a2.8 2.8 0 0 0-4-4L4 16z" /><path d="m13.5 6.5 4 4" /></svg></template>
              </BaseButton>
              <BaseButton variant="ghost" size="sm" icon class="llm-btn-delete" title="删除模型" aria-label="删除模型" @click="removeModel(model)">
                <template #icon><svg viewBox="0 0 24 24"><path d="M4 7h16" /><path d="M10 11v6M14 11v6" /><path d="m6 7 1 13h10l1-13" /><path d="M9 7V4h6v3" /></svg></template>
              </BaseButton>
            </div>
          </li>
        </ul>
        <div v-else-if="group.profile" class="llm-model-empty">
          <span>这个供应商下还没有模型</span>
          <BaseButton variant="ghost" size="sm" class="llm-btn-link" @click="openWizardFor(group.profile)">添加模型</BaseButton>
        </div>
      </article>
    </div>

    <!-- 默认 / 备用摘要：两项并排，与上方卡片同一套表面层级 -->
    <dl class="llm-summary">
      <div class="llm-summary-item">
        <dt>默认优化模型</dt>
        <dd :class="{ 'is-unset': !defaultModel }">{{ defaultModel?.display_name || defaultModel?.model || '未设置' }}</dd>
      </div>
      <div class="llm-summary-item">
        <dt>备用模型</dt>
        <dd :class="{ 'is-unset': !fallbackModelLabel }">
          {{ fallbackModelLabel || '未设置' }}
          <span class="llm-summary-note">{{ fallbackModelLabel ? '默认模型失败时自动切换' : '默认不自动切换，避免意外费用' }}</span>
        </dd>
      </div>
    </dl>

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
  errorCopy,
  formatLatency,
  groupModelsByProfile,
  keyDisplay,
  paramItems,
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

const groups = computed(() => groupModelsByProfile(profiles.value, models.value))
// 正在验证的档案 / 模型 id：图标按钮没有文字，靠转圈告诉用户请求已发出
const busyIds = ref(new Set())

async function withBusy(id, task) {
  busyIds.value = new Set(busyIds.value).add(id)
  try {
    await task()
  } finally {
    const next = new Set(busyIds.value)
    next.delete(id)
    busyIds.value = next
  }
}

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

function profileBase(profile) {
  return profile.api_base || providerBase(profile.provider) || '默认端点'
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
  // 'provider' = 从第 1 步开始；'model' = 已有档案时直接进第 3 步，优先落到默认模型所在档案
  const defaultProfileId = defaultModel.value?.profile_id
  settingsStore.currentLLMProfile = mode === 'model' && profiles.value.length
    ? profiles.value.find((p) => p.id === defaultProfileId) || profiles.value[0]
    : null
  settingsStore.llmWizardMode = mode
  appStore.openModal('llmWizard')
}

// 档案卡片内「添加模型」：直接给这个档案加模型
function openWizardFor(profile) {
  settingsStore.currentLLMProfile = profile
  settingsStore.llmWizardMode = 'model'
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

function verifyProfile(profile) {
  return withBusy(profile.id, () => verifyProfileNow(profile))
}

async function verifyProfileNow(profile) {
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

function verifyModel(model) {
  return withBusy(model.id, () => verifyModelNow(model))
}

async function verifyModelNow(model) {
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
.llm-section { display: flex; flex-direction: column; gap: var(--s-4); container-type: inline-size; }
.llm-head { display: flex; align-items: center; gap: var(--s-3); }
.llm-head-desc { margin: 0; font-size: var(--fs-cap); color: var(--text-3); line-height: 1.6; }
.settings-group-title + .llm-head-desc { margin-top: calc(-1 * var(--s-2)); }
.llm-head-actions { margin-left: auto; display: flex; gap: var(--s-2); flex: none; }

.llm-alert { border-radius: var(--r-md); padding: 10px 14px; font-size: var(--fs-cap); line-height: 1.7; }
.llm-alert-warn { background: var(--warn-soft); border: 1px solid var(--warn-line); color: var(--warn); }

.llm-empty {
  border: 1px dashed var(--line-strong); border-radius: var(--r-md);
  padding: var(--s-6); text-align: center; font-size: var(--fs-sm); color: var(--text-3);
}

/* ---------- 供应商卡片：档案头 + 模型行同处一卡 ---------- */
.llm-profile-list { display: flex; flex-direction: column; gap: var(--s-3); }
.llm-profile-card {
  /* 模型行左缩进 = 档案头左内边距 + logo + 间距，让模型名与档案名对齐 */
  --llm-indent: calc(var(--s-4) + 34px + var(--s-1) + var(--s-3));
  border: 1px solid var(--line); border-radius: var(--r-md);
  background: var(--bg-card); box-shadow: var(--edge-hi);
  overflow: hidden;
}

/* 档案头、模型行共用「信息 / 状态 / 操作」三列，状态列与操作列定宽，
   上下各行的状态文字因此对齐成一列 */
.llm-profile-head,
.llm-model-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 116px var(--llm-actions-w);
  grid-template-areas: "info status actions";
  align-items: center;
  column-gap: var(--s-3);
  --llm-actions-w: calc(30px * 4 + 2px * 3);
}
.llm-profile-head > .llm-profile-info,
.llm-model-row > .llm-model-info { grid-area: info; }
.llm-profile-head > .llm-status,
.llm-model-row > .llm-status { grid-area: status; }
.llm-row-actions { grid-area: actions; }

.llm-profile-head {
  grid-template-columns: 34px minmax(0, 1fr) 116px var(--llm-actions-w);
  grid-template-areas: "logo info status actions";
  padding: var(--s-3) var(--s-3) var(--s-3) var(--s-4);
}
.llm-profile-head > .llm-profile-logo { grid-area: logo; }
.llm-profile-logo {
  width: 34px; height: 34px; border-radius: var(--r-sm); margin-right: var(--s-1);
  display: flex; align-items: center; justify-content: center;
  background: var(--grad-raise); border: 1px solid var(--line-strong);
  color: var(--text-1); font-weight: 700; font-size: var(--fs-base);
}
.llm-profile-info { min-width: 0; }
.llm-profile-name {
  display: flex; align-items: center; gap: var(--s-2); min-width: 0;
  font-size: var(--fs-md); font-weight: 600; color: var(--text-1);
}
.llm-count { flex: none; font-size: var(--fs-cap); font-weight: 400; color: var(--text-3); }
.llm-profile-meta {
  display: flex; align-items: center; gap: 6px; min-width: 0; margin-top: 3px;
  font-size: var(--fs-cap); color: var(--text-3); white-space: nowrap;
}
.llm-profile-meta > span { flex: none; }
.llm-profile-meta > .llm-ellipsis { flex: 0 1 auto; }
.llm-profile-meta .mono { font-size: 11.5px; }
.llm-meta-sep { color: var(--gray-600); }

/* ---------- 模型行：压暗一档的内嵌列表 ---------- */
.llm-model-list {
  list-style: none; margin: 0; padding: 0;
  border-top: 1px solid var(--line);
  background: var(--scrim-veil);
}
.llm-model-row {
  position: relative;
  padding: var(--s-3) var(--s-3) var(--s-3) var(--llm-indent);
  transition: background-color var(--t-fast) var(--ease);
}
.llm-model-row + .llm-model-row { border-top: 1px solid var(--line-soft); }
.llm-model-row:hover { background: var(--white-veil); }
/* 默认模型：左缘一道金色刻线 + 极淡金底 */
.llm-model-row.is-default { background: linear-gradient(90deg, var(--accent-veil), transparent 40%); }
.llm-model-row.is-default::before {
  content: ''; position: absolute; top: 0; bottom: 0; left: 0; width: 2px;
  background: var(--accent);
}

.llm-model-info { min-width: 0; }
.llm-model-name {
  display: flex; align-items: center; gap: var(--s-2); min-width: 0;
  font-size: var(--fs-base); font-weight: 500; color: var(--text-1);
}
.llm-model-meta {
  display: flex; align-items: center; flex-wrap: wrap; gap: 6px; margin-top: 6px;
}
.llm-model-string { font-size: var(--fs-cap); color: var(--text-2); margin-right: var(--s-1); max-width: 100%; }
.llm-param {
  display: inline-flex; align-items: baseline; gap: 4px;
  height: 20px; padding: 0 6px; line-height: 20px;
  border-radius: var(--r-xs); background: var(--white-tint);
  font-family: var(--font-mono); font-size: 11px; color: var(--text-2);
}
.llm-param-label { color: var(--text-3); font-family: var(--font-sans); }
.llm-model-error { margin-top: 6px; font-size: var(--fs-cap); color: var(--err); line-height: 1.6; }

.llm-model-empty {
  display: flex; align-items: center; gap: var(--s-2);
  border-top: 1px solid var(--line); background: var(--scrim-veil);
  padding: var(--s-2) var(--s-3) var(--s-2) var(--llm-indent);
  font-size: var(--fs-cap); color: var(--text-3);
}

/* ---------- 状态与标签 ---------- */
.llm-status {
  display: inline-flex; align-items: center; gap: 6px;
  font-size: var(--fs-cap); white-space: nowrap; font-variant-numeric: tabular-nums;
}
.llm-dot { width: 6px; height: 6px; border-radius: var(--r-full); background: currentColor; flex: none; }
.status-ok { color: var(--ok); }
.status-err { color: var(--err); }
.status-neutral { color: var(--text-3); }

.llm-chip {
  flex: none; height: 20px; line-height: 18px; padding: 0 7px;
  font-size: 11px; font-weight: 500; color: var(--text-2);
  border: 1px solid var(--line-strong); border-radius: var(--r-xs); white-space: nowrap;
}
.llm-chip-gold { color: var(--accent-hi); background: var(--accent-dim); border-color: var(--accent-line); }

/* ---------- 操作：幽灵按钮，删除只在悬停时变红，不再用实心红块抢眼 ---------- */
.llm-row-actions { display: flex; align-items: center; justify-content: flex-end; gap: 2px; }
.llm-row-actions .ui-btn { color: var(--text-3); }
.llm-row-actions .ui-btn:hover:not(:disabled) { color: var(--text-1); }
.llm-star-current {
  width: 30px; height: 30px; display: inline-flex; align-items: center; justify-content: center;
}
.llm-star-current svg { width: 15px; height: 15px; fill: var(--accent); stroke: var(--accent); stroke-width: 1.5; stroke-linejoin: round; }
.llm-row-actions .llm-btn-delete:hover:not(:disabled) { color: var(--err); background: var(--err-soft); }
.llm-model-empty .llm-btn-link { color: var(--accent-hi); }
.llm-model-empty .llm-btn-link:hover:not(:disabled) { color: var(--accent-hi); background: var(--accent-faint); }

/* ---------- 默认 / 备用摘要：一条两格的信息条，窄时上下叠放 ---------- */
.llm-summary {
  display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); margin: 0;
  border: 1px solid var(--line); border-radius: var(--r-md);
}
.llm-summary-item { padding: 10px var(--s-4); min-width: 0; }
.llm-summary-item + .llm-summary-item { border-left: 1px solid var(--line); }
.llm-summary-item dt { font-size: var(--fs-cap); color: var(--text-3); }
.llm-summary-item dd {
  margin: 2px 0 0; font-size: var(--fs-sm); font-weight: 600; color: var(--text-1);
  display: flex; align-items: baseline; flex-wrap: wrap; column-gap: var(--s-2);
}
.llm-summary-item dd.is-unset { color: var(--text-2); font-weight: 500; }
.llm-summary-note { font-size: var(--fs-cap); font-weight: 400; color: var(--text-3); }

.llm-ellipsis { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.mono { font-family: var(--font-mono); }

/* 内容区变窄（按分区宽度而非窗口宽度判断）：状态挪到信息下方，图标按钮留在右侧，
   模型行缩进收窄，信息列不再被挤成竖条 */
@container (max-width: 600px) {
  .llm-profile-head {
    grid-template-columns: 34px minmax(0, 1fr) auto;
    grid-template-areas: "logo info actions" "logo status actions";
    align-items: start;
  }
  .llm-model-row {
    grid-template-columns: minmax(0, 1fr) auto;
    grid-template-areas: "info actions" "status actions";
    align-items: start;
  }
  .llm-profile-head > .llm-status,
  .llm-model-row > .llm-status { margin-top: 6px; }
  .llm-profile-card { --llm-indent: var(--s-4); }
  .llm-summary { grid-template-columns: minmax(0, 1fr); }
  .llm-summary-item + .llm-summary-item { border-left: none; border-top: 1px solid var(--line); }
}
</style>
