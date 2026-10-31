<template>
  <div class="settings-stack">
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">识别引擎</h3>
        <div class="choice-grid" role="radiogroup" aria-label="识别引擎">
          <button
            v-for="engine in engines"
            :key="engine.value"
            type="button"
            role="radio"
            class="choice-card"
            :class="{ 'is-muted': engine.pending }"
            :aria-checked="form.asr_provider === engine.value ? 'true' : 'false'"
            @click="form.asr_provider = engine.value"
          >
            <span class="choice-head">
              <span class="choice-title">{{ engine.label }}</span>
              <BaseBadge :tone="engine.tone">{{ engine.chip }}</BaseBadge>
            </span>
            <span class="choice-desc">{{ engine.desc }}</span>
          </button>
        </div>
      </div>
    </section>

    <!-- 豆包语音（火山引擎 · 在线流式）：Access Token 只写不读，后端加密落盘 -->
    <section v-if="form.asr_provider === 'doubao_asr'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">豆包语音 · 接入配置</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">Access Token</span>
            <span class="setting-desc">
              {{ form.doubao_asr_api_key_set ? '已保存（留空表示不修改）' : '火山语音技术控制台的 Access Token，注意与方舟大模型 key 不通用。' }}
            </span>
          </div>
          <BaseInput v-model.trim="form.doubao_asr_api_key" type="password" autocomplete="new-password" placeholder="请输入 Access Token" aria-label="Access Token" />
        </div>
        <div v-if="form.doubao_asr_api_key_set" class="setting-item">
          <div class="setting-info">
            <span class="setting-label">清除已保存的 Token</span>
            <span class="setting-desc">开启后保存设置会删除已加密存储的 Access Token。</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.doubao_asr_api_key_clear" type="checkbox" aria-label="清除已保存的 Token">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">Resource ID</span>
            <span class="setting-desc">必须与账号实际开通的产品一致，否则握手直接 403。ASR 1.0 用 bigasr，2.0 用 seedasr。</span>
          </div>
          <BaseSelect v-model="form.doubao_asr_resource_id" aria-label="Resource ID">
            <option value="volc.seedasr.sauc.duration">volc.seedasr.sauc.duration（ASR 2.0 · 时长）</option>
            <option value="volc.seedasr.sauc.concurrent">volc.seedasr.sauc.concurrent（ASR 2.0 · 并发）</option>
            <option value="volc.bigasr.sauc.duration">volc.bigasr.sauc.duration（ASR 1.0 · 时长）</option>
            <option value="volc.bigasr.sauc.concurrent">volc.bigasr.sauc.concurrent（ASR 1.0 · 并发）</option>
          </BaseSelect>
        </div>
        <details class="settings-advanced">
          <summary>高级：接入点、强制断句静音时长</summary>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">接入点</span>
              <span class="setting-desc">双向流式 WebSocket 地址，默认无需修改。</span>
            </div>
            <BaseInput v-model.trim="form.doubao_asr_endpoint" type="text" placeholder="wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async" aria-label="接入点" />
          </div>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">强制断句静音时长</span>
              <span class="setting-desc">毫秒，默认 800：停顿这么久就断句，悬浮窗里这句随即从淡白变为确认色。0 表示跟随服务端语义分段（停顿后基本不断句，要到松开才确认）；建议 200–800（最大 1000），超出该范围服务端可能拒绝并断开连接。</span>
            </div>
            <BaseInput v-model.number="form.doubao_asr_end_window_size" type="number" min="0" max="10000" step="100" aria-label="强制断句静音时长" />
          </div>
        </details>
      </div>
    </section>

    <!-- ChatGPT 听写（在线批量）：访问令牌只写不读，后端加密落盘 -->
    <section v-if="form.asr_provider === 'chatgpt_asr'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">ChatGPT 听写 · 接入配置</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">访问令牌</span>
            <span class="setting-desc">
              {{ form.chatgpt_asr_api_key_set ? '已保存（留空表示不修改）' : 'ChatGPT 网页账号的 access token（等同登录态，注意不是 platform.openai.com 的 API Key）。' }}
            </span>
          </div>
          <BaseInput v-model.trim="form.chatgpt_asr_api_key" type="password" autocomplete="new-password" placeholder="粘贴 ChatGPT 访问令牌（可带 Bearer 前缀）" aria-label="访问令牌" />
        </div>
        <div v-if="form.chatgpt_asr_api_key_set" class="setting-item">
          <div class="setting-info">
            <span class="setting-label">清除已保存的令牌</span>
            <span class="setting-desc">开启后保存设置会删除已加密存储的访问令牌。</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.chatgpt_asr_api_key_clear" type="checkbox" aria-label="清除已保存的令牌">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">请求超时</span>
            <span class="setting-desc">秒。批量转写要等整段音频处理完，长录音可适当调大。</span>
          </div>
          <BaseInput v-model.number="form.chatgpt_asr_timeout_seconds" type="number" min="10" max="600" step="10" aria-label="请求超时" />
        </div>
        <details class="settings-advanced">
          <summary>高级：端点</summary>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">端点</span>
              <span class="setting-desc">ChatGPT 听写接口地址，默认无需修改。</span>
            </div>
            <BaseInput v-model.trim="form.chatgpt_asr_endpoint" type="text" placeholder="https://chatgpt.com/backend-api/transcribe" aria-label="端点" />
          </div>
        </details>
        <div v-if="form.recording_mode === 'streaming'" class="settings-note warn">
          <span>ChatGPT 听写只支持单段 / 多段录音，当前录音模式是「流式」。</span>
          <BaseButton variant="outline" size="sm" @click="goSection('recording')">去切换</BaseButton>
        </div>
      </div>
    </section>

    <section v-if="form.asr_provider === 'whisper'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">faster-whisper · 本地模型</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">模型</span>
            <span class="setting-desc">模型越大识别越准，但速度也会更慢。</span>
          </div>
          <BaseSelect v-model="form.whisper_model" aria-label="Whisper 模型">
            <option value="tiny">tiny</option>
            <option value="base">base</option>
            <option value="small">small</option>
            <option value="medium">medium</option>
            <option value="large-v3">large-v3</option>
          </BaseSelect>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">推理设备</span>
            <span class="setting-desc">自动会优先使用 CUDA，否则回退 CPU。</span>
          </div>
          <BaseSelect v-model="form.whisper_device" aria-label="Whisper 推理设备">
            <option value="auto">自动</option>
            <option value="cuda">CUDA</option>
            <option value="cpu">CPU</option>
          </BaseSelect>
        </div>
      </div>
    </section>

    <section v-if="form.asr_provider === 'qwen_asr'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">Qwen3-ASR · 本地模型</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">模型</span>
            <span class="setting-desc">支持 Hugging Face 模型名或本地模型目录。</span>
          </div>
          <BaseInput v-model.trim="form.qwen_asr_model" type="text" placeholder="Qwen/Qwen3-ASR-1.7B" aria-label="Qwen3-ASR 模型" />
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">推理设备</span>
            <span class="setting-desc">自动会优先尝试 CUDA，否则回退 CPU。</span>
          </div>
          <BaseSelect v-model="form.qwen_asr_device" aria-label="Qwen 推理设备">
            <option value="auto">自动</option>
            <option value="cuda:0">CUDA:0</option>
            <option value="cpu">CPU</option>
          </BaseSelect>
        </div>
        <details class="settings-advanced">
          <summary>高级：精度、输出长度、时间戳</summary>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">dtype</span>
              <span class="setting-desc">默认自动推断；显存紧张时可手动调整。</span>
            </div>
            <BaseSelect v-model="form.qwen_asr_dtype" aria-label="Qwen dtype">
              <option value="auto">自动</option>
              <option value="bfloat16">bfloat16</option>
              <option value="float16">float16</option>
              <option value="float32">float32</option>
            </BaseSelect>
          </div>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">最大输出 token</span>
              <span class="setting-desc">限制单次生成长度，避免极端长音频时输出失控。</span>
            </div>
            <BaseInput v-model.number="form.qwen_asr_max_new_tokens" type="number" min="32" max="2048" step="32" aria-label="最大输出 token" />
          </div>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">音频长度上限</span>
              <span class="setting-desc">秒。单次送入模型的音频上限。</span>
            </div>
            <BaseInput v-model.number="form.qwen_asr_max_audio_seconds" type="number" min="5" max="600" step="5" aria-label="音频长度上限" />
          </div>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">返回时间戳</span>
              <span class="setting-desc">开启后返回更细粒度的时间信息，代价是更重的推理链路。</span>
            </div>
            <label class="toggle-switch">
              <input v-model="form.qwen_asr_return_timestamps" type="checkbox" aria-label="返回时间戳">
              <span class="toggle-slider"></span>
            </label>
          </div>
          <div v-if="form.qwen_asr_return_timestamps" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">启用强制对齐器</span>
              <span class="setting-desc">进一步细化时间戳，但会显著增加显存和加载时间。</span>
            </div>
            <label class="toggle-switch">
              <input v-model="form.qwen_asr_use_forced_aligner" type="checkbox" aria-label="启用强制对齐器">
              <span class="toggle-slider"></span>
            </label>
          </div>
          <div v-if="form.qwen_asr_return_timestamps && form.qwen_asr_use_forced_aligner" class="setting-item">
            <div class="setting-info">
              <span class="setting-label">强制对齐器模型</span>
              <span class="setting-desc">默认使用 Qwen/Qwen3-ForcedAligner-0.6B，也可改为本地目录。</span>
            </div>
            <BaseInput v-model.trim="form.qwen_asr_forced_aligner_model" type="text" placeholder="Qwen/Qwen3-ForcedAligner-0.6B" aria-label="强制对齐器模型" />
          </div>
        </details>
      </div>
    </section>

    <section class="settings-section">
      <div class="settings-group">
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">识别语言</span>
            <span class="setting-desc">所有引擎共用。</span>
          </div>
          <BaseSelect v-model="form.asr_language" aria-label="识别语言">
            <option value="zh">中文</option>
            <option value="en">英文</option>
            <option value="ja">日文</option>
            <option value="ko">韩文</option>
          </BaseSelect>
        </div>
      </div>
    </section>
  </div>
