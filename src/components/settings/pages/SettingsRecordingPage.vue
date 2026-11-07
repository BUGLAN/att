<template>
  <div class="settings-stack">
    <!-- 录音模式已移到「识别引擎」页：流式是否可用取决于引擎，两者放在一起选 -->
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">输入设备</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">麦克风</span>
          </div>
          <BaseSelect v-model="form.audio_input_device">
            <option value="default">系统默认</option>
            <option v-for="device in audioInputDevices" :key="device.deviceId" :value="device.deviceId">
              {{ device.label || `麦克风 ${device.deviceId.slice(0, 8)}` }}
            </option>
          </BaseSelect>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">录音时暂停其他音频</span>
            <span class="setting-desc">结束后自动恢复。</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.pause_audio_on_recording" type="checkbox" aria-label="录音时暂停其他音频">
            <span class="toggle-slider"></span>
          </label>
        </div>
      </div>
    </section>
  </div>
</template>

<script setup>
import { inject } from 'vue'
import BaseSelect from '@/components/common/BaseSelect.vue'
import { SETTINGS_CTX } from '../settingsContext.js'

const { form, audioInputDevices } = inject(SETTINGS_CTX)
</script>
