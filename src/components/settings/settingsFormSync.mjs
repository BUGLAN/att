// 设置表单与「服务端设置」的同步判定（纯逻辑，便于单测）。
//
// 背景（真实踩坑）：设置页的 watch(settings) 会在任何 store 设置变更时用服务端值
// 整体重建表单，并顺手刷新「已保存快照」。如果此刻用户刚改过表单、而 500ms 防抖的
// 保存还没发出，这次重建会：
//   ① 把用户的改动冲回旧值；
//   ② 让待保存内容与快照一致，于是 handleSave 判成「无变化」直接返回。
// 表现就是「勾了开关但没生效，而且没有任何提示」。
//
// 与之配套的另一半保护在组件里：初始化完成前禁用表单，避免用户在
// isInitialized 仍为 false 时改动被 watch(form) 的守卫直接丢弃。
//
// 表单往返（服务端设置 → 表单 → 提交载荷）也放在这里：normalizeSettingsForm 写出表单的每个字段，
// buildSettingsPayload 只从表单取值，数字字段统一归一成有限数。

import { hotwordsText, parseHotwords } from './asrHotwords.mjs'
import { normalizeLanguageForProvider } from './asrLanguages.mjs'
import { normalizeDownloadSource } from './modelManager.mjs'
import {
  normalizeDoubaoImeTimeoutSeconds,
  normalizeDoubaoWebTimeoutSeconds,
  normalizeLocalModelIdleMinutes
} from './asrEngineStatus.mjs'
import { normalizeLimit } from './clipboardLimits.mjs'

/**
 * 当前表单是否存在「尚未提交」的编辑。
 *
 * @param {object} params
 * @param {boolean} params.initialized 设置页是否已完成初始化（未完成时表单尚未与服务端对齐）
 * @param {string} params.draftSnapshot 当前表单的序列化结果
 * @param {string} params.savedSnapshot 最近一次提交 / 对齐后的表单序列化结果
 * @returns {boolean} true 表示有未提交改动，调用方不应再用服务端值覆盖表单
 */
export function hasPendingSettingsEdits({ initialized, draftSnapshot, savedSnapshot }) {
  // 初始化完成前表单还没与服务端对齐，快照无意义，一律允许对齐
  if (!initialized) {
    return false
  }

  // 快照为空说明还没建立基线（首次对齐中），同样允许对齐
  if (!savedSnapshot) {
    return false
  }

  return draftSnapshot !== savedSnapshot
}

/**
 * 只由服务端维护、不进提交载荷的表单字段：密钥「是否已保存」。
 *
 * 背景（真实踩坑）：保存带 Token 的设置时，store 写回服务端结果会先于 handleSave 清空明文 Token，
 * 此时表单与快照不一致，watch(settings) 按「有未提交改动」跳过重建，而这两个字段又不在快照里，
 * 之后再也不会被刷新——卡片一直显示「未配置」，直到重启应用。
 */
export const SERVER_OWNED_FIELDS = Object.freeze([
  'doubao_asr_api_key_set',
  'chatgpt_asr_api_key_set',
  'deepseek_asr_api_key_set',
  'api_token_set'
])
// 豆包网页语音无需凭据，不在这里登记「是否已保存」字段。

/**
 * 从服务端设置取出 {@link SERVER_OWNED_FIELDS} 的最新值（缺字段按「未保存」处理）。
 * 有未提交改动时也要照常写回表单：它们不影响快照，不会触发多余的保存。
 *
 * @param {object} serverSettings GET / PUT /api/settings 的响应（或 store 里的 settings）
 * @returns {Record<string, boolean>}
 */
export function serverOwnedPatch(serverSettings) {
  return Object.fromEntries(SERVER_OWNED_FIELDS.map((key) => [key, Boolean(serverSettings?.[key])]))
}

/**
 * 只存在于表单草稿、不进「已保存快照」的凭据字段。
 * 保存成功后要清掉明文 / 清除标记，否则下一次保存会把同一份 Token 再提交一遍。
 */
export const TRANSIENT_CREDENTIAL_FIELDS = Object.freeze([
  'doubao_asr_api_key',
  'doubao_asr_api_key_clear',
  'chatgpt_asr_api_key',
  'chatgpt_asr_api_key_clear',
  'deepseek_asr_api_key',
  'deepseek_asr_api_key_clear'
])