</template>

<script setup>
import { computed, inject } from 'vue'
import BaseBadge from '@/components/common/BaseBadge.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseInput from '@/components/common/BaseInput.vue'
import BaseSelect from '@/components/common/BaseSelect.vue'
import { SETTINGS_CTX } from '../settingsContext.js'

const { form, goSection } = inject(SETTINGS_CTX)

// 本地引擎（faster-whisper / Qwen3-ASR）在 Tauri 版已接入：进程内推理，模型目录见设置文档
const engines = computed(() => [
  {
    value: 'doubao_asr',
    label: '豆包语音',
    chip: form.value.doubao_asr_api_key_set ? '在线 · 已配置' : '在线 · 未配置',
    tone: form.value.doubao_asr_api_key_set ? 'ok' : 'warn',
    desc: '火山引擎大模型识别，支持单段 / 多段 / 流式'
  },
  {
    value: 'chatgpt_asr',
    label: 'ChatGPT 听写',
    chip: form.value.chatgpt_asr_api_key_set ? '在线 · 已配置' : '在线 · 未配置',
    tone: form.value.chatgpt_asr_api_key_set ? 'ok' : 'warn',
    desc: '使用 ChatGPT 账号令牌，适合单段 / 多段'
  },
  {
    value: 'whisper',
    label: 'faster-whisper',
    chip: '本地',
    tone: 'neutral',
    desc: '离线识别（CTranslate2 进程内推理），模型目录 models/faster_whisper'
  },
  {
    value: 'qwen_asr',
    label: 'Qwen3-ASR',
    chip: '本地',
    tone: 'neutral',
    desc: '离线识别（sherpa-onnx + ONNX，Qwen3-ASR-0.6B），CPU 即可'
  }
])
</script>
