<template>
  <div class="settings-stack">
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">转写完成后</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">自动 AI 优化</span>
            <span class="setting-desc">用默认 LLM 模型整理口语文本，优化结果边生成边输入到当前输入框；失败时兜底粘贴原文。</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.auto_optimize" type="checkbox" aria-label="自动 AI 优化">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <div v-if="form.auto_optimize && !hasLlmModel" class="settings-note warn">
          <span>还没有可用的 LLM 模型，自动优化不会生效。</span>
          <BaseButton variant="outline" size="sm" @click="goSection('llm')">去配置</BaseButton>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">默认优化预设</span>
            <span class="setting-desc">提示词来自 prompts/ 目录（只读）。</span>
          </div>
          <div class="setting-actions-inline">
            <BaseSelect v-model="selectedPresetId" :disabled="!builtinPresets.length" aria-label="默认优化预设">
              <option v-if="!builtinPresets.length" value="">未发现 prompts/*.txt</option>
              <option v-for="preset in builtinPresets" :key="preset.id" :value="preset.id">{{ preset.name }}</option>
            </BaseSelect>
            <BaseButton variant="outline" :disabled="!builtinPresets.length" @click="openPresetModal()">查看</BaseButton>
          </div>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">自动复制并粘贴</span>
            <span class="setting-desc">结果写入剪贴板，并输入到当前焦点输入框（开启自动 AI 优化时边生成边输入）。</span>
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
            <span class="setting-desc">按「优化选中文本」快捷键（{{ rewriteHotkeyDisplay }}）时用的提示词，可以和录音的默认预设不同。</span>
          </div>
          <div class="setting-actions-inline">
            <BaseSelect v-model="form.rewrite_preset_id" :disabled="!builtinPresets.length" aria-label="优化选中文本使用的预设">
              <option value="">跟随默认优化预设</option>
              <option v-for="preset in builtinPresets" :key="preset.id" :value="preset.id">{{ preset.name }}</option>
            </BaseSelect>
            <BaseButton variant="outline" :disabled="!builtinPresets.length" @click="openPresetModal(rewritePreset)">查看</BaseButton>
          </div>
        </div>
        <div v-if="!form.rewrite_enabled" class="settings-note">
          <span>「优化选中文本」快捷键当前已关闭。</span>
          <BaseButton variant="outline" size="sm" @click="goSection('hotkey')">去开启</BaseButton>
        </div>
        <div v-else-if="!hasLlmModel" class="settings-note warn">
          <span>还没有可用的 LLM 模型，优化选中文本不会生效。</span>
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
import { useSettingsStore } from '@/stores/settings'
import { SETTINGS_CTX } from '../settingsContext.js'

const { form, builtinPresets, selectedPresetId, openPresetModal, rewriteHotkeyDisplay, goSection } = inject(SETTINGS_CTX)
// 「跟随默认」或预设文件已被删掉时查看默认预设（openPresetModal 不传参即默认预设）
const rewritePreset = computed(() => builtinPresets.value.find((preset) => preset.id === form.value.rewrite_preset_id) || null)
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
