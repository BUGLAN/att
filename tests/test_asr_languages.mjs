// 识别语言清单与按引擎归一的纯逻辑测试。
// 覆盖：自动检测恒在首项、各引擎支持范围、旧短码兼容、不支持时回落并提示。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  AUTO_LANGUAGE,
  languageHint,
  languageLabel,
  languageOptions,
  normalizeLanguageForProvider
} from '../src/components/settings/asrLanguages.mjs'

const ALL_PROVIDERS = [
  'doubao_asr',
  'doubao_web_asr',
  'chatgpt_asr',
  'deepseek_asr',
  'wetype_asr',
  'whisper',
  'qwen_asr'
]

test('语言下拉：所有引擎首项都是自动检测，且选项不重复', () => {
  assert.equal(AUTO_LANGUAGE, 'auto')
  for (const provider of ALL_PROVIDERS) {
    const options = languageOptions(provider)
    assert.deepEqual(options[0], { value: 'auto', label: '自动检测', auto: true }, provider)
    assert.equal(new Set(options.map((item) => item.value)).size, options.length, `${provider} 不应有重复选项`)
  }
})

test('语言下拉：豆包网页 / 微信输入法 / Qwen / DeepSeek 只能自动判定，豆包与 Whisper 可选固定语种', () => {
  assert.deepEqual(languageOptions('doubao_web_asr').map((item) => item.value), ['auto'])
  assert.deepEqual(languageOptions('qwen_asr').map((item) => item.value), ['auto'])
  assert.deepEqual(languageOptions('deepseek_asr').map((item) => item.value), ['auto'])
  assert.deepEqual(languageOptions('wetype_asr').map((item) => item.value), ['auto'])

  const doubao = languageOptions('doubao_asr').map((item) => item.value)
  assert.ok(doubao.includes('zh'))
  assert.ok(doubao.includes('yue'), '豆包官方支持粤语')
  assert.ok(doubao.includes('uk'))

  for (const provider of ['whisper', 'chatgpt_asr']) {
    const codes = languageOptions(provider).map((item) => item.value)
    assert.ok(codes.includes('zh'))
    assert.ok(codes.includes('hi'))
    assert.ok(!codes.includes('yue'), `${provider} 的常用列表不单列粤语`)
  }
})

test('旧语言短码：带地区后缀时归一到基础语种', () => {
  assert.deepEqual(normalizeLanguageForProvider('doubao_asr', 'zh-CN'), {
    language: 'zh',
    changed: true,
    reset: false,
    message: ''
  })
  assert.equal(normalizeLanguageForProvider('whisper', 'EN_us').language, 'en')
  assert.equal(normalizeLanguageForProvider('doubao_asr', 'yue-CN').language, 'yue')
  assert.equal(normalizeLanguageForProvider('whisper', 'zh').language, 'zh', '默认中文保持不变')
})

test('自动检测：自动检测本身永远合法，不会触发回落提示', () => {
  for (const provider of ALL_PROVIDERS) {
    assert.deepEqual(normalizeLanguageForProvider(provider, 'auto'), {
      language: 'auto',
      changed: false,
      reset: false,
      message: ''
    }, provider)
  }
})

test('切换引擎：新引擎不支持当前语言时回落自动检测并提示', () => {
  const qwen = normalizeLanguageForProvider('qwen_asr', 'zh')
  assert.equal(qwen.language, 'auto')
  assert.equal(qwen.changed, true)
  assert.equal(qwen.reset, true)
  assert.match(qwen.message, /Qwen3-ASR不支持「中文」，已改为自动检测/)

  const deepseek = normalizeLanguageForProvider('deepseek_asr', 'ja')
  assert.equal(deepseek.language, 'auto')
  assert.match(deepseek.message, /DeepSeek 语音识别不支持「日语」，已改为自动检测/)

  const doubaoWeb = normalizeLanguageForProvider('doubao_web_asr', 'zh')
  assert.equal(doubaoWeb.language, 'auto')
  assert.match(doubaoWeb.message, /豆包网页语音不支持「中文」，已改为自动检测/)

  const doubao = normalizeLanguageForProvider('doubao_asr', 'xx')
  assert.equal(doubao.language, 'auto')
  assert.match(doubao.message, /豆包语音不支持「xx」，已改为自动检测/)
})

test('语言标签与说明：地区码显示基础语种中文名，自动判定引擎明确不能指定', () => {
  assert.equal(languageLabel('zh-CN'), '中文')
  assert.equal(languageLabel('EN_us'), '英语')
  assert.equal(languageLabel('xx'), 'xx')
  assert.match(languageHint('qwen_asr'), /不支持指定语言/)
  assert.match(languageHint('deepseek_asr'), /自动判定/)
  assert.match(languageHint('doubao_web_asr'), /自动判定/)
  assert.match(languageHint('doubao_asr'), /自动检测/)
  assert.match(languageHint('whisper'), /自动检测/)
})
