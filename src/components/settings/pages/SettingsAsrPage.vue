<template>
  <div class="settings-stack">
    <!-- 引擎与录音模式放在一起选：流式支持豆包与 Windows 微信输入法，切引擎时能立刻看到哪些模式可用 -->
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">识别引擎</h3>
        <div class="choice-grid" role="radiogroup" aria-label="识别引擎">
          <button
            v-for="engine in engines"
            :key="engine.value"
            type="button"
            role="radio"
            class="choice-card engine-card"
            :class="{ 'is-loading': engine.loading, 'is-muted': engine.disabled }"
            :aria-checked="form.asr_provider === engine.value ? 'true' : 'false'"
            :aria-busy="engine.loading ? 'true' : undefined"
            :aria-disabled="engine.disabled ? 'true' : undefined"
            :disabled="engine.disabled"
            @click="selectEngine(engine.value)"
          >
            <span class="choice-title">{{ engine.label }}</span>
            <!-- 状态只用一个小圆点 + 灰字，不做成像按钮的徽章 -->
            <span class="engine-meta" :class="`is-${engine.tone}`">
              <span class="engine-dot" aria-hidden="true"></span>{{ engine.chip }}
            </span>
            <span class="choice-desc">{{ engine.desc }}</span>
            <!-- 加载中：卡片底边一道扫过的光带，详细状态见下方引擎配置区的「模型状态」 -->
            <span v-if="engine.loading" class="engine-card-progress" aria-hidden="true"></span>
          </button>
        </div>
      </div>

      <div class="settings-group">
        <h3 class="settings-group-title">录音模式</h3>
        <div class="choice-grid" role="radiogroup" aria-label="录音模式">
          <button
            v-for="mode in recordingModes"
            :key="mode.value"
            type="button"
            role="radio"
            class="choice-card"
            :class="{ 'is-muted': mode.disabled }"
            :aria-checked="form.recording_mode === mode.value ? 'true' : 'false'"
            :aria-disabled="mode.disabled ? 'true' : undefined"
            :title="mode.hint || undefined"
            @click="selectRecordingMode(mode)"
          >
            <span class="choice-title">{{ mode.label }}</span>
            <span class="choice-desc">{{ mode.disabled ? mode.hint : mode.desc }}</span>
          </button>
        </div>

        <div v-if="modeConflict" class="settings-note warn">
          <span>{{ modeConflict }}</span>
        </div>
        <div v-else-if="form.recording_mode === 'streaming' && streamingRuntimeSummary" class="settings-note warn">
          <span>{{ streamingRuntimeSummary }}</span>
        </div>

        <div v-if="form.recording_mode === 'multi'" class="setting-item settings-advanced">
          <div class="setting-info">
            <span class="setting-label">静音分段阈值</span>
            <span class="setting-desc">停顿超过该秒数即切段。</span>
          </div>
          <BaseInput v-model.number="form.silence_threshold" type="number" min="0.5" max="5" step="0.5" aria-label="静音分段阈值" />
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
              {{ form.doubao_asr_api_key_set ? '已保存（留空表示不修改）' : '火山语音技术控制台的 Access Token（不是方舟 API Key）。' }}
            </span>
          </div>
          <BaseInput v-model.trim="form.doubao_asr_api_key" type="password" autocomplete="new-password" placeholder="请输入 Access Token" aria-label="Access Token" />
        </div>
        <div v-if="doubaoNote" class="settings-note" :class="doubaoNote.tone" role="status">
          <span>{{ doubaoNote.text }}</span>
        </div>
        <div v-if="form.doubao_asr_api_key_set" class="setting-item">
          <div class="setting-info">
            <span class="setting-label">清除已保存的 Token</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.doubao_asr_api_key_clear" type="checkbox" aria-label="清除已保存的 Token">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">Resource ID</span>
            <span class="setting-desc">与控制台开通的产品一致。</span>
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
              <span class="setting-desc">默认无需修改。</span>
            </div>
            <BaseInput v-model.trim="form.doubao_asr_endpoint" type="text" placeholder="wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async" aria-label="接入点" />
          </div>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">强制断句静音时长</span>
              <span class="setting-desc">毫秒；0 跟随服务端。</span>
            </div>
            <BaseInput v-model.number="form.doubao_asr_end_window_size" type="number" min="0" max="10000" step="100" aria-label="强制断句静音时长" />
          </div>
        </details>
      </div>
    </section>

    <!-- 豆包网页语音（匿名在线）：无需密钥，只配请求超时；音频会发往豆包 -->
    <section v-if="form.asr_provider === 'doubao_web_asr'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">豆包网页语音 · 接入配置</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">请求超时</span>
            <span class="setting-desc">秒；单次识别的网络等待上限（10–300）。</span>
          </div>
          <BaseInput v-model.number="form.doubao_web_timeout_seconds" type="number" min="10" max="300" step="10" aria-label="豆包网页语音请求超时" />
        </div>
        <div class="settings-note">
          <span>无需密钥，保存后即可用于录音、文件转写和重新识别；识别时音频会发往豆包。不支持流式录音，语种由音频自动判定。</span>
        </div>
      </div>
    </section>

    <!-- 豆包输入法语音（虚拟设备在线）：无需用户密钥，只配请求超时；音频会发往豆包 -->
    <section v-if="form.asr_provider === 'doubao_ime_asr'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">豆包输入法语音 · 接入配置</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">请求超时</span>
            <span class="setting-desc">秒；单次识别的网络等待上限（10–300）。</span>
          </div>
          <BaseInput v-model.number="form.doubao_ime_timeout_seconds" type="number" min="10" max="300" step="10" aria-label="豆包输入法语音请求超时" />
        </div>
        <div class="settings-note">
          <span>无需密钥：首次使用时自动注册匿名虚拟设备。识别时音频会发往豆包；不支持流式录音，语种由音频自动判定。</span>
        </div>
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
              {{ form.chatgpt_asr_api_key_set ? '已保存（留空表示不修改）' : 'ChatGPT 网页登录的 access token（不是 API Key）。' }}
            </span>
          </div>
          <BaseInput v-model.trim="form.chatgpt_asr_api_key" type="password" autocomplete="new-password" placeholder="粘贴 ChatGPT 访问令牌（可带 Bearer 前缀）" aria-label="访问令牌" />
        </div>
        <div v-if="chatgptNote" class="settings-note" :class="chatgptNote.tone" role="status">
          <span>{{ chatgptNote.text }}</span>
        </div>
        <div v-if="form.chatgpt_asr_api_key_set" class="setting-item">
          <div class="setting-info">
            <span class="setting-label">清除已保存的令牌</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.chatgpt_asr_api_key_clear" type="checkbox" aria-label="清除已保存的令牌">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">请求超时</span>
            <span class="setting-desc">秒。</span>
          </div>
          <BaseInput v-model.number="form.chatgpt_asr_timeout_seconds" type="number" min="10" max="600" step="10" aria-label="请求超时" />
        </div>
        <details class="settings-advanced">
          <summary>高级：端点</summary>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">端点</span>
              <span class="setting-desc">默认无需修改。</span>
            </div>
            <BaseInput v-model.trim="form.chatgpt_asr_endpoint" type="text" placeholder="https://chatgpt.com/backend-api/transcribe" aria-label="端点" />
          </div>
        </details>
      </div>
    </section>

    <!-- DeepSeek 语音识别（在线批量）：API Key 只写不读，后端加密落盘 -->
    <section v-if="form.asr_provider === 'deepseek_asr'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">DeepSeek 语音识别 · 接入配置</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">API Key</span>
            <span class="setting-desc">
              {{ form.deepseek_asr_api_key_set ? '已保存（留空表示不修改）' : 'DeepSeek 开放平台的 API Key（sk- 开头，不是网页登录态）。' }}
            </span>
          </div>
          <BaseInput v-model.trim="form.deepseek_asr_api_key" type="password" autocomplete="new-password" placeholder="粘贴 DeepSeek API Key（可带 Bearer 前缀）" aria-label="DeepSeek API Key" />
        </div>
        <div v-if="deepseekNote" class="settings-note" :class="deepseekNote.tone" role="status">
          <span>{{ deepseekNote.text }}</span>
        </div>
        <div v-if="form.deepseek_asr_api_key_set" class="setting-item">
          <div class="setting-info">
            <span class="setting-label">清除已保存的 Key</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.deepseek_asr_api_key_clear" type="checkbox" aria-label="清除已保存的 Key">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">请求超时</span>
            <span class="setting-desc">秒。</span>
          </div>
          <BaseInput v-model.number="form.deepseek_asr_timeout_seconds" type="number" min="10" max="600" step="10" aria-label="请求超时" />
        </div>
        <div class="settings-note">
          <span>批量识别，暂不支持流式；单次音频最长 120 秒，更长的录音会自动分段识别。</span>
        </div>
        <details class="settings-advanced">
          <summary>高级：端点</summary>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">端点</span>
              <span class="setting-desc">默认无需修改。</span>
            </div>
            <BaseInput v-model.trim="form.deepseek_asr_endpoint" type="text" placeholder="https://api.deepseek.com/v1/tools/asr" aria-label="端点" />
          </div>
        </details>
      </div>
    </section>

    <!-- 微信输入法语音：直接注入本机输入法，无需密钥；仅 Windows -->
    <section v-if="form.asr_provider === 'wetype_asr'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">微信输入法语音 · 本机接入</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">运行状态</span>
            <span class="setting-desc">直接连接本机微信输入法，不经过外部桥进程。</span>
          </div>
          <span class="engine-meta" :class="`is-${wetypeChip.tone}`">
            <span class="engine-dot" aria-hidden="true"></span>{{ wetypeChip.chip }}
          </span>
        </div>
        <div v-if="wetypeStatusNote" class="settings-note" :class="wetypeStatusNote.tone" role="status">
          <span>{{ wetypeStatusNote.text }}</span>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">请求超时</span>
            <span class="setting-desc">秒；长音频还会按“音频时长 × 1.5 + 15 秒”自动放宽。</span>
          </div>
          <BaseInput v-model.number="form.wetype_asr_timeout_seconds" type="number" min="10" max="600" step="10" aria-label="微信输入法语音请求超时" />
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">输入法 PID</span>
            <span class="setting-desc">0 = 自动发现；同时运行多个微信输入法时填具体 PID。</span>
          </div>
          <BaseInput v-model.number="form.wetype_asr_pid" type="number" min="0" step="1" aria-label="微信输入法进程 PID" />
        </div>
        <div class="settings-note">
          <span>识别音频会经微信输入法自己的网络层发往腾讯；首次录音前请确认微信输入法语音功能可正常联网。仅个人自用，不建议作为默认对外服务。</span>
        </div>
      </div>
    </section>

    <section v-if="form.asr_provider === 'whisper'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">faster-whisper · 本地模型</h3>
        <LocalModelStatus
          :view="localView"
          :models="engineModels"
          :job="modelJob"
          :models-dir="modelsDirectoryPath"
          :is-desktop="isDesktop"
          :starting-model-id="startingModelId"
          :vad-warning="modelVadWarning"
          :releasing="isReleasingModel"
          @download="downloadModel"
          @cancel="cancelDownload"
          @delete="askDeleteModel"
          @reveal="revealModels"
          @release="releaseLocalModel"
        >
          <template #directory-actions>
            <ModelsDirectorySetting />
          </template>
        </LocalModelStatus>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">模型</span>
            <span class="setting-desc">越大越准，也越慢。</span>
          </div>
          <BaseSelect v-model="form.whisper_model" aria-label="Whisper 模型">
            <option v-for="option in whisperOptions" :key="option.value" :value="option.value">
              {{ option.label }}
            </option>
          </BaseSelect>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">下载源</span>
            <span class="setting-desc">国内网络打不开 Hugging Face 时切到镜像；GitHub 项仍走官方。</span>
          </div>
          <BaseSelect v-model="form.model_download_source" aria-label="模型下载源">
            <option v-for="source in downloadSources" :key="source.value" :value="source.value">
              {{ source.label }}
            </option>
          </BaseSelect>
        </div>
      </div>
    </section>

    <section v-if="form.asr_provider === 'qwen_asr'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">Qwen3-ASR · 本地模型</h3>
        <LocalModelStatus
          :view="localView"
          :models="engineModels"
          :job="modelJob"
          :models-dir="modelsDirectoryPath"
          :is-desktop="isDesktop"
          :starting-model-id="startingModelId"
          :vad-warning="modelVadWarning"
          :releasing="isReleasingModel"
          @download="downloadModel"
          @cancel="cancelDownload"
          @delete="askDeleteModel"
          @reveal="revealModels"
          @release="releaseLocalModel"
        >
          <template #directory-actions>
            <ModelsDirectorySetting />
          </template>
        </LocalModelStatus>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">下载源</span>
            <span class="setting-desc">Qwen 模型目前只有 GitHub 官方源，镜像选择对它不生效。</span>
          </div>
          <BaseSelect v-model="form.model_download_source" aria-label="模型下载源">
            <option v-for="source in downloadSources" :key="source.value" :value="source.value">
              {{ source.label }}
            </option>
          </BaseSelect>
        </div>
        <!-- 模型目录自动查找（models/ 下的 Qwen3-ASR ONNX），推理固定走 CPU；
             旧版的设备 / dtype / 时间戳 / 强制对齐器选项在 Rust 引擎里没有作用，已从界面移除（字段保留兼容旧数据） -->
        <details class="settings-advanced">
          <summary>高级：输出长度、音频长度上限</summary>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">最大输出 token</span>
            </div>
            <BaseInput v-model.number="form.qwen_asr_max_new_tokens" type="number" min="32" max="2048" step="32" aria-label="最大输出 token" />
          </div>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">音频长度上限</span>
              <span class="setting-desc">秒。</span>
            </div>
            <BaseInput v-model.number="form.qwen_asr_max_audio_seconds" type="number" min="5" max="600" step="5" aria-label="音频长度上限" />
          </div>
        </details>
      </div>
    </section>

    <section v-if="form.asr_provider === 'sense_voice'" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">SenseVoice · 本地模型</h3>
        <LocalModelStatus
          :view="localView"
          :models="engineModels"
          :job="modelJob"
          :models-dir="modelsDirectoryPath"
          :is-desktop="isDesktop"
          :starting-model-id="startingModelId"
          :vad-warning="modelVadWarning"
          :releasing="isReleasingModel"
          @download="downloadModel"
          @cancel="cancelDownload"
          @delete="askDeleteModel"
          @reveal="revealModels"
          @release="releaseLocalModel"
        >
          <template #directory-actions>
            <ModelsDirectorySetting />
          </template>
        </LocalModelStatus>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">模型</span>
            <span class="setting-desc">单模型支持中 / 粤 / 英 / 日 / 韩。</span>
          </div>
          <BaseSelect v-model="form.sense_voice_model" aria-label="SenseVoice 模型">
            <option v-for="option in senseVoiceOptions" :key="option.value" :value="option.value">
              {{ option.label }}
            </option>
          </BaseSelect>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">下载源</span>
            <span class="setting-desc">SenseVoice 模型只有 GitHub 官方源，镜像选择对它不生效。</span>
          </div>
          <BaseSelect v-model="form.model_download_source" aria-label="模型下载源">
            <option v-for="source in downloadSources" :key="source.value" :value="source.value">
              {{ source.label }}
            </option>
          </BaseSelect>
        </div>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">逆文本归一化</span>
            <span class="setting-desc">把数字、金额等口语转成更易读的书面形式。</span>
          </div>
          <label class="toggle-switch">
            <input v-model="form.sense_voice_use_itn" type="checkbox" aria-label="SenseVoice 逆文本归一化">
            <span class="toggle-slider"></span>
          </label>
        </div>
        <details class="settings-advanced">
          <summary>高级：推理线程数</summary>
          <div class="setting-item">
            <div class="setting-info">
              <span class="setting-label">CPU 线程数</span>
              <span class="setting-desc">0 = 默认 2；过高通常收益有限。</span>
            </div>
            <BaseInput v-model.number="form.sense_voice_num_threads" type="number" min="0" max="16" step="1" aria-label="SenseVoice CPU 线程数" />
          </div>
        </details>
      </div>
    </section>

    <section v-if="isLocalProvider(form.asr_provider)" class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">本地模型内存</h3>
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">空闲多久自动释放</span>
            <span class="setting-desc">分钟；0 表示不自动释放。释放后下次使用会重新加载，第一次识别需多等几秒。</span>
          </div>
          <BaseInput
            v-model.number="form.local_model_idle_unload_minutes"
            type="number"
            min="0"
            max="1440"
            step="1"
            aria-label="本地模型空闲自动释放分钟数"
          />
        </div>
      </div>
    </section>

    <section class="settings-section">
      <div class="settings-group">
        <div class="setting-item">
          <div class="setting-info">
            <span class="setting-label">识别语言</span>
            <span class="setting-desc">{{ asrLanguageHint }}</span>
          </div>
          <BaseSelect v-model="form.asr_language" aria-label="识别语言">
            <option v-for="item in asrLanguageOptions" :key="item.value" :value="item.value">{{ item.label }}</option>
          </BaseSelect>
        </div>
      </div>
    </section>

    <!-- 热词：词表与引擎无关地保存，只有部分引擎能用；状态圆点与引擎卡片同一套语义 -->
    <section class="settings-section">
      <div class="settings-group">
        <h3 class="settings-group-title">热词</h3>
        <BaseTextarea
          v-model="form.asr_hotwords_text"
          class="hotword-input"
          rows="6"
          spellcheck="false"
          :placeholder="'每行一个，例如：\nTauri\n落地页\nOption + V'"
          aria-label="热词"
        />
        <!-- 两路各一行：识别随引擎变化，LLM 纠错随模型配置变化 -->
        <ul class="hotword-status" aria-label="热词生效情况">
          <li v-for="row in hotwords.rows" :key="row.key" class="hotword-row">
            <span class="engine-meta hotword-row-label" :class="`is-${row.tone}`">
              <span class="engine-dot" aria-hidden="true"></span>{{ row.label }}
            </span>
            <span class="hotword-row-text" :class="{ 'is-warn': row.tone === 'warn' }">{{ row.text }}</span>
          </li>
        </ul>
      </div>
    </section>

    <ConfirmDialog
      v-model="isDeleteVisible"
      :title="pendingDelete ? `删除模型「${pendingDelete.title}」？` : '删除模型？'"
      :description="deleteDescription"
      confirm-text="删除"
      tone="danger"
      :loading="isDeleting"
      @confirm="confirmDeleteModel"
    />
  </div>
