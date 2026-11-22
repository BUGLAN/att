// 记录「重新识别」纯逻辑（src/components/records/retranscribeModel.mjs）单元测试。
// 覆盖：哪些记录能重新识别、折叠行上的快捷按钮、引擎选项只列可用的且当前排第一、识别信息、结束提示。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  canRetranscribe,
  defaultRetranscribeEngine,
  needsRetranscribeConfirm,
  recordAsrSummary,
  retranscribeEngineOptions,
  retranscribeResultToast,
  showQuickRetranscribe
} from '../src/components/records/retranscribeModel.mjs'

test('有音频且不在识别中的记录才能重新识别', () => {
  assert.equal(canRetranscribe({ audio_path: 'a.flac', status: 'transcribed' }), true)
  assert.equal(canRetranscribe({ audio_path: 'a.flac', status: 'failed' }), true)
  assert.equal(canRetranscribe({ audio_path: '', status: 'optimized' }), false, '文本记录没有音频')
  assert.equal(canRetranscribe({ audio_path: 'a.flac', status: 'transcribing' }), false)
  assert.equal(canRetranscribe({ audio_path: 'a.flac', status: 'pending' }), false)
  assert.equal(canRetranscribe(null), false)
})

test('折叠行只给失败且有音频的记录显示快捷按钮', () => {
  assert.equal(showQuickRetranscribe({ audio_path: 'a.flac', status: 'failed' }), true)
  assert.equal(showQuickRetranscribe({ audio_path: 'a.flac', status: 'transcribed' }), false)
  assert.equal(showQuickRetranscribe({ audio_path: '', status: 'failed' }), false)
})

test('引擎选项：只列可用的，当前设置的排第一并标注', () => {
  const engines = [
    { provider: 'doubao_asr', label: '豆包语音', available: true, error: null },
    { provider: 'doubao_web_asr', label: '豆包网页语音', available: true, error: null },
    { provider: 'chatgpt_asr', label: 'ChatGPT 听写', available: false, error: '未配置' },
    { provider: 'deepseek_asr', label: 'DeepSeek 语音识别', available: true, error: null },
    { provider: 'whisper', label: '本地 Whisper', available: false, error: '模型未安装' }
  ]
  const options = retranscribeEngineOptions(engines, 'deepseek_asr')
  assert.deepEqual(options, [
    { value: 'deepseek_asr', label: 'DeepSeek（当前）' },
    { value: 'doubao_asr', label: '豆包语音' },
    { value: 'doubao_web_asr', label: '豆包网页语音' }
  ])
  assert.equal(defaultRetranscribeEngine(options, 'deepseek_asr'), 'deepseek_asr')

  // 当前引擎不可用：默认选第一个可用的
  const fallback = retranscribeEngineOptions(engines, 'whisper')
  assert.deepEqual(fallback.map((option) => option.value), ['doubao_asr', 'doubao_web_asr', 'deepseek_asr'])
  assert.equal(defaultRetranscribeEngine(fallback, 'whisper'), 'doubao_asr')

  assert.deepEqual(retranscribeEngineOptions([], 'doubao_asr'), [])
  assert.equal(defaultRetranscribeEngine([], 'doubao_asr'), '')
  assert.deepEqual(retranscribeEngineOptions(null, ''), [])
})

test('已有文字的记录重新识别前要确认，失败且没有文字的直接识别', () => {
  assert.equal(needsRetranscribeConfirm({ raw_text: '原文' }), true)
  assert.equal(needsRetranscribeConfirm({ raw_text: '', optimized_text: '优化稿' }), true)
  assert.equal(needsRetranscribeConfirm({ raw_text: '  ', optimized_text: null, status: 'failed' }), false)
  assert.equal(needsRetranscribeConfirm({}), false)
})

test('识别信息：引擎 · 语言，旧记录没有字段时为空', () => {
  assert.equal(recordAsrSummary({ engine: 'whisper', language: 'zh' }), '本地 Whisper · 中文')
  assert.equal(recordAsrSummary({ engine: 'doubao_asr' }), '豆包语音')
  assert.equal(recordAsrSummary({ engine: 'doubao_web_asr', language: 'auto' }), '豆包网页语音 · 自动检测')
  assert.equal(recordAsrSummary({ engine: 'deepseek_asr', language: 'auto' }), 'DeepSeek 语音识别 · 自动检测')
  assert.equal(recordAsrSummary({}), '')
})

test('结束提示：成功 / 带降级原因 / 失败', () => {
  assert.deepEqual(retranscribeResultToast({ status: 'transcribed', error: null }), { message: '重新识别完成', tone: 'success' })
  assert.equal(retranscribeResultToast({ status: 'transcribed', error: '第 2 段识别失败' }).tone, 'warn')
  assert.deepEqual(retranscribeResultToast({ status: 'failed', error: '网络连接失败' }), {
    message: '重新识别失败：网络连接失败',
    tone: 'error'
  })
  assert.equal(retranscribeResultToast({ status: 'failed' }).message, '重新识别失败：没有拿到识别结果')
})
