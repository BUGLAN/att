<template>
  <div class="settings-stack">
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">启动</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">开机时启动</span>
            <span class="setting-desc">登录系统后在后台启动，只显示托盘图标。</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.launch_at_login" type="checkbox" aria-label="开机时启动">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <div class="settings-note">
          以系统实际登录项状态为准；在系统设置中删除登录项后，这里会自动关闭。
        </div>
      </div>
    </section>

    <section v-if="platform === 'win32'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">主窗口</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">关闭主窗口时</span>
            <span class="setting-desc">驻留托盘时，全局快捷键、剪贴板和录音仍在后台运行。</span>
          </div>
          <BaseSelect v-model="form.main_window_close_action" aria-label="关闭主窗口时">
            <option value="tray">最小化到托盘</option>
            <option value="quit">退出落墨</option>
          </BaseSelect>
        </div>
        <div class="settings-note">
          选择「退出落墨」后，右上角关闭按钮会直接结束应用；托盘菜单中的「退出」始终可用。
        </div>
      </div>
    </section>
  </div>
</template>

<script setup>
import { inject } from 'vue'
import BaseSelect from '@/components/common/BaseSelect.vue'
import { useElectron } from '@/composables/useElectron'
import { SETTINGS_CTX } from '../settingsContext.js'

const { form } = inject(SETTINGS_CTX)
const { platform } = useElectron()
</script>
