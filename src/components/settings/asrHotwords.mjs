// 热词（个人词表）的纯逻辑：解析输入、估算 token 预算、按当前引擎与 LLM 配置给出生效状态
// （不依赖 Vue，供 tests/test_settings_asr_hotwords.mjs 锁定）。
//
// 词表与引擎无关地保存在设置 `asr_hotwords`，走两条路：
// - 识别：只有部分引擎能用，目前只有豆包支持热词直传；faster-whisper（ct2rs 不支持前置提示）、
//   Qwen3-ASR（sherpa-onnx 无此配置）、ChatGPT 听写（接口只收音频）都不支持；
// - LLM 纠错：AI 优化与文件校对的提示词里都会带上词表，与识别引擎无关，前提是配置了 LLM 模型。
// 归一化与 token 估算规则与后端 att-core `stt/hotwords.rs` 一致，改动时两边一起改。

import { providerLabel } from './asrEngineStatus.mjs'

/** 支持热词的引擎 */
export const HOTWORD_PROVIDERS = Object.freeze(['doubao_asr'])
/** 豆包双向流式的热词直传上限（tokens），超出部分按顺序截断不发送 */
export const HOTWORD_TOKEN_BUDGET = 100
export const MAX_HOTWORDS = 200
export const MAX_HOTWORD_CHARS = 32

export function supportsHotwords(provider) {
  return HOTWORD_PROVIDERS.includes(provider)
}

/**
 * 归一化：去首尾空白、内部空白合并、超长截断（按码点）、大小写不敏感去重（保留先出现的）。
 */
export function normalizeHotwords(words) {
  const seen = new Set()
  const out = []
  for (const raw of Array.isArray(words) ? words : []) {
    if (typeof raw !== 'string') {
      continue
    }
    const collapsed = raw.trim().split(/\s+/).join(' ')
    const word = Array.from(collapsed).slice(0, MAX_HOTWORD_CHARS).join('').trimEnd()
    const key = word.toLowerCase()
    if (!word || seen.has(key)) {
      continue
    }
    seen.add(key)
    out.push(word)
    if (out.length >= MAX_HOTWORDS) {
      break
    }
  }
  return out
}

/** 文本框内容 → 词表：按换行、中英文逗号、顿号、分号切分（空格不切，「Option + V」算一个词）。 */
export function parseHotwords(text) {
  return normalizeHotwords(String(text || '').split(/[\n\r,，、;；]+/))
}

/**
 * 服务端词表 → 文本框内容。用户正在编辑的文本解析结果与服务端一致时原样保留，
 * 否则保存后重建表单会吃掉用户刚敲的换行（光标跳动）。
 */
export function hotwordsText(words, currentText = '') {
  const normalized = normalizeHotwords(words)
  if (sameWords(parseHotwords(currentText), normalized)) {
    return currentText
  }
  return normalized.join('\n')
}

function sameWords(a, b) {
  return a.length === b.length && a.every((word, index) => word === b[index])
}

/**
 * 估算一个热词占多少 token：非 ASCII 字符（汉字等）各算 1；连续英文字母 / 数字每 4 个算 1
 * （不足 4 个也算 1）；空格与 ASCII 标点不计；至少算 1。
 */
export function estimateHotwordTokens(word) {
  let tokens = 0
  let asciiRun = 0
  for (const char of Array.from(String(word || ''))) {
    if (/^[A-Za-z0-9]$/.test(char)) {
      asciiRun += 1
      continue
    }
    tokens += Math.ceil(asciiRun / 4)
    asciiRun = 0
    if (char.codePointAt(0) > 0x7f && !/^\s$/.test(char)) {
      tokens += 1
    }
  }
  return Math.max(1, tokens + Math.ceil(asciiRun / 4))
}

/** 按顺序计入预算：返回 { used, sent, dropped }（第一个放不下的词及其后面的都不发送）。 */
export function hotwordBudget(words, budget = HOTWORD_TOKEN_BUDGET) {
  let used = 0
  for (const [index, word] of words.entries()) {
    const cost = estimateHotwordTokens(word)
    if (used + cost > budget) {
      return { used, sent: index, dropped: words.slice(index) }
    }
    used += cost
  }
  return { used, sent: words.length, dropped: [] }
}

/** 识别这一路的状态：{ tone, label, text, dropped } */
function asrRow(provider, list) {
  const label = `识别 · ${providerLabel(provider)}`
  if (!supportsHotwords(provider)) {
    return { tone: 'neutral', label, text: '不支持热词，切换到豆包语音后生效。', dropped: [] }
  }
  if (!list.length) {
    return { tone: 'neutral', label, text: `支持，按顺序发送，最多约 ${HOTWORD_TOKEN_BUDGET} token。`, dropped: [] }
  }
  const { used, sent, dropped } = hotwordBudget(list)
  if (dropped.length) {
    const preview = dropped.slice(0, 3).join('、') + (dropped.length > 3 ? ' 等' : '')
    return {
      tone: 'warn',
      label,
      text: `前 ${sent} 个词已生效（约 ${used} / ${HOTWORD_TOKEN_BUDGET} token）；后面 ${dropped.length} 个超出上限不会发送：${preview}。常用的词往前放。`,
      dropped
    }
  }
  return { tone: 'ok', label, text: `已生效：${list.length} 个词，约 ${used} / ${HOTWORD_TOKEN_BUDGET} token。`, dropped: [] }
}

/** LLM 纠错这一路的状态：{ tone, label, text }。hasLlmModel 未知时调用方按 true 传，免得误报 */
function llmRow(list, { hasLlmModel = true, autoOptimize = false } = {}) {
  const label = 'LLM 纠错'
  if (!hasLlmModel) {
    return { tone: 'warn', label, text: '还没有可用的 LLM 模型，词表暂时只用于识别。' }
  }
  if (!list.length) {
    return { tone: 'neutral', label, text: '填写后，AI 优化和文件校对会按词表纠正同音错字，所有引擎都适用。' }
  }
  if (!autoOptimize) {
    return { tone: 'neutral', label, text: '手动优化和文件校对时生效；开启「自动 AI 优化」后每次录音都会用到。' }
  }
  return { tone: 'ok', label, text: `已生效：AI 优化和文件校对会按这 ${list.length} 个词纠正同音错字。` }
}

/**
 * 热词区的展示模型：{ rows: [{ key, tone, label, text }], dropped }。
 * 两行分别是「识别」（随引擎变化）与「LLM 纠错」（随 LLM 配置变化）；tone 与引擎卡片的状态圆点同一套：ok / warn / neutral。
 */
export function hotwordView(provider, words, llm = {}) {
  const list = normalizeHotwords(words)
  const { dropped, ...asr } = asrRow(provider, list)
  return {
    rows: [{ key: 'asr', ...asr }, { key: 'llm', ...llmRow(list, llm) }],
    dropped
  }
}
