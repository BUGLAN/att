<template>
  <div class="settings-stack">
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">转写完成后</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">自动 AI 优化</span>
            <span class="setting-desc">用 LLM 整理口语。</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.auto_optimize" type="checkbox" aria-label="自动 AI 优化">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <div v-if="form.auto_optimize && !hasLlmModel" class="settings-note warn">
          <span>未配置 LLM 模型，自动优化不生效。</span>
          <BaseButton variant="outline" size="sm" @click="goSection('llm')">去配置</BaseButton>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">默认优化预设</span>
          </div>
          <div class="setting-actions-inline">
            <BaseSelect v-model="selectedPresetId" :disabled="!allPresets.length" aria-label="默认优化预设">
              <option v-if="!allPresets.length" value="">未发现预设</option>
              <PresetOptions :builtin="groups.builtin" :custom="groups.custom" />
            </BaseSelect>
            <BaseButton variant="outline" :disabled="!allPresets.length" @click="openPresetModal()">查看</BaseButton>
          </div>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">自动复制并粘贴</span>
            <span class="setting-desc">输入到当前输入框。</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.auto_copy_to_clipboard" type="checkbox" aria-label="自动复制并粘贴">
            <span class="toggle-slider"></span>
          </label>
        </div>
      </div>
    </section>

    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">优化选中文本</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">使用的预设</span>
          </div>
          <div class="setting-actions-inline">
            <BaseSelect v-model="form.rewrite_preset_id" :disabled="!allPresets.length" aria-label="优化选中文本使用的预设">
              <option value="">跟随默认优化预设</option>
              <PresetOptions :builtin="groups.builtin" :custom="groups.custom" />
            </BaseSelect>
            <BaseButton variant="outline" :disabled="!allPresets.length" @click="openPresetModal(rewritePreset)">查看</BaseButton>
          </div>
        </div>
        <div v-if="!form.rewrite_enabled" class="settings-note">
          <span>快捷键已关闭。</span>
          <BaseButton variant="outline" size="sm" @click="goSection('hotkey')">去开启</BaseButton>
        </div>
        <div v-else-if="!hasLlmModel" class="settings-note warn">
          <span>未配置 LLM 模型，优化选中文本不生效。</span>
          <BaseButton variant="outline" size="sm" @click="goSection('llm')">去配置</BaseButton>
        </div>
      </div>
    </section>
  </div>
</template>

<script setup>
import { computed, inject, onMounted, ref } from 'vue'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseSelect from '@/components/common/BaseSelect.vue'
import PresetOptions from '../PresetOptions.vue'
import { splitPresets } from '../presetLogic.mjs'
import { useSettingsStore } from '@/stores/settings'
import { SETTINGS_CTX } from '../settingsContext.js'

const { form, allPresets, selectedPresetId, openPresetModal, rewriteHotkeyDisplay, goSection } = inject(SETTINGS_CTX)
const groups = computed(() => splitPresets(allPresets.value))
// 「跟随默认」或预设已被删掉时查看默认预设（openPresetModal 不传参即默认预设）
const rewritePreset = computed(() => allPresets.value.find((preset) => preset.id === form.value.rewrite_preset_id) || null)
const settingsStore = useSettingsStore()
const llmLoaded = ref(false)
// 加载完成前不下结论，避免提示一闪而过
const hasLlmModel = computed(() => !llmLoaded.value || settingsStore.llmModels.length > 0)

// 没打开过「LLM 服务」页时模型列表还没加载，提示会误报：这里补拉一次
onMounted(() => {
  settingsStore.loadLLMState().catch(() => {}).finally(() => {
    llmLoaded.value = true
  })
})
</script>