</template>

<script setup>
import { computed, inject, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import BaseInput from '@/components/common/BaseInput.vue'
import BaseSelect from '@/components/common/BaseSelect.vue'
import BaseTextarea from '@/components/common/BaseTextarea.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import { useAppStore } from '@/stores/app'
import LocalModelStatus from '../LocalModelStatus.vue'
import ModelsDirectorySetting from '../ModelsDirectorySetting.vue'
import { modelsDirectoryView } from '../modelsDirectory.mjs'
import { useSettingsStore } from '@/stores/settings'
import { useElectron } from '@/composables/useElectron'
import { SETTINGS_CTX } from '../settingsContext.js'
import {
  isLocalProvider,
  localEngineView,
  recordingModeConflict,
  recordingModeOptions
} from '../asrEngineStatus.mjs'
import {
  languageHint,
  languageOptions as buildLanguageOptions,
  normalizeLanguageForProvider
} from '../asrLanguages.mjs'
import {
  DOWNLOAD_SOURCES,
  formatBytes,
  installedModelSelection,
  isJobActive,
  modelsForProvider,
  vadWarning,
  whisperOptionLabel
} from '../modelManager.mjs'
import { hotwordView, parseHotwords } from '../asrHotwords.mjs'
import {
  credentialNote,
  isWetypeProvider,
  onlineEngineChip,
  requiresCredential,
  wetypeEngineChip,
  WETYPE_PROVIDER
} from '../onlineEngineStatus.mjs'
import { useAsrCredentialCheck } from '@/composables/useAsrCredentialCheck'

const { form, streamingRuntimeSummary } = inject(SETTINGS_CTX)
const appStore = useAppStore()
const settingsStore = useSettingsStore()
const credential = useAsrCredentialCheck()
const { isDesktop, platform, revealAppPath } = useElectron()

const LOCAL_STATUS_POLL_MS = 1000
const MODEL_JOB_POLL_MS = 800

// 微信输入法流式的平台结论以 health/runtime 为准；页面同步推断只作缺省兜底
const wetypePlatformSupported = computed(() => (
  settingsStore.runtimeStatus?.wetype_engine?.platform_supported === true
))
const recordingModes = computed(() => recordingModeOptions(form.value.asr_provider, platform, wetypePlatformSupported.value))
const modeConflict = computed(() => recordingModeConflict(
  form.value.asr_provider,
  form.value.recording_mode,
  platform,
  wetypePlatformSupported.value
))
const asrLanguageOptions = computed(() => buildLanguageOptions(form.value.asr_provider))
const asrLanguageHint = computed(() => languageHint(form.value.asr_provider))
// LLM 模型列表没加载过时按「有模型」处理，免得误报（同输出页）；挂载时补拉一次
const llmLoaded = ref(false)
const hotwords = computed(() => hotwordView(form.value.asr_provider, parseHotwords(form.value.asr_hotwords_text), {
  hasLlmModel: !llmLoaded.value || settingsStore.llmModels.length > 0,
  autoOptimize: Boolean(form.value.auto_optimize)
}))

onMounted(() => {
  settingsStore.loadLLMState().catch(() => {}).finally(() => {
    llmLoaded.value = true
  })
})
// 「提前加载 / 重试」请求进行中：后端可能还没来得及置位 loading，前端先按加载中展示
const isLoadingModel = ref(false)
const isReleasingModel = ref(false)
const localView = computed(() => {
  const provider = form.value.asr_provider
  const status = isLoadingModel.value ? { provider, loading: true } : settingsStore.runtimeStatus.local_engine
  return localEngineView(provider, status)
})

// 本地模型清单：只展示当前引擎相关的项（Whisper 带 VAD；Qwen 只有模型本身）
const engineModels = computed(() => modelsForProvider(settingsStore.modelCatalog.models, form.value.asr_provider))
const modelJob = computed(() => settingsStore.modelCatalog.job)
const modelsDir = computed(() => settingsStore.modelCatalog.models_dir || '')
const modelsDirectoryPath = computed(() => modelsDirectoryView({
  savedDir: settingsStore.settings.models_dir,
  draftDir: form.value.models_dir,
  effectiveDir: settingsStore.settings.models_dir_effective,
  nextDir: settingsStore.settings.models_dir_next,
  envOverride: Boolean(settingsStore.settings.models_dir_env_override),
  desktop: isDesktop
}).path || modelsDir.value)
const downloadSources = DOWNLOAD_SOURCES
const modelVadWarning = computed(() => {
  if (localView.value.tip.includes('未安装 VAD')) {
    return ''
  }
  return vadWarning(form.value.asr_provider, settingsStore.modelCatalog.models)
})
const WHISPER_VALUES = ['tiny', 'base', 'small', 'medium', 'large-v3', 'large-v3-turbo']
const whisperOptions = computed(() => WHISPER_VALUES.map((value) => ({
  value,
  label: whisperOptionLabel(value, settingsStore.modelCatalog.models)
})))
const senseVoiceOptions = computed(() => {
  const options = engineModels.value
    .filter((model) => model.engine === 'sense_voice' && model.select_value)
    .map((model) => ({
      value: model.select_value,
      label: model.status === 'installed'
        ? `${model.select_value} · 已安装`
        : `${model.select_value} · 未安装（${formatBytes(model.download_size)}）`
    }))
  return options.length
    ? options
    : [{ value: form.value.sense_voice_model, label: form.value.sense_voice_model }]
})

// 删除模型前的二次确认（录音 / 转写进行中后端会拒绝，提示原因）
const pendingDelete = ref(null)
const isDeleteVisible = ref(false)
const isDeleting = ref(false)
const deleteDescription = computed(() => {
  if (!pendingDelete.value) {
    return ''
  }
  return `会释放 ${formatBytes(pendingDelete.value.installed_bytes)}，之后可以在同一位置重新下载。`
})

let modelPollTimer = null
let modelPollToken = 0
const startingModelId = ref('')

function stopModelPolling() {
  modelPollToken += 1
  if (modelPollTimer) {
    window.clearTimeout(modelPollTimer)
    modelPollTimer = null
  }
}

function startModelPolling() {
  if (modelPollTimer || !isJobActive(settingsStore.modelCatalog.job)) {
    return
  }
  const token = ++modelPollToken
  modelPollTimer = window.setTimeout(() => pollModelJob(token), MODEL_JOB_POLL_MS)
}

async function pollModelJob(token) {
  modelPollTimer = null
  if (token !== modelPollToken) {
    return
  }
  const jobId = settingsStore.modelCatalog.job?.id
  if (!jobId) {
    stopModelPolling()
    return
  }
  try {
    await settingsStore.loadModelJob(jobId)
  } catch {
    // 单次失败不打断：下一拍再试
  }
  if (token !== modelPollToken) {
    return
  }
  if (isJobActive(settingsStore.modelCatalog.job)) {
    modelPollTimer = window.setTimeout(() => pollModelJob(token), MODEL_JOB_POLL_MS)
    return
  }

  stopModelPolling()
  await settingsStore.loadModelCatalog().catch(() => {})
  applyInstalledModelSelection(settingsStore.modelCatalog.job)
  await settingsStore.loadRuntimeStatus(false).catch(() => {})
}

function applyInstalledModelSelection(job) {
  const selection = installedModelSelection(job, settingsStore.modelCatalog.models)
  if (selection) {
    form.value[selection.key] = selection.value
  }
}

onMounted(async () => {
  try {
    await settingsStore.loadModelCatalog()
    if (isJobActive(settingsStore.modelCatalog.job)) {
      startModelPolling()
    }
  } catch (error) {
    console.warn('[Settings] 读取本地模型清单失败:', error)
  }
})

onBeforeUnmount(stopModelPolling)

async function downloadModel(model) {
  if (startingModelId.value) {
    return
  }
  startingModelId.value = model.id
  try {
    await settingsStore.downloadModel(model.id, form.value.model_download_source)
    startModelPolling()
    try {
      await settingsStore.loadModelCatalog()
    } catch (error) {
      console.warn('[Settings] 下载启动后刷新模型清单失败:', error)
    }
  } catch (error) {
    startModelPolling()
    appStore.showToast(error.message || '下载失败')
  } finally {
    startingModelId.value = ''
  }
}

async function cancelDownload(jobId) {
  try {
    await settingsStore.cancelModelJob(jobId)
    startModelPolling()
  } catch (error) {
    appStore.showToast(error.message || '取消失败')
  }
}

async function revealModels() {
  if (!isDesktop) {
    return
  }
  try {
    await revealAppPath('models')
  } catch (error) {
    appStore.showToast(error.message || '无法打开模型目录')
  }
}

function askDeleteModel(model) {
  pendingDelete.value = model
  isDeleteVisible.value = true
}

async function confirmDeleteModel() {
  if (!pendingDelete.value) {
    return
  }
  isDeleting.value = true
  try {
    await settingsStore.deleteModel(pendingDelete.value.id)
    await settingsStore.loadRuntimeStatus(false).catch(() => {})
    isDeleteVisible.value = false
    pendingDelete.value = null
  } catch (error) {
    appStore.showToast(error.message || '删除模型失败')
  } finally {
    isDeleting.value = false
  }
}

// 点引擎卡片：切换引擎；再点已选中的本地引擎 = 提前加载（未加载时）或重试（加载失败时），界面上不另放按钮
function selectEngine(value) {
  const selected = engines.value.find((engine) => engine.value === value)
  if (selected?.disabled) {
    return
  }
  if (form.value.asr_provider === value && ['idle', 'failed'].includes(localView.value.state)) {
    loadLocalModel()
    return
  }
  // 再点已选中、已填凭据的在线引擎 = 重新验证（与本地引擎「再点一下重试」同一个手势）
  if (
    form.value.asr_provider === value &&
    (requiresCredential(value) || isWetypeProvider(value)) &&
    onlineKeySet(value)
  ) {
    credential.verify(value)
    return
  }
  form.value.asr_provider = value
  const adjusted = normalizeLanguageForProvider(value, form.value.asr_language)
  if (adjusted.changed) {
    form.value.asr_language = adjusted.language
  }
  if (adjusted.message) {
    appStore.showToast(adjusted.message, { tone: 'info' })
  }
}

function onlineKeySet(provider) {
  if (isWetypeProvider(provider)) return platform === 'win32'
  if (provider === 'doubao_asr') return Boolean(form.value.doubao_asr_api_key_set)
  if (provider === 'deepseek_asr') return Boolean(form.value.deepseek_asr_api_key_set)
  if (provider === 'chatgpt_asr') return Boolean(form.value.chatgpt_asr_api_key_set)
  // 豆包网页语音没有凭据
  return false
}

// 在线引擎：卡片状态与接入配置区的提示都跟随凭据验证结论
function onlineChip(provider) {
  return onlineEngineChip({
    keySet: onlineKeySet(provider),
    check: credential.checks[provider] || null,
    requiresCredentials: requiresCredential(provider)
  })
}

const doubaoNote = computed(() => (form.value.doubao_asr_api_key_set ? credentialNote(credential.checks.doubao_asr) : null))
const chatgptNote = computed(() => (form.value.chatgpt_asr_api_key_set ? credentialNote(credential.checks.chatgpt_asr) : null))
const deepseekNote = computed(() => (form.value.deepseek_asr_api_key_set ? credentialNote(credential.checks.deepseek_asr) : null))

const wetypeCheck = computed(() => {
  const runtime = settingsStore.runtimeStatus?.wetype_engine
  if (platform === 'win32' && runtime?.available === false) {
    return { status: 'invalid', message: runtime.message || '请先启动微信输入法' }
  }
  const check = credential.checks[WETYPE_PROVIDER]
  if (check) {
    return check
  }
  if (platform === 'win32' && runtime?.available === true) {
    return { status: 'ok', message: '' }
  }
  return null
})
const wetypeChip = computed(() => wetypeEngineChip({ platform, check: wetypeCheck.value }))
const wetypeStatusNote = computed(() => {
  if (platform !== 'win32') {
    return { tone: 'warn', text: '微信输入法语音只能在 Windows 上使用。' }
  }
  const check = wetypeCheck.value
  if (check) {
    return credentialNote(check)
  }
  return null
})

// 进入本页时：当前可检测引擎没有「可用」结论就补验一次（上次没验成、进程刚启动等）
onMounted(async () => {
  const provider = form.value.asr_provider
  if (!(requiresCredential(provider) || isWetypeProvider(provider)) || !onlineKeySet(provider)) {
    return
  }
  const check = await credential.load(provider)
  if (check?.status !== 'ok' && check?.status !== 'checking') {
    credential.verify(provider)
  }
})

function selectRecordingMode(mode) {
  if (mode.disabled) {
    appStore.showToast(`${mode.hint}，请先切换到豆包语音`)
    return
  }
  form.value.recording_mode = mode.value
}

// 切到本地引擎后后端在后台预加载：加载中每秒刷新一次状态，结束（就绪/失败/缺模型）即停
let pollTimer = null

function stopPolling() {
  if (pollTimer) {
    window.clearInterval(pollTimer)
    pollTimer = null
  }
}

watch(() => localView.value.state === 'loading', (loading) => {
  if (!loading) {
    stopPolling()
    return
  }
  if (pollTimer) {
    return
  }
  pollTimer = window.setInterval(() => {
    settingsStore.loadRuntimeStatus(false).catch(() => {
      // 单次探测失败不打断轮询：下一拍再试
    })
  }, LOCAL_STATUS_POLL_MS)
}, { immediate: true })

onBeforeUnmount(stopPolling)

async function loadLocalModel() {
  if (isLoadingModel.value) {
    return
  }
  isLoadingModel.value = true
  try {
    await settingsStore.startStreamingService(true)
    await settingsStore.loadRuntimeStatus(false)
  } catch (error) {
    appStore.showToast(error.message || '模型加载失败')
    await settingsStore.loadRuntimeStatus(false).catch(() => {})
  } finally {
    isLoadingModel.value = false
  }
}

async function releaseLocalModel() {
  if (isReleasingModel.value || localView.value.state !== 'ready') {
    return
  }
  isReleasingModel.value = true
  try {
    await settingsStore.stopStreamingService()
    appStore.showToast('本地模型内存已释放')
  } catch (error) {
    appStore.showToast(error.message || '释放本地模型失败')
    await settingsStore.loadRuntimeStatus(false).catch(() => {})
  } finally {
    isReleasingModel.value = false
  }
}

// 本地引擎的卡片徽章跟随加载状态（只描述当前选中的那个；其余显示「本地」）
function localChip(value) {
  if (form.value.asr_provider !== value || localView.value.state === 'none') {
    return { chip: '本地', tone: 'neutral', loading: false }
  }
  return {
    chip: `本地 · ${localView.value.chip}`,
    // 加载中用强调金，与卡片底边的光带同色
    tone: localView.value.state === 'loading' ? 'accent' : localView.value.tone,
    loading: localView.value.state === 'loading'
  }
}

// 本地引擎（faster-whisper / Qwen3-ASR / SenseVoice）在 Tauri 版已接入：进程内推理，模型目录见设置文档
const engines = computed(() => [
  {
    value: 'doubao_asr',
    label: '豆包语音',
    ...onlineChip('doubao_asr'),
    desc: '单段 / 多段 / 流式'
  },
  {
    value: 'doubao_web_asr',
    label: '豆包网页语音',
    ...onlineChip('doubao_web_asr'),
    desc: '单段 / 多段 · 无需密钥'
  },
  {
    value: 'doubao_ime_asr',
    label: '豆包输入法语音',
    ...onlineChip('doubao_ime_asr'),
    desc: '单段 / 多段 · 无需密钥'
  },
  {
    value: 'chatgpt_asr',
    label: 'ChatGPT 听写',
    ...onlineChip('chatgpt_asr'),
    desc: '单段 / 多段'
  },
  {
    value: 'deepseek_asr',
    label: 'DeepSeek',
    ...onlineChip('deepseek_asr'),
    desc: '单段 / 多段'
  },
  {
    value: WETYPE_PROVIDER,
    label: '微信输入法语音',
    ...wetypeChip.value,
    disabled: platform !== 'win32',
    desc: 'Windows · 本机注入'
  },
  {
    value: 'whisper',
    label: 'faster-whisper',
    ...localChip('whisper'),
    desc: '离线，需安装模型'
  },
  {
    value: 'qwen_asr',
    label: 'Qwen3-ASR',
    ...localChip('qwen_asr'),
    desc: '离线，需安装模型'
  },
  {
    value: 'sense_voice',
    label: 'SenseVoice',
    ...localChip('sense_voice'),
    desc: '中粤英日韩 · 本地离线'
  }
])
</script>

<style scoped>
/* 选择器带上父级：与 BaseTextarea 自身的 scoped 样式同权重时谁后加载谁赢，这里要稳定压过它 */
.settings-group .hotword-input {
  font-size: var(--fs-sm);
  line-height: 1.7;
}

/* 生效情况：左列是「通道 + 状态圆点」，右列是说明；窄屏时说明换到下一行 */
.hotword-status {
  display: grid;
  gap: var(--s-2);
  margin: var(--s-3) 0 0;
  padding: var(--s-3);
  list-style: none;
  border: 1px solid var(--line);
  border-radius: var(--r-sm);
  background: var(--bg-raise);
}

.hotword-row {
  display: grid;
  grid-template-columns: minmax(132px, max-content) 1fr;
  align-items: baseline;
  gap: var(--s-1) var(--s-3);
}

.hotword-row-label {
  color: var(--text-2);
  white-space: nowrap;
}

.hotword-row-text {
  font-size: var(--fs-cap);
  color: var(--text-3);
  line-height: 1.6;
}

.hotword-row-text.is-warn {
  color: var(--text-2);
}

@media (max-width: 560px) {
  .hotword-row {
    grid-template-columns: 1fr;
  }
}

.engine-card {
  position: relative;
  overflow: hidden;
}

/* 选中的本地引擎加载中：底边 2px 金色光带来回扫过 */
.engine-card-progress {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  height: 2px;
  background: var(--accent-faint);
  overflow: hidden;
}

.engine-card-progress::after {
  content: '';
  position: absolute;
  top: 0;
  bottom: 0;
  width: 40%;
  background: linear-gradient(90deg, transparent, var(--accent), transparent);
  animation: engine-sweep 1.4s var(--ease) infinite;
}

.engine-meta {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--fs-cap);
  color: var(--text-3);
  --dot: var(--text-3);
}

.engine-meta.is-ok { --dot: var(--ok); }
.engine-meta.is-warn { --dot: var(--warn); }
.engine-meta.is-err { --dot: var(--err); }
.engine-meta.is-accent { --dot: var(--accent); }

.engine-dot {
  flex: none;
  width: 6px;
  height: 6px;
  border-radius: var(--r-xs);
  background: var(--dot);
}

.engine-meta.is-accent .engine-dot {
  animation: engine-pulse 1.4s var(--ease) infinite;
}

@keyframes engine-pulse {
  50% { opacity: 0.35; }
}

@keyframes engine-sweep {
  from { left: -40%; }
  to { left: 100%; }
}

@media (prefers-reduced-motion: reduce) {
  .engine-meta.is-accent .engine-dot { animation: none; }
  .engine-card-progress::after { animation: none; left: 0; width: 100%; opacity: 0.5; }
}
</style>
