<template>
  <div class="settings-stack">
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">录音快捷键</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">按住录音，松开结束</span>
            <span class="setting-desc">点击右侧输入框后按下新的组合即可录入；支持 `Win+Alt`、`⌘⌥` 这类无主键组合。</span>
          </div>
          <div class="hotkey-input-wrapper">
            <BaseInput
              :value="hotkeyDisplay"
              class="hotkey-input"
              :class="{ listening: isCapturingHotkey }"
              type="text"
              readonly
              aria-label="录音快捷键"
              placeholder="点击录入快捷键"
              @click="startHotkeyCapture('hotkey')"
            />
            <BaseButton variant="ghost" icon title="恢复默认快捷键" aria-label="恢复默认快捷键" @click="resetHotkey('hotkey')">
              <template #icon>
                <svg viewBox="0 0 24 24" fill="none" aria-hidden="true">
                  <path d="M4 10.5A8 8 0 1 1 6.6 17" />
                  <path d="M4 5v5.5h5.5" />
                </svg>
              </template>
            </BaseButton>
          </div>
        </div>
        <div class="settings-note" :class="{ warn: Boolean(status.error) }">
          <span>{{ status.error || '全局快捷键已启用：窗口在后台或最小化时同样生效。' }}</span>
          <BaseButton v-if="status.error" variant="outline" size="sm" @click="goSection('permissions')">查看权限</BaseButton>
        </div>
        <div class="settings-note">
          <span>剪贴板悬浮框的快捷键在「剪贴板」页设置。</span>
          <BaseButton variant="outline" size="sm" @click="goSection('clipboard')">去设置</BaseButton>
        </div>
      </div>
    </section>

    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">优化选中文本</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">启用</span>
            <span class="setting-desc">在任意应用里选中一段文字后按快捷键，用 LLM 优化并流式替换原文，进度显示在录音悬浮条里。</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.rewrite_enabled" type="checkbox" aria-label="启用优化选中文本">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">快捷键</span>
            <span class="setting-desc">按一下即开始；优化途中点悬浮条上的 ✕ 取消。</span>
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
        <div class="settings-note">
          <span>用哪个提示词预设在「输出与优化」页选择。</span>
          <BaseButton variant="outline" size="sm" @click="goSection('output')">去设置</BaseButton>
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
  rewriteHotkeyDisplay,
  isCapturingRewriteHotkey,
  hotkeyError,
  startHotkeyCapture,
  resetHotkey,
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
