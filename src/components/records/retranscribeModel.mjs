// 记录「重新识别」的纯逻辑（不依赖 Vue，供 tests/test_record_retranscribe.mjs 锁定）。
//
// 后端契约：POST /api/records/{id}/retranscribe {provider?} 受理后返回 202 + 置为 transcribing 的记录，
// 识别在后台跑；前端按记录状态轮询，状态离开 transcribing 即结束（transcribed = 成功，failed = 失败）。
// 可选引擎来自 GET /api/asr/engines：只列凭据齐备 / 本地模型就位的引擎。

import { languageLabel } from '../settings/asrLanguages.mjs'

/** 轮询间隔与最长等待（本地引擎识别长录音可能要几分钟） */
export const RETRANSCRIBE_POLL_INTERVAL_MS = 1000
export const RETRANSCRIBE_MAX_WAIT_MS = 15 * 60 * 1000

const PROVIDER_LABELS = Object.freeze({
  doubao_asr: '豆包语音',
  doubao_web_asr: '豆包网页语音',
  chatgpt_asr: 'ChatGPT 听写',
  deepseek_asr: 'DeepSeek 语音识别',
  whisper: '本地 Whisper',
  qwen_asr: '本地 Qwen3-ASR'
})

// 引擎下拉里的短名称（下拉放在记录展开后的半栏里，全称会被截断）
const PROVIDER_SHORT_LABELS = Object.freeze({
  doubao_asr: '豆包语音',
  doubao_web_asr: '豆包网页语音',
  chatgpt_asr: 'ChatGPT 听写',
  deepseek_asr: 'DeepSeek',
  whisper: '本地 Whisper',
  qwen_asr: '本地 Qwen3-ASR'
})

export function asrProviderLabel(provider) {
  return PROVIDER_LABELS[provider] || provider || ''
}

/** 识别语言的界面名称（与「设置 → 识别引擎 → 识别语言」同一张表，auto = 自动检测） */
export function asrLanguageLabel(language) {
  return language ? languageLabel(language) : ''
}

/** 正在识别中（含旧版的 pending）：不能再点重新识别 */
export function isTranscribing(record) {
  return record?.status === 'transcribing' || record?.status === 'pending'
}

/** 有录音音频的记录才能重新识别（优化选中文本建的文本记录、录音落盘失败的没有音频） */
export function canRetranscribe(record) {
  return Boolean(record?.audio_path) && !isTranscribing(record)
}

/** 折叠行上直接给「重新识别」：识别失败且有音频 */
export function showQuickRetranscribe(record) {
  return record?.status === 'failed' && canRetranscribe(record)
}

/**
 * 重新识别的引擎选项：只列可用的，当前设置的引擎排第一并标「当前」。
 * engines：GET /api/asr/engines 的 engines（[{ provider, label, available, error }]）；current：设置里的 asr_provider。
 * 返回 [{ value, label }]
 */
export function retranscribeEngineOptions(engines = [], current = '') {
  const available = (Array.isArray(engines) ? engines : []).filter((engine) => engine?.available && engine?.provider)
  const ordered = [
    ...available.filter((engine) => engine.provider === current),
    ...available.filter((engine) => engine.provider !== current)
  ]
  return ordered.map((engine) => {
    const label = PROVIDER_SHORT_LABELS[engine.provider] || engine.label || engine.provider
    return { value: engine.provider, label: engine.provider === current ? `${label}（当前）` : label }
  })
}

/** 默认选中：当前引擎可用就用它，否则第一个可用的；一个都没有返回 '' */
export function defaultRetranscribeEngine(options = [], current = '') {
  if (options.some((option) => option.value === current)) {
    return current
  }
  return options[0]?.value || ''
}

/** 重新识别前的确认文案（成功才覆盖，失败不动文字） */
export const RETRANSCRIBE_CONFIRM = '识别成功后会覆盖现在的文字并清空优化结果（编辑过的也会被覆盖）；识别失败时文字保持不变。'

/** 记录里已经有文字（原文或优化稿）时，重新识别前要先确认 */
export function needsRetranscribeConfirm(record) {
  return Boolean(String(record?.raw_text || '').trim() || String(record?.optimized_text || '').trim())
}

/** 识别信息里的「引擎 · 语言」；旧记录没有这两个字段时返回 '' */
export function recordAsrSummary(record) {
  return [asrProviderLabel(record?.engine), asrLanguageLabel(record?.language)].filter(Boolean).join(' · ')
}

/** 识别结束后的提示：成功（含降级原因）/ 失败 */
export function retranscribeResultToast(record) {
  if (record?.status === 'transcribed') {
    return record.error
      ? { message: `重新识别完成，但有部分内容没识别到：${record.error}`, tone: 'warn' }
      : { message: '重新识别完成', tone: 'success' }
  }
  return { message: `重新识别失败：${record?.error || '没有拿到识别结果'}`, tone: 'error' }
}