/**
 * 把「这次实际提交的载荷」转成已保存快照。
 *
 * 不能保存成功后再用当前表单重算：保存期间用户继续修改时，当前表单已经包含
 * 下一笔尚未提交的改动，重算会把它们误标成「已保存」，随后补存又被判成无变化。
 *
 * @param {object} payload 本次提交给后端的设置载荷
 * @returns {string} 用于与最新表单比较的序列化快照
 */
export function serializeSavedSettingsPayload(payload) {
  const saved = { ...(payload || {}) }
  for (const field of TRANSIENT_CREDENTIAL_FIELDS) {
    delete saved[field]
  }
  return JSON.stringify(saved)
}

/**
 * 保存请求返回后清掉本次已提交的凭据草稿。
 *
 * 只清「值仍等于提交时」的字段：如果用户在慢保存期间又输入了新 Token /
 * 重新勾了清除，这些新值必须保留给补存，不能一并被清空。
 *
 * @param {object} current 当前表单值
 * @param {object} submitted 提交时的凭据字段值
 * @returns {Record<string, string | boolean>} 需要合并回表单的清理补丁
 */
export function submittedCredentialClearPatch(current, submitted) {
  const patch = {}

  for (const field of TRANSIENT_CREDENTIAL_FIELDS) {
    if (current?.[field] !== submitted?.[field]) {
      continue
    }
    patch[field] = field.endsWith('_clear') ? false : ''
  }

  return patch
}

/**
 * 载荷里原样提交的数字字段及其内置默认值。
 *
 * 背景（真实踩坑）：BaseInput 的 `.number` 在解析失败时原样返回字符串，清空数字框得到 `''`；
 * 原样提交会被后端判成「类型非法」整单拒绝，此后在任何分页改任何设置都保存失败。
 * 提交前统一归一：不是有限数就回落到上次保存的值（服务端设置），再没有就用这里的默认值。
 */
export const NUMERIC_SETTINGS_DEFAULTS = Object.freeze({
  qwen_asr_max_new_tokens: 256,
  qwen_asr_max_audio_seconds: 30,
  sense_voice_num_threads: 2,
  doubao_asr_end_window_size: 800,
  chatgpt_asr_timeout_seconds: 120,
  deepseek_asr_timeout_seconds: 120,
  wetype_asr_timeout_seconds: 60,
  wetype_asr_pid: 0,
  silence_threshold: 1.5,
  silence_level: -40
})

/** 有限数原样返回；空串、空白、非数字字符串、NaN / Infinity、null 都返回 null。 */
function toFiniteNumber(value) {
  if (typeof value === 'number') {
    return Number.isFinite(value) ? value : null
  }
  if (typeof value === 'string' && value.trim() !== '') {
    const parsed = Number(value)
    return Number.isFinite(parsed) ? parsed : null
  }
  return null
}

/**
 * 数字字段归一：表单值 → 上次保存的值 → 内置默认值，取第一个有限数。
 *
 * @param {string} field {@link NUMERIC_SETTINGS_DEFAULTS} 里的字段名
 * @param {unknown} value 表单里的值
 * @param {object} [saved] 上次保存的服务端设置
 * @returns {number}
 */
export function normalizeNumericSetting(field, value, saved = {}) {
  return toFiniteNumber(value) ?? toFiniteNumber(saved?.[field]) ?? NUMERIC_SETTINGS_DEFAULTS[field]
}

/** 默认不改写快捷键（单测用）；设置页传入按平台补默认键、生成显示文案的归一函数。 */
const keepHotkey = (_field, hotkey) => hotkey ?? null

/**
 * 服务端设置 → 设置表单。表单的每个字段都在这里写出，载荷只从表单取值。
 *
 * @param {object} value GET /api/settings 的响应（或 store 里的 settings）
 * @param {object} [options]
 * @param {object} [options.previous] 重建前的表单（保留用户正在敲的热词原文）
 * @param {(field: string, hotkey: unknown) => object | null} [options.normalizeHotkey] 快捷键归一（按字段决定默认键）
 * @returns {object} 新表单
 */
