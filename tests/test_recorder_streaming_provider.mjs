// 真流式 provider 展示名（src/composables/recorder/streamingProvider.mjs）单元测试。
import test from 'node:test'
import assert from 'node:assert/strict'

import { deriveStreamingProviderLabel } from '../src/composables/recorder/streamingProvider.mjs'

test('deriveStreamingProviderLabel：按 provider 给出展示名，未知回退通用文案', () => {
  assert.equal(deriveStreamingProviderLabel('qwen_asr'), 'Qwen 流式')
  assert.equal(deriveStreamingProviderLabel('doubao_asr'), '豆包在线流式')
  assert.equal(deriveStreamingProviderLabel('whisper'), '流式')
  assert.equal(deriveStreamingProviderLabel(undefined), '流式')
})
