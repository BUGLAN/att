<template>
  <div class="settings-stack">
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">全部预设</h3>
        <p v-if="!builtinPresets.length" class="setting-desc">未发现 prompts/*.txt：请确认安装目录或仓库里的 prompts/ 是否存在。</p>
        <div v-for="preset in builtinPresets" :key="preset.id" class="setting-item">
          <div class="setting-info">
            <span class="setting-label">{{ preset.name }}</span>
            <span class="setting-desc">prompts/{{ preset.name }}.txt</span>
          </div>
          <div class="setting-actions-inline preset-actions">
            <BaseBadge v-if="preset.id === selectedPresetId" tone="accent">默认</BaseBadge>
            <BaseButton variant="ghost" size="sm" @click="openPresetModal(preset)">查看</BaseButton>
            <BaseButton v-if="preset.id !== selectedPresetId" variant="outline" size="sm" @click="setDefault(preset.id)">设为默认</BaseButton>
          </div>
        </div>
      </div>
    </section>
  </div>
</template>

<script setup>
import { inject } from 'vue'
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import { SETTINGS_CTX } from '../settingsContext.js'

const { builtinPresets, selectedPresetId, openPresetModal } = inject(SETTINGS_CTX)

// 只改选中值：父组件的 watch 负责防抖保存默认预设
function setDefault(id) {
  selectedPresetId.value = id
}
</script>

<style scoped>
.preset-actions {
  justify-content: flex-end;
}
</style>