export function normalizeSettingsForm(value = {}, { previous = {}, normalizeHotkey = keepHotkey } = {}) {
  value = value || {}
  const provider = value.asr_provider || 'whisper'
  return {
    asr_provider: provider,
    asr_language: normalizeLanguageForProvider(provider, value.asr_language || 'zh').language,
    models_dir: value.models_dir || '',
    models_dir_effective: value.models_dir_effective || '',
    models_dir_next: value.models_dir_next || value.models_dir_effective || '',
    models_dir_env_override: value.models_dir_env_override === true,
    // 文本框原文：解析结果与服务端一致时保留用户正在敲的内容（见 hotwordsText）
    asr_hotwords_text: hotwordsText(value.asr_hotwords, previous?.asr_hotwords_text),
    whisper_model: value.whisper_model || 'large-v3',
    whisper_device: value.whisper_device || 'auto',
    qwen_asr_model: value.qwen_asr_model || 'Qwen/Qwen3-ASR-1.7B',
    qwen_asr_device: value.qwen_asr_device || 'auto',
    qwen_asr_max_new_tokens: value.qwen_asr_max_new_tokens ?? 256,
    qwen_asr_max_audio_seconds: value.qwen_asr_max_audio_seconds ?? 30,
    sense_voice_model: value.sense_voice_model || 'sense-voice-zh-en-ja-ko-yue-int8',
    // 漏写这一项时表单里是 undefined，识别引擎页挂载时会把它归一成 auto 并自动保存，用户选的设备被悄悄改回
    sense_voice_device: value.sense_voice_device || 'auto',
    sense_voice_use_itn: value.sense_voice_use_itn ?? true,
    sense_voice_num_threads: value.sense_voice_num_threads ?? 2,
    model_download_source: normalizeDownloadSource(value.model_download_source),
    local_model_idle_unload_minutes: normalizeLocalModelIdleMinutes(value.local_model_idle_unload_minutes),
    // 豆包语音：明文 Token 只存在于表单内存里，初始值恒为空
    doubao_asr_api_key: '',
    doubao_asr_api_key_clear: false,
    doubao_asr_api_key_set: value.doubao_asr_api_key_set ?? false,
    doubao_asr_resource_id: value.doubao_asr_resource_id || 'volc.seedasr.sauc.duration',
    doubao_asr_endpoint: value.doubao_asr_endpoint || 'wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async',
    doubao_asr_end_window_size: value.doubao_asr_end_window_size ?? 800,
    // 豆包网页语音：匿名在线识别，无需凭据，只保留请求超时
    doubao_web_timeout_seconds: normalizeDoubaoWebTimeoutSeconds(value.doubao_web_timeout_seconds),
    // 豆包输入法语音：虚拟设备匿名在线识别，只保留请求超时
    doubao_ime_timeout_seconds: normalizeDoubaoImeTimeoutSeconds(value.doubao_ime_timeout_seconds),
    // ChatGPT 听写：明文令牌只存在于表单内存里，初始值恒为空
    chatgpt_asr_api_key: '',
    chatgpt_asr_api_key_clear: false,
    chatgpt_asr_api_key_set: value.chatgpt_asr_api_key_set ?? false,
    chatgpt_asr_endpoint: value.chatgpt_asr_endpoint || 'https://chatgpt.com/backend-api/transcribe',
    chatgpt_asr_timeout_seconds: value.chatgpt_asr_timeout_seconds ?? 120,
    // DeepSeek 语音识别：明文 Key 同样只存在于表单内存里，初始值恒为空
    deepseek_asr_api_key: '',
    deepseek_asr_api_key_clear: false,
    deepseek_asr_api_key_set: value.deepseek_asr_api_key_set ?? false,
    deepseek_asr_endpoint: value.deepseek_asr_endpoint || 'https://api.deepseek.com/v1/tools/asr',
    deepseek_asr_timeout_seconds: value.deepseek_asr_timeout_seconds ?? 120,
    wetype_asr_timeout_seconds: value.wetype_asr_timeout_seconds ?? 60,
    wetype_asr_pid: value.wetype_asr_pid ?? 0,
    pause_audio_on_recording: value.pause_audio_on_recording ?? true,
    auto_optimize: value.auto_optimize ?? false,
    auto_copy_to_clipboard: value.auto_copy_to_clipboard ?? true,
    auto_download_updates: value.auto_download_updates ?? false,
    launch_at_login: value.launch_at_login ?? false,
    main_window_close_action: value.main_window_close_action === 'quit' ? 'quit' : 'tray',
    audio_input_device: value.audio_input_device || 'default',
    hotkey: normalizeHotkey('hotkey', value.hotkey),
    hotkey_enabled: value.hotkey_enabled ?? true,
    // 点按录音默认为空（未启用）
    record_toggle_hotkey: normalizeHotkey('record_toggle_hotkey', value.record_toggle_hotkey),
    record_toggle_enabled: value.record_toggle_enabled ?? false,
    // 显示主窗口默认为空（未启用）
    show_main_window_hotkey: normalizeHotkey('show_main_window_hotkey', value.show_main_window_hotkey),
    show_main_window_enabled: value.show_main_window_enabled === true,
    clipboard_panel_enabled: value.clipboard_panel_enabled ?? true,
    clipboard_panel_hotkey: normalizeHotkey('clipboard_panel_hotkey', value.clipboard_panel_hotkey),
    clipboard_panel_enter_action: value.clipboard_panel_enter_action === 'copy' ? 'copy' : 'paste',
    clipboard_max_items: normalizeLimit(value.clipboard_max_items),
    clipboard_retention_days: normalizeLimit(value.clipboard_retention_days),
    audio_retention_days: normalizeLimit(value.audio_retention_days),
    api_token_enabled: value.api_token_enabled === true,
    api_token_set: value.api_token_set === true,
    rewrite_enabled: value.rewrite_enabled ?? true,
    rewrite_hotkey: normalizeHotkey('rewrite_hotkey', value.rewrite_hotkey),
    // 语音改写默认没有组合键；服务端 disabled 时返回空 display，不能落回录音默认键
    voice_rewrite_enabled: value.voice_rewrite_enabled ?? false,
    voice_rewrite_hotkey: normalizeHotkey('voice_rewrite_hotkey', value.voice_rewrite_hotkey),
    // 切换优化预设同样默认没有组合键
    cycle_preset_enabled: value.cycle_preset_enabled ?? false,
    cycle_preset_hotkey: normalizeHotkey('cycle_preset_hotkey', value.cycle_preset_hotkey),
    // 空串 = 跟随默认优化预设
    rewrite_preset_id: value.rewrite_preset_id || '',
    recording_mode: value.recording_mode || 'single',
    silence_threshold: value.silence_threshold ?? 1.5,
    silence_level: value.silence_level ?? -40
  }
}

