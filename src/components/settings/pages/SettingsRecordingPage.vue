<template>
  <div class="settings-stack">
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">录音模式</h3>
        <div class="choice-grid" role="radiogroup" aria-label="录音模式">
          <button
            v-for="mode in RECORDING_MODES"
            :key="mode.value"
            type="button"
            role="radio"
            class="choice-card"
            :aria-checked="form.recording_mode === mode.value ? 'true' : 'false'"
            @click="form.recording_mode = mode.value"
          >
            <span class="choice-title">{{ mode.label }}</span>
            <span class="choice-desc">{{ mode.desc }}</span>
          </button>
        </div>

        <!-- 流式可用性只在选了流式时才有意义 -->
        <div
          v-if="form.recording_mode === 'streaming'"
          class="settings-note"
          :class="{ warn: !streamingRuntimeAvailable }"
        >
          <span>{{ streamingRuntimeSummary }}</span>
          <BaseBadge :tone="streamingRuntimeAvailable ? 'ok' : 'warn'">
            {{ streamingRuntimeAvailable ? '流式可用' : '流式不可用' }}
          </BaseBadge>
        </div>

        <div v-if="form.recording_mode === 'multi'" class="setting-item settings-advanced">
          <div class="setting-info">
            <span class="setting-label">静音分段阈值</span>
            <span class="setting-desc">秒。静音持续超过该时长即切出一段送去识别。</span>
          </div>
          <BaseInput v-model.number="form.silence_threshold" type="number" min="0.5" max="5" step="0.5" />
        </div>
      </div>
    </section>

    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">输入设备</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">麦克风</span>
            <span class="setting-desc">跟随系统默认，或固定使用某个输入设备。</span>
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
            <span class="setting-desc">录音期间静音系统输出，结束后自动恢复。</span>
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
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseInput from '@/components/common/BaseInput.vue'
import BaseSelect from '@/components/common/BaseSelect.vue'
import { SETTINGS_CTX } from '../settingsContext.js'

const RECORDING_MODES = [
  { value: 'single', label: '单段', desc: '录完整段后一次识别' },
  { value: 'multi', label: '多段', desc: '按静音自动切段，边录边出字' },
  { value: 'streaming', label: '流式', desc: '边说边出字，延迟最低（需引擎支持）' }
]

const { form, audioInputDevices, streamingRuntimeAvailable, streamingRuntimeSummary } = inject(SETTINGS_CTX)
</script>
