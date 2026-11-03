// 热词纯逻辑（src/components/settings/asrHotwords.mjs）单元测试。
// 覆盖：输入解析与归一化、token 估算（与后端 stt/hotwords.rs 用同一组样例）、预算截断、按引擎的生效状态。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  MAX_HOTWORD_CHARS,
  MAX_HOTWORDS,
  estimateHotwordTokens,
  hotwordBudget,
  hotwordView,
  hotwordsText,
  normalizeHotwords,
  parseHotwords,
  supportsHotwords
} from '../src/components/settings/asrHotwords.mjs'

test('解析：按换行 / 逗号 / 顿号 / 分号切分，空格不切，去重去空', () => {
  assert.deepEqual(
    parseHotwords(' Tauri \ntauri\n\n落地页，Option  +  V、WebView;Rust；'),
    ['Tauri', '落地页', 'Option + V', 'WebView', 'Rust']
  )
  assert.deepEqual(parseHotwords(''), [])
})

test('归一化：超长截断、数量上限、非字符串忽略', () => {
  assert.deepEqual(normalizeHotwords(['x'.repeat(40), 3, null]), ['x'.repeat(MAX_HOTWORD_CHARS)])
  const many = Array.from({ length: MAX_HOTWORDS + 10 }, (_, i) => `w${i}`)
  assert.equal(normalizeHotwords(many).length, MAX_HOTWORDS)
  assert.deepEqual(normalizeHotwords('Tauri'), [])
})

test('token 估算：与后端同一组样例', () => {
  assert.equal(estimateHotwordTokens('落地页'), 3)
  assert.equal(estimateHotwordTokens('Rust'), 1)
  assert.equal(estimateHotwordTokens('WebView'), 2)
  assert.equal(estimateHotwordTokens('Option + V'), 3)
  assert.equal(estimateHotwordTokens('CTranslate2'), 3)
  assert.equal(estimateHotwordTokens('Tauri 应用'), 4)
  assert.equal(estimateHotwordTokens('+'), 1)
})

test('预算：按顺序截断，第一个放不下的词及其后面的都不发送', () => {
  assert.deepEqual(hotwordBudget(['落地页', 'Rust', 'WebView'], 4), { used: 4, sent: 2, dropped: ['WebView'] })
  assert.deepEqual(hotwordBudget(['落地页'], 100), { used: 3, sent: 1, dropped: [] })
})

test('文本框：解析结果与服务端一致时保留用户原文（不吃掉刚敲的换行）', () => {
  assert.equal(hotwordsText(['Tauri'], 'Tauri\n'), 'Tauri\n')
  assert.equal(hotwordsText(['Tauri', 'Rust'], 'Tauri'), 'Tauri\nRust')
  assert.equal(hotwordsText(undefined, ''), '')
})

const row = (view, key) => view.rows.find((item) => item.key === key)

test('识别：只有豆包支持；其余引擎标「不支持」', () => {
  assert.equal(supportsHotwords('doubao_asr'), true)
  for (const provider of ['whisper', 'qwen_asr', 'chatgpt_asr']) {
    const asr = row(hotwordView(provider, ['Tauri']), 'asr')
    assert.equal(asr.tone, 'neutral', provider)
    assert.match(asr.label, /^识别 · /)
    assert.match(asr.text, /不支持热词/)
  }
})

test('识别：豆包按预算给出 支持 / 已生效 / 部分生效', () => {
  assert.equal(row(hotwordView('doubao_asr', []), 'asr').tone, 'neutral')

  const ok = row(hotwordView('doubao_asr', ['Tauri', '落地页']), 'asr')
  assert.equal(ok.tone, 'ok')
  assert.match(ok.text, /2 个词，约 5 \/ 100 token/)

  const many = Array.from({ length: 60 }, (_, i) => `词${i}`)
  const view = hotwordView('doubao_asr', many)
  assert.equal(row(view, 'asr').tone, 'warn')
  assert.equal(view.dropped.length, 10)
  assert.match(row(view, 'asr').text, /前 50 个词已生效.*后面 10 个超出上限不会发送：词50、词51、词52 等/)
})

test('LLM 纠错：与引擎无关，按模型与自动优化开关给状态', () => {
  const words = ['Tauri']
  assert.equal(row(hotwordView('whisper', words, { hasLlmModel: false }), 'llm').tone, 'warn')
  assert.equal(row(hotwordView('whisper', [], { hasLlmModel: true }), 'llm').tone, 'neutral')
  assert.match(row(hotwordView('whisper', words, { autoOptimize: false }), 'llm').text, /手动优化和文件校对时生效/)
  const on = row(hotwordView('qwen_asr', words, { hasLlmModel: true, autoOptimize: true }), 'llm')
  assert.equal(on.tone, 'ok')
  assert.match(on.text, /这 1 个词/)
})