/**
 * 设置表单 → PUT /api/settings 的载荷。只从表单取值；数字字段归一成有限数（见 {@link NUMERIC_SETTINGS_DEFAULTS}）。
 *
 * @param {object} form 当前表单
 * @param {object} [options]
 * @param {object} [options.saved] 上次保存的服务端设置：数字框被清空时回落到这里的值
 * @param {(field: string, hotkey: unknown) => object | null} [options.normalizeHotkey] 快捷键归一（与表单同一个）
 * @returns {object}
 */
export function buildSettingsPayload(form = {}, { saved = {}, normalizeHotkey = keepHotkey } = {}) {
  form = form || {}
  const number = (field) => normalizeNumericSetting(field, form[field], saved)
  // 微信输入法超时 0 没有意义，沿用旧行为回落（PID 0 = 自动查找，是合法值）
  const wetypeTimeout = number('wetype_asr_timeout_seconds')
  const payload = {
    asr_provider: form.asr_provider,
    asr_language: form.asr_language,
    models_dir: String(form.models_dir || '').trim(),
    asr_hotwords: parseHotwords(form.asr_hotwords_text),
    whisper_model: form.whisper_model,
    whisper_device: form.whisper_device,
    qwen_asr_model: form.qwen_asr_model,
    qwen_asr_device: form.qwen_asr_device,
    qwen_asr_max_new_tokens: number('qwen_asr_max_new_tokens'),
    qwen_asr_max_audio_seconds: number('qwen_asr_max_audio_seconds'),
    sense_voice_model: form.sense_voice_model,
    sense_voice_device: form.sense_voice_device,
    sense_voice_use_itn: Boolean(form.sense_voice_use_itn),
    sense_voice_num_threads: number('sense_voice_num_threads'),
    model_download_source: normalizeDownloadSource(form.model_download_source),
    local_model_idle_unload_minutes: normalizeLocalModelIdleMinutes(form.local_model_idle_unload_minutes),
    doubao_asr_resource_id: form.doubao_asr_resource_id,
    doubao_asr_endpoint: form.doubao_asr_endpoint,
    doubao_asr_end_window_size: number('doubao_asr_end_window_size'),
    doubao_web_timeout_seconds: normalizeDoubaoWebTimeoutSeconds(form.doubao_web_timeout_seconds),
    doubao_ime_timeout_seconds: normalizeDoubaoImeTimeoutSeconds(form.doubao_ime_timeout_seconds),
    chatgpt_asr_endpoint: form.chatgpt_asr_endpoint,
    chatgpt_asr_timeout_seconds: number('chatgpt_asr_timeout_seconds'),
    deepseek_asr_endpoint: form.deepseek_asr_endpoint,
    deepseek_asr_timeout_seconds: number('deepseek_asr_timeout_seconds'),
    wetype_asr_timeout_seconds: wetypeTimeout || NUMERIC_SETTINGS_DEFAULTS.wetype_asr_timeout_seconds,
    wetype_asr_pid: number('wetype_asr_pid'),
    pause_audio_on_recording: form.pause_audio_on_recording,
    auto_optimize: form.auto_optimize,
    auto_copy_to_clipboard: form.auto_copy_to_clipboard,
    auto_download_updates: Boolean(form.auto_download_updates),
    launch_at_login: Boolean(form.launch_at_login),
    main_window_close_action: form.main_window_close_action === 'quit' ? 'quit' : 'tray',
    audio_input_device: form.audio_input_device,
    hotkey: normalizeHotkey('hotkey', form.hotkey),
    hotkey_enabled: form.hotkey_enabled !== false,
    record_toggle_hotkey: normalizeHotkey('record_toggle_hotkey', form.record_toggle_hotkey),
    record_toggle_enabled: form.record_toggle_enabled === true,
    show_main_window_hotkey: normalizeHotkey('show_main_window_hotkey', form.show_main_window_hotkey),
    show_main_window_enabled: form.show_main_window_enabled === true,
    clipboard_panel_enabled: Boolean(form.clipboard_panel_enabled),
    clipboard_panel_hotkey: normalizeHotkey('clipboard_panel_hotkey', form.clipboard_panel_hotkey),
    clipboard_panel_enter_action: form.clipboard_panel_enter_action,
    clipboard_max_items: normalizeLimit(form.clipboard_max_items),
    clipboard_retention_days: normalizeLimit(form.clipboard_retention_days),
    audio_retention_days: normalizeLimit(form.audio_retention_days),
    api_token_enabled: Boolean(form.api_token_enabled),
    rewrite_enabled: Boolean(form.rewrite_enabled),
    rewrite_hotkey: normalizeHotkey('rewrite_hotkey', form.rewrite_hotkey),
    voice_rewrite_enabled: Boolean(form.voice_rewrite_enabled),
    voice_rewrite_hotkey: normalizeHotkey('voice_rewrite_hotkey', form.voice_rewrite_hotkey),
    cycle_preset_enabled: Boolean(form.cycle_preset_enabled),
    cycle_preset_hotkey: normalizeHotkey('cycle_preset_hotkey', form.cycle_preset_hotkey),
    rewrite_preset_id: form.rewrite_preset_id || '',
    recording_mode: form.recording_mode,
    silence_threshold: number('silence_threshold'),
    silence_level: number('silence_level')
  }

  // 明文 Token 只在「用户确实输入了」时才提交：
  // 空串代表「本次不修改」，后端会保留已保存的密钥（否则整份快照会把密钥清掉）。
  for (const provider of ['doubao', 'chatgpt', 'deepseek']) {
    const key = `${provider}_asr_api_key`
    if (form[key]) {
      payload[key] = form[key]
    }
    if (form[`${key}_clear`]) {
      payload[`${key}_clear`] = true
    }
  }

  return payload
}
