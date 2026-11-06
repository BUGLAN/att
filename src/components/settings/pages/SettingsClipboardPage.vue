<template>
  <div class="settings-stack">
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">剪贴板悬浮框</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">启用剪贴板悬浮框</span>
            <span class="setting-desc">按快捷键呼出最近的剪贴板记录，可直接打字搜索。</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.clipboard_panel_enabled" type="checkbox" aria-label="启用剪贴板悬浮框">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <template v-if="form.clipboard_panel_enabled">
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">唤起快捷键</span>
            </div>
            <div class="hotkey-input-wrapper">
              <BaseInput
                :value="panelHotkeyDisplay"
                class="hotkey-input"
                :class="{ listening: isCapturingPanelHotkey }"
                type="text"
                readonly
                aria-label="剪贴板悬浮框快捷键"
                placeholder="点击录入快捷键"
                @click="startHotkeyCapture('clipboard_panel_hotkey')"
              />
              <BaseButton
                variant="ghost"
                icon
                title="恢复默认快捷键"
                aria-label="恢复剪贴板悬浮框默认快捷键"
                @click="resetHotkey('clipboard_panel_hotkey')"
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
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">选中条目后</span>
              <span class="setting-desc">回车或点击时执行；{{ modKey }}↵ 执行另一种。</span>
            </div>
            <BaseSelect v-model="form.clipboard_panel_enter_action" aria-label="选中条目后">
              <option value="paste">粘贴到原来的输入框</option>
              <option value="copy">只复制到剪贴板</option>
            </BaseSelect>
          </div>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">悬浮框尺寸</span>
              <span class="setting-desc">拖动悬浮框边缘调整，自动记住。</span>
            </div>
            <div class="setting-actions-inline">
              <span class="ui-meta ui-mono">{{ sizeText }}</span>
              <BaseButton variant="outline" size="sm" :disabled="isDefaultSize" @click="resetSize">恢复默认</BaseButton>
            </div>
          </div>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">悬浮框位置</span>
              <span class="setting-desc">上下拖动悬浮框头部调整，双击回到底部。</span>
            </div>
            <div class="setting-actions-inline">
              <span class="ui-meta ui-mono">{{ positionText }}</span>
              <BaseButton variant="outline" size="sm" :disabled="!layout.bottom_offset" @click="resetPosition">恢复默认</BaseButton>
            </div>
          </div>
        </template>
      </div>
    </section>

    <div v-if="hotkeyError" class="settings-note warn">
      <span>{{ hotkeyError }}</span>
    </div>
  </div>
</template>

<script setup>
import { computed, inject, onBeforeUnmount, onMounted, ref } from 'vue'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseInput from '@/components/common/BaseInput.vue'
import BaseSelect from '@/components/common/BaseSelect.vue'
import { useElectron } from '@/composables/useElectron'
import { SETTINGS_CTX } from '../settingsContext.js'

const DEFAULT_SIZE = { max_width: 1280, height: 244 }

const {
  form,
  panelHotkeyDisplay,
  isCapturingPanelHotkey,
  hotkeyError,
  startHotkeyCapture,
  resetHotkey
} = inject(SETTINGS_CTX)
const { clipboardPanel, platform } = useElectron()
const modKey = platform === 'darwin' ? '⌘' : 'Ctrl+'

// 尺寸与位置由悬浮框拖动写入、不在设置表单里：这里只读显示，回到本窗口时刷新一次
const layout = ref({ ...DEFAULT_SIZE, bottom_offset: 0 })
const sizeText = computed(() => {
  const width = layout.value.max_width > 0 ? `宽 ${layout.value.max_width}` : '铺满屏幕'
  return `${width} · 高 ${layout.value.height}`
})
const positionText = computed(() => (
  layout.value.bottom_offset > 0 ? `离底 ${layout.value.bottom_offset}` : '贴底'
))
const isDefaultSize = computed(() => (
  layout.value.max_width === DEFAULT_SIZE.max_width && layout.value.height === DEFAULT_SIZE.height
))

async function loadLayout() {
  try {
    const context = await clipboardPanel.getContext()
    if (context?.height) {
      layout.value = {
        max_width: context.max_width ?? DEFAULT_SIZE.max_width,
        height: context.height,
        bottom_offset: context.bottom_offset ?? 0
      }
    }
  } catch (error) {
    console.warn('[Settings] 读取剪贴板悬浮框布局失败：', error)
  }
}

async function resetSize() {
  try {
    await clipboardPanel.resetSize()
  } catch (error) {
    console.warn('[Settings] 恢复剪贴板悬浮框尺寸失败：', error)
  }
  await loadLayout()
}

async function resetPosition() {
  try {
    await clipboardPanel.resetPosition()
  } catch (error) {
    console.warn('[Settings] 恢复剪贴板悬浮框位置失败：', error)
  }
  await loadLayout()
}

onMounted(() => {
  void loadLayout()
  window.addEventListener('focus', loadLayout)
})

onBeforeUnmount(() => {
  window.removeEventListener('focus', loadLayout)
})
</script>
