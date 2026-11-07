<template>
  <div class="settings-stack">
    <section class="settings-section">
      <div class="settings-group">
        <div class="preset-group-head">
          <h3 class="settings-group-title">自定义预设</h3>
          <BaseButton variant="solid" size="sm" @click="openPresetCreator()">新增预设</BaseButton>
        </div>
        <p v-if="!groups.custom.length" class="setting-desc">
          暂无自定义预设。
        </p>
        <div v-for="preset in groups.custom" :key="preset.id" class="setting-item">
          <div class="setting-info">
            <span class="setting-label preset-name">
              {{ preset.name }}
              <BaseBadge v-if="preset.id === selectedPresetId" tone="accent">默认</BaseBadge>
              <BaseBadge v-if="preset.id === form.rewrite_preset_id" tone="neutral">优化选中文本</BaseBadge>
            </span>
            <span class="setting-desc preset-excerpt">{{ excerpt(preset) }}</span>
          </div>
          <div class="setting-actions-inline preset-actions">
            <BaseButton variant="ghost" size="sm" @click="openPresetModal(preset)">编辑</BaseButton>
            <BaseButton variant="ghost" size="sm" @click="askDelete(preset)">删除</BaseButton>
            <BaseButton v-if="preset.id !== selectedPresetId" variant="outline" size="sm" @click="setDefault(preset.id)">设为默认</BaseButton>
          </div>
        </div>
      </div>
    </section>

    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">内置预设</h3>
        <p v-if="!groups.builtin.length" class="setting-desc">没有找到内置预设，请重新安装应用。</p>
        <div v-for="preset in groups.builtin" :key="preset.id" class="setting-item">
          <div class="setting-info">
            <span class="setting-label preset-name">
              {{ preset.name }}
              <BaseBadge v-if="preset.id === selectedPresetId" tone="accent">默认</BaseBadge>
              <BaseBadge v-if="preset.id === form.rewrite_preset_id" tone="neutral">优化选中文本</BaseBadge>
            </span>
            <span class="setting-desc preset-excerpt">{{ excerpt(preset) }}</span>
          </div>
          <div class="setting-actions-inline preset-actions">
            <BaseButton variant="ghost" size="sm" @click="openPresetModal(preset)">查看</BaseButton>
            <BaseButton v-if="preset.id !== selectedPresetId" variant="outline" size="sm" @click="setDefault(preset.id)">设为默认</BaseButton>
          </div>
        </div>
      </div>
    </section>

    <ConfirmDialog
      v-model="isDeleteVisible"
      :title="pendingDelete ? `删除预设「${pendingDelete.name}」？` : '删除预设？'"
      :description="deleteDescription"
      confirm-text="删除"
      tone="danger"
      :loading="isDeleting"
      @confirm="confirmDelete"
    />
  </div>
</template>

<script setup>
import { computed, inject, ref } from 'vue'
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import { describeDeleteImpact, presetExcerpt, splitPresets } from '../presetLogic.mjs'
import { SETTINGS_CTX } from '../settingsContext.js'

const { form, allPresets, selectedPresetId, openPresetModal, openPresetCreator, deletePreset } = inject(SETTINGS_CTX)
const groups = computed(() => splitPresets(allPresets.value))

const pendingDelete = ref(null)
const isDeleteVisible = ref(false)
const isDeleting = ref(false)
const deleteDescription = computed(() => describeDeleteImpact(pendingDelete.value, {
  defaultPresetId: selectedPresetId.value,
  rewritePresetId: form.value.rewrite_preset_id
}, allPresets.value))

// 只改选中值：父组件的 watch 负责防抖保存默认预设
function setDefault(id) {
  selectedPresetId.value = id
}

// 列表里显示提示词开头一行，方便区分
function excerpt(preset) {
  return presetExcerpt(preset.prompt_template)
}

function askDelete(preset) {
  pendingDelete.value = preset
  isDeleteVisible.value = true
}

async function confirmDelete() {
  if (!pendingDelete.value) {
    return
  }
  isDeleting.value = true
  try {
    await deletePreset(pendingDelete.value)
    isDeleteVisible.value = false
    pendingDelete.value = null
  } catch {
    // 错误已由 deletePreset 弹 toast；对话框保持打开便于重试
  } finally {
    isDeleting.value = false
  }
}
</script>

<style scoped>
.preset-actions {
  justify-content: flex-end;
  flex-shrink: 0;
}

.preset-name {
  display: inline-flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--s-2);
}

.preset-group-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-3);
  margin-bottom: var(--s-4);
}

.preset-group-head .settings-group-title {
  margin-bottom: 0;
}

.preset-excerpt {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
</style>
