// 识别语言清单（纯逻辑，供设置页、转文字页与 Node 用例共用）。
//
// 各引擎支持范围以官方文档为准，并保留「自动检测」兜底：
// - 豆包 SAUC：官方列出中文（含方言）与常用外语，自动检测由服务端判定；
// - 豆包网页语音：接口没有 language 参数，只能自动判定；
// - Whisper / ChatGPT 听写：列常用语种，模型支持的其余语种仍可通过自动检测识别；
// - DeepSeek：接口没有 language 参数，只能自动判定；
// - 微信输入法语音：请求不带 language，由输入法服务端自动判定；
// - Qwen3-ASR：sherpa 配置没有 language 参数，由模型自行判定。

import { providerLabel } from './asrEngineStatus.mjs'

export const AUTO_LANGUAGE = 'auto'

export const LANGUAGE_LABELS = Object.freeze({
  auto: '自动检测',
  zh: '中文',
  yue: '粤语',
  en: '英语',
  ja: '日语',
  ko: '韩语',
  es: '西班牙语',
  fr: '法语',
  de: '德语',
  pt: '葡萄牙语',
  ru: '俄语',
  it: '意大利语',
  id: '印度尼西亚语',
  vi: '越南语',
  th: '泰语',
  ar: '阿拉伯语',
  hi: '印地语',
  fil: '菲律宾语',
  ms: '马来语',
  bn: '孟加拉语',
  el: '希腊语',
  nl: '荷兰语',
  tr: '土耳其语',
  pl: '波兰语',
  ro: '罗马尼亚语',
  ne: '尼泊尔语',
  uk: '乌克兰语'
})

// Whisper / ChatGPT 常用语种（官方还支持更多，未单列时交给自动检测）。
const COMMON_LANGUAGE_CODES = Object.freeze([
  'zh', 'en', 'ja', 'ko', 'es', 'fr', 'de', 'pt', 'ru', 'it', 'id', 'vi', 'th', 'ar', 'hi'
])

// 豆包大模型语音识别官方语种列表（「中文」包含多种方言，另有单独的粤语）。
const DOUBAO_LANGUAGE_CODES = Object.freeze([
  'zh', 'yue', 'en', 'ja', 'ko', 'id', 'es', 'pt', 'de', 'fr', 'fil', 'ms', 'th', 'ar', 'it', 'bn',
  'el', 'nl', 'ru', 'tr', 'vi', 'pl', 'ro', 'ne', 'uk'
])

const PROVIDER_LANGUAGE_CODES = Object.freeze({
  doubao_asr: DOUBAO_LANGUAGE_CODES,
  doubao_web_asr: Object.freeze([]),
  chatgpt_asr: COMMON_LANGUAGE_CODES,
  deepseek_asr: Object.freeze([]),
  wetype_asr: Object.freeze([]),
  whisper: COMMON_LANGUAGE_CODES,
  qwen_asr: Object.freeze([])
})

function languageCodes(provider) {
  return PROVIDER_LANGUAGE_CODES[provider] || COMMON_LANGUAGE_CODES
}

function languageKey(code) {
  return String(code || '').trim().toLowerCase().split(/[-_]/)[0]
}

export function languageLabel(code) {
  const value = String(code || '').trim().toLowerCase()
  const key = languageKey(value)
  return LANGUAGE_LABELS[key] || value
}

/** 当前引擎的语言下拉选项；「自动检测」恒在首项。 */
export function languageOptions(provider) {
  return [AUTO_LANGUAGE, ...languageCodes(provider)].map((value) => ({
    value,
    label: languageLabel(value),
    auto: value === AUTO_LANGUAGE
  }))
}

/**
 * 把设置里的语言归一为当前引擎可用的值。
 * 兼容旧数据与带地区的短码（`zh-CN` → `zh`）；当前引擎不支持时回落到自动检测。
 * 返回 { language, changed, reset, message }：changed 表示值需要写回，message 只在回落时非空。
 */
export function normalizeLanguageForProvider(provider, language) {
  const raw = String(language || '').trim().toLowerCase()
  const base = languageKey(raw)
  const supported = languageCodes(provider)
  const allowed = !base || base === AUTO_LANGUAGE || supported.includes(base)
  const reset = Boolean(base) && base !== AUTO_LANGUAGE && !allowed
  const value = allowed ? (base || AUTO_LANGUAGE) : AUTO_LANGUAGE
  const previous = languageLabel(base || raw) || '当前语言'
  return {
    language: value,
    changed: value !== raw,
    reset,
    message: reset ? `${providerLabel(provider)}不支持「${previous}」，已改为自动检测` : ''
  }
}

/** 语言设置下方的一句话说明；Qwen / DeepSeek 明确告诉用户语言不能手动指定。 */
export function languageHint(provider) {
  if (provider === 'qwen_asr') {
    return 'Qwen3-ASR 不支持指定语言，由模型自行判定。'
  }
  if (provider === 'doubao_web_asr') {
    return '豆包网页语音由音频自动判定语种，不能手动指定。'
  }
  if (provider === 'deepseek_asr') {
    return 'DeepSeek 语音识别由音频自动判定语种，不能手动指定。'
  }
  if (provider === 'wetype_asr') {
    return '微信输入法语音由输入法服务端自动判定语种，请求不携带语言。'
  }
  if (provider === 'doubao_asr') {
    return '自动检测时豆包支持中英混说与粤语等方言；固定语种可识别更多外语。'
  }
  return '自动检测适合多语种或中英混说；固定语种可减少误判。'
}
