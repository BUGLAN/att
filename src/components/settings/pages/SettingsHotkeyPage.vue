<template>
  <div class="settings-stack">
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">录音快捷键</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">按住录音</span>
            <span class="setting-desc">按住说话，松开结束。</span>
          </div>
          <div class="hotkey-input-wrapper">
            <BaseInput
              :value="hotkeyDisplay"
              class="hotkey-input"
              :class="{ listening: isCapturingHotkey }"
              type="text"
              readonly
              aria-label="按住录音快捷键"
              placeholder="未设置 · 点击录入"
              @click="startHotkeyCapture('hotkey')"
            />
            <BaseButton v-if="hotkeyDisplay" variant="ghost" icon title="清空快捷键" aria-label="清空按住录音快捷键" @click="clearHotkey('hotkey')">
              <template #icon>
                <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </template>
            </BaseButton>
            <BaseButton variant="ghost" icon title="恢复默认快捷键" aria-label="恢复按住录音默认快捷键" @click="resetHotkey('hotkey')">
              <template #icon>
                <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
                  <path d="M4 10.5A8 8 0 1 1 6.6 17" />
                  <path d="M4 5v5.5h5.5" />
                </svg>
              </template>
            </BaseButton>
          </div>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">点按录音</span>
            <span class="setting-desc">按一下开始，再按一下结束。</span>
          </div>
          <div class="hotkey-input-wrapper">
            <BaseInput
              :value="toggleHotkeyDisplay"
              class="hotkey-input"
              :class="{ listening: isCapturingToggleHotkey }"
              type="text"
              readonly
              aria-label="点按录音快捷键"
              placeholder="未设置 · 点击录入"
              @click="startHotkeyCapture('record_toggle_hotkey')"
            />
            <BaseButton
              v-if="toggleHotkeyDisplay"
              variant="ghost"
              icon
              title="清空快捷键"
              aria-label="清空点按录音快捷键"
              @click="clearHotkey('record_toggle_hotkey')"
            >
              <template #icon>
                <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
                  <path d="M6 6l12 12M18 6L6 18" />
                </svg>
              </template>
            </BaseButton>
          </div>
        </div>
        <div v-if="!hotkeyDisplay && !toggleHotkeyDisplay" class="settings-note warn">
          <span>两种录音快捷键都为空，只能在首页点印章录音。</span>
        </div>
        <div v-if="status.error" class="settings-note warn">
          <span>{{ status.error }}</span>
          <BaseButton variant="outline" size="sm" @click="goSection('permissions')">查看权限</BaseButton>
        </div>
      </div>
    </section>

    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">优化选中文本</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">启用</span>
            <span class="setting-desc">选中文字后按快捷键，用 LLM 改写并替换原文。</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.rewrite_enabled" type="checkbox" aria-label="启用优化选中文本">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">快捷键</span>
          </div>
          <div class="hotkey-input-wrapper">
            <BaseInput
              :value="rewriteHotkeyDisplay"
              class="hotkey-input"
              :class="{ listening: isCapturingRewriteHotkey }"
              type="text"
              readonly
              :disabled="!form.rewrite_enabled"
              aria-label="优化选中文本快捷键"
              placeholder="点击录入快捷键"
              @click="startHotkeyCapture('rewrite_hotkey')"
            />
            <BaseButton
              variant="ghost"
              icon
              title="恢复默认快捷键"
              aria-label="恢复默认快捷键"
              :disabled="!form.rewrite_enabled"
              @click="resetHotkey('rewrite_hotkey')"
            >
              <template #icon>
                <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
                  <path d="M4 10.5A8 8 0 1 1 6.6 17" />
                  <path d="M4 5v5.5h5.5" />
                </svg>
              </template>
            </BaseButton>
          </div>
        </div>
      </div>
    </section>

    <div v-if="hotkeyError" class="settings-note warn">
      <span>{{ hotkeyError }}</span>
    </div>
  </div>
</template>

<script setup>
import { inject, onMounted, ref } from 'vue'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseInput from '@/components/common/BaseInput.vue'
import { useElectron } from '@/composables/useElectron'
import { SETTINGS_CTX } from '../settingsContext.js'

const {
  form,
  hotkeyDisplay,
  isCapturingHotkey,
  toggleHotkeyDisplay,
  isCapturingToggleHotkey,
  rewriteHotkeyDisplay,
  isCapturingRewriteHotkey,
  hotkeyError,
  startHotkeyCapture,
  resetHotkey,
  clearHotkey,
  goSection
} = inject(SETTINGS_CTX)
const { getHotkeyStatus } = useElectron()
const status = ref({ enabled: true, error: null })

onMounted(async () => {
  try {
    status.value = (await getHotkeyStatus()) || status.value
  } catch (error) {
    console.warn('[Settings] 读取快捷键状态失败：', error)
  }
})
</script>
